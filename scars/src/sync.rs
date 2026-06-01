//! Synchronization primitives.
//!
//! Low-level per-core lock primitives live in [`lock`] and are re-exported
//! here for convenience. Higher-level primitives — [`Mutex`], [`Channel`],
//! [`Condvar`], [`Once`], [`OnceLock`], [`Shared`] — are built on top of
//! those locks.
//!
//! See [`lock`] for the distinction between nesting and scoped locks and
//! their key-token model.

//pub mod async_channel;
//pub mod async_condvar;
//pub mod async_lock;
//pub mod async_mutex;
pub mod channel;
#[cfg(feature = "raii-locks")]
pub mod condvar;
#[cfg(feature = "raii-locks")]
pub mod guarded;
pub mod lock;
#[cfg(feature = "raii-locks")]
pub mod mutex;
pub mod notify;
pub mod once;
pub mod once_lock;
pub mod protected;
pub mod rendezvous;
pub mod semaphore;
#[cfg(feature = "raii-locks")]
pub mod shared;

//pub use async_channel::AsyncChannel;
//pub use async_condvar::AsyncCondvar;
//pub use async_lock::AsyncLock;
//pub use async_mutex::{AsyncMutex, AsyncMutexGuard};
pub use channel::{CeilingChannel, Channel};
#[cfg(feature = "raii-locks")]
pub use condvar::{CeilingCondvar, Condvar};
#[cfg(feature = "raii-locks")]
pub use guarded::{Guard, Guarded};
#[cfg(all(feature = "priority-inheritance", feature = "raii-locks"))]
pub use mutex::Mutex;
#[cfg(feature = "raii-locks")]
pub use mutex::{CeilingMutex, MutexGuard};
pub use notify::Notify;
pub use once::Once;
pub use once_lock::OnceLock;
pub use protected::{BarrierResult, Protected, TimedOut, WaitMarker};
pub use semaphore::Semaphore;
#[cfg(feature = "raii-locks")]
pub use shared::Shared;

pub use ::portable_atomic as atomic;

pub use critical_section::{self, CriticalSection};

pub use lock::{
    CeilingLock, CoreCeilingLock, CoreInterruptLock, CorePreemptLock, InterruptLock,
    InterruptLockKey, LockOps, LockResult, NestingLock, NoLock, PreemptLock, PreemptLockKey,
    RawCeilingLock, ScopedLock, SpinLock, TryLockError, TryLockResult, Unlock,
};
#[cfg(feature = "priority-inheritance")]
pub use lock::{CoreInheritanceLock, InheritanceLock};
