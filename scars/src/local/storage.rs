use super::borrow::{LocalRef, LocalRefMut};
use super::cell::{ConstLocalCell, LocalCell, LocalHandle};
use super::list::{StorageListHead, StorageListNode};
use super::publish::Publish;
use crate::Priority;
use crate::kernel::{Scheduler, scheduler::ExecutionContext};
use core::any::TypeId;

/// An owner's local storage slot. Either owns a list, or redirects to
/// another owner's list. The owned/shared distinction is an
/// implementation detail; user code interacts via the static-dispatch
/// methods below or via [`share_with`](Self::share_with).
pub struct LocalStorage {
    storage: Storage,
}

enum Storage {
    Owned(StorageListHead),
    Shared(&'static StorageListHead),
}

impl LocalStorage {
    pub const fn new() -> LocalStorage {
        LocalStorage {
            storage: Storage::Owned(StorageListHead::new()),
        }
    }

    /// Return the head this owner is currently using.
    ///
    /// Crate-internal: user code reaches the namespace through
    /// [`LocalStorage`]'s static-dispatch methods (`get` / `with` /
    /// `publish` / etc.) or via [`StorageListHead::publish`] /
    /// [`StorageListHead::try_publish`] off a builder. Bypassing those
    /// to operate on the raw head is reserved for crate plumbing.
    #[inline(always)]
    pub(crate) fn head(&self) -> &StorageListHead {
        match &self.storage {
            Storage::Owned(h) => h,
            Storage::Shared(h) => h,
        }
    }

    /// Redirect this owner's storage to `target`. Subsequent lookups
    /// against this `LocalStorage` resolve to `target` instead of the
    /// originally owned list.
    #[inline(always)]
    pub(crate) fn share_with(&mut self, target: &'static StorageListHead) {
        self.storage = Storage::Shared(target);
    }

    // ---- Static dispatch: access storage of the currently executing context ----
    //
    // The execution-context dispatch lives in the non-generic
    // `current_head`. Per-`T` methods are thin wrappers that delegate to
    // the corresponding `StorageListHead` method, monomorphized per `T`
    // for the type-specific work (cast / read / write).

    /// Returns `true` if the current execution context's storage holds a
    /// value of type `T`.
    #[inline(always)]
    pub fn contains<T: 'static>() -> bool {
        Self::current_head().contains::<T>()
    }

