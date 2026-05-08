use super::borrow::{LocalRef, LocalRefMut};
use super::cell::{ConstLocalCell, LocalCell, LocalHandle};
use super::publish::{Publish, PublishCtx};
use core::any::TypeId;
use core::cell::Cell;
use core::marker::PhantomData;

pub(crate) struct StorageListNode {
    pub(super) next: Cell<*const StorageListNode>,
    pub(super) key: TypeId,
    pub(super) value: Cell<*mut ()>,
}

impl StorageListNode {
    pub(super) const fn new(key: TypeId) -> StorageListNode {
        StorageListNode {
            next: Cell::new(core::ptr::null()),
            key,
            value: Cell::new(core::ptr::null_mut()),
        }
    }
}

/// Linked list of [`StorageListNode`]s keyed by [`TypeId`].
pub(crate) struct StorageListHead {
    head: Cell<*const StorageListNode>,
}

impl StorageListHead {
    pub(crate) const fn new() -> StorageListHead {
        StorageListHead {
            head: Cell::new(core::ptr::null()),
        }
    }

    #[inline(never)]
    fn find(&self, key: TypeId) -> Option<&'static StorageListNode> {
        let mut cur = self.head.get();
        while !cur.is_null() {
            // SAFETY: nodes are 'static (cells are 'static and only
            // pushed via &'static methods).
            let node: &'static StorageListNode = unsafe { &*cur };
            if node.key == key {
                return Some(node);
            }
            cur = node.next.get();
        }
        None
    }

    #[inline(never)]
    pub(super) fn push_node(&self, node: &'static StorageListNode) {
        // Storage is indexed by type: a different node already mapping
        // this `TypeId` would shadow lookups and is a configuration bug.
        if self.find(node.key).is_some() {
            panic!("LocalStorage already contains an entry of this type");
        }
        node.next.set(self.head.get());
        self.head.set(node as *const _);
    }

    /// Walk the list and unlink the node whose key matches `key`. Clears
    /// the unlinked node's `next` pointer so it can be re-pushed later.
    #[inline(never)]
    pub(super) fn unlink_by_key(&self, key: TypeId) -> Option<&'static StorageListNode> {
        let mut prev_link: &Cell<*const StorageListNode> = &self.head;
        loop {
            let cur = prev_link.get();
            if cur.is_null() {
                return None;
            }
            // SAFETY: nodes on the list are 'static.
            let node: &'static StorageListNode = unsafe { &*cur };
            if node.key == key {
                prev_link.set(node.next.get());
                node.next.set(core::ptr::null());
                return Some(node);
            }
            prev_link = &node.next;
        }
    }

    /// Returns `true` if a value of type `T` is stored.
    #[inline]
    pub fn contains<T: 'static>(&self) -> bool {
        self.find(TypeId::of::<T>()).is_some()
    }

    /// Returns a raw pointer to the stored `T`, or `None` if absent.
    ///
    /// Obtaining the pointer is safe; dereferencing it is `unsafe` and
    /// caller must respect Rust's aliasing rules. Prefer [`with`] /
    /// [`with_mut`] / [`get`] / [`set`] when they fit.
    ///
    /// [`with`]: Self::with
    /// [`with_mut`]: Self::with_mut
    /// [`get`]: Self::get
    /// [`set`]: Self::set
    #[inline]
    pub fn as_ptr<T: 'static>(&self) -> Option<*mut T> {
        self.find(TypeId::of::<T>())
            .map(|n| n.value.get() as *mut T)
    }

    /// Read a `Copy` value out of the storage.
    #[inline]
    pub fn get<T: 'static + Copy>(&self) -> Option<T> {
        // SAFETY: copies bytes by value without retaining any reference;
        // no aliasing concern. The pointer points into a 'static cell.
        self.as_ptr::<T>().map(|p| unsafe { p.read() })
    }

    /// Write `val` into the stored `T`, dropping the previous value.
    /// Returns `Err(val)` (handing the input back) if no entry of type
    /// `T` exists.
    #[inline]
    pub fn set<T: 'static>(&self, val: T) -> Result<(), T> {
        match self.as_ptr::<T>() {
            Some(p) => {
                // SAFETY: the head is only accessed from one execution
                // context at a time (per-priority isolation). `with` /
                // `with_mut` of the same `T` would have unlinked the
                // node, so `find` would have returned `None` and we
                // would not be here. So no live `&T`/`&mut T` from those
                // safe APIs aliases this write.
                unsafe { p.write(val) };
                Ok(())
            }
            None => Err(val),
        }
    }

    /// Write `val` into the stored `T`, returning the previous value.
    /// Returns `Err(val)` (handing the input back) if no entry of type
    /// `T` exists.
    #[inline]
    pub fn replace<T: 'static>(&self, val: T) -> Result<T, T> {
        match self.as_ptr::<T>() {
            // SAFETY: same justification as `set`.
            Some(p) => Ok(unsafe { p.replace(val) }),
            None => Err(val),
        }
    }

    /// Run `f` with shared access to the stored `T`.
    ///
    /// The node holding `T` is unlinked from this storage for the
    /// duration of the call — any nested `get` / `with` / `with_mut` of
    /// the same type returns `None` while `f` runs. The node is
    /// reinserted at the head before this method returns; the list may
    /// be reordered as a side effect (lookups are by `TypeId`, so
    /// position is not load-bearing).
    #[inline]
    pub fn with<T: 'static, R>(&self, f: impl FnOnce(&T) -> R) -> Option<R> {
        let node = self.unlink_by_key(TypeId::of::<T>())?;
        let _guard = Reinsert { head: self, node };
        // SAFETY: see the safety note on `with_mut` — the node is unlinked,
        // so no other path through this head can reach the value.
        let value = unsafe { &*(node.value.get() as *const T) };
        Some(f(value))
    }

    /// Run `f` with exclusive mutable access to the stored `T`.
    ///
    /// Like [`with`](Self::with) but hands `f` an `&mut T`.
    #[inline]
    pub fn with_mut<T: 'static, R>(&self, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let node = self.unlink_by_key(TypeId::of::<T>())?;
        let _guard = Reinsert { head: self, node };
        // SAFETY: the node is unlinked, so no other path through this
        // storage head can reach this value during `f`. The head is only
        // accessed from one execution context at a time (per-priority
        // isolation), so there is no concurrent aliasing either.
        let value = unsafe { &mut *(node.value.get() as *mut T) };
        Some(f(value))
    }

    // ---- Handle API ----

    /// Link a previously-initialized cell into this list. Consumes the
    /// handle.
    ///
    /// Panics if a different cell already maps this `TypeId` in this list.
    #[inline]
    pub fn put<T: 'static>(&self, handle: LocalHandle<T>) {
        self.push_node(handle.into_node());
    }

    /// Find and unlink the entry of type `T`. Returns the owning handle.
    ///
    /// The cell stays initialized — the handle can be `put` here or into
    /// any other [`StorageListHead`].
    #[inline]
    #[allow(dead_code)]
    pub fn remove<T: 'static>(&self) -> Option<LocalHandle<T>> {
        let node = self.unlink_by_key(TypeId::of::<T>())?;
        Some(LocalHandle::from_node(node))
    }

    // ---- Direct API (handle hidden) ----

    /// Initialize and link a [`LocalCell`] in one step.
    ///
    /// For setup that needs `&mut T` after install, use
    /// [`with_mut`](Self::with_mut).
    #[inline]
    pub fn put_init<T: 'static>(&self, cell: &'static LocalCell<T>, val: T) {
        // Slot check before cell.init so a collision cannot leave the
        // cell initialized-but-unlinked.
        self.assert_slot_empty::<T>();
        self.put(cell.init(val));
    }

    /// Like [`put_init`](Self::put_init) but the value is produced by a
    /// closure (lazy init).
    #[inline]
    pub fn put_init_with<T: 'static>(&self, cell: &'static LocalCell<T>, init: impl FnOnce() -> T) {
        self.assert_slot_empty::<T>();
        self.put(cell.init_with(init));
    }

    /// Take and link a [`ConstLocalCell`] in one step.
    ///
    /// For setup that needs `&mut T` after install, use
    /// [`with_mut`](Self::with_mut).
    #[inline]
    pub fn put_take<T: 'static>(&self, cell: &'static ConstLocalCell<T>) {
        self.assert_slot_empty::<T>();
        self.put(cell.take());
    }

    #[inline(never)]
    fn assert_slot_empty<T: 'static>(&self) {
        if self.find(TypeId::of::<T>()).is_some() {
            panic!(
                "LocalStorage already contains an entry of type {}",
                core::any::type_name::<T>()
            );
        }
    }

    /// Borrow the stored `T` for as long as the returned guard is alive.
    ///
    /// The node holding `T` is unlinked from this storage for the
    /// lifetime of the guard — any other `get` / `with` / `with_mut` /
    /// `borrow` / `borrow_mut` of the same type returns `None` until
    /// the guard is dropped.
    ///
    /// # Caveats
    ///
    /// While a borrow is held, calling `put_init` / `put_init_with` /
    /// `put_take` / `put` for the same `T` will succeed (the borrow has
    /// already removed the node), and the guard's `Drop` will then
    /// panic when it tries to reinsert into a slot that's now occupied.
    /// The same precondition applies as for `with` / `with_mut`.
    #[inline]
    pub fn borrow<T: 'static>(&self) -> Option<LocalRef<'_, T>> {
        let node = self.unlink_by_key(TypeId::of::<T>())?;
        Some(LocalRef {
            head: self,
            node,
            _phantom: PhantomData,
        })
    }

    /// Mutably borrow the stored `T` for as long as the returned guard
    /// is alive. See [`borrow`](Self::borrow) for the unlink discipline.
    #[inline]
    pub fn borrow_mut<T: 'static>(&self) -> Option<LocalRefMut<'_, T>> {
        let node = self.unlink_by_key(TypeId::of::<T>())?;
        Some(LocalRefMut {
            head: self,
            node,
            _phantom: PhantomData,
        })
    }

    /// Publish `src` into this head, returning `Err` on the first
    /// failure surfaced by `src`'s `try_publish_to`. See [`Publish`]
    /// for the writer-side contract.
    #[inline]
    pub fn try_publish<P: Publish>(&self, src: &'static P) -> Result<(), P::Error> {
        src.try_publish_to(&mut PublishCtx::new(self))
    }

    /// Publish `src` into this head, panicking on failure. Convenience
    /// wrapper over [`try_publish`](Self::try_publish).
    #[inline]
    pub fn publish<P: Publish>(&self, src: &'static P)
    where
        P::Error: core::fmt::Debug,
    {
        src.publish_to(&mut PublishCtx::new(self));
    }
}

