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
pub mod condvar;
pub mod lock;
pub mod mutex;
pub mod notify;
pub mod once;
pub mod once_lock;
pub mod protected;
pub mod rendezvous;
pub mod shared;

//pub use async_channel::AsyncChannel;
//pub use async_condvar::AsyncCondvar;
//pub use async_lock::AsyncLock;
//pub use async_mutex::{AsyncMutex, AsyncMutexGuard};
pub use channel::{CeilingChannel, Channel};
pub use condvar::{CeilingCondvar, Condvar};
pub use notify::Notify;
pub use once::Once;
pub use once_lock::OnceLock;
pub use protected::{BarrierResult, Protected, TimedOut, WaitMarker};
pub use semaphore::Semaphore;
pub use shared::Shared;

pub use ::portable_atomic as atomic;

pub use critical_section::{self, CriticalSection};

pub use lock::{
    CeilingLock, CoreCeilingLock, CoreInheritanceLock, CoreInterruptLock, CorePreemptLock,
    InheritanceLock, InterruptLock, InterruptLockKey, LockOps, LockResult, NestingLock, NoLock,
    PreemptLock, PreemptLockKey, RawCeilingLock, ScopedLock, TryLockError, TryLockResult, Unlock,
};
