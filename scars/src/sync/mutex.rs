use crate::kernel::Priority;
use crate::kernel::hal::CoreId;
use crate::sync::{CoreCeilingLock, CoreInheritanceLock, CoreInterruptLock, LockOps, ScopedLock};
use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};

pub type Mutex<T, const CORE: CoreId = { CoreId::DEFAULT }> = Locked<T, CoreInheritanceLock<CORE>>;
pub type CeilingMutex<T, const CEILING: Priority, const CORE: CoreId = { CoreId::DEFAULT }> =
    Locked<T, CoreCeilingLock<CEILING, CORE>>;
pub type InterruptMutex<T, const CORE: CoreId = { CoreId::DEFAULT }> =
    Locked<T, CoreInterruptLock<CORE>>;

pub struct Locked<T: ?Sized, L: LockOps> {
    lock: L,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send, L: LockOps> Send for Locked<T, L> {}
unsafe impl<T: ?Sized + Send, L: LockOps> Sync for Locked<T, L> {}

impl<T, L: ScopedLock> Locked<T, L> {
    #[inline(always)]
    pub const fn new(t: T) -> Locked<T, L> {
        Locked {
            lock: L::DEFAULT,
            data: UnsafeCell::new(t),
        }
    }
}

impl<T: ?Sized, L: LockOps> Locked<T, L> {
    #[inline(always)]
    pub fn lock(&self) -> MutexGuard<'_, T, L> {
        MutexGuard {
            guard: self.lock.lock(),
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
pub(crate) fn guard_raw<'a, 'b, T: ?Sized, L: LockOps>(
    guard: &'b mut MutexGuard<'a, T, L>,
) -> &'b mut L::Guard<'a> {
    &mut guard.guard
}

pub struct MutexGuard<'a, T: ?Sized + 'a, L: LockOps + 'a> {
    guard: L::Guard<'a>,
    data: &'a UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Sync, L: LockOps> Sync for MutexGuard<'_, T, L> {}

impl<'a, T: ?Sized, L: LockOps> Deref for MutexGuard<'a, T, L> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        unsafe { &*self.data.get() }
    }
}

impl<'a, T: ?Sized, L: LockOps> DerefMut for MutexGuard<'a, T, L> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.data.get() }
    }
}
