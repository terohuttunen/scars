#[cfg(feature = "raii-locks")]
use super::{LockOps, ScopedLock, TryLockResult, Unlock};
use super::{NestingLock, PreemptLock, TryLockError};
#[cfg(feature = "raii-locks")]
use crate::interrupt::RawInterruptHandler;
use crate::kernel::hal::{self, CoreId, CoreToken};
#[cfg(feature = "raii-locks")]
use crate::kernel::list::{Node, impl_linked};
use crate::kernel::{
    Priority,
    scheduler::{ExecutionContext, Scheduler},
};
use crate::priority::PriorityOpt;
use crate::runtime_error;
#[cfg(feature = "raii-locks")]
use crate::sync::atomic::{AtomicPtr, Ordering};
use crate::thread::IDLE_THREAD_ID;
#[cfg(feature = "raii-locks")]
use crate::thread::{LockListTag, RawThread};
use core::marker::PhantomData;
#[cfg(feature = "raii-locks")]
use core::ops::{Deref, DerefMut};
#[cfg(feature = "raii-locks")]
use core::pin::Pin;
use pin_project::pin_project;

/// CeilingLock is a locking primitive that allows raising the priority
/// of a thread to a ceiling priority while the lock is being held.
///
/// At interrupt priorities, the lock will prevent interrupts up to the
/// ceiling priority when it is being held by a thread. Only one thread at
/// a time can own the lock, and it must be released by the same thread
/// that acquired it. Acquiring the lock recursively from the owner
/// is not allowed. Acquiring the lock in an interrupt handler, raises
/// the current interrupt threshold to the ceiling to prevent access
/// from nested interrupts.
pub struct RawCeilingLock {
    // Ceiling priority
    pub ceiling_priority: Priority,

    // Core this lock is bound to.
    pub core: CoreId,

    // The owning thread or interrupt ptr, or null if free
    #[cfg(feature = "raii-locks")]
    pub(crate) owner: AtomicPtr<()>,

    // Node for thread lock list.
    // Only one thread owns the lock at any given time, and
    // the thread maintains a list of locks it holds.
    #[cfg(feature = "raii-locks")]
    lock_list_node: Node<Self, LockListTag>,
}

#[cfg(feature = "raii-locks")]
impl_linked!(lock_list_node, RawCeilingLock, LockListTag);

unsafe impl Send for RawCeilingLock {}
unsafe impl Sync for RawCeilingLock {}

impl RawCeilingLock {
    #[cfg(feature = "raii-locks")]
    pub const fn new(ceiling_priority: Priority, core: CoreId) -> RawCeilingLock {
        RawCeilingLock {
            ceiling_priority,
            core,
            owner: AtomicPtr::new(core::ptr::null_mut()),
            lock_list_node: Node::new(),
        }
    }

    #[cfg(feature = "raii-locks")]
    unsafe fn acquire_scoped_lock_in_interrupt(
        self: Pin<&Self>,
        current_interrupt: Pin<&'static RawInterruptHandler>,
    ) {
        // Ceiling check: If locking interrupt has priority higher than the
        // mutex ceiling, then it violates the priority ceiling protocol.
        if current_interrupt.priority() > self.ceiling_priority {
            runtime_error!(RuntimeError::CeilingPriorityViolation);
        }

        // Raise interrupt threshold to ceiling priority BEFORE ownership acquisition
        // This prevents higher priority interrupts from acquiring the lock before
        // the lock is released by this interrupt.
        let ceiling_priority_opt = PriorityOpt::from(self.ceiling_priority);
        Scheduler::set_ceiling(ceiling_priority_opt);

        // Atomically acquire ownership and check for recursive lock
        let current_interrupt_ptr = current_interrupt.as_ptr() as *const () as *mut ();
        match self.owner.compare_exchange(
            core::ptr::null_mut(),
            current_interrupt_ptr,
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => {
                // Successfully acquired the lock
                unsafe {
                    current_interrupt.ceiling_lock_acquired(self);
                }
                // Note: No need for additional set_ceiling_threshold call here.
                // After acquisition, interrupt priority equals lock ceiling priority,
                // so the threshold is already correctly set.
            }
            Err(current_owner) => {
                if current_owner == current_interrupt_ptr {
                    runtime_error!(RuntimeError::RecursiveLock);
                } else {
                    panic!("Lock already owned. The scheduler should have prevented this.")
                }
            }
        }
    }

