//! Local storage for threads, interrupt handlers, and event handlers.
//!
//! Per-context storage keyed by type. Access uses the methods on
//! [`LocalStorage`] (which dispatch to the currently executing
//! context's storage) — see the quick reference below.
//!
//! Same-priority handlers may share a single storage list via the
//! `with_shared_storage` builder methods.
//!
//! # Quick reference
//!
//! | Goal                                  | API                                                |
//! | ------------------------------------- | -------------------------------------------------- |
//! | Check presence                        | [`LocalStorage::contains`]`::<T>()`                |
//! | Read a `Copy` value                   | [`LocalStorage::get`]`::<T>()`                     |
//! | Write (drop old)                      | [`LocalStorage::set`]`::<T>(val)`                  |
//! | Write and return the old value        | [`LocalStorage::replace`]`::<T>(val)`              |
//! | Borrow briefly in a closure (`&T`)    | [`LocalStorage::with`]`::<T, _>(\|t\| ...)`        |
//! | Mutate briefly in a closure (`&mut T`)| [`LocalStorage::with_mut`]`::<T, _>(\|t\| ...)`    |
//! | RAII shared borrow (`&T`)             | [`LocalStorage::borrow`]`::<T>()`                  |
//! | RAII exclusive borrow (`&mut T`)      | [`LocalStorage::borrow_mut`]`::<T>()`              |
//! | Raw pointer (deref is `unsafe`)       | [`LocalStorage::as_ptr`]`::<T>()`                  |
//! | Install with a runtime value          | [`LocalStorage::put_init`]`(&CELL, val)`           |
//! | Install with a lazy init closure      | [`LocalStorage::put_init_with`]`(&CELL, \|\| ...)` |
//! | Install a const-initialized cell      | [`LocalStorage::put_take`]`(&CELL)`                |
//! | Link a previously-taken cell          | [`LocalStorage::put`]`(handle)`                    |
//! | Take a cell out (move to other slot)  | [`LocalStorage::remove`]`::<T>()`                  |
//!
//! All access methods return `None` / `Err` if no entry of type `T` is
//! present. The `with` / `with_mut` closures additionally return `None`
//! when invoked re-entrantly against the same `T` (the node is unlinked
//! from the storage list while the closure runs — see *Mutability*).
//!
//! Each of the methods above also exists on [`StorageListHead`], the
//! type that owns the underlying list, for cases where you have a
//! direct handle (e.g. inside a builder).
//!
//! # Cell lifecycle
//!
//! Cells ([`LocalCell<T>`] / [`ConstLocalCell<T>`]) are first
//! *initialized* (or *taken*), yielding a move-only [`LocalHandle<T>`]
//! that represents exclusive ownership of an initialized-but-unlinked
//! cell. The handle can be passed to a storage's [`StorageListHead::put`]
//! to link the cell, and recovered later via
//! [`StorageListHead::remove`]. The same handle can then be `put`
//! into a different storage — cells move between storages over time.
//!
//! For the common one-shot pattern, [`StorageListHead::put_init`] /
//! [`StorageListHead::put_take`] hide the intermediate handle.
//!
//! # Mutability
//!
//! Two safe disciplines hand out `&T` / `&mut T`, both built on the
//! same trick: the node holding `T` is unlinked from the storage list
//! while the borrow is alive, so any nested lookup of the same type
//! returns `None`. Nothing else can reach the value until the borrow
//! ends and the node is reinserted — that's what makes the handed-out
//! reference sound.
//!
//! - **Closure-scoped** — [`LocalStorage::with`] / [`LocalStorage::with_mut`]
//!   take a closure and reinsert the node when the closure returns:
//!
//!   ```ignore
//!   LocalStorage::with_mut::<u32, _>(|c| *c += 1);
//!   ```
//!
//! - **RAII-scoped** — [`LocalStorage::borrow`] / [`LocalStorage::borrow_mut`]
//!   return a guard ([`LocalRef`] / [`LocalRefMut`]) that derefs to
//!   `&T` / `&mut T` and reinserts the node on drop. Use this when the
//!   borrow needs to span multiple statements that don't fit nicely
//!   inside one closure:
//!
//!   ```ignore
//!   let mut c = LocalStorage::borrow_mut::<u32>().unwrap();
//!   *c += 1;
//!   *c *= 2;
//!   // c dropped here, node reinserted
//!   ```
//!
//! Both disciplines forbid nested borrows of the same `T` — the second
//! call returns `None` because the first unlinked the node. To install
//! a different value of the same type while a borrow is held is also
//! forbidden: it would leave the storage with two entries for one
//! `TypeId` once the borrow drops, which the reinsert detects and
//! panics on.
//!
//! For `Copy` types, [`LocalStorage::get`] returns the value by copy
//! and [`LocalStorage::set`] / [`LocalStorage::replace`] write a new
//! value (`set` drops the old; `replace` returns it). These are safe by
//! construction — no reference is handed out.
//!
//! For cases that genuinely need a `&'static T` outliving the call,
//! [`LocalStorage::as_ptr`] returns `Option<*mut T>`. Obtaining the
//! pointer is safe; dereferencing it is `unsafe` and the caller must
//! guarantee no aliasing for the lifetime of any reference produced.

