pub use super::{BorrowError, BorrowMutError};
pub use crate::cell::refcell::{BorrowFlag, BorrowRef};
use core::cell::{Cell, UnsafeCell};
use core::pin::Pin;

pub struct PinRefCell<T: ?Sized> {
    borrow: Cell<BorrowFlag>,
    value: UnsafeCell<T>,
}

pub struct PinRef<'a, T: ?Sized> {
    pub(crate) reference: Pin<&'a T>,
    pub(crate) borrow_ref: BorrowRef<'a>,
}

impl<'a, T: ?Sized> Clone for PinRef<'a, T> {
    fn clone(&self) -> Self {
        PinRef {
            reference: self.reference,
            borrow_ref: BorrowRef::new_immutable(&self.borrow_ref.0),
        }
    }
}

pub struct PinRefMut<'a, T: ?Sized> {
    pub(crate) reference: Pin<&'a mut T>,
    pub(crate) _borrow_ref: BorrowRef<'a>,
}

impl<'a, T: ?Sized> PinRef<'a, T> {
    pub fn as_ref(&self) -> Pin<&T> {
        self.reference
    }
}

impl<'a, T: ?Sized> PinRefMut<'a, T> {
    pub fn as_ref(&self) -> Pin<&T> {
        self.reference.as_ref()
    }

    pub fn as_mut(&mut self) -> Pin<&mut T> {
        self.reference.as_mut()
    }
}

impl<T> PinRefCell<T> {
    pub const fn new(value: T) -> PinRefCell<T> {
        PinRefCell {
            borrow: Cell::new(0),
            value: UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn borrow(self: Pin<&Self>) -> PinRef<T> {
        let reference = unsafe { Pin::map_unchecked(self, |s| &*s.value.get()) };
        let borrow_ref = BorrowRef::new_immutable(&self.get_ref().borrow);

        PinRef {
            reference,
            borrow_ref,
        }
    }

    #[inline]
    pub fn borrow_mut(self: Pin<&Self>) -> PinRefMut<'_, T> {
        let reference = unsafe { Pin::new_unchecked(&mut *self.value.get()) };
        let _borrow_ref = BorrowRef::new_mutable(&self.get_ref().borrow);

        PinRefMut {
            reference,
            _borrow_ref,
        }
    }

    pub fn try_borrow_mut(self: Pin<&Self>) -> Result<PinRefMut<'_, T>, BorrowMutError> {
        if self.borrow.get() != 0 {
            return Err(BorrowMutError);
        }

        let reference = unsafe { Pin::new_unchecked(&mut *self.value.get()) };
        let _borrow_ref = BorrowRef::new_mutable(&self.get_ref().borrow);

        Ok(PinRefMut {
            reference,
            _borrow_ref,
        })
    }

    pub fn as_ptr(&self) -> *mut T {
        self.value.get()
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

impl<T: Default> Default for PinRefCell<T> {
    #[inline]
    fn default() -> PinRefCell<T> {
        PinRefCell::new(Default::default())
    }
}

impl<T> From<T> for PinRefCell<T> {
    fn from(t: T) -> PinRefCell<T> {
        PinRefCell::new(t)
    }
}
