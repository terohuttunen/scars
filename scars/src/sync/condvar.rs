use crate::priority::Priority;
use crate::sync::{
    CeilingLock, InterruptLock, MutexGuard, NestingLock, PreemptLock, ScopedLock, Unlock,
    mutex::guard_raw,
};
use crate::{interrupt::in_interrupt, kernel::waiter::WaitQueue};

pub type Condvar = LockedCondvar<PreemptLock>;
pub type CeilingCondvar<const CEILING: Priority> = LockedCondvar<CeilingLock<CEILING>>;
pub type InterruptCondvar = LockedCondvar<InterruptLock>;

pub struct WaitTimeoutResult(bool);

impl WaitTimeoutResult {
    pub fn timed_out(&self) -> bool {
        self.0
    }
}

pub struct LockedCondvar<L: NestingLock> {
    waiter_queue: WaitQueue<L>,
}

impl<L: NestingLock> LockedCondvar<L> {
    pub const fn new() -> LockedCondvar<L> {
        LockedCondvar {
            waiter_queue: WaitQueue::new(),
        }
    }

    #[inline(never)]
    fn wait_lock<G: ScopedLock>(&self, guard: &mut G::Guard<'_>)
    where
        for<'a> G::Guard<'a>: Unlock,
    {
        unsafe {
            guard.unlock();
        }

        self.waiter_queue.wait();

        guard.relock();
    }

    #[inline(always)]
    pub fn wait<'a, T, G: ScopedLock>(
        &self,
        mut guard: MutexGuard<'a, T, G>,
    ) -> MutexGuard<'a, T, G>
    where
        for<'b> G::Guard<'b>: Unlock,
    {
        if in_interrupt() {
            // Error: cannot wait condition variable in interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        let raw = guard_raw(&mut guard);

        self.wait_lock::<G>(raw);
        guard
    }

    pub fn wait_while<'a, T, G: ScopedLock, F>(
        &self,
        mut guard: MutexGuard<'a, T, G>,
        mut condition: F,
    ) -> MutexGuard<'a, T, G>
    where
        F: FnMut(&mut T) -> bool,
        for<'b> G::Guard<'b>: Unlock,
    {
        if in_interrupt() {
            // Error: cannot wait condition variable in interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        while condition(&mut *guard) {
            guard = self.wait(guard);
        }
        guard
    }

    pub async fn async_wait<'a, T, G: ScopedLock>(
        &'static self,
        mut guard: MutexGuard<'static, T, G>,
    ) -> MutexGuard<'static, T, G>
    where
        for<'b> G::Guard<'b>: Unlock,
    {
        let raw = guard_raw(&mut guard);

        unsafe {
            raw.unlock();
        }

        self.waiter_queue.async_wait().await;

        raw.relock();

        guard
    }

    pub async fn async_wait_while<T, G: ScopedLock, F>(
        &'static self,
        mut guard: MutexGuard<'static, T, G>,
        condition: F,
    ) -> MutexGuard<'static, T, G>
    where
        F: FnOnce(&mut T) -> bool + 'static + core::marker::Copy,
        for<'b> G::Guard<'b>: Unlock,
    {
        while condition(&mut *guard) {
            guard = self.async_wait(guard).await;
        }
        guard
    }

    pub fn notify_one(&self) {
        self.waiter_queue.notify_one()
    }

    pub fn notify_all(&self) {
        self.waiter_queue.notify_all()
    }
}

impl<L: NestingLock> Default for LockedCondvar<L> {
    fn default() -> LockedCondvar<L> {
        LockedCondvar::new()
    }
}
