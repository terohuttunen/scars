//! Backing cells for entries in the local namespace.
//!
//! A cell pairs the storage for one `T` with the list node that links
//! it into a [`LocalStorage`](super::LocalStorage). Two flavors:
//!
//! - [`LocalCell<T>`] — initialized at runtime via [`init`](LocalCell::init)
//!   or a lazy [`init_with`](LocalCell::init_with) closure. Backed by
//!   [`StaticCell`].
//! - [`ConstLocalCell<T>`] — initialized in `const` context at the
//!   declaration site; the value is moved out via
//!   [`take`](ConstLocalCell::take). Backed by [`ConstStaticCell`].
//!
//! Both are one-shot: a second `init` / `take` panics. The
//! [`try_init`](LocalCell::try_init) / [`try_take`](ConstLocalCell::try_take)
//! variants return `None` instead.
//!
//! Initializing a cell yields a [`LocalHandle<T>`] — a move-only token
//! that owns an initialized-but-unlinked cell. Pass the handle to
//! [`LocalStorage::put`](super::LocalStorage::put) to link the cell
//! into a namespace; recover it with
//! [`LocalStorage::remove`](super::LocalStorage::remove). The same
//! handle can be linked into a different namespace — cells move
//! between contexts over time.
//!
//! For the common one-shot pattern (init + link in one call), use
//! [`LocalStorage::put_init`](super::LocalStorage::put_init) /
//! [`put_init_with`](super::LocalStorage::put_init_with) /
//! [`put_take`](super::LocalStorage::put_take), which hide the
//! intermediate handle.
//!
//! ```ignore
//! use scars::local::{ConstLocalCell, LocalStorage};
//!
//! static COUNT: ConstLocalCell<u32> = ConstLocalCell::new(0);
//!
//! // Inside a thread, interrupt handler, or event handler:
//! LocalStorage::put_take::<u32>(&COUNT);
//! LocalStorage::with_mut::<u32, _>(|c| *c += 1);
//! ```

use super::list::StorageListNode;
use core::any::TypeId;
use core::marker::PhantomData;
use static_cell::{ConstStaticCell, StaticCell};

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
/// Pass to [`super::StorageListHead::put`] / [`super::LocalStorage::put`]
/// to link. Recover via [`super::StorageListHead::remove`] /
/// [`super::LocalStorage::remove`].
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

    #[inline(always)]
    pub(super) fn into_node(self) -> &'static StorageListNode {
        self.node
    }

    #[inline(always)]
    pub(super) fn from_node(node: &'static StorageListNode) -> Self {
        LocalHandle {
            node,
            _phantom: PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
