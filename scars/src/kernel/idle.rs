use crate::Scheduler;
use crate::Stack;
use crate::priority::Priority;
use crate::sync::PreemptLock;
use crate::task::ThreadExecutor;
use crate::thread::{
    RawThread, Thread, ThreadBuilder, ThreadExecutionState, ThreadFn, ThreadHandle, ThreadRef,
};
use crate::{make_thread, thread};
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
    unsafe fn _start_main_thread();
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
pub(crate) fn init_idle_thread() -> &'static RawThread {
    crate::printkln!("Init idle thread");

    static IDLE_STACK: Stack<IDLE_THREAD_STACK_SIZE> = Stack::new();
    static IDLE_THREAD: Thread<IDLE_THREAD_PRIO, IdleFn> = Thread::new(IDLE_THREAD_NAME);

    let mut idle_thread = IDLE_THREAD.init(IDLE_STACK.init()).attach(|| idle());
    idle_thread.modify(|t| {
        PreemptLock::with(|pkey| {
            t.state.set(pkey, ThreadExecutionState::Running);
        })
    });

    let idle_thread = idle_thread.get_ref();

    unsafe { idle_thread.as_ref() }
}

fn idle() -> ! {
    #[cfg(not(test))]
    unsafe {
        _start_main_thread();
    }

    #[cfg(test)]
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

    loop {
        crate::kernel::idle::idle_thread_hook();
    }
}

#[cfg(test)]
fn test() -> ! {
    crate::test_main();
    scars_test::test_succeed();
}
