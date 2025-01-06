use crate::kernel::Priority;
use crate::sync::RawCeilingLock;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;

pub struct Mutex<T: ?Sized, const CEILING: Priority> {
    lock: RawCeilingLock,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send, const CEILING: Priority> Send for Mutex<T, CEILING> {}
unsafe impl<T: ?Sized + Send, const CEILING: Priority> Sync for Mutex<T, CEILING> {}

impl<T, const CEILING: Priority> Mutex<T, CEILING> {
    #[inline(always)]
    pub const fn new(t: T) -> Mutex<T, CEILING> {
        Mutex {
            lock: RawCeilingLock::new(CEILING),
            data: UnsafeCell::new(t),
        }
    }
}

impl<T: ?Sized, const CEILING: Priority> Mutex<T, CEILING> {
    #[inline(always)]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        let pinned_lock = unsafe { Pin::new_unchecked(&self.lock) };
        pinned_lock.lock();
        MutexGuard {
            lock: pinned_lock,
            data: &self.data,
        }
    }

    #[inline(always)]
    pub fn try_lock(&self) -> Result<MutexGuard<'_, T>, ()> {
        Ok(self.lock())
    }

    #[inline(always)]
    pub fn unlock(guard: MutexGuard<'_, T>) {
        drop(guard)
    }
}

#[inline(always)]
pub(crate) fn guard_lock<'a, T: ?Sized>(guard: &'a MutexGuard<'a, T>) -> Pin<&'a RawCeilingLock> {
    guard.lock.as_ref()
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    lock: Pin<&'a RawCeilingLock>,
    data: &'a UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Sync> Sync for MutexGuard<'_, T> {}

impl<'a, T: ?Sized> Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.data.get() }
    }
}

impl<T: ?Sized> Drop for MutexGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.unlock();
    }
}