    /// Returns a raw pointer to the stored `T` from the current
    /// execution context. See [`StorageListHead::as_ptr`].
    #[inline(always)]
    pub fn as_ptr<T: 'static>() -> Option<*mut T> {
        Self::current_head().as_ptr::<T>()
    }

    /// Read a `Copy` value out of the current context's storage.
    #[inline(always)]
    pub fn get<T: 'static + Copy>() -> Option<T> {
        Self::current_head().get::<T>()
    }

    /// Write `val` into the current context's stored `T`, dropping the
    /// previous value. Returns `Err(val)` if no entry of type `T` exists.
    #[inline(always)]
    pub fn set<T: 'static>(val: T) -> Result<(), T> {
        Self::current_head().set::<T>(val)
    }

    /// Write `val` into the current context's stored `T`, returning the
    /// previous value. Returns `Err(val)` if no entry of type `T` exists.
    #[inline(always)]
    pub fn replace<T: 'static>(val: T) -> Result<T, T> {
        Self::current_head().replace::<T>(val)
    }

    /// Run `f` with shared access to the current execution context's
    /// stored `T`. See [`StorageListHead::with`] for the re-entrancy
    /// semantics.
    #[inline(always)]
    pub fn with<T: 'static, R>(f: impl FnOnce(&T) -> R) -> Option<R> {
        Self::current_head().with::<T, R>(f)
    }

    /// Run `f` with exclusive mutable access to the current execution
    /// context's stored `T`. See [`StorageListHead::with_mut`] for the
    /// re-entrancy semantics.
    #[inline(always)]
    pub fn with_mut<T: 'static, R>(f: impl FnOnce(&mut T) -> R) -> Option<R> {
        Self::current_head().with_mut::<T, R>(f)
    }

    /// Link a previously-initialized cell into the current context's
    /// storage. Consumes the handle. Panics if a cell of type `T` is
    /// already present.
    #[inline(always)]
    pub fn put<T: 'static>(handle: LocalHandle<T>) {
        Self::current_push_node(handle.into_node());
    }

    /// Unlink the entry of type `T` from the current context's storage,
    /// returning its owning handle. The cell stays initialized; the
    /// handle can be `put` here or into any other storage.
    #[inline(always)]
    pub fn remove<T: 'static>() -> Option<LocalHandle<T>> {
        let node = Self::current_unlink_by_key(TypeId::of::<T>())?;
        Some(LocalHandle::from_node(node))
    }

    /// Take and link a [`ConstLocalCell`] into the current context's
    /// storage in one step.
    #[inline(always)]
    pub fn put_take<T: 'static>(cell: &'static ConstLocalCell<T>) {
        let handle = cell.take();
        Self::current_push_node(handle.into_node());
    }

    /// Initialize a [`LocalCell`] with `val` and link it into the
    /// current context's storage in one step.
    #[inline(always)]
    pub fn put_init<T: 'static>(cell: &'static LocalCell<T>, val: T) {
        let handle = cell.init(val);
        Self::current_push_node(handle.into_node());
    }

    /// Initialize a [`LocalCell`] with `init()` and link it into the
    /// current context's storage in one step.
    #[inline(always)]
    pub fn put_init_with<T: 'static>(cell: &'static LocalCell<T>, init: impl FnOnce() -> T) {
        let handle = cell.init_with(init);
        Self::current_push_node(handle.into_node());
    }

    /// Borrow the current context's stored `T` for as long as the
    /// returned guard is alive. See [`StorageListHead::borrow`].
    #[inline(always)]
    pub fn borrow<T: 'static>() -> Option<LocalRef<'static, T>> {
        Self::current_head().borrow::<T>()
    }

    /// Mutably borrow the current context's stored `T` for as long as
    /// the returned guard is alive. See [`StorageListHead::borrow_mut`].
    #[inline(always)]
    pub fn borrow_mut<T: 'static>() -> Option<LocalRefMut<'static, T>> {
        Self::current_head().borrow_mut::<T>()
    }

    /// Publish `src` into the current execution context's storage.
    /// See [`StorageListHead::try_publish`].
    #[inline(always)]
    pub fn try_publish<P: Publish>(src: &'static P) -> Result<(), P::Error> {
        Self::current_head().try_publish(src)
    }

    /// Publish `src` into the current execution context's storage.
    /// Panics on failure. See [`StorageListHead::publish`].
    #[inline(always)]
    pub fn publish<P: Publish>(src: &'static P)
    where
        P::Error: core::fmt::Debug,
    {
        Self::current_head().publish(src);
    }

    // Non-generic dispatch helpers. Single copy regardless of how many
    // distinct types are stored. `#[inline(never)]` keeps them as one
    // standalone function each — without it, LLVM would inline them into
    // every per-`T` wrapper above, undoing the whole point of factoring
    // them out.

    #[inline(never)]
    fn current_head() -> &'static StorageListHead {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(interrupt) => {
                interrupt.get_ref().current_local_storage().head()
            }
            ExecutionContext::Thread(thread) => thread.get_ref().local_storage.head(),
        }
    }

    #[inline(never)]
    fn current_push_node(node: &'static StorageListNode) {
        Self::current_head().push_node(node);
    }

    #[inline(never)]
    fn current_unlink_by_key(key: TypeId) -> Option<&'static StorageListNode> {
        Self::current_head().unlink_by_key(key)
    }
}

/// Handle to a shared local-storage namespace at priority `PRIO`.
#[derive(Copy, Clone)]
pub struct SharedStorage<const PRIO: Priority> {
    head: &'static StorageListHead,
}

