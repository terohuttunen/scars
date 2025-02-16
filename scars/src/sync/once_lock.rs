use crate::cell::LockedOnceCell;
use crate::sync::{InterruptLock, NestingLock};

pub struct OnceLock<T, L: NestingLock = InterruptLock> {
    cell: LockedOnceCell<T, L>,
}

impl<T, L: NestingLock> OnceLock<T, L> {
    pub const fn new() -> OnceLock<T, L> {
        OnceLock {
            cell: LockedOnceCell::new(),
        }
    }

    pub fn get(&self) -> Option<&T> {
        self.cell.get()
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.cell.get_mut()
    }

    pub fn set(&self, value: T) -> Result<(), T> {
        <L as NestingLock>::with(|key| self.cell.set(key, value))
    }

    pub fn get_or_init<F>(&self, init: F) -> &T
    where
        F: FnOnce() -> T,
    {
        <L as NestingLock>::with(|key| self.cell.get_or_init(key, init))
    }

    pub fn into_inner(self) -> Option<T> {
        self.cell.into_inner()
    }

    pub fn take(&mut self) -> Option<T> {
        self.cell.take()
    }
}
