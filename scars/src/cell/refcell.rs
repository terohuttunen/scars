pub use super::{BorrowError, BorrowMutError};
use core::cell::{Cell, UnsafeCell};
use core::ops::{Deref, DerefMut};

pub type BorrowFlag = isize;

pub struct BorrowRef<'a>(pub &'a Cell<BorrowFlag>);

impl<'a> BorrowRef<'a> {
    pub(crate) fn new_mutable(flag: &'a Cell<BorrowFlag>) -> BorrowRef<'a> {
        if flag.get() != 0 {
            panic!("already borrowed");
        }
        flag.set(-1);
        BorrowRef(flag)
    }

    pub(crate) fn new_immutable(flag: &'a Cell<BorrowFlag>) -> BorrowRef<'a> {
        if flag.get() < 0 {
            panic!("already borrowed mutably");
        }

        if flag.get() == isize::MAX {
            panic!("overflow");
        }

        flag.set(flag.get() + 1);
        BorrowRef(flag)
    }
}

impl<'a> Drop for BorrowRef<'a> {
    fn drop(&mut self) {
        let flag = self.0.get();
        if flag < 0 {
            // Release mutable borrow
            self.0.set(0);
        } else if flag > 0 {
            // Release immutable borrow
            self.0.set(flag - 1);
        } else {
            unreachable!();
        }
    }
}

pub struct RefCell<T: ?Sized> {
    borrow: Cell<BorrowFlag>,
    value: UnsafeCell<T>,
}

impl<T> RefCell<T> {
    pub const fn new(value: T) -> RefCell<T> {
        RefCell {
            borrow: Cell::new(0),
            value: UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn replace(&mut self, t: T) -> T {
        let mut dest = self.value.get_mut();
        core::mem::replace(&mut dest, t)
    }

    #[inline]
    pub fn replace_with<F: FnOnce(&mut T) -> T>(&mut self, f: F) -> T {
        let mut dest = self.value.get_mut();
        let replacement = f(&mut dest);
        core::mem::replace(&mut dest, replacement)
    }

    #[inline]
    pub fn swap(&mut self, other: &mut Self) {
        core::mem::swap(self.value.get_mut(), other.value.get_mut());
    }
}

impl<T: ?Sized> RefCell<T> {
    pub fn is_mutably_borrowed(&self) -> bool {
        self.borrow.get() < 0
    }

    pub fn is_immutably_borrowed(&self) -> bool {
        self.borrow.get() > 0
    }

    pub fn is_borrowed(&self) -> bool {
        self.borrow.get() != 0
    }

    #[inline]
    pub fn borrow(&self) -> Ref<'_, T> {
        let reference = unsafe { &*self.value.get() };
        let borrow_ref = BorrowRef::new_immutable(&self.borrow);
        Ref {
            reference,
            borrow_ref,
        }
    }

    #[inline]
    pub fn try_borrow(&self) -> Result<Ref<'_, T>, BorrowError> {
        if self.is_immutably_borrowed() {
            return Err(BorrowError);
        }

        let reference = unsafe { &*self.value.get() };
        let borrow_ref = BorrowRef::new_immutable(&self.borrow);
        Ok(Ref {
            reference,
            borrow_ref,
        })
    }

    #[inline]
    pub fn borrow_mut(&mut self) -> RefMut<'_, T> {
        let reference = unsafe { &mut *self.value.get() };
        let _borrow_ref = BorrowRef::new_mutable(&self.borrow);
        RefMut {
            reference,
            _borrow_ref,
        }
    }

    #[inline]
    pub fn try_borrow_mut(&mut self) -> Result<RefMut<'_, T>, BorrowMutError> {
        if self.is_borrowed() {
            return Err(BorrowMutError);
        }

        let reference = unsafe { &mut *self.value.get() };
        let _borrow_ref = BorrowRef::new_mutable(&self.borrow);
        Ok(RefMut {
            reference,
            _borrow_ref,
        })
    }

    #[inline]
    pub fn as_ptr(&self) -> *mut T {
        self.value.get()
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

impl<T: Default> RefCell<T> {
    pub fn take(&mut self) -> T {
        self.replace(Default::default())
    }
}

impl<T: Default> Default for RefCell<T> {
    #[inline]
    fn default() -> RefCell<T> {
        RefCell::new(Default::default())
    }
}

impl<T> From<T> for RefCell<T> {
    fn from(t: T) -> RefCell<T> {
        RefCell::new(t)
    }
}

pub struct Ref<'a, T: ?Sized> {
    reference: &'a T,
    borrow_ref: BorrowRef<'a>,
}

impl<'a, T: ?Sized> Ref<'a, T> {
    pub fn as_ref(&self) -> &T {
        self.reference
    }
}

impl<'a, T: ?Sized> Deref for Ref<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.reference
    }
}

impl<'a, T: ?Sized> Clone for Ref<'a, T> {
    fn clone(&self) -> Ref<'a, T> {
        Ref {
            reference: self.reference,
            borrow_ref: BorrowRef::new_immutable(self.borrow_ref.0),
        }
    }
}

pub struct RefMut<'a, T: ?Sized> {
    reference: &'a mut T,
    _borrow_ref: BorrowRef<'a>,
}

impl<'a, T: ?Sized> RefMut<'a, T> {
    pub fn as_ref(&self) -> &T {
        self.reference
    }

    pub fn as_mut(&mut self) -> &mut T {
        self.reference
    }
}

impl<'a, T: ?Sized> Deref for RefMut<'a, T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.reference
    }
}

impl<'a, T: ?Sized> DerefMut for RefMut<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.reference
    }
}
