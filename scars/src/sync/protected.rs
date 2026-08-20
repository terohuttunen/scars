//! Closure-scoped exclusive access to a wrapped value.
//!
//! [`Protected<T, L>`] owns a `T` and exposes it only inside a closure
//! passed to [`Protected::with`] or [`Protected::with_key`]. Mutual
//! exclusion is provided by acquiring `L` for the duration of the
//! closure. Same-task re-entry raises
//! [`RuntimeError::RecursiveLock`].
//!
//! ```ignore
//! use scars::prelude::*;
//! use scars::sync::{CoreCeilingLock, Protected};
//!
//! const CEILING: Priority = Priority::thread(5);
//!
//! static COUNTER: Protected<u32, CoreCeilingLock<CEILING>> = Protected::new(0);
//!
//! COUNTER.with(|_, c| *c += 1);
//! let v = COUNTER.with(|_, c| *c);
//! ```

use crate::cell::LockedCell;
#[cfg(feature = "multithreading")]
use crate::kernel::scheduler::{RESCHEDULE_KIND_BLOCK_CURRENT, Scheduler};
use crate::runtime_error;
use crate::sync::{NestingLock, TryLockError};
use core::cell::UnsafeCell;
use core::pin::Pin;

pub struct Protected<T, L: NestingLock> {
    inner: UnsafeCell<T>,
    in_use: LockedCell<bool, L>,
}

impl<T, L: NestingLock> Protected<T, L> {
    /// Create a `Protected` wrapping `value`.
    pub const fn new(value: T) -> Self {
        Self {
            inner: UnsafeCell::new(value),
            in_use: LockedCell::new(false),
        }
    }

