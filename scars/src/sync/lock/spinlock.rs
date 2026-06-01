//! Spinlock for cross-core shared state.
//!
//! `SpinLock` protects state that lives outside any per-core partition.
//! It is not usable across partitions of an IPCP-style design: IPCP
//! guarantees mutual exclusion within one core only, so any state that
//! a second core can also touch needs a primitive that does not depend
//! on per-core interrupt thresholds.
//!
//! Acquisition disables interrupts on the calling core and busy-waits
//! on an atomic flag. The combination keeps the lock holder from being
//! preempted (no interrupt on this core can re-enter the same lock) and
//! prevents other cores from observing partial state.

use super::{LockOps, ScopedLock, TryLockError, TryLockResult, Unlock};
use crate::kernel::hal::{acquire, restore};
use crate::sync::atomic::{AtomicBool, Ordering};
use core::marker::PhantomData;

pub struct SpinLock {
    locked: AtomicBool,
}

pub struct SpinLockGuard<'a> {
    lock: &'a SpinLock,
    saved: bool,
    _phantom: PhantomData<*const ()>,
}

impl SpinLock {
    pub const fn new() -> Self {
        SpinLock {
            locked: AtomicBool::new(false),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_> {
        let saved = acquire();
        while self.locked.swap(true, Ordering::Acquire) {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinLockGuard {
            lock: self,
            saved,
            _phantom: PhantomData,
        }
    }

    pub fn try_lock(&self) -> TryLockResult<SpinLockGuard<'_>> {
        let saved = acquire();
        if self.locked.swap(true, Ordering::Acquire) {
            restore(saved);
            Err(TryLockError::WouldBlock)
        } else {
            Ok(SpinLockGuard {
                lock: self,
                saved,
                _phantom: PhantomData,
            })
        }
    }
}

// `SpinLock` is `{ AtomicBool }`, so it derives `Send + Sync` on its own.

impl LockOps for SpinLock {
    type Guard<'lock> = SpinLockGuard<'lock>;

    fn lock(&self) -> Self::Guard<'_> {
        self.lock()
    }

    fn try_lock(&self) -> TryLockResult<Self::Guard<'_>> {
        self.try_lock()
    }
}

impl ScopedLock for SpinLock {
    const DEFAULT: Self = Self::new();
}

impl Drop for SpinLockGuard<'_> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        restore(self.saved);
    }
}

impl Unlock for SpinLockGuard<'_> {
    unsafe fn unlock(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        restore(self.saved);
    }

    fn relock(&mut self) {
        self.saved = acquire();
        while self.lock.locked.swap(true, Ordering::Acquire) {
            while self.lock.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }
}
