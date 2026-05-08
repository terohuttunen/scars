use super::list::{StorageListHead, StorageListNode};
use core::marker::PhantomData;

/// RAII shared-borrow guard returned by [`super::StorageListHead::borrow`] /
/// [`super::LocalStorage::borrow`]. Derefs to `&T`. Reinserts the node
/// into the storage when dropped.
pub struct LocalRef<'h, T: 'static> {
    pub(super) head: &'h StorageListHead,
    pub(super) node: &'static StorageListNode,
    pub(super) _phantom: PhantomData<&'h T>,
}

impl<T: 'static> core::ops::Deref for LocalRef<'_, T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        // SAFETY: the node is unlinked from the storage list, so no
        // other path through the storage can produce an aliasing
        // reference to this value while the guard is alive.
        unsafe { &*(self.node.value.get() as *const T) }
    }
}

impl<T: 'static> Drop for LocalRef<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.head.push_node(self.node);
    }
}

/// RAII exclusive-borrow guard returned by
/// [`super::StorageListHead::borrow_mut`] /
/// [`super::LocalStorage::borrow_mut`]. Derefs to `&mut T`. Reinserts
/// the node into the storage when dropped.
pub struct LocalRefMut<'h, T: 'static> {
    pub(super) head: &'h StorageListHead,
    pub(super) node: &'static StorageListNode,
    pub(super) _phantom: PhantomData<&'h mut T>,
}

impl<T: 'static> core::ops::Deref for LocalRefMut<'_, T> {
    type Target = T;
    #[inline(always)]
    fn deref(&self) -> &T {
        // SAFETY: as for `LocalRef::deref`.
        unsafe { &*(self.node.value.get() as *const T) }
    }
}

impl<T: 'static> core::ops::DerefMut for LocalRefMut<'_, T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as for `LocalRef::deref`. `&mut self` rules out a
        // simultaneous shared borrow through this guard.
        unsafe { &mut *(self.node.value.get() as *mut T) }
    }
}

impl<T: 'static> Drop for LocalRefMut<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.head.push_node(self.node);
    }
}

#[cfg(test)]
mod tests {
    use super::super::cell::ConstLocalCell;
    use super::*;

    #[test_case]
    fn borrow_basic() {
        static CELL: ConstLocalCell<u32> = ConstLocalCell::new(7);
        let head = StorageListHead::new();
        head.put(CELL.take());

        // borrow + Deref
        {
            let r = head.borrow::<u32>().unwrap();
            assert_eq!(*r, 7);
        }
        // After drop, the value is reachable again.
        assert_eq!(head.get::<u32>(), Some(7));
    }

    #[test_case]
    fn borrow_mut_basic_and_persists() {
        static CELL: ConstLocalCell<u32> = ConstLocalCell::new(0);
        let head = StorageListHead::new();
        head.put(CELL.take());

        {
            let mut g = head.borrow_mut::<u32>().unwrap();
            *g += 100;
        }
        assert_eq!(head.get::<u32>(), Some(100));
    }

    #[test_case]
    fn borrow_blocks_other_access_for_same_type() {
        static CELL: ConstLocalCell<u32> = ConstLocalCell::new(5);
        let head = StorageListHead::new();
        head.put(CELL.take());

        let g = head.borrow::<u32>().unwrap();

        // Every other access path for the same T returns None / Err
        // while the guard is alive (the node is unlinked).
        assert!(head.get::<u32>().is_none());
        assert!(head.as_ptr::<u32>().is_none());
        assert!(head.contains::<u32>() == false);
        assert!(head.set::<u32>(99).is_err());
        assert!(head.replace::<u32>(99).is_err());
        assert!(head.with::<u32, _>(|_| ()).is_none());
        assert!(head.with_mut::<u32, _>(|_| ()).is_none());
        assert!(head.borrow::<u32>().is_none());
        assert!(head.borrow_mut::<u32>().is_none());
        assert!(head.remove::<u32>().is_none());

        // The original guard is still valid.
        assert_eq!(*g, 5);
        drop(g);

        // Once dropped, normal access resumes.
        assert!(head.contains::<u32>());
        assert_eq!(head.get::<u32>(), Some(5));
    }

    #[test_case]
    fn borrow_mut_does_not_block_other_types() {
        struct A(u32);
        struct B(u32);
        static A_CELL: ConstLocalCell<A> = ConstLocalCell::new(A(1));
        static B_CELL: ConstLocalCell<B> = ConstLocalCell::new(B(2));
        let head = StorageListHead::new();
        head.put(A_CELL.take());
        head.put(B_CELL.take());

        let mut a = head.borrow_mut::<A>().unwrap();
        a.0 = 11;
        // B is independent — both kinds of access work.
        let b = head.borrow::<B>().unwrap();
        assert_eq!(b.0, 2);
        drop(b);
        head.with::<B, _>(|b| assert_eq!(b.0, 2));
        drop(a);

        assert_eq!(head.with::<A, _>(|a| a.0), Some(11));
    }

    #[test_case]
    fn borrow_returns_none_when_absent() {
        struct Missing;
        let head = StorageListHead::new();
        assert!(head.borrow::<Missing>().is_none());
        assert!(head.borrow_mut::<Missing>().is_none());
    }
}
