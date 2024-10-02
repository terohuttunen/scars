use crate::kernel::list::LinkedList;
use crate::kernel::waiter::{WaitObject, WaitQueueTag};
use crate::task::TaskControlBlock;
use core::cell::{RefCell, UnsafeCell};
use core::future::{poll_fn, Future};
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::task::{Context, Poll};

pub(crate) struct AsyncLock {
    locked: bool,
    waiters: LinkedList<WaitObject, WaitQueueTag>,
}

pub(crate) struct AsyncMutexLock {
    lock: RefCell<AsyncLock>,
}

impl AsyncMutexLock {
    pub const fn new() -> AsyncMutexLock {
        AsyncMutexLock {
            lock: RefCell::new(AsyncLock {
                locked: false,
                waiters: LinkedList::new(),
            }),
        }
    }
}

pub struct AsyncMutex<T: ?Sized> {
    lock: AsyncMutexLock,
    data: UnsafeCell<T>,
}

impl<T> AsyncMutex<T> {
    pub const fn new(t: T) -> AsyncMutex<T> {
        AsyncMutex {
            lock: AsyncMutexLock::new(),
            data: UnsafeCell::new(t),
        }
    }
}

impl<T: ?Sized> AsyncMutex<T> {
    pub async fn lock(&self) -> AsyncMutexGuard<'_, T> {
        poll_fn(|cx| {
            let mut lock = self.lock.borrow_mut();
            if lock.locked {
                let task = unsafe { &*(cx.waker().as_raw().data() as *const TaskControlBlock) };
                lock.waiters.push_back(&task.waiter);
                Poll::Pending
            } else {
                lock.locked = true;
                Poll::Ready(AsyncMutexGuard { mutex: self })
            }
        })
        .await
    }
}

#[inline(always)]
pub(crate) fn guard_lock<'a, T: ?Sized>(guard: &AsyncMutexGuard<'a, T>) -> &'a AsyncMutexLock {
    &guard.mutex.lock
}

pub struct AsyncMutexGuard<'a, T: ?Sized> {
    pub(crate) mutex: &'a AsyncMutex<T>,
}

impl<'a, T: ?Sized> Deref for AsyncMutexGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for AsyncMutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.mutex.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for AsyncMutexGuard<'a, T> {
    fn drop(&mut self) {
        let mut lock = self.mutex.lock.borrow_mut();
        lock.locked = false;
        if let Some(waiter) = lock.waiters.pop_front() {
            waiter.wake();
        }
    }
}
