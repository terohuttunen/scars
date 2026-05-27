use crate::sync::{LockOps, ScopedLock};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

pub struct Guarded<T: ?Sized, L: LockOps> {
    lock: L,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send, L: LockOps> Send for Guarded<T, L> {}
unsafe impl<T: ?Sized + Send, L: LockOps> Sync for Guarded<T, L> {}

impl<T, L: ScopedLock> Guarded<T, L> {
    #[inline(always)]
    pub const fn new(t: T) -> Guarded<T, L> {
        Guarded {
            lock: L::DEFAULT,
            data: UnsafeCell::new(t),
        }
    }
}

impl<T: ?Sized, L: LockOps> Guarded<T, L> {
    #[inline(always)]
    pub fn lock(&self) -> Guard<'_, T, L> {
        Guard {
            guard: self.lock.lock(),
            data: &self.data,
        }
    }

    #[inline(always)]
    pub fn try_lock(&self) -> Result<Guard<'_, T, L>, ()> {
        Ok(self.lock())
    }

    #[inline(always)]
    pub fn unlock(guard: Guard<'_, T, L>) {
        drop(guard)
    }
}

#[inline(always)]
pub(crate) fn guard_raw<'a, 'b, T: ?Sized, L: LockOps>(
    guard: &'b mut Guard<'a, T, L>,
) -> &'b mut L::Guard<'a> {
    &mut guard.guard
}

pub struct Guard<'a, T: ?Sized + 'a, L: LockOps + 'a> {
    guard: L::Guard<'a>,
    data: &'a UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Sync, L: LockOps> Sync for Guard<'_, T, L> {}

impl<'a, T: ?Sized, L: LockOps> Deref for Guard<'a, T, L> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.data.get() }
    }
}

impl<'a, T: ?Sized, L: LockOps> DerefMut for Guard<'a, T, L> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.data.get() }
    }
}
