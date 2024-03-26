//! Application Programming Interface
pub use crate::kernel::hal::{breakpoint, idle};
pub use crate::kernel::interrupt::in_interrupt;
pub(crate) use crate::kernel::list::LinkedList;
pub use crate::kernel::priority::AnyPriority;
use crate::kernel::scheduler::Scheduler;
use crate::kernel::task::TaskControlBlock;
pub use crate::kernel::wait_queue::WaitQueueTag;
pub use crate::kernel::{syscall, task::TaskRef};
pub use crate::time::{Duration, Instant};

#[allow(dead_code)]
pub fn task_start(task: &mut TaskControlBlock) {
    syscall::start_task(task)
}

#[allow(dead_code)]
pub fn task_yield() {
    syscall::task_yield()
}

#[allow(dead_code)]
pub(crate) fn task_wait(
    wait_list: *mut LinkedList<TaskControlBlock, WaitQueueTag>,
    ceiling: AnyPriority,
) {
    syscall::task_wait(wait_list, ceiling)
}

#[allow(dead_code)]
pub fn delay(duration: crate::time::Duration) {
    syscall::delay(duration)
}

#[allow(dead_code)]
pub fn delay_until(time: Instant) {
    syscall::delay_until(time)
}

#[allow(dead_code)]
pub fn current_task() -> TaskRef {
    TaskRef::new(Scheduler::current_task())
}
