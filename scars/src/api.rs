//! Application Programming Interface
pub use crate::kernel::hal::{breakpoint, idle};
pub use crate::kernel::interrupt::in_interrupt;
use crate::kernel::scheduler::Scheduler;
use crate::kernel::task::TaskControlBlock;
pub use crate::kernel::{syscall, task::TaskRef};
pub use crate::time::{Duration, Instant};

use crate::sync::InterruptLock;

#[allow(dead_code)]
pub fn task_start(task: &mut TaskControlBlock) {
    InterruptLock::with(|ikey| syscall::start_task(ikey, task))
}

#[allow(dead_code)]
pub fn task_yield() {
    InterruptLock::with(|ikey| syscall::task_yield(ikey))
}

#[allow(dead_code)]
pub(crate) fn task_wait() {
    InterruptLock::with(|ikey| syscall::task_wait(ikey))
}

#[allow(dead_code)]
pub fn delay(duration: crate::time::Duration) {
    InterruptLock::with(|ikey| syscall::delay(ikey, duration))
}

#[allow(dead_code)]
pub fn delay_until(time: Instant) {
    InterruptLock::with(|ikey| syscall::delay_until(ikey, time))
}

#[allow(dead_code)]
pub fn current_task() -> TaskRef {
    TaskRef::new(Scheduler::current_task())
}