    #[cfg(feature = "raii-locks")]
    unsafe fn acquire_scoped_lock_in_thread(
        self: Pin<&Self>,
        current_thread: Pin<&'static RawThread>,
    ) {
        PreemptLock::with(|pkey| {
            if current_thread.thread_id == IDLE_THREAD_ID {
                runtime_error!(RuntimeError::IdleThreadCeilingLock);
            }

            // Ceiling check: If locking thread has priority higher than the
            // mutex ceiling, then it violates the priority ceiling protocol.
            if current_thread.priority(pkey) > self.ceiling_priority {
                runtime_error!(RuntimeError::CeilingPriorityViolation);
            }

            // Priority ceiling protocol and priority inheritance may not
            // be combined: inheritance could later boost this thread
            // above the ceiling, breaking the `priority <= ceiling`
            // invariant the protocol relies on.
            #[cfg(feature = "priority-inheritance")]
            if current_thread.holds_inheritance_lock(pkey) {
                runtime_error!(RuntimeError::CeilingLockNotAllowed);
            }

            // Raise interrupt threshold to ceiling priority BEFORE ownership acquisition
            // This prevents lower or equal priority interrupts and threads from acquiring the lock
            // before the lock is released.
            let ceiling_priority_opt = PriorityOpt::from(self.ceiling_priority);
            Scheduler::set_ceiling(ceiling_priority_opt);

            // Atomically acquire ownership and check for recursive lock
            let current_thread_ptr = current_thread.get_ref() as *const _ as *mut ();
            match self.owner.compare_exchange(
                core::ptr::null_mut(),
                current_thread_ptr,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    // Successfully acquired the lock
                    unsafe {
                        current_thread.ceiling_lock_acquired(pkey, self);
                    }
                    // Note: No need for additional set_ceiling_threshold call here.
                    // After acquisition, thread priority equals lock ceiling priority,
                    // so the threshold is already correctly set.
                }
                Err(current_owner) => {
                    if current_owner == current_thread_ptr {
                        runtime_error!(RuntimeError::RecursiveLock);
                    } else {
                        panic!("Lock already owned. The scheduler should have prevented this.")
                    }
                }
            }
        })
    }

    #[cfg(feature = "raii-locks")]
    unsafe fn acquire_scoped_lock(self: Pin<&Self>) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => unsafe {
                self.acquire_scoped_lock_in_interrupt(current_interrupt)
            },
            ExecutionContext::Thread(current_thread) => unsafe {
                self.acquire_scoped_lock_in_thread(current_thread)
            },
        }
    }

    #[cfg(feature = "raii-locks")]
    pub fn lock(self: Pin<&Self>) -> RawCeilingLockGuard<'_> {
        unsafe {
            self.acquire_scoped_lock();
        }
        RawCeilingLockGuard {
            lock: self,
            _phantom: PhantomData,
        }
    }

    #[cfg(feature = "raii-locks")]
    unsafe fn release_scoped_lock_in_interrupt(
        self: Pin<&Self>,
        current_interrupt: Pin<&'static RawInterruptHandler>,
    ) {
        // Validate ownership before proceeding
        let current_interrupt_ptr = current_interrupt.as_ptr() as *const () as *mut ();
        let owner = self.owner.load(Ordering::Relaxed);

        // If lock has not been acquired by any interrupt. Most likely
        // an attempt to release a lock twice. For example, guard is
        // used to unlock the lock, and then the guard is dropped.
        if owner.is_null() {
            return;
        }

        // Update internal state (remove from lock list, calculate new priority)
        // This must happen before clearing ownership to maintain consistency
        unsafe {
            current_interrupt.ceiling_lock_released(self);
        }

        // Atomically clear ownership to signal lock is available
        // This should always succeed since we validated ownership above.
        self.owner
            .compare_exchange(
                current_interrupt_ptr,
                core::ptr::null_mut(),
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .unwrap_or_else(|_| runtime_error!(RuntimeError::LockOwnerViolation));

        // Set final ceiling threshold based on new interrupt priority
        // The release_lock method has already updated the interrupt's priority,
        // so we set the ceiling threshold to the new effective priority
        let new_priority = current_interrupt.priority();
        let new_ceiling_priority_opt = PriorityOpt::from(new_priority);
        Scheduler::set_ceiling(new_ceiling_priority_opt);
    }

    #[cfg(feature = "raii-locks")]
    unsafe fn release_scoped_lock_in_thread(
        self: Pin<&Self>,
        current_thread: Pin<&'static RawThread>,
    ) {
        PreemptLock::with(|pkey| {
            // Validate ownership before proceeding
            let current_thread_ptr = current_thread.get_ref() as *const _ as *mut ();
            let owner = self.owner.load(Ordering::Relaxed);

            // If lock has not been acquired by any thread. Most likely
            // an attempt to release a lock twice. For example, guard is
            // used to unlock the lock, and then the guard is dropped.
            if owner.is_null() {
                return;
            }

            // Update internal state (remove from lock list, calculate new priority)
            // This must happen before clearing ownership to maintain consistency
            unsafe {
                current_thread.ceiling_lock_released(pkey, self);
            }

            // Atomically clear ownership to signal lock is available.
            // This should always succeed since we validated ownership above.
            self.owner
                .compare_exchange(
                    current_thread_ptr,
                    core::ptr::null_mut(),
                    Ordering::AcqRel,
                    Ordering::Relaxed,
                )
                .unwrap_or_else(|_| runtime_error!(RuntimeError::LockOwnerViolation));

            // Set final ceiling threshold based on thread's new effective priority
            let new_priority = current_thread.priority(pkey);
            let new_ceiling_priority_opt = PriorityOpt::from(new_priority);
            Scheduler::set_ceiling(new_ceiling_priority_opt);

            // Trigger rescheduling if needed
            Scheduler::cond_reschedule(pkey);
        });
    }

    #[cfg(feature = "raii-locks")]
    unsafe fn release_scoped_lock(self: Pin<&Self>) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => unsafe {
                self.release_scoped_lock_in_interrupt(current_interrupt)
            },
            ExecutionContext::Thread(current_thread) => unsafe {
                self.release_scoped_lock_in_thread(current_thread)
            },
        }
    }

    pub(crate) unsafe fn acquire_nesting_lock(ceiling: Priority) -> CeilingLockRestoreState {
        unsafe { Self::try_acquire_nesting_lock(ceiling) }
            .unwrap_or_else(|_| runtime_error!(RuntimeError::CeilingPriorityViolation))
    }

    /// Like [`Self::acquire_nesting_lock`] but returns `Err(())` on
    /// ceiling violation instead of faulting. Used by `try_with`-style
    /// callers that prefer to defer rather than panic.
    pub(crate) unsafe fn try_acquire_nesting_lock(
        ceiling: Priority,
    ) -> Result<CeilingLockRestoreState, ()> {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                if current_interrupt.priority() > ceiling {
                    return Err(());
                }
                let saved_ceiling = Scheduler::get_ceiling();
                let ceiling_priority_opt = PriorityOpt::from(ceiling);
                Scheduler::set_ceiling(ceiling_priority_opt);
                let saved_priority = current_interrupt.raise_nesting_lock_priority(ceiling);
                Ok(CeilingLockRestoreState {
                    saved_priority,
                    saved_ceiling,
                })
            }
            ExecutionContext::Thread(current_thread) => PreemptLock::with(|pkey| {
                if current_thread.thread_id == IDLE_THREAD_ID {
                    runtime_error!(RuntimeError::IdleThreadCeilingLock);
                }
                if current_thread.priority(pkey) > ceiling {
                    return Err(());
                }
                // PCP and priority inheritance may not be combined — a
                // later inheritance boost could lift this thread above
                // the ceiling. Fault directly (not `Err`, which would
                // surface as a misleading `CeilingPriorityViolation`).
                #[cfg(feature = "priority-inheritance")]
                if current_thread.holds_inheritance_lock(pkey) {
                    runtime_error!(RuntimeError::CeilingLockNotAllowed);
                }
                let saved_ceiling = Scheduler::get_ceiling();
                let ceiling_priority_opt = PriorityOpt::from(ceiling);
                Scheduler::set_ceiling(ceiling_priority_opt);
                let saved_priority = current_thread.raise_nesting_lock_priority(ceiling);
                Ok(CeilingLockRestoreState {
                    saved_priority,
                    saved_ceiling,
                })
            }),
        }
    }

    pub(crate) unsafe fn release_nesting_lock(restore_state: CeilingLockRestoreState) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                current_interrupt.set_nesting_lock_priority(restore_state.saved_priority);
                Scheduler::set_ceiling(restore_state.saved_ceiling);
            }
            ExecutionContext::Thread(current_thread) => {
                current_thread.set_nesting_lock_priority(restore_state.saved_priority);

                PreemptLock::with(|pkey| {
                    Scheduler::set_ceiling(restore_state.saved_ceiling);
                    Scheduler::cond_reschedule(pkey);
                });

                // A thread leaving a wait-list closure clears the
                // `Protected` in-use guard that may have deferred a
                // kernel wake; retry it. Only the thread path can have
                // blocked the drain — the service call cannot preempt
                // an interrupt-context holder.
                Scheduler::retry_deferred_work();
            }
        }
    }

    /// Like [`Self::try_acquire_nesting_lock`], but lets the kernel
    /// drain (service-call handler) acquire a wait list's ceiling lock.
    ///
    /// Thread ceilings skip the caller-priority check: it rejects an
    /// interrupt-context caller above the ceiling, but the drain at
    /// `Interrupt(0)` only unlinks a waiter, and serializes against the
    /// thread notifier via the held preempt lock plus the `Protected`
    /// in-use guard. Interrupt ceilings keep the regular path: there
    /// the notifier may be an interrupt, excluded only by the ceiling
    /// masking it — which the drain (below the ceiling) gets by
    /// acquiring normally, and which the preempt lock could not provide.
    pub(crate) unsafe fn kernel_try_acquire_nesting_lock(
        ceiling: Priority,
    ) -> Result<CeilingLockRestoreState, ()> {
        if ceiling.is_interrupt() {
            return unsafe { Self::try_acquire_nesting_lock(ceiling) };
        }
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                let saved_ceiling = Scheduler::get_ceiling();
                let ceiling_priority_opt = PriorityOpt::from(ceiling);
                Scheduler::set_ceiling(ceiling_priority_opt);
                let saved_priority = current_interrupt.raise_nesting_lock_priority(ceiling);
                Ok(CeilingLockRestoreState {
                    saved_priority,
                    saved_ceiling,
                })
            }
            ExecutionContext::Thread(current_thread) => {
                let saved_ceiling = Scheduler::get_ceiling();
                let ceiling_priority_opt = PriorityOpt::from(ceiling);
                Scheduler::set_ceiling(ceiling_priority_opt);
                let saved_priority = current_thread.raise_nesting_lock_priority(ceiling);
                Ok(CeilingLockRestoreState {
                    saved_priority,
                    saved_ceiling,
                })
            }
        }
    }
}