impl<const PRIO: Priority> SharedStorage<PRIO> {
    /// # Safety
    /// Caller asserts that all sharers of `head` run mutually
    /// exclusively at priority `PRIO` (no preemption between them).
    pub(crate) const unsafe fn from_head(head: &'static StorageListHead) -> Self {
        Self { head }
    }

    pub(crate) const fn head(&self) -> &'static StorageListHead {
        self.head
    }
}

/// Source of a [`SharedStorage`] for same-priority storage sharing.
pub trait SharedStorageProvider<const PRIO: Priority> {
    fn shared_storage(&self) -> SharedStorage<PRIO>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Static-dispatch surface, exercised against the test thread's
    // own local storage.

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AU1(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AU2(u64);
    #[derive(Debug, PartialEq, Eq)]
    struct AU3(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AU4(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AU5(u64);

    static AU1_CELL: ConstLocalCell<AU1> = ConstLocalCell::new(AU1(1));
    static AU2_CELL: ConstLocalCell<AU2> = ConstLocalCell::new(AU2(2));
    static AU3_CELL: ConstLocalCell<AU3> = ConstLocalCell::new(AU3(3));
    static AU4_CELL: LocalCell<AU4> = LocalCell::new();
    static AU5_CELL: LocalCell<AU5> = LocalCell::new();

    #[test_case]
    fn audit_local_storage_static_dispatch_methods() {
        LocalStorage::put_take::<AU1>(&AU1_CELL);
        LocalStorage::put::<AU2>(AU2_CELL.take());
        LocalStorage::put_take::<AU3>(&AU3_CELL);
        LocalStorage::put_init::<AU4>(&AU4_CELL, AU4(4));
        LocalStorage::put_init_with::<AU5>(&AU5_CELL, || AU5(5));

        assert!(LocalStorage::contains::<AU1>());
        assert_eq!(LocalStorage::get::<AU1>(), Some(AU1(1)));
        assert!(LocalStorage::as_ptr::<AU1>().is_some());

        LocalStorage::set::<AU1>(AU1(11)).unwrap();
        assert_eq!(LocalStorage::replace::<AU1>(AU1(111)), Ok(AU1(11)));

        LocalStorage::with::<AU1, _>(|v| assert_eq!(*v, AU1(111)));
        LocalStorage::with::<AU3, _>(|v| assert_eq!(*v, AU3(3)));
        LocalStorage::with_mut::<AU2, _>(|v| v.0 = 22);
        LocalStorage::with_mut::<AU3, _>(|v| v.0 = 33);

        let h = LocalStorage::remove::<AU4>().unwrap();
        LocalStorage::put::<AU4>(h);
        assert!(LocalStorage::contains::<AU4>());

        {
            let r = LocalStorage::borrow::<AU1>().unwrap();
            assert_eq!(*r, AU1(111));
        }
        {
            let mut r = LocalStorage::borrow_mut::<AU2>().unwrap();
            r.0 = 222;
        }

        #[derive(Copy, Clone, Debug, PartialEq, Eq)]
        struct AUMissing(u8);
        assert!(!LocalStorage::contains::<AUMissing>());
        assert!(LocalStorage::as_ptr::<AUMissing>().is_none());
        assert!(LocalStorage::get::<AUMissing>().is_none());
        assert!(LocalStorage::set::<AUMissing>(AUMissing(0)).is_err());
        assert!(LocalStorage::replace::<AUMissing>(AUMissing(0)).is_err());
        assert!(LocalStorage::with::<AUMissing, _>(|_| ()).is_none());
        assert!(LocalStorage::with_mut::<AUMissing, _>(|_| ()).is_none());
        assert!(LocalStorage::borrow::<AUMissing>().is_none());
        assert!(LocalStorage::borrow_mut::<AUMissing>().is_none());
        assert!(LocalStorage::remove::<AUMissing>().is_none());
    }
}
