use super::{LockOps, NestingLock, ScopedLock, TryLockError};
use crate::kernel::hal::{CoreId, CoreToken, NUM_CORES, acquire, restore};
use crate::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::marker::PhantomData;
use core::pin::Pin;

static INTERRUPT_LOCK_NESTING: [AtomicUsize; NUM_CORES] =
    [const { AtomicUsize::new(0) }; NUM_CORES];

/// CORE-erased interrupt-lock primitive. Mirrors
/// [`CoreInterruptLock<CORE>`] but stores the core affinity at
/// runtime (`pub core: u8`) instead of as a const generic.
///
/// Two complementary APIs:
///
/// - **Static**: [`InterruptLock::with`] / [`InterruptLock::try_with`]
///   take no instance; they acquire the *calling core's* interrupt
///   lock and yield an [`InterruptLockKey`] with `core =
///   CoreId::current()`. This is the nesting-lock-style API used by
///   `LockedCell<_, InterruptLock>` field types (e.g.
///   `EventTimer.pending`).
///
/// - **Instance**: [`InterruptLock::lock`] / [`InterruptLock::try_lock`]
///   act on an owned `InterruptLock` instance. They check
///   `CoreId::current() == self.core` before acquiring; the returned
///   [`InterruptLockGuard`] releases on drop.
pub struct InterruptLock {
    owned: AtomicBool,
    pub core: CoreId,
}

impl InterruptLock {
    pub const fn new(core: CoreId) -> Self {
        Self {
            owned: AtomicBool::new(false),
            core,
        }
    }

    // SAFETY: caller must pair with `release_scoped_lock`.
    unsafe fn acquire_scoped_lock(self: Pin<&Self>) {
        acquire();
        let core = CoreId::current().as_usize();
        INTERRUPT_LOCK_NESTING[core].fetch_add(1, Ordering::Acquire);
        if self.owned.swap(true, Ordering::Relaxed) {
            crate::runtime_error!(RuntimeError::RecursiveLock);
        }
    }

    unsafe fn release_scoped_lock(self: Pin<&Self>) {
        if self.owned.swap(false, Ordering::Relaxed) {
            let core = CoreId::current().as_usize();
            if INTERRUPT_LOCK_NESTING[core].fetch_sub(1, Ordering::Release) == 1 {
                restore(true);
            }
        }
    }

    pub fn lock(&self) -> InterruptLockGuard<'_> {
        if CoreId::current() != self.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        let this = unsafe { Pin::new_unchecked(self) };
        unsafe { this.acquire_scoped_lock() };
        InterruptLockGuard {
            lock: this,
            _phantom: PhantomData,
        }
    }

    pub fn try_lock(&self) -> Result<InterruptLockGuard<'_>, TryLockError> {
        Ok(self.lock())
    }

    pub fn with<R>(f: impl FnOnce(InterruptLockKey<'_>) -> R) -> R {
        let saved = unsafe { acquire_interrupt_lock_inner() };
        let key = unsafe { InterruptLockKey::new(CoreId::current()) };
        let result = f(key);
        unsafe { release_interrupt_lock_inner(saved) };
        result
    }

    pub fn try_with<R>(f: impl FnOnce(InterruptLockKey<'_>) -> R) -> Result<R, TryLockError> {
        Ok(Self::with(f))
    }

    /// Acquire the interrupt lock; trip `WrongCore` if
    /// `CoreId::current() != core`. Runtime-`CoreId` analog of
    /// [`CoreInterruptLock::<CORE>::with_core`], which takes a
    /// [`CoreToken<CORE>`] witness instead.
    pub fn with_core<R>(core: CoreId, f: impl FnOnce(InterruptLockKey<'_>) -> R) -> R {
        if CoreId::current() != core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        Self::with(f)
    }

    /// Trait-shape variant of [`Self::with_core`]; always returns `Ok`.
    /// Wrong-core trips inside `with_core`, same as `with` / `try_with`.
    pub fn try_with_core<R>(
        core: CoreId,
        f: impl FnOnce(InterruptLockKey<'_>) -> R,
    ) -> Result<R, TryLockError> {
        Ok(Self::with_core(core, f))
    }
}

unsafe impl Send for InterruptLock {}
unsafe impl Sync for InterruptLock {}

impl LockOps for InterruptLock {
    type Guard<'lock> = InterruptLockGuard<'lock>;

    fn lock(&self) -> Self::Guard<'_> {
        self.lock()
    }

    fn try_lock(&self) -> Result<Self::Guard<'_>, TryLockError> {
        self.try_lock()
    }
}

impl ScopedLock for InterruptLock {
    const DEFAULT: Self = Self::new(CoreId::DEFAULT);
}

impl NestingLock for InterruptLock {
    type Key<'a> = InterruptLockKey<'a>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { InterruptLockKey::new(CoreId::current()) }
    }
}