#[cfg(feature = "raii-locks")]
pub struct RawCeilingLockGuard<'lock> {
    lock: Pin<&'lock RawCeilingLock>,
    // Core-affine: `Drop` releases on the current core, so the guard must be
    // released on the core that acquired it. `!Send` (+ `!Sync`).
    _phantom: PhantomData<*const ()>,
}

#[cfg(feature = "raii-locks")]
impl<'lock> Unlock for RawCeilingLockGuard<'lock> {
    unsafe fn unlock(&mut self) {
        unsafe {
            self.lock.release_scoped_lock();
        }
    }

    fn relock(&mut self) {
        unsafe { self.lock.acquire_scoped_lock() }
    }
}

#[cfg(feature = "raii-locks")]
impl<'lock> Deref for RawCeilingLockGuard<'lock> {
    type Target = Pin<&'lock RawCeilingLock>;

    fn deref(&self) -> &Self::Target {
        &self.lock
    }
}

#[cfg(feature = "raii-locks")]
impl<'lock> DerefMut for RawCeilingLockGuard<'lock> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.lock
    }
}

#[cfg(feature = "raii-locks")]
impl Drop for RawCeilingLockGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            self.unlock();
        }
    }
}

pub struct CeilingLockRestoreState {
    /// Acquirer's own priority before this acquire. Floored at the
    /// acquirer's base priority, so it can't go as low as the real
    /// hardware ceiling sometimes needs to.
    saved_priority: PriorityOpt,
    /// Hardware ceiling actually in effect before this acquire, with no
    /// such floor. Restored verbatim on release.
    saved_ceiling: PriorityOpt,
}

