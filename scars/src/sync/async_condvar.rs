use crate::in_interrupt;
use crate::kernel::waiter::AsyncWaiterQueue;
use crate::sync::async_mutex::AsyncMutexGuard;

pub struct AsyncCondvar {
    waiter_queue: AsyncWaiterQueue,
}

impl AsyncCondvar {
    pub const fn new() -> AsyncCondvar {
        AsyncCondvar {
            waiter_queue: AsyncWaiterQueue::new(),
        }
    }

    pub async fn wait<'a, T>(&self, guard: AsyncMutexGuard<'a, T>) -> AsyncMutexGuard<'a, T> {
        if in_interrupt() {
            // Error: cannot wait async condition variable in interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        guard.mutex.unlock();

        self.waiter_queue.wait().await;

        guard.mutex.lock();
        guard
    }

    pub async fn wait_while<'a, T, F>(
        &self,
        mut guard: AsyncMutexGuard<'a, T>,
        mut condition: F,
    ) -> AsyncMutexGuard<'a, T>
    where
        F: FnMut(&mut T) -> bool,
    {
        if in_interrupt() {
            // Error: cannot wait async condition variable in interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        while condition(&mut *guard) {
            guard = self.wait(guard).await;
        }
        guard
    }

    pub fn notify_one(&self) {
        self.waiter_queue.notify_one();
    }

    pub fn notify_all(&self) {
        self.waiter_queue.notify_all();
    }
}

impl Default for AsyncCondvar {
    fn default() -> AsyncCondvar {
        AsyncCondvar::new()
    }
}
