//! A cell with single-owner access, gated by an atomic owner pointer.
//!
//! [`OwnerCell<T>`] hands out access to its value only to the caller-supplied
//! *owner*, an opaque `*const ()` identity compared by address. The owner that
//! acquires the cell while it is free becomes the current owner; the same owner
//! may re-enter for shared access, and a different owner is rejected by the
//! acquiring compare-exchange rather than racing. Acquisition and release are a
//! single atomic operation on the owner pointer, so the data path carries no
//! lock or critical section, and the cell is safe to access under preemption and
//! from multiple cores: a concurrent different owner is turned away, not raced.
//!
//! Shared access ([`with`](OwnerCell::with) / [`try_with`](OwnerCell::try_with))
//! is re-entrant and closure-scoped: the scope that found the cell free is the
//! one that frees it again, so re-entry needs no borrow count. Exclusive access
//! ([`try_lock`](OwnerCell::try_lock) and the [`with_mut`](OwnerCell::with_mut)
//! wrappers) requires the cell to be free and is non-reentrant; the
//! [`OwnerRefMut`] guard may be held across an `.await`.
//!
//! The cell stores no shared/exclusive mode, so it does not police the two
//! against each other *within a single owner*: a shared `with` nested inside an
//! exclusive hold by the same owner would alias `&` with `&mut`. The reverse is
//! rejected, since exclusive acquisition requires the cell to be free. Callers
//! must not take a shared borrow of a cell they already hold exclusively. The
//! cross-owner guarantee is fully enforced by the compare-exchange.

use crate::sync::atomic::{AtomicPtr, Ordering};
use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};

/// Opaque owner identity for an [`OwnerCell`]. Compared by address only.
pub type Owner = *const ();

pub struct OwnerCell<T> {
    /// Null when free, otherwise the current owner's identity.
    owner: AtomicPtr<()>,
    data: UnsafeCell<T>,
}

// SAFETY: the compare-exchange on `owner` serializes acquisition, so at most
// one owner reaches `data` at a time and a concurrent different owner is
// rejected. `T: Send` is required because the value is reachable from whichever
// owner acquires the cell.
unsafe impl<T: Send> Sync for OwnerCell<T> {}

impl<T> OwnerCell<T> {
    pub const fn new(value: T) -> OwnerCell<T> {
        OwnerCell {
            owner: AtomicPtr::new(core::ptr::null_mut()),
            data: UnsafeCell::new(value),
        }
    }

    /// Run `f` with shared access for `owner`. Re-entrant for the same owner.
    ///
    /// Panics if a different owner currently holds the cell. Use
    /// [`try_with`](Self::try_with) to handle that case without panicking.
    pub fn with<R>(&self, owner: Owner, f: impl FnOnce(&T) -> R) -> R {
        self.try_with(owner, f)
            .unwrap_or_else(|| panic!("OwnerCell accessed by a different owner"))
    }

    /// Run `f` with shared access for `owner`, or return `None` if a different
    /// owner currently holds the cell. Re-entrant for the same owner.
    pub fn try_with<R>(&self, owner: Owner, f: impl FnOnce(&T) -> R) -> Option<R> {
        let _release = self.acquire_shared(owner)?;
        // SAFETY: this owner holds the cell for shared access; `with`/`try_with`
        // only hand out `&T`, and exclusive acquisition cannot succeed while the
        // cell is held, so no live `&mut T` aliases this reference.
        let value = unsafe { &*self.data.get() };
        Some(f(value))
    }

