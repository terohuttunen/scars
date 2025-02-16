use crate::kernel::{interrupt::in_interrupt, priority::Priority, waiter::WaitQueue};
use crate::sync::{MutexGuard, Unlock, ceiling_lock::RawCeilingLockGuard, mutex};

pub struct WaitTimeoutResult(bool);

impl WaitTimeoutResult {
    pub fn timed_out(&self) -> bool {
        self.0
    }
}

pub struct Condvar<const CEILING: Priority> {
    waiter_queue: WaitQueue<CEILING>,
}

impl<const CEILING: Priority> Condvar<CEILING> {
    pub const fn new() -> Condvar<CEILING> {
        Condvar {
            waiter_queue: WaitQueue::new(),
        }
    }

    #[inline(never)]
    fn wait_lock(&self, guard: &mut RawCeilingLockGuard) {
        unsafe {
            guard.unlock();
        }

        self.waiter_queue.wait();

        guard.relock();
    }

    #[inline(always)]
    pub fn wait<'a, T>(&self, mut guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        if in_interrupt() {
            // Error: cannot wait condition variable in interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        let raw = mutex::guard_raw(&mut guard);
        self.wait_lock(raw);
        guard
    }

    pub fn wait_while<'a, T, F>(
        &self,
        mut guard: MutexGuard<'a, T>,
        mut condition: F,
    ) -> MutexGuard<'a, T>
    where
        F: FnMut(&mut T) -> bool,
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

    pub async fn async_wait<'a, T>(
        &'static self,
        mut guard: MutexGuard<'static, T>,
    ) -> MutexGuard<'static, T> {
        let raw = mutex::guard_raw(&mut guard);
        unsafe {
            raw.unlock();
        }

        self.waiter_queue.async_wait().await;

        raw.lock();
        guard
    }

    pub async fn async_wait_while<T, F>(
        &'static self,
        mut guard: MutexGuard<'static, T>,
        condition: F,
    ) -> MutexGuard<'static, T>
    where
        F: FnOnce(&mut T) -> bool + 'static + core::marker::Copy,
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

impl<const CEILING: Priority> Default for Condvar<CEILING> {
    fn default() -> Condvar<CEILING> {
        Condvar::new()
    }
}
