//! Async mutex for tasks running on a single executor.
//!
//! The value lives in an [`OwnerCell`](crate::cell::OwnerCell) keyed by the
//! executor that touches it, so access is confined to one executor; finding the
//! cell held by a different executor is a panic, not a race or a silent wait. A
//! contended `lock()` self-requeues the task; releasing the guard wakes the
//! executor so a waiter re-polls.
//!
//! An `AsyncMutex` is a plain `static`. For cross-executor / thread / interrupt
//! messaging use the synchronous [`Channel`](crate::sync::channel).

use crate::cell::{OwnerCell, OwnerRefMut};
use crate::task::{ExecutorHandle, LocalExecutor, task_and_executor};
use core::future::poll_fn;
use core::ops::{Deref, DerefMut};
use core::task::Poll;

pub struct AsyncMutex<T> {
    cell: OwnerCell<T>,
}

impl<T> AsyncMutex<T> {
    pub const fn new(value: T) -> AsyncMutex<T> {
        AsyncMutex {
            cell: OwnerCell::new(value),
        }
    }

    /// Acquire the mutex, parking the task until it is free.
    ///
    /// Panics if the mutex is held by a task on a different executor; waiting
    /// for that would starve, as the holder's release notifies its own executor.
    pub async fn lock(&self) -> AsyncMutexGuard<'_, T> {
        poll_fn(|cx| {
            let (task, executor) = task_and_executor(cx);
            match self.cell.try_lock(executor.executor_ptr()) {
                Ok(inner) => Poll::Ready(AsyncMutexGuard {
                    inner,
                    mutex: self,
                    executor,
                }),
                Err(holder) if holder == executor.executor_ptr() => {
                    executor.resume_task(task);
                    Poll::Pending
                }
                Err(_) => panic!("AsyncMutex is confined to a single executor"),
            }
        })
        .await
    }

    /// Acquire the mutex without blocking; returns `None` if it is held.
    ///
    /// Panics if the mutex is held by a task on a different executor.
    pub fn try_lock(&self) -> Option<AsyncMutexGuard<'_, T>> {
        let executor = *LocalExecutor::get();
        match self.cell.try_lock(executor.executor_ptr()) {
            Ok(inner) => Some(AsyncMutexGuard {
                inner,
                mutex: self,
                executor,
            }),
            Err(holder) if holder == executor.executor_ptr() => None,
            Err(_) => panic!("AsyncMutex is confined to a single executor"),
        }
    }
}

pub struct AsyncMutexGuard<'a, T> {
    inner: OwnerRefMut<'a, T>,
    /// The mutex this guard locks; [`AsyncCondvar`](crate::sync::AsyncCondvar)
    /// re-acquires it after waiting.
    pub(crate) mutex: &'a AsyncMutex<T>,
    /// Executor that was polling when the lock was taken; the release path
    /// notifies it so a waiting task re-polls.
    pub(crate) executor: ExecutorHandle,
}

impl<T> Deref for AsyncMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &*self.inner
    }
}

impl<T> DerefMut for AsyncMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut *self.inner
    }
}

impl<T> Drop for AsyncMutexGuard<'_, T> {
    fn drop(&mut self) {
        // `inner` releases the cell as it drops after this; the wakeup is
        // delivered as an event, so the waiter re-polls only once the cell is
        // free.
        self.executor.notify();
    }
}