    /// Acquire exclusive access for `owner`, returning a guard that releases the
    /// cell when dropped. If the cell is held (by any owner, including `owner` —
    /// exclusive access is non-reentrant), returns the identity of the holder,
    /// letting the caller tell its own contention apart from a foreign owner.
    /// The guard may be held across an `.await`.
    pub fn try_lock(&self, owner: Owner) -> Result<OwnerRefMut<'_, T>, Owner> {
        match self.owner.compare_exchange(
            core::ptr::null_mut(),
            owner as *mut (),
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => Ok(OwnerRefMut {
                cell: self,
                _not_send: PhantomData,
            }),
            Err(holder) => Err(holder as Owner),
        }
    }

    /// Run `f` with exclusive access for `owner`.
    ///
    /// Panics if the cell is held. Use [`try_with_mut`](Self::try_with_mut) to
    /// handle that case without panicking.
    pub fn with_mut<R>(&self, owner: Owner, f: impl FnOnce(&mut T) -> R) -> R {
        self.try_with_mut(owner, f)
            .unwrap_or_else(|| panic!("OwnerCell already held"))
    }

    /// Run `f` with exclusive access for `owner`, or return `None` if the cell
    /// is held.
    pub fn try_with_mut<R>(&self, owner: Owner, f: impl FnOnce(&mut T) -> R) -> Option<R> {
        let mut guard = self.try_lock(owner).ok()?;
        Some(f(&mut guard))
    }

    /// Acquire shared ownership for `owner`. Returns a release guard on success,
    /// or `None` if a different owner holds the cell. The guard releases the
    /// cell only when this call was the outermost acquisition.
    fn acquire_shared(&self, owner: Owner) -> Option<SharedRelease<'_, T>> {
        match self.owner.compare_exchange(
            core::ptr::null_mut(),
            owner as *mut (),
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            // Free: this call owns the cell and frees it on drop.
            Ok(_) => Some(SharedRelease {
                cell: self,
                outermost: true,
            }),
            // Already held by this owner: nested shared access, no release.
            Err(current) if current == owner as *mut () => Some(SharedRelease {
                cell: self,
                outermost: false,
            }),
            // Held by a different owner.
            Err(_) => None,
        }
    }

    fn release(&self) {
        self.owner.store(core::ptr::null_mut(), Ordering::Release);
    }
}

/// Releases the cell when the outermost shared borrow ends.
struct SharedRelease<'a, T> {
    cell: &'a OwnerCell<T>,
    outermost: bool,
}

impl<T> Drop for SharedRelease<'_, T> {
    fn drop(&mut self) {
        if self.outermost {
            self.cell.release();
        }
    }
}

/// Exclusive-access guard from [`OwnerCell::try_lock`]. Dereferences to the
/// cell's value and releases the cell when dropped.
///
/// It is `!Send`: a held exclusive borrow must not move to another executor.
pub struct OwnerRefMut<'a, T> {
    cell: &'a OwnerCell<T>,
    _not_send: PhantomData<*const ()>,
}

impl<T> Deref for OwnerRefMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: this guard holds the cell exclusively.
        unsafe { &*self.cell.data.get() }
    }
}

impl<T> DerefMut for OwnerRefMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: this guard holds the cell exclusively.
        unsafe { &mut *self.cell.data.get() }
    }
}

impl<T> Drop for OwnerRefMut<'_, T> {
    fn drop(&mut self) {
        self.cell.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two distinct stack addresses serve as synthetic owner identities.
    fn owners() -> (Owner, Owner) {
        static A: u8 = 0;
        static B: u8 = 0;
        (&A as *const u8 as Owner, &B as *const u8 as Owner)
    }

    #[test_case]
    fn exclusive_roundtrip_and_release() {
        let (a, _b) = owners();
        let cell = OwnerCell::new(0u32);

        cell.with_mut(a, |v| *v = 42);
        assert_eq!(cell.with(a, |v| *v), 42);

        // Held guard excludes any further acquisition until dropped.
        let guard = cell.try_lock(a).unwrap();
        assert_eq!(cell.try_lock(a).err(), Some(a)); // non-reentrant, same owner
        drop(guard);
        assert!(cell.try_lock(a).is_ok());
    }

    #[test_case]
    fn shared_is_reentrant_for_same_owner() {
        let (a, _b) = owners();
        let cell = OwnerCell::new(7u32);

        let total = cell.with(a, |outer| {
            let inner = cell.with(a, |v| *v); // nested shared, same owner
            *outer + inner
        });
        assert_eq!(total, 14);

        // Outermost shared scope released the cell.
        assert!(cell.try_lock(a).is_ok());
    }

    #[test_case]
    fn different_owner_is_blocked_while_held() {
        let (a, b) = owners();
        let cell = OwnerCell::new(0u32);

        cell.with(a, |_| {
            assert!(cell.try_with(b, |_| ()).is_none());
            assert!(cell.try_with_mut(b, |_| ()).is_none());
            assert_eq!(cell.try_lock(b).err(), Some(a)); // reports the holder
        });

        // Released after the shared scope ends.
        assert!(cell.try_lock(b).is_ok());
    }

    #[test_case]
    fn exclusive_blocks_other_owner() {
        let (a, b) = owners();
        let cell = OwnerCell::new(0u32);

        let guard = cell.try_lock(a).unwrap();
        assert!(cell.try_with(b, |_| ()).is_none());
        assert_eq!(cell.try_lock(b).err(), Some(a)); // reports the holder
        drop(guard);
        assert_eq!(cell.with(b, |v| *v), 0);
    }
}