pub struct InterruptLockGuard<'lock> {
    lock: Pin<&'lock InterruptLock>,
    _phantom: PhantomData<*const ()>,
}

impl<'lock> Drop for InterruptLockGuard<'lock> {
    fn drop(&mut self) {
        unsafe {
            self.lock.release_scoped_lock();
        }
    }
}

/// CORE-erased interrupt-lock proof token. Carries the originating
/// core id as a runtime field.
#[derive(Clone, Copy, Debug)]
pub struct InterruptLockKey<'lock> {
    pub core: CoreId,
    _private: PhantomData<&'lock ()>,
}

impl<'lock> InterruptLockKey<'lock> {
    /// SAFETY: caller must hold the interrupt lock on `core`.
    #[inline(always)]
    pub unsafe fn new(core: CoreId) -> Self {
        InterruptLockKey {
            core,
            _private: PhantomData,
        }
    }

    /// Re-type into a CORE-typed token; runtime check.
    #[inline(always)]
    pub fn lift<const CORE: CoreId>(self) -> CoreInterruptLockKey<'lock, CORE> {
        if self.core != CORE {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        CoreInterruptLockKey {
            _private: PhantomData,
        }
    }
}

/// CORE-typed wrapper over [`InterruptLock`]. The static `CORE` const
/// generic is reflected at construction into the inner
/// `InterruptLock`'s runtime `core` field — `CoreInterruptLock::<CORE>::new()`
/// creates an `InterruptLock::new(CORE)`. All lock operations
/// delegate to the inner; the wrapper only adds the static
/// [`CoreToken<CORE>`] wrong-core check at the API entry.
#[repr(transparent)]
pub struct CoreInterruptLock<const CORE: CoreId = { CoreId::DEFAULT }> {
    inner: InterruptLock,
}

impl<const CORE: CoreId> CoreInterruptLock<CORE> {
    pub const fn new() -> CoreInterruptLock<CORE> {
        CoreInterruptLock {
            inner: InterruptLock::new(CORE),
        }
    }

    /// Pin projection to the inner [`InterruptLock`].
    #[inline(always)]
    fn inner(self: Pin<&Self>) -> Pin<&InterruptLock> {
        unsafe { self.map_unchecked(|s| &s.inner) }
    }

