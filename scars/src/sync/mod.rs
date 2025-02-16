//! SCARS provides three kinds of global locks, and a one priority based per-resource
//! lock. The global locks provide sections where the lock is acquired, and force release
//! of nested locks in reverse order as they were acquired.
//!
//! - `InterruptLock`: Critical section that blocks all interrupts.
//! - `PreemptLock`: Prevents thread switching, but allows interrupts.
//! - `CeilingLock`: Prevents thread switching or interrupts below the lock ceiling, and
//!    allows anything above the ceiling.
//!
//! In addition to global lock, the `CeilingLock` also provides the priority based per-resource
//! lock that has the semantics of a Rust Mutex. Nested per-resource locks can be released
//! in an arbitrary order.
//!
//! The scheduler will always raise the active priority of a thread or an interrupt at
//! the highest priority of any locks acquired at any given time. Threads or interrupts
//! below the ceiling are not allowed to run while higher priority locks are held.
//!
//! Each lock has an associated key token, which can be used to access locked data within
//! provided `LockedCell` and `LockedRefCell` types.
//pub mod async_channel;
//pub mod async_condvar;
//pub mod async_lock;
//pub mod async_mutex;
pub mod ceiling_lock;
pub mod channel;
pub mod condvar;
pub mod interrupt_lock;
pub mod mutex;
pub mod no_lock;
pub mod once;
pub mod once_lock;
pub mod preempt_lock;
pub mod rendezvous;
pub mod shared;

//pub use async_channel::AsyncChannel;
//pub use async_condvar::AsyncCondvar;
//pub use async_lock::AsyncLock;
//pub use async_mutex::{AsyncMutex, AsyncMutexGuard};
pub use channel::Channel;
pub use condvar::Condvar;
pub use mutex::{Mutex, MutexGuard};
pub use once::Once;
pub use once_lock::OnceLock;
pub use shared::Shared;

pub use ::core::sync::atomic;

pub use ceiling_lock::{CeilingLock, RawCeilingLock};
pub use critical_section::{self, CriticalSection};
pub use interrupt_lock::InterruptLock;
pub use no_lock::NoLock;
pub use preempt_lock::PreemptLock;
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

pub trait ScopedLock {
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

// A lock that can be acquired and released in a nested fashion. The lock is released
// in reverse order of acquisition.
pub trait NestingLock {
    type Key<'guard>: Copy;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R;

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError>;

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a>;
}
