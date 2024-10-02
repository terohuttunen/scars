use super::TryLockError;
use crate::sync::{KeyToken, Lock};
use core::cell::Cell;
use core::marker::PhantomData;

pub struct NoLock {
    _phantom: PhantomData<Cell<()>>,
}

impl NoLock {
    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(<Self as Lock>::Key<'_>) -> R) -> R
    where
        Self: Sized,
    {
        <Self as Lock>::with(f)
    }

    #[inline(always)]
    pub fn try_with<R>(f: impl FnOnce(<Self as Lock>::Key<'_>) -> R) -> Result<R, TryLockError> {
        <Self as Lock>::try_with(f)
    }

    #[inline(always)]
    pub fn new_key() -> NoLockKey<'static> {
        NoLockKey {
            _private: PhantomData,
        }
    }
}

impl Lock for NoLock {
    type RestoreState = ();
    type Key<'a> = NoLockKey<'a>;

    #[inline(always)]
    unsafe fn section_start() -> Self::RestoreState {
        ()
    }

    #[inline(always)]
    unsafe fn try_section_start() -> Result<Self::RestoreState, TryLockError> {
        Ok(())
    }

    // SAFETY: there must always be a corresponding section_start() call
    // in exactly reverse order to section ends.
    #[inline(always)]
    unsafe fn section_end(_restore_state: Self::RestoreState) {}
}

#[derive(Clone, Copy, Debug)]
pub struct NoLockKey<'lock> {
    _private: PhantomData<&'lock ()>,
}

impl<'lock> KeyToken<'lock> for NoLockKey<'lock> {
    unsafe fn new() -> NoLockKey<'lock>
    where
        Self: Sized,
    {
        NoLockKey {
            _private: PhantomData,
        }
    }
}
