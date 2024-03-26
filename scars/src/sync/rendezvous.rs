use crate::sync::{Condvar, Mutex};
use crate::AnyPriority;
use core::marker::PhantomData;

#[macro_export]
macro_rules! make_rendezvous {
    ($prio:expr) => {{
        type A = impl ::core::marker::Sized + ::core::marker::Send + 'static;
        type R = impl ::core::marker::Sized + ::core::marker::Send + 'static;
        static mut RENDEZVOUS: $crate::sync::rendezvous::Rendezvous<A, R, { $prio }> =
            $crate::sync::rendezvous::Rendezvous::new();

        unsafe { RENDEZVOUS.split() }
    }};
}

pub struct Rendezvous<A, R, const CEILING: AnyPriority>
where
    A: Send + 'static,
    R: Send + 'static,
{
    arg: Mutex<Option<A>, CEILING>,
    result: Mutex<Option<R>, CEILING>,
    waiter: Condvar<CEILING>,
}

impl<A, R, const CEILING: AnyPriority> Rendezvous<A, R, CEILING>
where
    A: Send + 'static,
    R: Send + 'static,
{
    pub const fn new() -> Rendezvous<A, R, CEILING> {
        Rendezvous {
            arg: Mutex::new(None),
            result: Mutex::new(None),
            waiter: Condvar::new(),
        }
    }

    pub const fn split(&'static mut self) -> (Entry<A, R, CEILING>, Accept<A, R, CEILING>) {
        (Entry { rendezvous: self }, Accept { rendezvous: self })
    }
}

pub struct Entry<A, R, const CEILING: AnyPriority>
where
    A: Send + 'static,
    R: Send + 'static,
{
    rendezvous: &'static Rendezvous<A, R, CEILING>,
}

impl<A, R, const CEILING: AnyPriority> Entry<A, R, CEILING>
where
    A: Send + 'static,
    R: Send + 'static,
{
    pub fn entry(&self, arg: A) -> R {
        // Provide argument
        let mut arg_guard = self.rendezvous.arg.lock();
        *arg_guard = Some(arg);
        drop(arg_guard);

        // Notify task waiting for the argument if any
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

unsafe impl<A, R, const CEILING: AnyPriority> Send for Entry<A, R, CEILING>
where
    A: Send + 'static,
    R: Send + 'static,
{
}

pub struct Accept<A, R, const CEILING: AnyPriority>
where
    A: Send + 'static,
    R: Send + 'static,
{
    rendezvous: &'static Rendezvous<A, R, CEILING>,
}

impl<A, R, const CEILING: AnyPriority> Accept<A, R, CEILING>
where
    A: Send + 'static,
    R: Send + 'static,
{
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

        // Notify task waiting for the result
        self.rendezvous.waiter.notify_one();
    }
}

unsafe impl<A, R, const CEILING: AnyPriority> Send for Accept<A, R, CEILING>
where
    A: Send + 'static,
    R: Send + 'static,
{
}
