use crate::kernel::priority::ThreadPriority;
use crate::make_thread;
use crate::sync::PreemptLock;
use crate::thread::{RawThread, ThreadExecutionState};

const IDLE_THREAD_NAME: &'static str = "[idle]";
const IDLE_THREAD_PRIO: ThreadPriority = 0;

#[cfg(not(feature = "khal-sim"))]
const IDLE_THREAD_STACK_SIZE: usize = 1024;

#[cfg(feature = "khal-sim")]
const IDLE_THREAD_STACK_SIZE: usize = 1024 * 16;

mod internal {
    #[allow(dead_code)]
    extern "Rust" {
        #[link_name = "_scars_idle_thread_hook"]
        pub(super) fn idle_thread_hook();
    }
}

extern "Rust" {
    #[cfg(not(test))]
    fn _start_main_thread();
}

#[allow(unused)]
#[inline(always)]
fn idle_thread_hook() {
    unsafe { internal::idle_thread_hook() }
}

#[export_name = "_scars_default_idle_thread_hook"]
fn default_idle_thread_hook() {
    crate::idle();
}

pub(crate) fn init_idle_thread() -> &'static RawThread {
    crate::printkln!("Init idle thread");
    let mut idle_thread = make_thread!(IDLE_THREAD_NAME, IDLE_THREAD_PRIO, IDLE_THREAD_STACK_SIZE);
    idle_thread.modify(|t| {
        PreemptLock::with(|pkey| {
            t.state.set(pkey, ThreadExecutionState::Running);
        })
        //t.state.set(ThreadExecutionState::Running);
    });
    let idle_thread = idle_thread.init(|| {
        #[cfg(not(test))]
        unsafe {
            _start_main_thread();
        }

        #[cfg(test)]
        {
            let test_thread = make_thread!("test", 1, IDLE_THREAD_STACK_SIZE, 1, executor = true);
            test_thread.start(|| {
                crate::test_main();
                crate::scars_test::test_succeed();
            });
        }

        loop {
            idle_thread_hook();
        }
    });

    unsafe { idle_thread.as_ref() }
}
