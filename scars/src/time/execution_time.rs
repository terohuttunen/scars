//! Public execution-time API.
//!
//! A thread's CPU clock is read through
//! [`ThreadRef::execution_time`](crate::thread::ThreadRef::execution_time).
//! [`interrupt_clock`] gives the aggregate time spent in interrupt handlers,
//! which is excluded from thread CPU time.

use crate::kernel::hal::CoreId;
use crate::time::Duration;

/// Aggregate time spent in interrupt handlers on the calling core. This time
/// is excluded from every thread's
/// [`execution_time`](crate::thread::ThreadRef::execution_time).
pub fn interrupt_clock() -> Duration {
    Duration::from_ticks(crate::kernel::execution_time::interrupt_clock_ticks(
        CoreId::current(),
    ))
}
