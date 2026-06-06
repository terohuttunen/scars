//! Async condition variable for tasks on a single executor.
//!
//! Pairs with [`AsyncMutex`](crate::sync::async_mutex). Waking is tracked by a
//! generation counter in an [`OwnerCell`](crate::cell::OwnerCell):
//! `notify_one`/`notify_all` bump the generation and wake the executor, and
//! every parked waiter re-checks. `notify_one` therefore wakes *all* current
//! waiters, which re-evaluate their predicate and re-wait if it still holds; use
//! [`wait_while`](AsyncCondvar::wait_while) to loop on the predicate.
//!
//! Like [`AsyncMutex`](crate::sync::async_mutex), it is a plain `static`.

use crate::cell::OwnerCell;
use crate::sync::async_mutex::AsyncMutexGuard;
use crate::task::{LocalExecutor, task_and_executor};
use core::future::poll_fn;
use core::task::Poll;

pub struct AsyncCondvar {
    generation: OwnerCell<u32>,
}

impl AsyncCondvar {
    pub const fn new() -> AsyncCondvar {
        AsyncCondvar {
            generation: OwnerCell::new(0),
        }
    }

    /// Release `guard`, park until notified, then re-acquire and return the
    /// guard. May wake spuriously; callers should re-check a predicate (see
    /// [`wait_while`](Self::wait_while)).
    pub async fn wait<'a, T>(&self, guard: AsyncMutexGuard<'a, T>) -> AsyncMutexGuard<'a, T> {
        let mutex = guard.mutex;
        let observed = self.generation.with(guard.executor.executor_ptr(), |g| *g);
        // Releasing the guard wakes the executor so mutex waiters re-poll.
        drop(guard);

        poll_fn(|cx| {
            let (task, executor) = task_and_executor(cx);
            if self.generation.with(executor.executor_ptr(), |g| *g) != observed {
                Poll::Ready(())
            } else {
                executor.resume_task(task);
                Poll::Pending
            }
        })
        .await;

        mutex.lock().await
    }

    /// Wait until `condition` is false, re-checking after each notification.
    pub async fn wait_while<'a, T, F>(
        &self,
        mut guard: AsyncMutexGuard<'a, T>,
        mut condition: F,
    ) -> AsyncMutexGuard<'a, T>
    where
        F: FnMut(&mut T) -> bool,
    {
        while condition(&mut *guard) {
            guard = self.wait(guard).await;
        }
        guard
    }

    /// Wake waiters. With a generation counter this wakes all parked waiters,
    /// which then re-check their predicate; see the module note.
    pub fn notify_one(&self) {
        self.bump();
    }

    pub fn notify_all(&self) {
        self.bump();
    }

    fn bump(&self) {
        let executor = LocalExecutor::get();
        self.generation
            .with_mut(executor.executor_ptr(), |g| *g = g.wrapping_add(1));
        executor.notify();
    }
}

impl Default for AsyncCondvar {
    fn default() -> AsyncCondvar {
        AsyncCondvar::new()
    }
}