/// CORE-erased ceiling lock primitive. `CEILING` is typed (the ceiling
/// priority is a compile-time constant), but the core affinity is
/// stored at runtime (`raw.core`).
///
/// Two complementary APIs:
///
/// - **Static**: [`CeilingLock::with`] / [`CeilingLock::try_with`]
///   take no instance; they raise the calling core's ceiling without
///   instance state. The acquire-side check enforces the priority
///   ceiling protocol against the calling thread/interrupt priority.
///
/// - **Instance**: [`CeilingLock::lock`] / [`CeilingLock::try_lock`]
///   act on an owned `CeilingLock` instance; check
///   `CoreId::current() == self.raw.core` before acquiring; return a
///   [`CeilingLockGuard`] that releases on drop.
#[pin_project]
pub struct CeilingLock<const CEILING: Priority> {
    #[pin]
    raw: RawCeilingLock,
}

impl<const CEILING: Priority> CeilingLock<CEILING> {
    #[cfg(feature = "raii-locks")]
    pub const fn new(core: CoreId) -> Self {
        Self {
            raw: RawCeilingLock::new(CEILING, core),
        }
    }

    #[cfg(feature = "raii-locks")]
    pub fn lock(self: Pin<&Self>) -> CeilingLockGuard<'_, CEILING> {
        hal::check_core(self.raw.core);
        unsafe { self.lock_unchecked() }
    }

    /// Acquire without the wrong-core check.
    ///
    /// # Safety
    ///
    /// Caller must ensure `CoreId::current() == self.raw.core`. Use a
    /// CORE-typed wrapper ([`CoreCeilingLock<CEILING, CORE>`]) that
    /// vends a [`CoreToken<CORE>`] for a safe entry point.
    #[cfg(feature = "raii-locks")]
    #[inline(always)]
    pub unsafe fn lock_unchecked(self: Pin<&Self>) -> CeilingLockGuard<'_, CEILING> {
        let this = self.project_ref();
        let raw_guard = this.raw.lock();
        CeilingLockGuard { raw: raw_guard }
    }

    #[cfg(feature = "raii-locks")]
    pub unsafe fn unlock(self: Pin<&Self>) {
        let this = self.project_ref();
        unsafe {
            this.raw.release_scoped_lock();
        }
    }

    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(CeilingLockKey<'_, CEILING>) -> R) -> R {
        let restore_state = unsafe { RawCeilingLock::acquire_nesting_lock(CEILING) };
        let key = unsafe { CeilingLockKey::new() };

        let result = f(key);

        unsafe { RawCeilingLock::release_nesting_lock(restore_state) };
        result
    }

    #[inline(always)]
    pub fn try_with<R>(
        f: impl FnOnce(CeilingLockKey<'_, CEILING>) -> R,
    ) -> Result<R, TryLockError> {
        let restore_state = unsafe { RawCeilingLock::try_acquire_nesting_lock(CEILING) }
            .map_err(|_| TryLockError::WouldBlock)?;
        let key = unsafe { CeilingLockKey::new() };
        let result = f(key);
        unsafe { RawCeilingLock::release_nesting_lock(restore_state) };
        Ok(result)
    }
}

