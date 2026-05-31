use super::{LockOps, PreemptLock, ScopedLock, TryLockError, TryLockResult, Unlock};
use crate::kernel::hal::{CoreId, CoreToken};
use crate::kernel::{
    list::{Node, impl_linked},
    scheduler::{ExecutionContext, Scheduler},
};
use crate::runtime_error;
use crate::sync::Notify;
use crate::sync::atomic::{AtomicPtr, Ordering};
use crate::thread::{InheritanceLockListTag, RawThread};
use core::pin::Pin;

/// CORE-erased inheritance lock primitive. Mirrors
/// [`CoreInheritanceLock<CORE>`] but stores the core affinity at
/// runtime (`pub core: u8`) instead of as a const generic.
pub struct InheritanceLock {
    // The current owner of the lock
    owner: AtomicOwner,

    // Threads waiting for the lock
    wait_list: Notify<PreemptLock>,

    pub core: CoreId,

    // Node for thread lock list.
    // Only one thread owns the lock at any given time, and
    // the thread maintains a list of locks it holds.
    lock_list_node: Node<Self, InheritanceLockListTag>,
}

impl_linked!(lock_list_node, InheritanceLock, InheritanceLockListTag);

impl InheritanceLock {
    pub const fn new(core: CoreId) -> Self {
        Self {
            owner: AtomicOwner::new(),
            wait_list: Notify::new(),
            core,
            lock_list_node: Node::new(),
        }
    }

    unsafe fn acquire_lock_unchecked(self: Pin<&Self>) {
        let ExecutionContext::Thread(current_thread) = Scheduler::current_execution_context()
        else {
            runtime_error!(RuntimeError::InterruptHandlerViolation)
        };

        loop {
            match self.owner.take_ownership(current_thread) {
                Ok(_) => {
                    PreemptLock::with(|pkey| unsafe {
                        current_thread.inheritance_lock_acquired(pkey, self);
                    });
                    break;
                }
                Err(owner) => {
                    PreemptLock::with(|pkey| {
                        // When the lock cannot be acquired, the owner of the lock
                        // inherits the priority of the current thread.
                        let current_priority = current_thread.priority(pkey);
                        owner.inherit_priority(pkey, current_priority);
                    });

                    self.wait_list.wait();
                }
            }
        }
    }

    unsafe fn try_acquire_lock_unchecked(self: Pin<&Self>) -> TryLockResult<()> {
        let ExecutionContext::Thread(current_thread) = Scheduler::current_execution_context()
        else {
            runtime_error!(RuntimeError::InterruptHandlerViolation)
        };

        match self.owner.take_ownership(current_thread) {
            Ok(_) => {
                PreemptLock::with(|pkey| unsafe {
                    current_thread.inheritance_lock_acquired(pkey, self);
                });
                Ok(())
            }
            Err(_) => Err(TryLockError::WouldBlock),
        }
    }

    fn release_lock(self: Pin<&Self>) {
        let ExecutionContext::Thread(current_thread) = Scheduler::current_execution_context()
        else {
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        };

        PreemptLock::with(|pkey| {
            unsafe { current_thread.inheritance_lock_released(pkey, self) };
            self.owner.release_ownership(current_thread);
            self.wait_list.notify_one();
        });
    }

    pub fn lock(self: Pin<&Self>) -> InheritanceLockGuard<'_> {
        if CoreId::current() != self.core {
            runtime_error!(RuntimeError::WrongCore);
        }
        unsafe { self.lock_unchecked() }
    }

    pub fn try_lock(self: Pin<&Self>) -> TryLockResult<InheritanceLockGuard<'_>> {
        if CoreId::current() != self.core {
            runtime_error!(RuntimeError::WrongCore);
        }
        unsafe { self.try_lock_unchecked() }
    }

    /// Acquire without the wrong-core check.
    ///
    /// # Safety
    ///
    /// Caller must ensure `CoreId::current() == self.core`. Use a
    /// CORE-typed wrapper ([`CoreInheritanceLock<CORE>`]) that vends
    /// a [`CoreToken<CORE>`] for a safe entry point.
    #[inline(always)]
    pub unsafe fn lock_unchecked(self: Pin<&Self>) -> InheritanceLockGuard<'_> {
        unsafe { self.acquire_lock_unchecked() };
        InheritanceLockGuard { lock: self }
    }

    /// Try-acquire without the wrong-core check.
    ///
    /// # Safety
    ///
    /// Caller must ensure `CoreId::current() == self.core`.
    #[inline(always)]
    pub unsafe fn try_lock_unchecked(self: Pin<&Self>) -> TryLockResult<InheritanceLockGuard<'_>> {
        unsafe { self.try_acquire_lock_unchecked() }?;
        Ok(InheritanceLockGuard { lock: self })
    }
}

unsafe impl Send for InheritanceLock {}
unsafe impl Sync for InheritanceLock {}

impl LockOps for InheritanceLock {
    type Guard<'lock> = InheritanceLockGuard<'lock>;

    fn lock(&self) -> Self::Guard<'_> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.lock()
    }

    fn try_lock(&self) -> TryLockResult<Self::Guard<'_>> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.try_lock()
    }
}

impl ScopedLock for InheritanceLock {
    const DEFAULT: Self = Self::new(CoreId::DEFAULT);
}

pub struct InheritanceLockGuard<'lock> {
    lock: Pin<&'lock InheritanceLock>,
}

impl<'lock> Drop for InheritanceLockGuard<'lock> {
    fn drop(&mut self) {
        self.lock.release_lock();
    }
}

