//! Application Programming Interface
pub use crate::kernel::hal::{breakpoint, idle};
pub use crate::kernel::interrupt::in_interrupt;
pub use crate::kernel::syscall;
use crate::thread::RawThread;
pub use crate::time::{Duration, Instant};

#[allow(dead_code)]
pub fn thread_start(thread: &mut RawThread) {
    syscall::start_thread(thread)
}

#[allow(dead_code)]
pub fn thread_yield() {
    syscall::thread_yield()
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
pub fn thread_suspend(thread: Option<&RawThread>) {
    syscall::thread_suspend(thread)
}
