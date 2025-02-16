use crate::sync::{NestingLock, Once};
use core::cell::{BorrowError, BorrowMutError, Ref, RefCell, RefMut, UnsafeCell};
use core::marker::PhantomData;
use core::mem::MaybeUninit;

#[repr(transparent)]
pub struct LockedCell<T: ?Sized, L: NestingLock> {
    _phantom: PhantomData<L>,
    value: UnsafeCell<T>,
}

unsafe impl<T: ?Sized, L: NestingLock> Send for LockedCell<T, L>
where
    T: Send,
    L: Send,
{
}

unsafe impl<T: ?Sized, L: NestingLock> Sync for LockedCell<T, L> where L: Sync {}

impl<T, L: NestingLock> LockedCell<T, L> {
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

impl<T: Copy, L: NestingLock> LockedCell<T, L> {
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

impl<T: ?Sized, L: NestingLock> LockedCell<T, L> {
    #[inline]
    pub const fn as_ptr(&self) -> *mut T {
        self.value.get()
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }

    #[inline]
    pub fn from_mut(t: &mut T) -> &LockedCell<T, L> {
        unsafe { &*(t as *mut T as *const LockedCell<T, L>) }
    }
}

impl<T: Default, L: NestingLock> LockedCell<T, L> {
    pub fn take(&self, key: L::Key<'_>) -> T {
        self.replace(key, Default::default())
    }
}

impl<T: Default, L: NestingLock> Default for LockedCell<T, L> {
    #[inline]
    fn default() -> LockedCell<T, L> {
        LockedCell::new(Default::default())
    }
}

impl<T, L: NestingLock> From<T> for LockedCell<T, L> {
    fn from(t: T) -> LockedCell<T, L> {
        LockedCell::new(t)
    }
}

impl<T, L: NestingLock> LockedCell<[T], L> {
    pub fn as_slice_of_cells(&self) -> &[LockedCell<T, L>] {
        // SAFETY: `LockedCell<T, L>` has the same memory layout as `T`
        unsafe { &*(self as *const LockedCell<[T], L> as *const [LockedCell<T, L>]) }
    }
}

impl<T, L: NestingLock, const N: usize> LockedCell<[T; N], L> {
    pub fn as_array_of_cells(&self) -> &[LockedCell<T, L>; N] {
        // SAFETY: `LockedCell<T, L>` has the same memory layout as `T`
        unsafe { &*(self as *const LockedCell<[T; N], L> as *const [LockedCell<T, L>; N]) }
    }
}

pub struct LockedRefCell<T: ?Sized, L: NestingLock> {
    _phantom: PhantomData<L>,
    value: RefCell<T>,
}

impl<T, L: NestingLock> LockedRefCell<T, L> {
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

impl<T: ?Sized, L: NestingLock> LockedRefCell<T, L> {
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

impl<T: Default, L: NestingLock> LockedRefCell<T, L> {
    pub fn take<'key>(&self, _key: L::Key<'key>) -> T {
        self.value.replace(Default::default())
    }
}

unsafe impl<T: ?Sized, L: NestingLock> Send for LockedRefCell<T, L>
where
    T: Send,
    L: Send,
{
}

unsafe impl<T: ?Sized, L: NestingLock> Sync for LockedRefCell<T, L> where L: Sync {}

impl<T: Default, L: NestingLock> Default for LockedRefCell<T, L> {
    #[inline]
    fn default() -> LockedRefCell<T, L> {
        LockedRefCell::new(Default::default())
    }
}

impl<T, L: NestingLock> From<T> for LockedRefCell<T, L> {
    fn from(t: T) -> LockedRefCell<T, L> {
        LockedRefCell::new(t)
    }
}

pub struct LockedOnceCell<T, L: NestingLock> {
    once: Once,
    value: LockedCell<MaybeUninit<T>, L>,
    _phantom: PhantomData<(L, T)>,
}

impl<T, L: NestingLock> LockedOnceCell<T, L> {
    pub const fn new() -> LockedOnceCell<T, L> {
        LockedOnceCell {
            once: Once::new(),
            value: LockedCell::new(MaybeUninit::uninit()),
            _phantom: PhantomData,
        }
    }

    pub fn get(&self) -> Option<&T> {
        if self.once.is_completed() {
            let _key = unsafe { L::get_key_unchecked() };
            let value = unsafe { (&*self.value.as_ptr()).assume_init_ref() };
            Some(value)
        } else {
            None
        }
    }

    pub fn get_mut(&mut self) -> Option<&mut T> {
        if self.once.is_completed() {
            let _key = unsafe { L::get_key_unchecked() };
            let value = unsafe { (&mut *self.value.as_ptr()).assume_init_mut() };
            Some(value)
        } else {
            None
        }
    }

    pub fn set(&self, key: L::Key<'_>, value: T) -> Result<(), T> {
        let mut value = Some(value);
        self.once.call_once(|| {
            let value = MaybeUninit::new(value.take().unwrap());
            self.value.set(key, value);
        });
        match value {
            None => Ok(()),
            Some(value) => Err(value),
        }
    }

    pub fn get_or_init(&self, key: L::Key<'_>, init: impl FnOnce() -> T) -> &T {
        if !self.once.is_completed() {
            let value = init();
            let _ = self.set(key, value);
        }
        let value = unsafe { (&*self.value.as_ptr()).assume_init_ref() };
        value
    }

    pub fn into_inner(self) -> Option<T> {
        if self.once.is_completed() {
            let key = unsafe { L::get_key_unchecked() };
            let value = unsafe { self.value.replace(key, MaybeUninit::uninit()).assume_init() };
            Some(value)
        } else {
            None
        }
    }

    pub fn take(&mut self) -> Option<T> {
        if self.once.is_completed() {
            let key = unsafe { L::get_key_unchecked() };
            let value = unsafe { self.value.replace(key, MaybeUninit::uninit()).assume_init() };
            Some(value)
        } else {
            None
        }
    }
}