#[cfg(feature = "raii-locks")]
pub struct CeilingLockGuard<'lock, const CEILING: Priority> {
    raw: RawCeilingLockGuard<'lock>,
}

#[cfg(feature = "raii-locks")]
impl<'lock, const CEILING: Priority> Unlock for CeilingLockGuard<'lock, CEILING> {
    unsafe fn unlock(&mut self) {
        unsafe {
            self.raw.unlock();
        }
    }

    fn relock(&mut self) {
        self.raw.lock();
    }
}

#[cfg(feature = "raii-locks")]
impl<const CEILING: Priority> LockOps for CeilingLock<CEILING> {
    type Guard<'guard> = CeilingLockGuard<'guard, CEILING>;

    fn lock(&self) -> Self::Guard<'_> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.lock()
    }

    fn try_lock(&self) -> TryLockResult<Self::Guard<'_>> {
        Ok(self.lock())
    }
}

#[cfg(feature = "raii-locks")]
impl<const CEILING: Priority> ScopedLock for CeilingLock<CEILING> {
    const DEFAULT: Self = Self::new(CoreId::DEFAULT);
}

impl<const CEILING: Priority> NestingLock for CeilingLock<CEILING> {
    type Key<'guard> = CeilingLockKey<'guard, CEILING>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    fn kernel_try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        let restore_state = unsafe { RawCeilingLock::kernel_try_acquire_nesting_lock(CEILING) }
            .map_err(|_| TryLockError::WouldBlock)?;
        let key = unsafe { CeilingLockKey::new() };
        let result = f(key);
        unsafe { RawCeilingLock::release_nesting_lock(restore_state) };
        Ok(result)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { CeilingLockKey::new() }
    }
}

