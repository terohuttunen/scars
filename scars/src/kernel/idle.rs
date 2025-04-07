use crate::priority::Priority;
use crate::sync::PreemptLock;
use crate::thread::{InitializedThread, RawThread, ThreadBuilder, ThreadExecutionState};
use crate::{make_local_storage, make_thread, make_thread_executor, thread};

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

pub(crate) fn init_idle_thread() -> &'static RawThread {
    crate::printkln!("Init idle thread");
    let mut idle_thread = idle();
    idle_thread.modify(|t| {
        PreemptLock::with(|pkey| {
            t.state.set(pkey, ThreadExecutionState::Running);
        })
    });

    let idle_thread = idle_thread.finish();

    unsafe { idle_thread.as_ref() }
}

#[thread(name = IDLE_THREAD_NAME, priority = IDLE_THREAD_PRIO, stack_size = IDLE_THREAD_STACK_SIZE)]
fn idle() -> ! {
    #[cfg(not(test))]
    unsafe {
        _start_main_thread();
    }

    #[cfg(test)]
    {
        let local_storage = make_local_storage!(1);
        let executor = make_thread_executor!();
        let mut test_thread = test();
        test_thread.set_local_storage(local_storage);
        test_thread.start_executor(executor);
        test_thread.start();
    }

    loop {
        crate::kernel::idle::idle_thread_hook();
    }
}

#[cfg(test)]
#[thread(priority = TEST_THREAD_PRIO, stack_size = IDLE_THREAD_STACK_SIZE)]
fn test() -> ! {
    crate::test_main();
    crate::scars_test::test_succeed();
}
