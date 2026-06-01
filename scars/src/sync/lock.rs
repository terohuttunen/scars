//! Lock primitives that underlie the higher-level synchronization types in
//! [`crate::sync`].
//!
//! All locks here are per-core: each core has its own nesting state and
//! ceiling, and a lock acquired on one core does not exclude another core.
//!
//! Two shapes of lock are provided:
//!
//! - [`NestingLock`]: locks released in reverse acquisition order on the
//!   current core. [`CoreInterruptLock`] masks all interrupts;
//!   [`CorePreemptLock`] prevents thread switching while leaving interrupts
//!   enabled.
//! - [`ScopedLock`]: per-resource locks whose guards may be dropped in any
//!   order. [`CeilingLock`] raises the active priority of the current core to
//!   the lock's ceiling so that threads or interrupts below the ceiling cannot
//!   run while the guard is held.
//!
//! Each lock has an associated key token, used to access locked data within
//! the [`LockedCell`](crate::cell::LockedCell) and
//! [`LockedRefCell`](crate::cell::LockedRefCell) types.

pub mod ceiling_lock;
#[cfg(feature = "priority-inheritance")]
pub mod inheritance_lock;
pub mod interrupt_lock;
pub mod no_lock;
pub mod preempt_lock;
pub mod spinlock;

pub use ceiling_lock::{CeilingLock, CoreCeilingLock, RawCeilingLock};
#[cfg(feature = "priority-inheritance")]
pub use inheritance_lock::{CoreInheritanceLock, InheritanceLock};
pub use interrupt_lock::{CoreInterruptLock, InterruptLock, InterruptLockKey};
pub use no_lock::NoLock;
pub use preempt_lock::{CorePreemptLock, PreemptLock, PreemptLockKey};
pub use spinlock::SpinLock;

pub type LockResult<Guard> = Result<Guard, ()>;
pub type TryLockResult<Guard> = Result<Guard, TryLockError>;

pub enum TryLockError {
    WouldBlock,
}

// A Guard may or may not implement this trait, depending on the lock type.
pub trait Unlock {
    unsafe fn unlock(&mut self);

    fn relock(&mut self);
}

pub trait LockOps {
    // RAII-style lock guard that releases the lock when dropped. Guards
    // may be dropped in any order.
    type Guard<'lock>
    where
        Self: 'lock;

    fn lock(&self) -> Self::Guard<'_>;

    fn try_lock(&self) -> Result<Self::Guard<'_>, TryLockError>;

    fn get_key<'guard, 'lock: 'guard>(_guard: &'guard Self::Guard<'lock>) -> Self::Key<'guard>
    where
        Self: NestingLock,
    {
        unsafe { Self::get_key_unchecked() }
    }
}

pub trait ScopedLock: LockOps {
    // Const initializer.
    const DEFAULT: Self;
}

// A lock that can be acquired and released in a nested fashion. The lock is released
// in reverse order of acquisition.
pub trait NestingLock {
    type Key<'guard>: Copy;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R;

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError>;

    // Upcast a key from a longer lifetime to a shorter one.
    //
    // Needed because there is no way to control the variance of the associated
    // key type lifetime. Keys are covariant, i.e. Key<'long> can be used as Key<'short>.
    fn upcast_key<'short, 'long: 'short>(_key: Self::Key<'long>) -> Self::Key<'short> {
        unsafe { Self::get_key_unchecked() }
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a>;

    fn required_ceiling() -> Option<i16> {
        None
    }
}