    /// Run `f` with exclusive access to the wrapped value, using a key
    /// the caller already holds.
    pub fn with_key<R>(&self, key: L::Key<'_>, f: impl FnOnce(L::Key<'_>, &mut T) -> R) -> R {
        if self.in_use.replace(key, true) {
            runtime_error!(RuntimeError::RecursiveLock);
        }
        // SAFETY: `key` witnesses `L` is held on this core; `in_use`
        // rejects same-task re-entry; the `&mut T` lifetime is bound
        // to the closure body.
        let t = unsafe { &mut *self.inner.get() };
        let r = f(key, t);
        self.in_use.set(key, false);
        r
    }

    /// Acquire `L`, run `f` with exclusive access, release.
    pub fn with<R>(&self, f: impl FnOnce(L::Key<'_>, &mut T) -> R) -> R {
        L::with(|key| self.with_key(key, f))
    }

    /// Raw pointer to the wrapped value, bypassing the lock and the
    /// re-entry guard. The caller is responsible for synchronization.
    pub(crate) fn as_ptr(&self) -> *mut T {
        self.inner.get()
    }

    /// Try to acquire `L` without blocking and run `f`. Returns
    /// `Err(TryLockError::WouldBlock)` if `L` is unavailable or the
    /// calling task is already inside this `Protected`.
    pub fn try_with<R>(&self, f: impl FnOnce(L::Key<'_>, &mut T) -> R) -> Result<R, TryLockError> {
        L::try_with(|key| {
            if self.in_use.replace(key, true) {
                Err(TryLockError::WouldBlock)
            } else {
                // SAFETY: as in `with_key`.
                let t = unsafe { &mut *self.inner.get() };
                let r = f(key, t);
                self.in_use.set(key, false);
                Ok(r)
            }
        })?
    }

    /// Like [`with_pin_key`](Self::with_pin_key), but returns
    /// `Err(TryLockError::WouldBlock)` instead of raising
    /// `RecursiveLock` when the value is already borrowed. For kernel
    /// paths that may legitimately be reached while the value is in
    /// use and can skip their work in that case.
    pub(crate) fn try_with_pin_key<R>(
        self: Pin<&Self>,
        key: L::Key<'_>,
        f: impl FnOnce(L::Key<'_>, Pin<&mut T>) -> R,
    ) -> Result<R, TryLockError> {
        if self.in_use.replace(key, true) {
            return Err(TryLockError::WouldBlock);
        }
        // SAFETY: as in `with_pin_key`.
        let t = unsafe { Pin::new_unchecked(&mut *self.inner.get()) };
        let r = f(key, t);
        self.in_use.set(key, false);
        Ok(r)
    }

    /// Run `f` with pinned exclusive access to the wrapped value,
    /// using a key the caller already holds. The pin receiver
    /// witnesses that `Protected` is pinned in place; `T` is declared
    /// structurally pinned in `Protected`.
    pub fn with_pin_key<R>(
        self: Pin<&Self>,
        key: L::Key<'_>,
        f: impl FnOnce(L::Key<'_>, Pin<&mut T>) -> R,
    ) -> R {
        if self.in_use.replace(key, true) {
            runtime_error!(RuntimeError::RecursiveLock);
        }
        // SAFETY: `T` is structurally pinned in `Protected` — the
        // wrapper never moves it, the `UnsafeCell` shares the wrapper's
        // storage, and the wrapper itself is pinned via the receiver.
        // `in_use` rejects same-task re-entry; the `Pin<&mut T>`'s
        // lifetime is bound to the closure body.
        let t = unsafe { Pin::new_unchecked(&mut *self.inner.get()) };
        let r = f(key, t);
        self.in_use.set(key, false);
        r
    }

    /// Acquire `L`, run `f` with pinned exclusive access, release.
    pub fn with_pin<R>(self: Pin<&Self>, f: impl FnOnce(L::Key<'_>, Pin<&mut T>) -> R) -> R {
        L::with(|key| self.with_pin_key(key, f))
    }

    /// Try to acquire `L` and run `f` with pinned exclusive access.
    /// Returns `Err(TryLockError::WouldBlock)` if `L` is unavailable
    /// or the calling task is already inside this `Protected`.
    pub fn try_with_pin<R>(
        self: Pin<&Self>,
        f: impl FnOnce(L::Key<'_>, Pin<&mut T>) -> R,
    ) -> Result<R, TryLockError> {
        L::try_with(|key| {
            if self.in_use.replace(key, true) {
                Err(TryLockError::WouldBlock)
            } else {
                // SAFETY: as in `with_pin_key`.
                let t = unsafe { Pin::new_unchecked(&mut *self.inner.get()) };
                let r = f(key, t);
                self.in_use.set(key, false);
                Ok(r)
            }
        })?
    }

    /// [`try_with_pin`](Self::try_with_pin) with the kernel-drain
    /// admission rule of [`NestingLock::kernel_try_with`]. The
    /// `in_use` check is what rejects a wait list whose holder was
    /// preempted mid-closure.
    pub(crate) fn kernel_try_with_pin<R>(
        self: Pin<&Self>,
        f: impl FnOnce(L::Key<'_>, Pin<&mut T>) -> R,
    ) -> Result<R, TryLockError> {
        L::kernel_try_with(|key| {
            if self.in_use.replace(key, true) {
                Err(TryLockError::WouldBlock)
            } else {
                // SAFETY: as in `with_pin_key`.
                let t = unsafe { Pin::new_unchecked(&mut *self.inner.get()) };
                let r = f(key, t);
                self.in_use.set(key, false);
                Ok(r)
            }
        })?
    }
}

// Block-and-retry barrier helpers.
#[cfg(feature = "multithreading")]
impl<T, L: NestingLock> Protected<T, L> {
    /// Run `f` repeatedly under `with` until it returns
    /// [`BarrierResult::Done`]. On [`BarrierResult::Wait`] the calling
    /// thread suspends until notified, then the loop retries.
    pub fn with_barrier<R, F>(&self, mut f: F) -> R
    where
        F: FnMut(L::Key<'_>, &mut T) -> BarrierResult<R>,
    {
        loop {
            match self.with(|key, t| f(key, t)) {
                BarrierResult::Done(r) => return r,
                BarrierResult::Wait(_marker) => {
                    // L is released here. Pend the block *outside* the
                    // closure so the lock's ceiling threshold (if any)
                    // drops before `block_current` runs.
                    Scheduler::set_pending_reschedule(RESCHEDULE_KIND_BLOCK_CURRENT);
                }
            }
            // Wake; loop and re-run the closure.
        }
    }

    /// Like [`with_barrier`](Self::with_barrier) but bounded by
    /// `deadline`. Returns `Err(TimedOut)` if `deadline` elapses
    /// before the closure returns [`BarrierResult::Done`].
    pub fn with_barrier_until<R, F>(
        &self,
        deadline: crate::time::Instant,
        mut f: F,
    ) -> Result<R, TimedOut>
    where
        F: FnMut(L::Key<'_>, &mut T) -> BarrierResult<R>,
    {
        loop {
            match self.with(|key, t| f(key, t)) {
                BarrierResult::Done(r) => return Ok(r),
                BarrierResult::Wait(_marker) => {
                    // L released; write the deadline + pend the kind
                    // outside the closure.
                    Scheduler::set_current_pending_block_deadline(Some(deadline));
                    Scheduler::set_pending_reschedule(RESCHEDULE_KIND_BLOCK_CURRENT);
                }
            }
            Scheduler::take_last_wait_timed_out()?;
        }
    }
}

/// Witness that the current thread has armed a wait inside a
/// `Protected::with_barrier{,_until}` closure. Only constructible
/// inside this crate, so user code cannot fabricate
/// [`BarrierResult::Wait`] without going through a primitive like
/// [`Notify::wait`](crate::sync::Notify::wait) that pairs construction
/// with enqueueing the thread.
#[cfg(feature = "multithreading")]
pub struct WaitMarker {
    _seal: (),
}

#[cfg(feature = "multithreading")]
impl WaitMarker {
    #[inline]
    pub(crate) fn new() -> Self {
        WaitMarker { _seal: () }
    }
}

/// Outcome of a [`Protected::with_barrier`] closure iteration.
#[cfg(feature = "multithreading")]
pub enum BarrierResult<R> {
    /// Return this value from `with_barrier`.
    Done(R),
    /// Suspend the calling thread; on wake, retry the closure.
    Wait(WaitMarker),
}

#[cfg(feature = "multithreading")]
pub use crate::kernel::scheduler::TimedOut;

unsafe impl<T: Send, L: NestingLock> Send for Protected<T, L> {}
unsafe impl<T: Send, L: NestingLock + Sync> Sync for Protected<T, L> {}
