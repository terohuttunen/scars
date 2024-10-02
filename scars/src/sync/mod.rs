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

pub trait KeyToken<'a>: Copy {
    unsafe fn new() -> Self
    where
        Self: Sized;
}

pub trait Lock {
    type RestoreState;
    type Key<'a>: KeyToken<'a>;

    // SAFETY: there must always be a corresponding section_end() call
    // in exactly reverse order to section starts.
    unsafe fn section_start() -> Self::RestoreState;

    unsafe fn try_section_start() -> Result<Self::RestoreState, TryLockError>;

    // SAFETY: there must always be a corresponding section_start() call
    // in exactly reverse order to section ends.
    unsafe fn section_end(restore_state: Self::RestoreState);

    // Nested locks
    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R
    where
        Self: Sized,
    {
        let restore_state = unsafe { Self::section_start() };
        let key = unsafe { Self::Key::new() };
        let rval = f(key);
        unsafe { Self::section_end(restore_state) };
        rval
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        match unsafe { Self::try_section_start() } {
            Ok(restore_state) => {
                let key = unsafe { Self::Key::new() };
                let rval = f(key);
                unsafe { Self::section_end(restore_state) };
                Ok(rval)
            }
            Err(err) => Err(err),
        }
    }
}