unsafe impl<const CEILING: Priority> Send for CeilingLock<CEILING> {}
unsafe impl<const CEILING: Priority> Sync for CeilingLock<CEILING> {}

#[derive(Clone, Copy, Debug)]
pub struct CeilingLockKey<'lock, const CEILING: Priority> {
    _private: PhantomData<&'lock ()>,
}

impl<'key, const CEILING: Priority> CeilingLockKey<'key, CEILING> {
    #[inline(always)]
    pub unsafe fn new() -> Self {
        CeilingLockKey {
            _private: PhantomData,
        }
    }
}

/// CORE-typed wrapper over [`CeilingLock<CEILING>`].
/// `CoreCeilingLock::<CEILING, CORE>::new()` constructs a
/// `CeilingLock::new(CORE)`; lock operations delegate to the inner.
/// The wrapper adds the static [`CoreToken<CORE>`] wrong-core check at
/// the API entry, so the inner's runtime core comparison is skipped
/// via `lock_unchecked`.
#[repr(transparent)]
pub struct CoreCeilingLock<const CEILING: Priority, const CORE: CoreId = { CoreId::DEFAULT }> {
    inner: CeilingLock<CEILING>,
}

impl<const CEILING: Priority, const CORE: CoreId> CoreCeilingLock<CEILING, CORE> {
    #[cfg(feature = "raii-locks")]
    pub const fn new() -> Self {
        Self {
            inner: CeilingLock::new(CORE),
        }
    }

    #[cfg(feature = "raii-locks")]
    pub fn lock(self: Pin<&Self>) -> CoreCeilingLockGuard<'_, CEILING, CORE> {
        let _core = CoreToken::<CORE>::current();
        // SAFETY: `CoreToken::<CORE>::current()` verified
        // `CoreId::current() == CORE`, and `self.inner.raw.core == CORE`
        // by construction (`Self::new()` reflects CORE into the inner).
        let inner = unsafe { self.map_unchecked(|s| &s.inner) };
        let inner_guard = unsafe { inner.lock_unchecked() };
        CoreCeilingLockGuard { inner: inner_guard }
    }

    #[cfg(feature = "raii-locks")]
    pub unsafe fn unlock(self: Pin<&Self>) {
        let inner = unsafe { self.map_unchecked(|s| &s.inner) };
        unsafe {
            inner.unlock();
        }
    }

    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(CoreCeilingLockKey<'_, CEILING, CORE>) -> R) -> R {
        Self::with_core(CoreToken::<CORE>::current(), f)
    }

    #[inline(always)]
    pub fn try_with<R>(
        f: impl FnOnce(CoreCeilingLockKey<'_, CEILING, CORE>) -> R,
    ) -> Result<R, TryLockError> {
        Self::try_with_core(CoreToken::<CORE>::current(), f)
    }

    /// Like [`with`](Self::with) but the caller passes in a
    /// [`CoreToken<CORE>`] they already hold instead of re-acquiring
    /// one. Saves the wrong-core check at the call site.
    #[inline(always)]
    pub fn with_core<R>(
        _core: CoreToken<'_, CORE>,
        f: impl FnOnce(CoreCeilingLockKey<'_, CEILING, CORE>) -> R,
    ) -> R {
        let restore_state = unsafe { RawCeilingLock::acquire_nesting_lock(CEILING) };
        let key = unsafe { CoreCeilingLockKey::new() };

        let result = f(key);

        unsafe { RawCeilingLock::release_nesting_lock(restore_state) };
        result
    }

    /// Like [`try_with`](Self::try_with) but the caller passes in a
    /// [`CoreToken<CORE>`] they already hold.
    #[inline(always)]
    pub fn try_with_core<R>(
        _core: CoreToken<'_, CORE>,
        f: impl FnOnce(CoreCeilingLockKey<'_, CEILING, CORE>) -> R,
    ) -> Result<R, TryLockError> {
        let restore_state = unsafe { RawCeilingLock::try_acquire_nesting_lock(CEILING) }
            .map_err(|_| TryLockError::WouldBlock)?;
        let key = unsafe { CoreCeilingLockKey::new() };
        let result = f(key);
        unsafe { RawCeilingLock::release_nesting_lock(restore_state) };
        Ok(result)
    }
}

#[cfg(feature = "raii-locks")]
impl<const CEILING: Priority, const CORE: CoreId> Default for CoreCeilingLock<CEILING, CORE> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "raii-locks")]
pub struct CoreCeilingLockGuard<
    'lock,
    const CEILING: Priority,
    const CORE: CoreId = { CoreId::DEFAULT },