pub mod publish;
pub use publish::{Publish, PublishCtx, PublishError};

use crate::Priority;
use crate::kernel::{Scheduler, scheduler::ExecutionContext};
use core::any::TypeId;
use core::cell::Cell;
use core::marker::PhantomData;
use static_cell::{ConstStaticCell, StaticCell};

pub(crate) struct StorageListNode {
    next: Cell<*const StorageListNode>,
    key: TypeId,
    value: Cell<*mut ()>,
}

impl StorageListNode {
    pub(crate) const fn new(key: TypeId) -> StorageListNode {
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
    fn push_node(&self, node: &'static StorageListNode) {
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
    fn unlink_by_key(&self, key: TypeId) -> Option<&'static StorageListNode> {
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
        self.push_node(handle.node);
        // Handle is consumed by-value; nothing more to do.
    }

    /// Find and unlink the entry of type `T`. Returns the owning handle.
    ///
    /// The cell stays initialized — the handle can be `put` here or into
    /// any other [`StorageListHead`].
    #[inline]
    #[allow(dead_code)]
    pub fn remove<T: 'static>(&self) -> Option<LocalHandle<T>> {
        let node = self.unlink_by_key(TypeId::of::<T>())?;
        Some(LocalHandle {
            node,
            _phantom: PhantomData,
        })
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

/// RAII shared-borrow guard returned by [`StorageListHead::borrow`] /
/// [`LocalStorage::borrow`]. Derefs to `&T`. Reinserts the node into
/// the storage when dropped.
pub struct LocalRef<'h, T: 'static> {
    head: &'h StorageListHead,
    node: &'static StorageListNode,
    _phantom: PhantomData<&'h T>,
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
/// [`StorageListHead::borrow_mut`] / [`LocalStorage::borrow_mut`].
/// Derefs to `&mut T`. Reinserts the node into the storage when
/// dropped.
pub struct LocalRefMut<'h, T: 'static> {
    head: &'h StorageListHead,
    node: &'static StorageListNode,
    _phantom: PhantomData<&'h mut T>,
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
        Self::current_push_node(handle.node);
    }

    /// Unlink the entry of type `T` from the current context's storage,
    /// returning its owning handle. The cell stays initialized; the
    /// handle can be `put` here or into any other storage.
    #[inline(always)]
    pub fn remove<T: 'static>() -> Option<LocalHandle<T>> {
        let node = Self::current_unlink_by_key(TypeId::of::<T>())?;
        Some(LocalHandle {
            node,
            _phantom: PhantomData,
        })
    }

    /// Take and link a [`ConstLocalCell`] into the current context's
    /// storage in one step.
    #[inline(always)]
    pub fn put_take<T: 'static>(cell: &'static ConstLocalCell<T>) {
        let handle = cell.take();
        Self::current_push_node(handle.node);
    }

    /// Initialize a [`LocalCell`] with `val` and link it into the
    /// current context's storage in one step.
    #[inline(always)]
    pub fn put_init<T: 'static>(cell: &'static LocalCell<T>, val: T) {
        let handle = cell.init(val);
        Self::current_push_node(handle.node);
    }

    /// Initialize a [`LocalCell`] with `init()` and link it into the
    /// current context's storage in one step.
    #[inline(always)]
    pub fn put_init_with<T: 'static>(cell: &'static LocalCell<T>, init: impl FnOnce() -> T) {
        let handle = cell.init_with(init);
        Self::current_push_node(handle.node);
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

/// Storage cell with runtime initialization.
pub struct LocalCell<T: 'static> {
    node: StorageListNode,
    data: StaticCell<T>,
}

