use crate::kernel::priority::TaskPriority;
use crate::kernel::task::{TaskControlBlock, TaskState};
use crate::make_task;

const IDLE_TASK_NAME: &'static str = "[idle]";
const IDLE_TASK_PRIO: TaskPriority = 0;

#[cfg(not(feature = "hal-std"))]
const IDLE_TASK_STACK_SIZE: usize = 1024;

#[cfg(feature = "hal-std")]
const IDLE_TASK_STACK_SIZE: usize = 1024 * 16;

mod internal {
    #[allow(dead_code)]
    extern "Rust" {
        #[link_name = "_scars_idle_task_hook"]
        pub(super) fn idle_task_hook();
    }
}

extern "Rust" {
    #[cfg(not(test))]
    fn _start_main_task();
}

#[allow(unused)]
#[inline(always)]
fn idle_task_hook() {
    unsafe { internal::idle_task_hook() }
}

#[export_name = "_scars_default_idle_task_hook"]
fn default_idle_task_hook() {
    crate::idle();
}

pub(crate) fn init_idle_task() -> &'static TaskControlBlock {
    let idle_task = make_task!(IDLE_TASK_NAME, IDLE_TASK_PRIO, IDLE_TASK_STACK_SIZE);
    idle_task.init(|| {
        #[cfg(not(test))]
        unsafe {
            _start_main_task();
        }

        #[cfg(test)]
        {
            let test_task = make_task!("test", 1, IDLE_TASK_STACK_SIZE);
            test_task.start(|| {
                crate::test_main();
                crate::scars_test::test_succeed();
            });
        }

        loop {
            idle_task_hook();
        }
    });

    let idle_tcb = idle_task.get_tcb();
    idle_tcb.state.set(TaskState::Running);
    idle_tcb
}