impl<'lock> Unlock for InheritanceLockGuard<'lock> {
    unsafe fn unlock(&mut self) {
        self.lock.release_lock();
    }

    fn relock(&mut self) {
        // Relock runs in the same execution context as the original
        // acquire — by construction we're already on the lock's core.
        unsafe { self.lock.acquire_lock_unchecked() };
    }
}

/// CORE-typed wrapper over [`InheritanceLock`]. `CoreInheritanceLock::<CORE>::new()`
/// constructs an `InheritanceLock::new(CORE)`; all lock operations
/// delegate to the inner. The wrapper adds the static
/// [`CoreToken<CORE>`] wrong-core check at the API entry.
#[repr(transparent)]
pub struct CoreInheritanceLock<const CORE: CoreId = { CoreId::DEFAULT }> {
    inner: InheritanceLock,
}

impl<const CORE: CoreId> CoreInheritanceLock<CORE> {
    pub const fn new() -> Self {
        Self {
            inner: InheritanceLock::new(CORE),
        }
    }

    pub fn lock(self: Pin<&Self>) -> CoreInheritanceLockGuard<'_, CORE> {
        self.lock_core(CoreToken::<CORE>::current())
    }

    pub fn try_lock(self: Pin<&Self>) -> TryLockResult<CoreInheritanceLockGuard<'_, CORE>> {
        self.try_lock_core(CoreToken::<CORE>::current())
    }

    /// Like [`lock`](Self::lock) but the caller passes in a
    /// [`CoreToken<CORE>`] they already hold instead of re-acquiring
    /// one. Saves the wrong-core check at the call site.
    pub fn lock_core(
        self: Pin<&Self>,
        _core: CoreToken<'_, CORE>,
    ) -> CoreInheritanceLockGuard<'_, CORE> {
        // SAFETY: `CoreToken::<CORE>` proves we're on CORE; the inner
        // lock's `core` is CORE by construction (`Self::new()` reflects
        // CORE into the inner).
        let inner = unsafe { self.map_unchecked(|s| &s.inner) };
        unsafe { inner.acquire_lock_unchecked() };
        CoreInheritanceLockGuard { lock: inner }
    }

    /// Like [`try_lock`](Self::try_lock) but the caller passes in a
    /// [`CoreToken<CORE>`] they already hold.
    pub fn try_lock_core(
        self: Pin<&Self>,
        _core: CoreToken<'_, CORE>,
    ) -> TryLockResult<CoreInheritanceLockGuard<'_, CORE>> {
        let inner = unsafe { self.map_unchecked(|s| &s.inner) };
        unsafe { inner.try_acquire_lock_unchecked() }?;
        Ok(CoreInheritanceLockGuard { lock: inner })
    }
}

unsafe impl<const CORE: CoreId> Send for CoreInheritanceLock<CORE> {}
unsafe impl<const CORE: CoreId> Sync for CoreInheritanceLock<CORE> {}

impl<const CORE: CoreId> LockOps for CoreInheritanceLock<CORE> {
    type Guard<'lock> = CoreInheritanceLockGuard<'lock, CORE>;

    fn lock(&self) -> Self::Guard<'_> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.lock()
    }

    fn try_lock(&self) -> TryLockResult<Self::Guard<'_>> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.try_lock()
    }
}

impl<const CORE: CoreId> ScopedLock for CoreInheritanceLock<CORE> {
    const DEFAULT: Self = Self::new();
}

pub struct CoreInheritanceLockGuard<'lock, const CORE: CoreId = { CoreId::DEFAULT }> {
    // Holds the inner `InheritanceLock` directly so Drop delegates to
    // its `release_lock`; the const `CORE` parameter is a type-level
    // witness that the guard came from a CORE-typed entry point.
    lock: Pin<&'lock InheritanceLock>,
}

impl<'lock, const CORE: CoreId> Drop for CoreInheritanceLockGuard<'lock, CORE> {
    fn drop(&mut self) {
        self.lock.release_lock();
    }
}

impl<'lock, const CORE: CoreId> Unlock for CoreInheritanceLockGuard<'lock, CORE> {
    unsafe fn unlock(&mut self) {
        self.lock.release_lock();
    }

    fn relock(&mut self) {
        // Relock runs in the same execution context as the original
        // acquire — by construction we're already on CORE.
        unsafe { self.lock.acquire_lock_unchecked() };
    }
}

pub struct AtomicOwner {
    owner: AtomicPtr<RawThread>,
}

impl AtomicOwner {
    pub const fn new() -> Self {
        Self {
            owner: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    pub(crate) fn take_ownership(
        &self,
        current_thread: Pin<&'static RawThread>,
    ) -> Result<(), Pin<&'static RawThread>> {
        let current_thread = current_thread.get_ref() as *const _ as *mut _;
        self.owner
            .compare_exchange(
                core::ptr::null_mut(),
                current_thread,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .map(|_| ())
            .map_err(|owner| {
                if owner == current_thread {
                    crate::runtime_error!(RuntimeError::RecursiveLock);
                }
                unsafe { Pin::new_unchecked(&*owner) }
            })
    }

    pub(crate) fn release_ownership(
        &self,
        current_thread: Pin<&'static RawThread>,
    ) -> Pin<&'static RawThread> {
        if self.owner.load(Ordering::Relaxed) != current_thread.get_ref() as *const _ as *mut _ {
            crate::runtime_error!(RuntimeError::LockOwnerViolation);
        }

        let owner = self.owner.swap(core::ptr::null_mut(), Ordering::Release);

        unsafe { Pin::new_unchecked(&*owner) }
    }
}