impl<T: 'static> LocalCell<T> {
    pub const fn new() -> LocalCell<T> {
        LocalCell {
            node: StorageListNode::new(TypeId::of::<T>()),
            data: StaticCell::new(),
        }
    }

    /// Initialize the cell, returning an owning handle.
    ///
    /// Panics if `init` / `init_with` was already called (the underlying
    /// [`StaticCell`] is one-shot).
    #[inline]
    pub fn init(&'static self, val: T) -> LocalHandle<T> {
        let r: &'static mut T = self.data.init(val);
        self.node.value.set(r as *mut T as *mut ());
        LocalHandle {
            node: &self.node,
            _phantom: PhantomData,
        }
    }

    #[inline]
    pub fn init_with(&'static self, init: impl FnOnce() -> T) -> LocalHandle<T> {
        let r: &'static mut T = self.data.init_with(init);
        self.node.value.set(r as *mut T as *mut ());
        LocalHandle {
            node: &self.node,
            _phantom: PhantomData,
        }
    }

    /// Initialize the cell, returning an owning handle. Returns `None`
    /// if the cell has already been initialized.
    #[inline]
    pub fn try_init(&'static self, val: T) -> Option<LocalHandle<T>> {
        let r: &'static mut T = self.data.try_init(val)?;
        self.node.value.set(r as *mut T as *mut ());
        Some(LocalHandle {
            node: &self.node,
            _phantom: PhantomData,
        })
    }

    #[inline]
    pub fn try_init_with(&'static self, init: impl FnOnce() -> T) -> Option<LocalHandle<T>> {
        let r: &'static mut T = self.data.try_init_with(init)?;
        self.node.value.set(r as *mut T as *mut ());
        Some(LocalHandle {
            node: &self.node,
            _phantom: PhantomData,
        })
    }
}

// SAFETY: `LocalCell` data is reachable only through the local storage
// APIs, which are per-context and never concurrent.
unsafe impl<T> Sync for LocalCell<T> {}

/// Storage cell with const initialization.
pub struct ConstLocalCell<T: 'static> {
    node: StorageListNode,
    data: ConstStaticCell<T>,
}

impl<T: 'static> ConstLocalCell<T> {
    pub const fn new(data: T) -> ConstLocalCell<T> {
        ConstLocalCell {
            node: StorageListNode::new(TypeId::of::<T>()),
            data: ConstStaticCell::new(data),
        }
    }

    /// Take exclusive access to the cell, returning an owning handle.
    ///
    /// Panics if already taken (the underlying [`ConstStaticCell`] is
    /// one-shot).
    #[inline]
    pub fn take(&'static self) -> LocalHandle<T> {
        let r: &'static mut T = self.data.take();
        self.node.value.set(r as *mut T as *mut ());
        LocalHandle {
            node: &self.node,
            _phantom: PhantomData,
        }
    }

    /// Take exclusive access to the cell, returning an owning handle.
    /// Returns `None` if the cell has already been taken.
    #[inline]
    pub fn try_take(&'static self) -> Option<LocalHandle<T>> {
        let r: &'static mut T = self.data.try_take()?;
        self.node.value.set(r as *mut T as *mut ());
        Some(LocalHandle {
            node: &self.node,
            _phantom: PhantomData,
        })
    }
}

// SAFETY: same as LocalCell.
unsafe impl<T> Sync for ConstLocalCell<T> {}

/// Move-only ownership token for an initialized [`LocalCell`] or
/// [`ConstLocalCell`] that is currently *not* linked into any storage.
///
/// Pass to [`StorageListHead::put`] / [`LocalStorage::put`] to link.
/// Recover via [`StorageListHead::remove`] / [`LocalStorage::remove`].
pub struct LocalHandle<T: 'static> {
    node: &'static StorageListNode,
    _phantom: PhantomData<&'static mut T>,
}