> {
    // Holds the bare guard directly; Drop runs through it for the
    // actual release. The const `CORE` parameter is a type-level
    // witness that the guard came from a CORE-typed entry point.
    inner: CeilingLockGuard<'lock, CEILING>,
}

#[cfg(feature = "raii-locks")]
impl<'lock, const CEILING: Priority, const CORE: CoreId> Unlock
    for CoreCeilingLockGuard<'lock, CEILING, CORE>
{
    unsafe fn unlock(&mut self) {
        unsafe {
            self.inner.unlock();
        }
    }

    fn relock(&mut self) {
        self.inner.relock();
    }
}

#[cfg(feature = "raii-locks")]
impl<const CEILING: Priority, const CORE: CoreId> LockOps for CoreCeilingLock<CEILING, CORE> {
    type Guard<'guard> = CoreCeilingLockGuard<'guard, CEILING, CORE>;

    fn lock(&self) -> Self::Guard<'_> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.lock()
    }

    fn try_lock(&self) -> TryLockResult<Self::Guard<'_>> {
        Ok(self.lock())
    }
}

#[cfg(feature = "raii-locks")]
impl<const CEILING: Priority, const CORE: CoreId> ScopedLock for CoreCeilingLock<CEILING, CORE> {
    const DEFAULT: Self = Self::new();
}

impl<const CEILING: Priority, const CORE: CoreId> NestingLock for CoreCeilingLock<CEILING, CORE> {
    type Key<'guard> = CoreCeilingLockKey<'guard, CEILING, CORE>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    fn kernel_try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        let restore_state = unsafe { RawCeilingLock::kernel_try_acquire_nesting_lock(CEILING) }
            .map_err(|_| TryLockError::WouldBlock)?;
        let key = unsafe { CoreCeilingLockKey::new() };
        let result = f(key);
        unsafe { RawCeilingLock::release_nesting_lock(restore_state) };
        Ok(result)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { CoreCeilingLockKey::new() }
    }
}

unsafe impl<const CEILING: Priority, const CORE: CoreId> Send for CoreCeilingLock<CEILING, CORE> {}
unsafe impl<const CEILING: Priority, const CORE: CoreId> Sync for CoreCeilingLock<CEILING, CORE> {}

#[derive(Clone, Copy, Debug)]
pub struct CoreCeilingLockKey<
    'lock,
    const CEILING: Priority,
    const CORE: CoreId = { CoreId::DEFAULT },
> {
    _private: PhantomData<&'lock ()>,
}

impl<'key, const CEILING: Priority, const CORE: CoreId> CoreCeilingLockKey<'key, CEILING, CORE> {
    #[inline(always)]
    pub unsafe fn new() -> Self {
        CoreCeilingLockKey {
            _private: PhantomData,
        }
    }
}
