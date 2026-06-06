//! Synchronization primitives.
//!
//! Low-level per-core lock primitives live in [`lock`] and are re-exported
//! here for convenience. Higher-level primitives — [`Mutex`], [`Channel`],
//! [`Condvar`], [`Once`], [`OnceLock`] — are built on top of
//! those locks.
//!
//! See [`lock`] for the distinction between nesting and scoped locks and
//! their key-token model.

//pub mod async_channel;
#[cfg(feature = "async")]
pub mod async_condvar;
//pub mod async_lock;
#[cfg(feature = "async")]
pub mod async_mutex;
#[cfg(feature = "multithreading")]
pub mod channel;
#[cfg(feature = "raii-locks")]
pub mod condvar;
pub mod fifo;
#[cfg(feature = "raii-locks")]
pub mod guarded;
pub mod lock;
#[cfg(feature = "raii-locks")]
pub mod mutex;
#[cfg(feature = "multithreading")]
pub mod notify;
pub mod once;
pub mod once_lock;
pub mod protected;
#[cfg(feature = "multithreading")]
pub mod rendezvous;
#[cfg(feature = "multithreading")]
pub mod semaphore;

//pub use async_channel::AsyncChannel;
#[cfg(feature = "async")]
pub use async_condvar::AsyncCondvar;
//pub use async_lock::AsyncLock;
#[cfg(feature = "async")]
pub use async_mutex::{AsyncMutex, AsyncMutexGuard};
#[cfg(feature = "raii-locks")]
pub use condvar::{CeilingCondvar, Condvar};
#[cfg(feature = "raii-locks")]
pub use guarded::{Guard, Guarded};
#[cfg(all(feature = "priority-inheritance", feature = "raii-locks"))]
pub use mutex::Mutex;
#[cfg(feature = "raii-locks")]
pub use mutex::{CeilingMutex, MutexGuard};
pub use once::Once;
pub use once_lock::OnceLock;
pub use protected::Protected;
#[cfg(feature = "multithreading")]
pub use {
    channel::{CeilingChannel, Channel},
    notify::Notify,
    protected::{BarrierResult, TimedOut, WaitMarker},
    semaphore::Semaphore,
};

pub use ::portable_atomic as atomic;

pub use critical_section::{self, CriticalSection};

pub use lock::{
    CeilingLock, CoreCeilingLock, CoreInterruptLock, CorePreemptLock, InterruptLock,
    InterruptLockKey, LockOps, LockResult, NestingLock, NoLock, PreemptLock, PreemptLockKey,
    RawCeilingLock, ScopedLock, SpinLock, TryLockError, TryLockResult, Unlock,
};
#[cfg(feature = "priority-inheritance")]
pub use lock::{CoreInheritanceLock, InheritanceLock};
