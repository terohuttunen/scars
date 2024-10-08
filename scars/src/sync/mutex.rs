use crate::kernel::Priority;
use crate::sync::RawCeilingLock;
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

pub struct MutexLock {
    lock: RawCeilingLock,
}

impl MutexLock {
    pub const fn new(ceiling: Priority) -> MutexLock {
        MutexLock {
            lock: RawCeilingLock::new(ceiling),
        }
    }

    #[inline(always)]
    pub fn lock(&self) {
        self.lock.lock();
    }

    #[inline(always)]
    pub fn try_lock(&self) -> bool {
        self.lock.lock();
        true
    }

    pub fn unlock(&self) {
        self.lock.unlock()
    }
}

pub struct Mutex<T: ?Sized, const CEILING: Priority> {
    lock: MutexLock,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send, const CEILING: Priority> Send for Mutex<T, CEILING> {}
unsafe impl<T: ?Sized + Send, const CEILING: Priority> Sync for Mutex<T, CEILING> {}

impl<T, const CEILING: Priority> Mutex<T, CEILING> {
    #[inline(always)]
    pub const fn new(t: T) -> Mutex<T, CEILING> {
        Mutex {
            lock: MutexLock::new(CEILING),
            data: UnsafeCell::new(t),
        }
    }
}

impl<T: ?Sized, const CEILING: Priority> Mutex<T, CEILING> {
    #[inline(always)]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.lock.lock();
        MutexGuard {
            lock: &self.lock,
            data: &self.data,
        }
    }

    #[inline(always)]
    pub fn try_lock(&self) -> Result<MutexGuard<'_, T>, ()> {
        self.lock.lock();
        Ok(MutexGuard {
            lock: &self.lock,
            data: &self.data,
        })
    }

    #[inline(always)]
    pub fn unlock(guard: MutexGuard<'_, T>) {
        drop(guard)
    }
}

#[inline(always)]
pub(crate) fn guard_lock<'a, T: ?Sized>(guard: &MutexGuard<'a, T>) -> &'a RawCeilingLock {
    &guard.lock.lock
}

pub struct MutexGuard<'a, T: ?Sized + 'a> {
    lock: &'a MutexLock,
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
