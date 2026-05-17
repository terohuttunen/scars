//! Rendezvous-style synchronization: a synchronized remote procedure
//! call between two threads.
//!
//! A rendezvous splits into two halves via `split()`. The caller side
//! ([`LockedEntry::entry`]) hands an argument to the callee side and
//! blocks until the callee returns a value. The callee side
//! ([`LockedAccept::accept`]) blocks until an argument arrives, runs a
//! closure on it, and stores the closure's return value for the
//! caller side to pick up.
//!
//! Use [`make_rendezvous!`] to construct a `(LockedEntry,
//! LockedAccept)` pair. With a `ceiling` argument the underlying
//! mutexes use [`CeilingLock`]; without it they use [`InheritanceLock`]
//! plus [`CorePreemptLock`].
//!
//! ```ignore
//! use scars::sync::rendezvous::make_rendezvous;
//!
//! let (entry, accept) = make_rendezvous!(3);
//!
//! // Move `entry` into a thread that will issue the call:
//! let join = some_thread.spawn(move || {
//!     let result = entry.entry(42);
//!     scars::printkln!("Result: {}", result);
//! });
//!
//! // Service the call in this thread:
//! accept.accept(|arg| arg * 2);
//! ```

use crate::Priority;
use crate::kernel::hal::CoreId;
use crate::sync::{
    CoreCeilingLock, CoreInheritanceLock, CorePreemptLock, LockOps, NestingLock, ScopedLock,
    Unlock, condvar::LockedCondvar, mutex::Locked,
};

/// Allocates a static `Rendezvous` and returns a `(LockedEntry,
/// LockedAccept)` pair. With a `$prio` argument the rendezvous uses
/// [`CeilingLock<$prio>`]; without one it uses [`InheritanceLock`]
/// plus [`CorePreemptLock`].
#[macro_export]
macro_rules! make_rendezvous {
    ($prio:expr) => {{
        type A = impl ::core::marker::Sized + ::core::marker::Send + 'static;
        type R = impl ::core::marker::Sized + ::core::marker::Send + 'static;
        static mut RENDEZVOUS: $crate::sync::rendezvous::CeilingRendezvous<A, R, { $prio }> =
            $crate::sync::rendezvous::CeilingRendezvous::new();

        unsafe { RENDEZVOUS.split() }
    }};
    () => {{
        type A = impl ::core::marker::Sized + ::core::marker::Send + 'static;
        type R = impl ::core::marker::Sized + ::core::marker::Send + 'static;
        static mut RENDEZVOUS: $crate::sync::rendezvous::Rendezvous<A, R> =
            $crate::sync::rendezvous::Rendezvous::new();

        unsafe { RENDEZVOUS.split() }
    }};
}

pub use make_rendezvous;

pub type CeilingRendezvous<
    A,
    R,
    const CEILING: Priority,
    const CORE: CoreId = { CoreId::DEFAULT },
> = LockedRendezvous<A, R, CoreCeilingLock<CEILING, CORE>, CoreCeilingLock<CEILING, CORE>>;

pub type Rendezvous<A, R, const CORE: CoreId = { CoreId::DEFAULT }> =
    LockedRendezvous<A, R, CoreInheritanceLock<CORE>, CorePreemptLock<CORE>>;

/// Two-thread RPC channel parameterised over its mutex lock type `L`
/// and the nesting-lock kind `N` used by the internal condvar. Use
/// the [`Rendezvous`] / [`CeilingRendezvous`] aliases for the
/// supported configurations.
pub struct LockedRendezvous<A, R, L: LockOps, N: NestingLock>
where
    A: Send + 'static,
    R: Send + 'static,
{
    arg: Locked<Option<A>, L>,
    result: Locked<Option<R>, L>,
    waiter: LockedCondvar<N>,
}

impl<A, R, L: ScopedLock, N: NestingLock> LockedRendezvous<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
{
    pub const fn new() -> LockedRendezvous<A, R, L, N> {
        LockedRendezvous {
            arg: Locked::new(None),
            result: Locked::new(None),
            waiter: LockedCondvar::new(),
        }
    }

    /// Splits the rendezvous into the caller-side and callee-side
    /// halves.
    pub const fn split(&'static mut self) -> (LockedEntry<A, R, L, N>, LockedAccept<A, R, L, N>) {
        (
            LockedEntry { rendezvous: self },
            LockedAccept { rendezvous: self },
        )
    }
}

/// Caller-side handle of a rendezvous. Hand the argument to
/// [`entry`](Self::entry) and block until the callee returns a value.
pub struct LockedEntry<A, R, L: LockOps + 'static, N: NestingLock + 'static>
where
    A: Send + 'static,
    R: Send + 'static,
{
    rendezvous: &'static LockedRendezvous<A, R, L, N>,
}

impl<A, R, L: LockOps + 'static, N: NestingLock + 'static> LockedEntry<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
    for<'b> L::Guard<'b>: Unlock,
{
    /// Hands `arg` to the callee side and blocks until it stores a
    /// result.
    pub fn entry(&self, arg: A) -> R {
        // Provide argument
        let mut arg_guard = self.rendezvous.arg.lock();
        *arg_guard = Some(arg);
        drop(arg_guard);

        // Notify thread waiting for the argument if any
        self.rendezvous.waiter.notify_one();

        // Wait for the result
        let result_guard = self.rendezvous.result.lock();
        let mut result_guard = self
            .rendezvous
            .waiter
            .wait_while(result_guard, |result| result.is_none());
        let result = result_guard.take().unwrap();
        drop(result_guard);

        result
    }
}

unsafe impl<A, R, L: LockOps + 'static, N: NestingLock + 'static> Send for LockedEntry<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
{
}

/// Callee-side handle of a rendezvous. Wait for an argument with
/// [`accept`](Self::accept), run a closure on it, and stash the
/// closure's result for the caller side to retrieve.
pub struct LockedAccept<A, R, L: LockOps + 'static, N: NestingLock + 'static>
where
    A: Send + 'static,
    R: Send + 'static,
{
    rendezvous: &'static LockedRendezvous<A, R, L, N>,
}

impl<A, R, L: LockOps + 'static, N: NestingLock + 'static> LockedAccept<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
    for<'b> L::Guard<'b>: Unlock,
{
    /// Blocks until the caller side issues an argument, runs `closure`
    /// on it, and stores the closure's return value for the caller's
    /// `entry` call to receive.
    pub fn accept<F: FnMut(A) -> R>(&self, mut closure: F) {
        // Wait for closure argument
        let arg_guard = self.rendezvous.arg.lock();
        let mut arg_guard = self
            .rendezvous
            .waiter
            .wait_while(arg_guard, |arg| arg.is_none());
        let arg = arg_guard.take().unwrap();
        drop(arg_guard);

        // Compute result
        let result = closure(arg);

        // Return result
        let mut result_guard = self.rendezvous.result.lock();
        *result_guard = Some(result);
        drop(result_guard);

        // Notify thread waiting for the result
        self.rendezvous.waiter.notify_one();
    }
}

unsafe impl<A, R, L: LockOps + 'static, N: NestingLock + 'static> Send for LockedAccept<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
{
}
