use crate::sync::Lock;
use core::cell::{BorrowError, BorrowMutError, Ref, RefCell, RefMut, UnsafeCell};
use core::marker::PhantomData;

#[repr(transparent)]
pub struct LockedCell<T: ?Sized, L: Lock> {
    _phantom: PhantomData<L>,
    value: UnsafeCell<T>,
}

unsafe impl<T: ?Sized, L: Lock> Send for LockedCell<T, L> where T: Send {}
unsafe impl<T: ?Sized, L: Lock> Sync for LockedCell<T, L> {}

impl<T, L: Lock> LockedCell<T, L> {
    #[inline]
    pub const fn new(value: T) -> LockedCell<T, L> {
        LockedCell {
            _phantom: PhantomData,
            value: UnsafeCell::new(value),
        }
    }

    #[inline]
    pub fn get(&self, _key: L::Key<'_>) -> T
    where
        T: Copy,
    {
        let inner = unsafe { &*self.value.get() };
        *inner
    }

    #[inline]
    pub fn set(&self, key: L::Key<'_>, value: T) {
        let old = self.replace(key, value);
        drop(old);
    }

    #[inline]
    pub fn replace(&self, _key: L::Key<'_>, value: T) -> T {
        let inner = unsafe { &mut *self.value.get() };
        core::mem::replace(inner, value)
    }
}

impl<T: Copy, L: Lock> LockedCell<T, L> {
    #[inline]
    pub fn update<F>(&self, key: L::Key<'_>, f: F) -> T
    where
        F: FnOnce(T) -> T,
    {
        let old = self.get(key);
        let new = f(old);
        self.set(key, new);
        new
    }
}

impl<T: ?Sized, L: Lock> LockedCell<T, L> {
    #[inline]
    pub const fn as_ptr(&self) -> *mut T {
        self.value.get()
    }

    #[inline]
    pub fn get_mut(&mut self, _key: L::Key<'_>) -> &mut T {
        self.value.get_mut()
    }

    #[inline]
    pub fn from_mut(t: &mut T) -> &LockedCell<T, L> {
        unsafe { &*(t as *mut T as *const LockedCell<T, L>) }
    }
}

impl<T: Default, L: Lock> LockedCell<T, L> {
    pub fn take(&self, key: L::Key<'_>) -> T {
        self.replace(key, Default::default())
    }
}

impl<T: Default, L: Lock> Default for LockedCell<T, L> {
    #[inline]
    fn default() -> LockedCell<T, L> {
        LockedCell::new(Default::default())
    }
}

impl<T, L: Lock> From<T> for LockedCell<T, L> {
    fn from(t: T) -> LockedCell<T, L> {
        LockedCell::new(t)
    }
}

impl<T, L: Lock> LockedCell<[T], L> {
    pub fn as_slice_of_cells(&self) -> &[LockedCell<T, L>] {
        // SAFETY: `LockedCell<T, L>` has the same memory layout as `T`
        unsafe { &*(self as *const LockedCell<[T], L> as *const [LockedCell<T, L>]) }
    }
}

impl<T, L: Lock, const N: usize> LockedCell<[T; N], L> {
    pub fn as_array_of_cells(&self) -> &[LockedCell<T, L>; N] {
        // SAFETY: `LockedCell<T, L>` has the same memory layout as `T`
        unsafe { &*(self as *const LockedCell<[T; N], L> as *const [LockedCell<T, L>; N]) }
    }
}

pub struct LockedRefCell<T: ?Sized, L: Lock> {
    _phantom: PhantomData<L>,
    value: RefCell<T>,
}

impl<T, L: Lock> LockedRefCell<T, L> {
    pub const fn new(value: T) -> LockedRefCell<T, L> {
        LockedRefCell {
            _phantom: PhantomData,
            value: RefCell::new(value),
        }
    }

    #[inline]
    pub fn replace<'key>(&self, _key: L::Key<'key>, t: T) -> T {
        self.value.replace(t)
    }

    #[inline]
    pub fn replace_with<'key, F: FnOnce(&mut T) -> T>(&self, _key: L::Key<'key>, f: F) -> T {
        self.value.replace_with(f)
    }

    #[inline]
    pub fn swap<'key>(&self, _key: L::Key<'key>, other: &Self) {
        self.value.swap(&other.value)
    }
}

impl<T: ?Sized, L: Lock> LockedRefCell<T, L> {
    #[inline]
    pub fn borrow<'key, 'a: 'key>(&'a self, _key: L::Key<'key>) -> Ref<'key, T> {
        self.value.borrow()
    }

    #[inline]
    pub fn try_borrow<'key, 'a: 'key>(
        &'a self,
        _key: L::Key<'key>,
    ) -> Result<Ref<'key, T>, BorrowError> {
        self.value.try_borrow()
    }

    #[inline]
    pub fn borrow_mut<'key, 'a: 'key>(&'a self, _key: L::Key<'key>) -> RefMut<'key, T> {
        self.value.borrow_mut()
    }

    #[inline]
    pub fn try_borrow_mut<'key, 'a: 'key>(
        &'a self,
        _key: L::Key<'key>,
    ) -> Result<RefMut<'key, T>, BorrowMutError> {
        self.value.try_borrow_mut()
    }

    #[inline]
    pub fn as_ptr(&self) -> *mut T {
        self.value.as_ptr()
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

impl<T: Default, L: Lock> LockedRefCell<T, L> {
    pub fn take<'key>(&self, _key: L::Key<'key>) -> T {
        self.value.replace(Default::default())
    }
}

unsafe impl<T: ?Sized, L: Lock> Send for LockedRefCell<T, L> where T: Send {}

unsafe impl<T: ?Sized, L: Lock> Sync for LockedRefCell<T, L> {}

impl<T: Default, L: Lock> Default for LockedRefCell<T, L> {
    #[inline]
    fn default() -> LockedRefCell<T, L> {
        LockedRefCell::new(Default::default())
    }
}

impl<T, L: Lock> From<T> for LockedRefCell<T, L> {
    fn from(t: T) -> LockedRefCell<T, L> {
        LockedRefCell::new(t)
    }
}
