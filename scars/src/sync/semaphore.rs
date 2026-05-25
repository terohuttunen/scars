//! Counting semaphore.
//!
//! ```ignore
//! use scars::prelude::*;
//! use scars::sync::{CoreCeilingLock, Semaphore};
//!
//! const CEILING: Priority = Priority::thread(5);
//!
//! static SLOTS: Semaphore<CoreCeilingLock<CEILING>> = Semaphore::new(0);
//!
//! // Producer
//! SLOTS.release();
//!
//! // Consumer (blocks until a producer releases)
//! SLOTS.acquire();
//! ```

use crate::sync::{BarrierResult, NestingLock, Notify, Protected, TimedOut};
use crate::time::Instant;

pub struct Semaphore<L: NestingLock> {
    count: Protected<u32, L>,
    notify: Notify<L>,
}

impl<L: NestingLock> Semaphore<L> {
    /// Create a semaphore with the given initial count.
    pub const fn new(initial: u32) -> Semaphore<L> {
        Semaphore {
            count: Protected::new(initial),
            notify: Notify::new(),
        }
    }

    /// Decrement the count, blocking until it is positive.
    pub fn acquire(&'static self) {
        self.count.with_barrier(|key, c| {
            if *c > 0 {
                *c -= 1;
                BarrierResult::Done(())
            } else {
                BarrierResult::Wait(self.notify.arm(key))
            }
        })
    }

    /// Like [`acquire`](Self::acquire), but bounded by `deadline`.
    /// Returns `Err(TimedOut)` if the deadline elapses before the
    /// count becomes positive.
    pub fn acquire_until(&'static self, deadline: Instant) -> Result<(), TimedOut> {
        self.count.with_barrier_until(deadline, |key, c| {
            if *c > 0 {
                *c -= 1;
                BarrierResult::Done(())
            } else {
                BarrierResult::Wait(self.notify.arm(key))
            }
        })
    }

    /// Decrement the count if positive; return `true` on success,
    /// `false` if the count is zero.
    pub fn try_acquire(&self) -> bool {
        self.count.with(|_, c| {
            if *c > 0 {
                *c -= 1;
                true
            } else {
                false
            }
        })
    }

    /// Increment the count, saturating at `u32::MAX`, and wake one
    /// waiter if any.
    pub fn release(&self) {
        self.count.with(|_, c| {
            *c = c.saturating_add(1);
            self.notify.notify_one();
        });
    }

    /// Current count.
    pub fn available(&self) -> u32 {
        self.count.with(|_, c| *c)
    }
}

unsafe impl<L: NestingLock + Sync> Sync for Semaphore<L> {}