// SAFETY: a `LocalHandle<T>` represents unique ownership of an
// initialized-but-currently-unlinked cell. The wrapped node lives at a
// `'static` address but, while the handle exists, is not reachable from
// any `StorageListHead` list and therefore not subject to any other
// context's access. Sending the handle transfers that ownership to the
// receiving thread, which may then `put` it into its own local storage.
// Requires `T: Send` because moving the handle moves access to `T`.
unsafe impl<T: 'static + Send> Send for LocalHandle<T> {}

impl<T: 'static> LocalHandle<T> {
    /// Borrow the value.
    #[inline(always)]
    pub fn get(&self) -> &T {
        // SAFETY: the handle has exclusive ownership of the cell while
        // it exists; node.value points at the cell's `T`.
        unsafe { &*(self.node.value.get() as *const T) }
    }

    /// Mutably borrow the value.
    #[inline(always)]
    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: &mut self ensures no other live reference into the cell.
        unsafe { &mut *(self.node.value.get() as *mut T) }
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

    #[test_case]
    fn local_handle_is_send_when_t_is_send() {
        // Compile-time check: the unsafe impl requires `T: Send`, so a
        // handle for a `Send` type should also be `Send`. (A `!Send`
        // bound case would need a compile-fail test, omitted.)
        fn assert_send<T: Send>() {}
        assert_send::<LocalHandle<u32>>();
        assert_send::<LocalHandle<u64>>();
        assert_send::<LocalHandle<&'static str>>();
    }

    // -------- Comprehensive monomorphization audit --------
    //
    // Each `T*` / `U*` type forces a distinct generic instantiation so
    // the per-`T` symbols are visible in the test binary and can be
    // inspected with `nm` to verify the `#[inline]` annotation effect.
    // This complements the surface-feature tests above with
    // call-site-coverage breadth.

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT1(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT2(u64);
    #[derive(Debug, PartialEq, Eq)]
    struct AT3(u32); // not Copy
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT4(u32);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT5(u64);
    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct AT6(u32); // never installed — negative-case calls only

    static AT1_CELL: ConstLocalCell<AT1> = ConstLocalCell::new(AT1(1));
    static AT2_CELL: ConstLocalCell<AT2> = ConstLocalCell::new(AT2(2));
    static AT3_CELL: ConstLocalCell<AT3> = ConstLocalCell::new(AT3(3));
    static AT4_CELL: LocalCell<AT4> = LocalCell::new();
    static AT5_CELL: LocalCell<AT5> = LocalCell::new();
    #[test_case]
    fn audit_storage_list_head_methods() {
        let head = StorageListHead::new();
        // Three install paths.
        head.put(AT1_CELL.take());
        head.put_take::<AT2>(&AT2_CELL);
        head.put::<AT3>(AT3_CELL.take());
        head.put_init::<AT4>(&AT4_CELL, AT4(4));
        head.put_init_with::<AT5>(&AT5_CELL, || AT5(5));

        // Read paths.
        assert!(head.contains::<AT1>());
        assert_eq!(head.get::<AT1>(), Some(AT1(1)));
        assert_eq!(head.get::<AT2>(), Some(AT2(2)));
        assert!(head.as_ptr::<AT1>().is_some());

        // Write paths (Copy types).
        head.set::<AT1>(AT1(11)).unwrap();
        assert_eq!(head.replace::<AT1>(AT1(111)), Ok(AT1(11)));
        head.set::<AT4>(AT4(44)).unwrap();
        head.set::<AT5>(AT5(55)).unwrap();

        // Closure-scoped paths.
        head.with::<AT1, _>(|v| assert_eq!(*v, AT1(111)));
        head.with::<AT3, _>(|v| assert_eq!(*v, AT3(3)));
        head.with_mut::<AT2, _>(|v| v.0 = 22);
        head.with_mut::<AT3, _>(|v| v.0 = 33);
        assert_eq!(head.get::<AT2>(), Some(AT2(22)));
        head.with::<AT3, _>(|v| assert_eq!(*v, AT3(33)));

        // Move via handle: remove + put.
        let h = head.remove::<AT4>().unwrap();
        head.put::<AT4>(h);
        assert!(head.contains::<AT4>());

        // RAII borrow + borrow_mut.
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

        // Negative cases.
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

    // Static-dispatch surface, exercised against the test thread's
    // own local storage. Distinct types from the head-direct audit
    // above so they can't shadow each other within one test binary.

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
