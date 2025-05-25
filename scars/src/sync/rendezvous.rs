use crate::Priority;
/// This module provides synchronization primitives for rendezvous-style communication.
///
/// The `Rendezvous` struct allows two threads to synchronize and exchange data. It acts as a
/// synchronized remote procedure call into another thread's context.
///
/// The `Entry` struct represents the entry point for one of the threads. It provides a method
/// called `entry` that allows the thread to provide an argument and wait for the result.
///
/// The `Accept` struct represents the entry point for the other thread. It provides a method
/// called `accept` that allows the thread to wait for the argument, compute the result using a
/// closure, and return the result.
///
/// # Example
///
/// ```Rust
/// use scars::sync::rendezvous::{Rendezvous, Entry, Accept};
///
/// // Create a rendezvous with a ceiling priority of 3
/// let (entry, accept) = make_rendezvous!(3);
///
/// // Create a task with priority 3
/// let thread = make_thread!("thread", 3, 1024);
///
/// // Start the thread to execute the entry
/// thread.start(move || {
///     let result = entry.entry(42);
///     println!("Result: {}", result);
/// });
///
/// // Execute the accept in the current thread
/// let result = accept.accept(|arg| arg * 2);
/// assert_eq!(result, 84);
/// ```
use crate::sync::{
    CeilingLock, InheritanceLock, NestingLock, PreemptLock, ScopedLock, Unlock,
    condvar::LockedCondvar, mutex::LockedMutex,
};

/// Creates a new statically allocated `Rendezvous` with the specified ceiling priority.
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

pub type CeilingRendezvous<A, R, const CEILING: Priority> =
    LockedRendezvous<A, R, CeilingLock<CEILING>, CeilingLock<CEILING>>;

pub type Rendezvous<A, R> = LockedRendezvous<A, R, InheritanceLock, PreemptLock>;

/// Represents a rendezvous synchronization primitive. It allows two threads to synchronize and
/// exchange data. It acts as a synchronized remote procedure call into another thread's context.
pub struct LockedRendezvous<A, R, L: ScopedLock, N: NestingLock>
where
    A: Send + 'static,
    R: Send + 'static,
{
    arg: LockedMutex<Option<A>, L>,
    result: LockedMutex<Option<R>, L>,
    waiter: LockedCondvar<N>,
}

impl<A, R, L: ScopedLock, N: NestingLock> LockedRendezvous<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
{
    /// Creates a new `Rendezvous` instance.
    ///
    /// # Returns
    ///
    /// A new `Rendezvous` instance.
    pub const fn new() -> LockedRendezvous<A, R, L, N> {
        LockedRendezvous {
            arg: LockedMutex::new(None),
            result: LockedMutex::new(None),
            waiter: LockedCondvar::new(),
        }
    }

    /// Splits the `Rendezvous` instance into an `Entry` and an `Accept` instance.
    ///
    /// # Returns
    ///
    /// A tuple containing the `Entry` and `Accept` instances.
    pub const fn split(&'static mut self) -> (LockedEntry<A, R, L, N>, LockedAccept<A, R, L, N>) {
        (LockedEntry { rendezvous: self }, LockedAccept {
            rendezvous: self,
        })
    }
}

/// Represents the entry point for one of the threads. It provides a method called `entry` that
/// allows the thread to provide an argument and wait for the result. The `Entry` struct is
/// created by calling the `split` method on a `Rendezvous` instance. The `Entry` struct is
/// `Send` because it is intended to be passed to another thread.
pub struct LockedEntry<A, R, L: ScopedLock + 'static, N: NestingLock + 'static>
where
    A: Send + 'static,
    R: Send + 'static,
{
    rendezvous: &'static LockedRendezvous<A, R, L, N>,
}

impl<A, R, L: ScopedLock + 'static, N: NestingLock + 'static> LockedEntry<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
    for<'b> L::Guard<'b>: Unlock,
{
    /// Provides an argument and waits for the result.
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

unsafe impl<A, R, L: ScopedLock + 'static, N: NestingLock + 'static> Send
    for LockedEntry<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
{
}

/// Represents the entry point for the other thread. It provides a method called `accept` that
/// allows the thread to wait for the argument, compute the result using a closure, and return
/// the result. The `Accept` struct is created by calling the `split` method on a `Rendezvous`
/// instance. The `Accept` struct is `Send` because it is intended to be passed to another
/// thread.
pub struct LockedAccept<A, R, L: ScopedLock + 'static, N: NestingLock + 'static>
where
    A: Send + 'static,
    R: Send + 'static,
{
    rendezvous: &'static LockedRendezvous<A, R, L, N>,
}

impl<A, R, L: ScopedLock + 'static, N: NestingLock + 'static> LockedAccept<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
    for<'b> L::Guard<'b>: Unlock,
{
    /// Waits for the argument, computes the result using a closure, and returns the result to
    /// the other thread.
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

unsafe impl<A, R, L: ScopedLock + 'static, N: NestingLock + 'static> Send
    for LockedAccept<A, R, L, N>
where
    A: Send + 'static,
    R: Send + 'static,
{
}
