use super::TryLockError;
use crate::sync::{NestingLock, ScopedLock};
use core::cell::Cell;
use core::marker::PhantomData;

pub struct NoLock {
    _phantom: PhantomData<Cell<()>>,
}

impl NoLock {
    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(<Self as NestingLock>::Key<'_>) -> R) -> R
    where
        Self: Sized,
    {
        let key = unsafe { NoLockKey::new() };
        f(key)
    }

    #[inline(always)]
    pub fn try_with<R>(
        f: impl FnOnce(<Self as NestingLock>::Key<'_>) -> R,
    ) -> Result<R, TryLockError> {
        Ok(Self::with(f))
    }

    #[inline(always)]
    pub fn new_key() -> NoLockKey<'static> {
        NoLockKey {
            _private: PhantomData,
        }
    }
}

pub struct NoLockGuard<'lock> {
    _private: PhantomData<&'lock ()>,
}

impl ScopedLock for NoLock {
    type Guard<'guard> = NoLockGuard<'guard>;

    fn lock(&self) -> Self::Guard<'_> {
        NoLockGuard {
            _private: PhantomData,
        }
    }

    fn try_lock(&self) -> Result<Self::Guard<'_>, TryLockError> {
        Ok(NoLockGuard {
            _private: PhantomData,
        })
    }
}

impl NestingLock for NoLock {
    type Key<'guard> = NoLockKey<'guard>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { NoLockKey::new() }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NoLockKey<'lock> {
    _private: PhantomData<&'lock ()>,
}

impl<'lock> NoLockKey<'lock> {
    unsafe fn new() -> NoLockKey<'lock>
    where
        Self: Sized,
    {
        NoLockKey {
            _private: PhantomData,
        }
    }
}