// RAII guard used by `StorageListHead::with` / `with_mut`: reinserts
// the unlinked node at the head when the guard drops, even if the
// closure panics. (In no_std builds with panic=abort the panic case is
// academic, but the guard costs nothing.)
struct Reinsert<'a> {
    head: &'a StorageListHead,
    node: &'static StorageListNode,
}

impl Drop for Reinsert<'_> {
    fn drop(&mut self) {
        self.head.push_node(self.node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn with_mut_basic_mutate() {
        static CELL: ConstLocalCell<u32> = ConstLocalCell::new(7);
        let head = StorageListHead::new();

        head.put(CELL.take());

        let prev = head.with_mut::<u32, _>(|x| {
            let p = *x;
            *x += 35;
            p
        });
        assert_eq!(prev, Some(7));
        assert_eq!(head.get::<u32>(), Some(42));
    }

    #[test_case]
    fn with_mut_reentrant_lookup_returns_none() {
        static CELL: ConstLocalCell<u64> = ConstLocalCell::new(0);
        let head = StorageListHead::new();

        head.put(CELL.take());

        let inner_get_was_none = head
            .with_mut::<u64, _>(|outer| {
                *outer = 1;
                let nested_get = head.get::<u64>();
                let nested_with = head.with_mut::<u64, _>(|_| {
                    panic!("inner closure must not run while node is unlinked");
                });
                nested_get.is_none() && nested_with.is_none()
            })
            .unwrap();
        assert!(inner_get_was_none);
        // Outer mutation persisted.
        assert_eq!(head.get::<u64>(), Some(1));
    }

    #[test_case]
    fn with_mut_no_value_present_returns_none() {
        struct Absent(#[allow(dead_code)] u8);
        let head = StorageListHead::new();

        // Empty storage: no node of any type.
        let r = head.with_mut::<Absent, _>(|_| panic!("must not run"));
        assert!(r.is_none());
    }

    #[test_case]
    fn with_mut_other_types_unaffected() {
        struct A(u32);
        struct B(u32);
        static A_CELL: ConstLocalCell<A> = ConstLocalCell::new(A(11));
        static B_CELL: ConstLocalCell<B> = ConstLocalCell::new(B(22));
        let head = StorageListHead::new();

        head.put(A_CELL.take());
        head.put(B_CELL.take());

        let saw_b_inside = head
            .with_mut::<A, _>(|a| {
                a.0 = 111;
                head.with::<B, _>(|b| b.0)
            })
            .unwrap();
        assert_eq!(saw_b_inside, Some(22));
        assert_eq!(head.with::<A, _>(|a| a.0), Some(111));
        assert_eq!(head.with::<B, _>(|b| b.0), Some(22));
    }

    #[test_case]
    fn with_mut_reachability_after_return() {
        struct X(u32);
        struct Y(u32);
        struct Z(u32);
        static X_CELL: ConstLocalCell<X> = ConstLocalCell::new(X(1));
        static Y_CELL: ConstLocalCell<Y> = ConstLocalCell::new(Y(2));
        static Z_CELL: ConstLocalCell<Z> = ConstLocalCell::new(Z(3));
        let head = StorageListHead::new();

        // Inserted in order X, Y, Z. (push_node prepends, so head order
        // ends up Z, Y, X — but lookups don't care.)
        head.put(X_CELL.take());
        head.put(Y_CELL.take());
        head.put(Z_CELL.take());

        // Touch the middle one. Allowed to reorder; all three must remain
        // reachable after return.
        head.with_mut::<Y, _>(|y| y.0 = 200).unwrap();

        assert_eq!(head.with::<X, _>(|v| v.0), Some(1));
        assert_eq!(head.with::<Y, _>(|v| v.0), Some(200));
        assert_eq!(head.with::<Z, _>(|v| v.0), Some(3));
    }

    #[test_case]
    fn set_and_replace_for_copy_value() {
        static CELL: ConstLocalCell<u32> = ConstLocalCell::new(10);
        let head = StorageListHead::new();

        head.put(CELL.take());

        // set drops the old value and writes the new one.
        assert_eq!(head.set::<u32>(20), Ok(()));
        assert_eq!(head.get::<u32>(), Some(20));

        // replace returns the old value.
        assert_eq!(head.replace::<u32>(30), Ok(20));
        assert_eq!(head.get::<u32>(), Some(30));

        // set/replace return Err(val) when the type isn't stored.
        struct Missing(u8);
        assert!(head.set::<Missing>(Missing(7)).is_err());
        assert!(head.replace::<Missing>(Missing(7)).is_err());
    }

    #[test_case]
    fn as_ptr_returns_none_when_absent() {
        struct Nope;
        let head = StorageListHead::new();
        assert!(head.as_ptr::<Nope>().is_none());
    }

    // -------- Comprehensive monomorphization audit --------
    //
    // Each `T*` type forces a distinct generic instantiation so the
    // per-`T` symbols are visible in the test binary and can be
    // inspected with `nm` to verify the `#[inline]` annotation effect.

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT1(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT2(u64);
    #[derive(Debug, PartialEq, Eq)]
    struct AT3(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT4(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT5(u64);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT6(u32);

    static AT1_CELL: ConstLocalCell<AT1> = ConstLocalCell::new(AT1(1));
    static AT2_CELL: ConstLocalCell<AT2> = ConstLocalCell::new(AT2(2));
    static AT3_CELL: ConstLocalCell<AT3> = ConstLocalCell::new(AT3(3));
    static AT4_CELL: LocalCell<AT4> = LocalCell::new();
    static AT5_CELL: LocalCell<AT5> = LocalCell::new();

    #[test_case]
    fn audit_storage_list_head_methods() {
        let head = StorageListHead::new();
        head.put(AT1_CELL.take());
        head.put_take::<AT2>(&AT2_CELL);
        head.put::<AT3>(AT3_CELL.take());
        head.put_init::<AT4>(&AT4_CELL, AT4(4));
        head.put_init_with::<AT5>(&AT5_CELL, || AT5(5));

        assert!(head.contains::<AT1>());
        assert_eq!(head.get::<AT1>(), Some(AT1(1)));
        assert_eq!(head.get::<AT2>(), Some(AT2(2)));
        assert!(head.as_ptr::<AT1>().is_some());

        head.set::<AT1>(AT1(11)).unwrap();
        assert_eq!(head.replace::<AT1>(AT1(111)), Ok(AT1(11)));
        head.set::<AT4>(AT4(44)).unwrap();
        head.set::<AT5>(AT5(55)).unwrap();

        head.with::<AT1, _>(|v| assert_eq!(*v, AT1(111)));
        head.with::<AT3, _>(|v| assert_eq!(*v, AT3(3)));
        head.with_mut::<AT2, _>(|v| v.0 = 22);
        head.with_mut::<AT3, _>(|v| v.0 = 33);
        assert_eq!(head.get::<AT2>(), Some(AT2(22)));
        head.with::<AT3, _>(|v| assert_eq!(*v, AT3(33)));

        let h = head.remove::<AT4>().unwrap();
        head.put::<AT4>(h);
        assert!(head.contains::<AT4>());

        {
            let r1 = head.borrow::<AT1>().unwrap();
            assert_eq!(*r1, AT1(111));
        }
        {
            let mut r2 = head.borrow_mut::<AT2>().unwrap();
            r2.0 = 222;
        }
        {
            let r3 = head.borrow::<AT3>().unwrap();
            assert_eq!(r3.0, 33);
        }

        assert!(!head.contains::<AT6>());
        assert!(head.as_ptr::<AT6>().is_none());
        assert_eq!(head.get::<AT6>(), None);
        assert!(head.set::<AT6>(AT6(0)).is_err());
        assert!(head.replace::<AT6>(AT6(0)).is_err());
        assert!(head.with::<AT6, _>(|_| ()).is_none());
        assert!(head.with_mut::<AT6, _>(|_| ()).is_none());
        assert!(head.borrow::<AT6>().is_none());
        assert!(head.borrow_mut::<AT6>().is_none());
        assert!(head.remove::<AT6>().is_none());
    }
}
