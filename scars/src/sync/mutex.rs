use crate::kernel::Priority;
use crate::sync::{CeilingLock, InheritanceLock, InterruptLock, ScopedLock};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

pub type Mutex<T> = LockedMutex<T, InheritanceLock>;
pub type CeilingMutex<T, const CEILING: Priority> = LockedMutex<T, CeilingLock<CEILING>>;
pub type InterruptMutex<T> = LockedMutex<T, InterruptLock>;

pub struct LockedMutex<T: ?Sized, L: ScopedLock> {
    lock: L,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send, L: ScopedLock> Send for LockedMutex<T, L> {}
unsafe impl<T: ?Sized + Send, L: ScopedLock> Sync for LockedMutex<T, L> {}

impl<T, L: ScopedLock> LockedMutex<T, L> {
    #[inline(always)]
    pub const fn new(t: T) -> LockedMutex<T, L> {
        LockedMutex {
            lock: L::DEFAULT,
            data: UnsafeCell::new(t),
        }
    }
}

impl<T: ?Sized, L: ScopedLock> LockedMutex<T, L> {
    #[inline(always)]
    pub fn lock(&self) -> MutexGuard<'_, T, L> {
        let guard = self.lock.lock();

        MutexGuard {
            guard,
            data: &self.data,
        }
    }

    #[inline(always)]
    pub fn try_lock(&self) -> Result<MutexGuard<'_, T, L>, ()> {
        Ok(self.lock())
    }

    #[inline(always)]
    pub fn unlock(guard: MutexGuard<'_, T, L>) {
        drop(guard)
    }
}

#[inline(always)]
pub(crate) fn guard_raw<'a, 'b, T: ?Sized, L: ScopedLock>(
    guard: &'b mut MutexGuard<'a, T, L>,
) -> &'b mut L::Guard<'a> {
    &mut guard.guard
}

pub struct MutexGuard<'a, T: ?Sized + 'a, L: ScopedLock + 'a> {
    guard: L::Guard<'a>,
    data: &'a UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Sync, L: ScopedLock> Sync for MutexGuard<'_, T, L> {}

impl<'a, T: ?Sized, L: ScopedLock> Deref for MutexGuard<'a, T, L> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.data.get() }
    }
}

impl<'a, T: ?Sized, L: ScopedLock> DerefMut for MutexGuard<'a, T, L> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.data.get() }
    }
}
