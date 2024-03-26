//! SCARS provides three kinds of global locks, and a one priority based per-resource
//! lock. The global locks provide sections where the lock is acquired, and force release
//! of nested locks in reverse order as they were acquired.
//!
//! - `InterruptLock`: Critical section that blocks all interrupts.
//! - `PreemptLock`: Prevents task switching, but allows interrupts.
//! - `CeilingLock`: Prevents task switching or interrupts below the lock ceiling, and
//!    allows anything above the ceiling.
//!
//! In addition to global lock, the `CeilingLock` also provides the priority based per-resource
//! lock that has the semantics of a Rust Mutex. Nested per-resource locks can be released
//! in an arbitrary order.
//!
//! The scheduler will always raise the active priority of a task or an interrupt at
//! the highest priority of any locks acquired at any given time. Tasks or interrupts
//! below the ceiling are not allowed to run while higher priority locks are held.
//!
//! Each lock has an associated key token, which can be used to access locked data within
//! provided `LockedCell` and `LockedRefCell` types.
pub mod ceiling_lock;
pub mod channel;
pub mod condvar;
pub mod interrupt_lock;
pub mod mutex;
pub mod preempt_lock;
pub mod rendezvous;
pub mod shared;

pub use channel::Channel;
pub use condvar::Condvar;
pub use mutex::{Mutex, MutexGuard};
pub use shared::Shared;

pub use ::core::sync::atomic;

pub use ceiling_lock::{CeilingLock, RawCeilingLock};
pub use critical_section::{self, CriticalSection};
pub use interrupt_lock::InterruptLock;
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

// Interleaved locking sequence
pub fn atomic_pair_lock<L1: Lock, L2: Lock, R, P, Q>(
    f: impl FnOnce(L1::Key<'_>) -> R,
    fg: impl FnOnce(L1::Key<'_>, L2::Key<'_>, R) -> P,
    g: impl FnOnce(L2::Key<'_>, P) -> Q,
) -> Q {
    let restore_state_lock1 = unsafe { L1::section_start() };

    let key_lock1 = unsafe { L1::Key::new() };
    let r = f(key_lock1);

    // TODO: if L1 and L2 are equal, this code should be let restore_state_lock2 = restore_state_lock1;
    let restore_state_lock2 = unsafe { L2::section_start() };
    let key_lock2 = unsafe { L2::Key::new() };
    let p = fg(key_lock1, key_lock2, r);
    unsafe { L1::section_end(restore_state_lock1) };

    let q = g(key_lock2, p);

    unsafe { L2::section_end(restore_state_lock2) };
    q
}

pub fn atomic_triple_lock<L1: Lock, L2: Lock, L3: Lock, R, P, Q>(
    f: impl FnOnce(L1::Key<'_>) -> R,
    g: impl FnOnce(L2::Key<'_>, R) -> P,
    h: impl FnOnce(L3::Key<'_>, P) -> Q,
) -> Q {
    let restore_state_lock1 = unsafe { L1::section_start() };

    let key_lock1 = unsafe { L1::Key::new() };
    let r = f(key_lock1);

    // TODO: if L1 and L2 are equal, this code should be let restore_state_lock2 = restore_state_lock1;
    let restore_state_lock2 = unsafe { L2::section_start() };
    unsafe { L1::section_end(restore_state_lock1) }

    let key_lock2 = unsafe { L2::Key::new() };
    let p = g(key_lock2, r);

    // TODO: if L1 and L2 are equal, this code should be let restore_state_lock3 = restore_state_lock2;
    let restore_state_lock3 = unsafe { L3::section_start() };
    unsafe { L2::section_end(restore_state_lock2) }

    let key_lock3 = unsafe { L3::Key::new() };
    let q = h(key_lock3, p);

    unsafe { L3::section_end(restore_state_lock3) }
    q
}

pub fn atomic_pair_try_lock<L1: Lock, L2: Lock, R, P, Q>(
    f: impl FnOnce(L1::Key<'_>) -> R,
    fg: impl FnOnce(L1::Key<'_>, L2::Key<'_>, R) -> P,
    g: impl FnOnce(L2::Key<'_>, P) -> Q,
) -> Result<Q, TryLockError> {
    let restore_state_lock1 = unsafe { L1::try_section_start()? };

    let key_lock1 = unsafe { L1::Key::new() };
    let r = f(key_lock1);

    match unsafe { L2::try_section_start() } {
        Ok(restore_state_lock2) => {
            let key_lock2 = unsafe { L2::Key::new() };
            let p = fg(key_lock1, key_lock2, r);
            unsafe { L1::section_end(restore_state_lock1) };

            let q = g(key_lock2, p);

            unsafe { L2::section_end(restore_state_lock2) };
            Ok(q)
        }
        Err(err) => {
            unsafe { L1::section_end(restore_state_lock1) };
            Err(err)
        }
    }
}
