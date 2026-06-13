//! Application Programming Interface
pub use crate::interrupt::in_interrupt;
pub use crate::kernel::hal::{breakpoint, idle};
pub use crate::kernel::syscall;
pub use crate::time::{Duration, Instant};

#[cfg(feature = "multithreading")]
#[allow(dead_code)]
pub fn thread_yield() {
    syscall::thread_yield()
}

#[cfg(feature = "multithreading")]
#[allow(dead_code)]
pub fn delay(duration: crate::time::Duration) {
    syscall::delay(duration)
}

#[cfg(feature = "multithreading")]
#[allow(dead_code)]
pub fn delay_until(time: Instant) {
    syscall::delay_until(time)
}
