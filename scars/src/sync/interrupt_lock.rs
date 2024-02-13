use super::TryLockError;
use crate::kernel::hal::{acquire, restore};
use crate::sync::{KeyToken, Lock};
use core::marker::PhantomData;

pub struct InterruptLock {}

impl InterruptLock {
    pub fn with<R>(f: impl FnOnce(<Self as Lock>::Key<'_>) -> R) -> R
    where
        Self: Sized,
    {
        <Self as Lock>::with(f)
    }

    pub fn try_with<R>(f: impl FnOnce(<Self as Lock>::Key<'_>) -> R) -> Result<R, TryLockError> {
        <Self as Lock>::try_with(f)
    }
}

impl Lock for InterruptLock {
    type RestoreState = bool;
    type Key<'a> = InterruptLockKey<'a>;

    unsafe fn section_start() -> Self::RestoreState {
        acquire()
    }

    unsafe fn try_section_start() -> Result<Self::RestoreState, TryLockError> {
        Ok(acquire())
    }

    // SAFETY: there must always be a corresponding section_start() call
    // in exactly reverse order to section ends.
    unsafe fn section_end(restore_state: Self::RestoreState) {
        restore(restore_state)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct InterruptLockKey<'lock> {
    _private: PhantomData<&'lock ()>,
}

impl<'lock> KeyToken<'lock> for InterruptLockKey<'lock> {
    unsafe fn new() -> InterruptLockKey<'lock>
    where
        Self: Sized,
    {
        InterruptLockKey {
            _private: PhantomData,
        }
    }
}