    pub fn lock(&self) -> CoreInterruptLockGuard<'_, CORE> {
        let _core = CoreToken::<CORE>::current();
        self.lock_internal()
    }

    pub fn try_lock(&self) -> Result<CoreInterruptLockGuard<'_, CORE>, TryLockError> {
        Ok(self.lock())
    }

    pub fn lock_with_core<'k>(
        &'k self,
        _core: &'k CoreToken<'_, CORE>,
    ) -> CoreInterruptLockGuard<'k, CORE> {
        self.lock_internal()
    }

    pub fn try_lock_with_core<'k>(
        &'k self,
        _core: &'k CoreToken<'_, CORE>,
    ) -> Result<CoreInterruptLockGuard<'k, CORE>, TryLockError> {
        Ok(self.lock_internal())
    }

    fn lock_internal(&self) -> CoreInterruptLockGuard<'_, CORE> {
        let inner = unsafe { Pin::new_unchecked(&self.inner) };
        unsafe { inner.acquire_scoped_lock() };
        CoreInterruptLockGuard {
            lock: inner,
            _phantom: PhantomData,
        }
    }

    pub fn with<R>(f: impl FnOnce(CoreInterruptLockKey<'_, CORE>) -> R) -> R {
        let core = CoreToken::<CORE>::current();
        Self::with_core(&core, f)
    }

    pub fn try_with<R>(
        f: impl FnOnce(CoreInterruptLockKey<'_, CORE>) -> R,
    ) -> Result<R, TryLockError> {
        let core = CoreToken::<CORE>::current();
        Self::try_with_core(&core, f)
    }

    pub fn with_core<'k, R>(
        _core: &'k CoreToken<'_, CORE>,
        f: impl FnOnce(CoreInterruptLockKey<'k, CORE>) -> R,
    ) -> R {
        let saved = unsafe { acquire_interrupt_lock_inner() };
        let key = unsafe { CoreInterruptLockKey::new() };
        let result = f(key);
        unsafe { release_interrupt_lock_inner(saved) };
        result
    }

    pub fn try_with_core<'k, R>(
        _core: &'k CoreToken<'_, CORE>,
        f: impl FnOnce(CoreInterruptLockKey<'k, CORE>) -> R,
    ) -> Result<R, TryLockError> {
        Ok(Self::with_core(_core, f))
    }
}

// Non-generic helpers shared by `CoreInterruptLock<CORE>::with_core` —
// one copy of the body regardless of NUM_CORES.

#[inline(always)]
unsafe fn acquire_interrupt_lock_inner() -> bool {
    acquire();
    let core = CoreId::current().as_usize();
    INTERRUPT_LOCK_NESTING[core].fetch_add(1, Ordering::Acquire);
    true
}

#[inline(always)]
unsafe fn release_interrupt_lock_inner(_saved: bool) {
    let core = CoreId::current().as_usize();
    if INTERRUPT_LOCK_NESTING[core].fetch_sub(1, Ordering::Release) == 1 {
        restore(true);
    }
}

impl<const CORE: CoreId> LockOps for CoreInterruptLock<CORE> {
    type Guard<'lock> = CoreInterruptLockGuard<'lock, CORE>;

    fn lock(&self) -> Self::Guard<'_> {
        self.lock()
    }

    fn try_lock(&self) -> Result<Self::Guard<'_>, TryLockError> {
        self.try_lock()
    }
}

impl<const CORE: CoreId> ScopedLock for CoreInterruptLock<CORE> {
    const DEFAULT: Self = Self::new();
}

impl<const CORE: CoreId> NestingLock for CoreInterruptLock<CORE> {
    type Key<'a> = CoreInterruptLockKey<'a, CORE>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { CoreInterruptLockKey::new() }
    }
}

pub struct CoreInterruptLockGuard<'lock, const CORE: CoreId = { CoreId::DEFAULT }> {
    // Holds the inner `InterruptLock` directly so Drop can delegate to
    // its `release_scoped_lock`; the const `CORE` parameter stays only
    // as a type-level witness that the guard came from a CORE-typed
    // entry point.
    lock: Pin<&'lock InterruptLock>,
    _phantom: PhantomData<*const ()>,
}

impl<'lock, const CORE: CoreId> Drop for CoreInterruptLockGuard<'lock, CORE> {
    fn drop(&mut self) {
        unsafe {
            self.lock.release_scoped_lock();
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CoreInterruptLockKey<'lock, const CORE: CoreId = { CoreId::DEFAULT }> {
    _private: PhantomData<&'lock ()>,
}

impl<'lock, const CORE: CoreId> CoreInterruptLockKey<'lock, CORE> {
    /// Creates an interrupt lock token.
    #[inline(always)]
    pub unsafe fn new() -> Self {
        CoreInterruptLockKey {
            _private: PhantomData,
        }
    }

    /// A held interrupt lock proves the holder is executing on core `CORE`.
    #[inline(always)]
    pub fn as_core_token(&self) -> &CoreToken<'lock, CORE> {
        unsafe { &*(self as *const Self as *const CoreToken<'lock, CORE>) }
    }

    /// Type-erase the static `CORE` into a runtime field. The returned
    /// [`InterruptLockKey`] honestly carries `core = CORE`.
    #[inline(always)]
    pub(crate) fn erase(self) -> InterruptLockKey<'lock> {
        InterruptLockKey {
            core: CORE,
            _private: PhantomData,
        }
    }
}
