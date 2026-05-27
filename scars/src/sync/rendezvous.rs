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
//! LockedAccept)` pair. With a `ceiling` argument the underlying lock
//! uses [`CeilingLock`]; without it uses [`CorePreemptLock`].
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
    BarrierResult, CoreCeilingLock, CorePreemptLock, NestingLock, Notify, Protected,
};

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
> = LockedRendezvous<A, R, CoreCeilingLock<CEILING, CORE>>;

pub type Rendezvous<A, R, const CORE: CoreId = { CoreId::DEFAULT }> =
    LockedRendezvous<A, R, CorePreemptLock<CORE>>;

struct RendezvousInner<A, R> {
    arg: Option<A>,
    result: Option<R>,
}

/// Two-thread RPC channel parameterised over a single nesting-lock `L`.
/// Use the [`Rendezvous`] / [`CeilingRendezvous`] aliases for the
/// supported configurations.
pub struct LockedRendezvous<A, R, L: NestingLock>
where
    A: Send + 'static,
    R: Send + 'static,
{
    inner: Protected<RendezvousInner<A, R>, L>,
    notify: Notify<L>,
}

impl<A, R, L: NestingLock> LockedRendezvous<A, R, L>
where
    A: Send + 'static,
    R: Send + 'static,
{
    pub const fn new() -> LockedRendezvous<A, R, L> {
        LockedRendezvous {
            inner: Protected::new(RendezvousInner {
                arg: None,
                result: None,
            }),
            notify: Notify::new(),
        }
    }

    /// Splits the rendezvous into the caller-side and callee-side
    /// halves.
    pub const fn split(&'static mut self) -> (LockedEntry<A, R, L>, LockedAccept<A, R, L>) {
        (
            LockedEntry { rendezvous: self },
            LockedAccept { rendezvous: self },
        )
    }
}

/// Caller-side handle of a rendezvous. Hand the argument to
/// [`entry`](Self::entry) and block until the callee returns a value.
pub struct LockedEntry<A, R, L: NestingLock + 'static>
where
    A: Send + 'static,
    R: Send + 'static,
{
    rendezvous: &'static LockedRendezvous<A, R, L>,
}

impl<A, R, L: NestingLock + 'static> LockedEntry<A, R, L>
where
    A: Send + 'static,
    R: Send + 'static,
{
    /// Hands `arg` to the callee side and blocks until it stores a
    /// result.
    pub fn entry(&self, arg: A) -> R {
        // Deposit argument and wake the accept side.
        self.rendezvous.inner.with(|_, r| {
            r.arg = Some(arg);
        });
        self.rendezvous.notify.notify_one();

        // Wait for the result.
        self.rendezvous
            .inner
            .with_barrier(|key, r| match r.result.take() {
                Some(v) => BarrierResult::Done(v),
                None => BarrierResult::Wait(self.rendezvous.notify.arm(key)),
            })
    }
}

unsafe impl<A, R, L: NestingLock + 'static> Send for LockedEntry<A, R, L>
where
    A: Send + 'static,
    R: Send + 'static,
{
}

/// Callee-side handle of a rendezvous. Wait for an argument with
/// [`accept`](Self::accept), run a closure on it, and stash the
/// closure's result for the caller side to retrieve.
pub struct LockedAccept<A, R, L: NestingLock + 'static>
where
    A: Send + 'static,
    R: Send + 'static,
{
    rendezvous: &'static LockedRendezvous<A, R, L>,
}

impl<A, R, L: NestingLock + 'static> LockedAccept<A, R, L>
where
    A: Send + 'static,
    R: Send + 'static,
{
    /// Blocks until the caller side issues an argument, runs `closure`
    /// on it, and stores the closure's return value for the caller's
    /// `entry` call to receive.
    pub fn accept<F: FnMut(A) -> R>(&self, mut closure: F) {
        // Wait for the argument.
        let arg = self
            .rendezvous
            .inner
            .with_barrier(|key, r| match r.arg.take() {
                Some(a) => BarrierResult::Done(a),
                None => BarrierResult::Wait(self.rendezvous.notify.arm(key)),
            });

        // Compute result outside the lock.
        let result = closure(arg);

        // Deposit result and wake the entry side.
        self.rendezvous.inner.with(|_, r| {
            r.result = Some(result);
        });
        self.rendezvous.notify.notify_one();
    }
}

unsafe impl<A, R, L: NestingLock + 'static> Send for LockedAccept<A, R, L>
where
    A: Send + 'static,
    R: Send + 'static,
{
}
