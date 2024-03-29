use crate::kernel::{interrupt::in_interrupt, priority::AnyPriority, WaitQueue};
use crate::sync::{mutex, MutexGuard, RawCeilingLock};

pub struct WaitTimeoutResult(bool);

impl WaitTimeoutResult {
    pub fn timed_out(&self) -> bool {
        self.0
    }
}

pub struct Condvar<const CEILING: AnyPriority> {
    wait_queue: WaitQueue<CEILING>,
}

impl<const CEILING: AnyPriority> Condvar<CEILING> {
    pub const fn new() -> Condvar<CEILING> {
        Condvar {
            wait_queue: WaitQueue::new(),
        }
    }

    #[inline(never)]
    fn wait_lock(&self, lock: &RawCeilingLock) {
        lock.unlock();

        self.wait_queue.wait();

        lock.lock();
    }

    #[inline(always)]
    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        if in_interrupt() {
            // Error: cannot wait condition variable in interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        let lock = mutex::guard_lock(&guard);
        self.wait_lock(lock);
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

    pub fn notify_one(&self) {
        self.wait_queue.notify_one()
    }

    pub fn notify_all(&self) {
        self.wait_queue.notify_all()
    }
}

impl<const CEILING: AnyPriority> Default for Condvar<CEILING> {
    fn default() -> Condvar<CEILING> {
        Condvar::new()
    }
}
