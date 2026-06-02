use crate::Scheduler;
use crate::Stack;
use crate::kernel::hal::CoreId;
use crate::priority::Priority;
use crate::sync::PreemptLock;
#[cfg(all(test, feature = "multithreading"))]
use crate::task::ThreadExecutor;
use crate::thread;
use crate::thread::{
    RawThread, Thread, ThreadBuilder, ThreadExecutionState, ThreadFn, ThreadHandle, ThreadRef,
};
use static_cell::StaticCell;

const IDLE_THREAD_NAME: &'static str = "[idle]";
const IDLE_THREAD_PRIO: Priority = Priority::Thread(0);

#[cfg(test)]
const TEST_THREAD_PRIO: Priority = Priority::Thread(1);

#[cfg(not(feature = "khal-sim"))]
const IDLE_THREAD_STACK_SIZE: usize = 1024;

#[cfg(feature = "khal-sim")]
const IDLE_THREAD_STACK_SIZE: usize = 1024 * 16;

mod internal {
    #[allow(dead_code)]
    unsafe extern "Rust" {
        #[link_name = "_scars_idle_thread_hook"]
        pub(super) unsafe fn idle_thread_hook();
    }
}

unsafe extern "Rust" {
    #[cfg(not(test))]
    unsafe fn _scars_app_init();
}

#[allow(unused)]
#[inline(always)]
fn idle_thread_hook() {
    unsafe { internal::idle_thread_hook() }
}

#[unsafe(export_name = "_scars_default_idle_thread_hook")]
fn default_idle_thread_hook() {
    crate::idle();
}

type IdleFn = impl ThreadFn;

#[define_opaque(IdleFn)]
pub(crate) fn init_idle_thread(core: CoreId) -> &'static RawThread {
    use crate::kernel::hal::NUM_CORES;

    crate::printkln!("Init idle thread for core {}", core.as_u8());

    static IDLE_STACKS: [Stack<IDLE_THREAD_STACK_SIZE>; NUM_CORES] =
        [const { Stack::new() }; NUM_CORES];
    static IDLE_THREADS: [Thread<IDLE_THREAD_PRIO, IdleFn>; NUM_CORES] =
        [const { Thread::new(IDLE_THREAD_NAME) }; NUM_CORES];

    let idle_static: &'static Thread<IDLE_THREAD_PRIO, IdleFn> = &IDLE_THREADS[core.as_usize()];
    let idle_stack = IDLE_STACKS[core.as_usize()].init();
    let mut idle_thread = idle_static.init(idle_stack).attach(|| idle());
    idle_thread.modify(|t| {
        PreemptLock::with(|pkey| {
            t.state.set(pkey, ThreadExecutionState::Running);
        })
    });

    let idle_thread = idle_thread.get_ref();

    unsafe { idle_thread.as_ref() }
}

fn idle() -> ! {
    // Application init runs exactly once, on core CoreId::DEFAULT. Secondary cores
    // pick up threads bound to them after core CoreId::DEFAULT finishes init.
    if CoreId::current() == CoreId::DEFAULT {
        #[cfg(not(test))]
        unsafe {
            _scars_app_init();
        }

        // With multithreading the test body runs in its own thread (so it
        // can exercise blocking APIs); otherwise it runs directly in idle
        // context.
        #[cfg(all(test, feature = "multithreading"))]
        {
            static THREAD_EXECUTOR: StaticCell<ThreadExecutor> = StaticCell::new();
            static THREAD_STACK: crate::Stack<IDLE_THREAD_STACK_SIZE> = Stack::new();
            static TEST_THREAD: crate::Thread<TEST_THREAD_PRIO, fn() -> !> = Thread::new("test");

            let executor = THREAD_EXECUTOR.init_with(|| ThreadExecutor::new());
            let test_thread = TEST_THREAD.init(THREAD_STACK.init()).attach(test);
            let test_thread_ref = test_thread.get_ref();
            // SAFETY: test thread is 'static (created from a static cell).
            let raw_thread: &'static crate::thread::RawThread = unsafe { test_thread_ref.as_ref() };
            raw_thread.local_storage().head().publish(executor);
            test_thread.start();
        }

        #[cfg(all(test, not(feature = "multithreading")))]
        {
            crate::test_main();
            scars_test::test_succeed();
        }
    }

    loop {
        crate::kernel::idle::idle_thread_hook();
    }
}

#[cfg(all(test, feature = "multithreading"))]
fn test() -> ! {
    crate::test_main();
    scars_test::test_succeed();
}
