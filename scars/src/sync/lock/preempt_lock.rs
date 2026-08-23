use super::{LockOps, NestingLock, ScopedLock, TryLockError};
use crate::kernel::hal::{self, CoreId, CoreToken, NUM_CORES, acquire, pend_service_call, restore};
use crate::kernel::scheduler::{ExecutionContext, Scheduler};
use crate::sync::atomic::{AtomicPtr, Ordering};
use core::marker::PhantomData;

/// CORE-erased preempt-lock proof token. Carries the originating core
/// id as a runtime field; the type system makes no static claim about
/// which core holds the lock.
///
/// Construct via [`CorePreemptLockKey::erase`] (at CORE-typed → erased
/// boundary) or via [`PreemptLock::with`] (when the caller has
/// no static `CORE` and just wants the local-core preempt lock).
#[derive(Clone, Copy, Debug)]
pub struct PreemptLockKey<'lock> {
    pub core: CoreId,
    _private: PhantomData<&'lock ()>,
}

impl<'lock> PreemptLockKey<'lock> {
    /// SAFETY: caller must hold the preempt lock on `core`.
    #[inline(always)]
    pub unsafe fn new(core: CoreId) -> Self {
        PreemptLockKey {
            core,
            _private: PhantomData,
        }
    }

    /// Re-type into a CORE-typed proof token, with a runtime check
    /// that the runtime core matches the requested const `CORE`.
    /// Raises [`RuntimeError::WrongCore`] on mismatch.
    #[inline(always)]
    pub fn lift<const CORE: CoreId>(self) -> CorePreemptLockKey<'lock, CORE> {
        if self.core != CORE {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        CorePreemptLockKey {
            _private: PhantomData,
        }
    }
}

/// CORE-erased preempt-lock primitive. Mirrors [`CorePreemptLock<CORE>`]
/// but stores the core affinity at runtime (`pub core: u8`) instead of
/// as a const generic.
///
/// The actual preempt-lock state lives in the per-core `PREEMPT_LOCK`
/// static array; the `PreemptLock` instance itself only carries the
/// runtime `core` so instance-based `.lock()` can perform a wrong-core
/// check.
///
/// Two complementary APIs:
///
/// - **Static**: [`PreemptLock::with`] / [`PreemptLock::try_with`]
///   take no instance; they acquire the *calling core's* preempt lock
///   and yield a [`PreemptLockKey`] with `core = CoreId::current()`.
///   This is the nesting-lock-style API used throughout kernel
///   internals.
///
/// - **Instance**: [`PreemptLock::lock`] / [`PreemptLock::try_lock`]
///   act on an owned `PreemptLock` instance, check
///   `CoreId::current() == self.core`, and return a
///   [`PreemptLockGuard`] that releases on drop.
pub struct PreemptLock {
    pub core: CoreId,
}

impl PreemptLock {
    pub const fn new(core: CoreId) -> Self {
        Self { core }
    }

    pub fn lock(&self) -> PreemptLockGuard<'_> {
        hal::check_core(self.core);
        unsafe { self.lock_unchecked() }
    }

    pub fn try_lock(&self) -> Result<PreemptLockGuard<'_>, TryLockError> {
        hal::check_core(self.core);
        unsafe { self.try_lock_unchecked() }
    }

    /// Acquire the preempt lock without the wrong-core check.
    ///
    /// # Safety
    ///
    /// Caller must ensure `CoreId::current() == self.core`. Use a
    /// CORE-typed wrapper ([`CorePreemptLock<CORE>`]) that vends a
    /// [`CoreToken<CORE>`] for a safe entry point.
    #[inline(always)]
    pub unsafe fn lock_unchecked(&self) -> PreemptLockGuard<'_> {
        let restore_state = unsafe { acquire_nesting_lock_inner() };
        PreemptLockGuard {
            _lock: PhantomData,
            restore_state: Some(restore_state),
        }
    }

    /// Try-acquire without the wrong-core check.
    ///
    /// # Safety
    ///
    /// Caller must ensure `CoreId::current() == self.core`.
    #[inline(always)]
    pub unsafe fn try_lock_unchecked(&self) -> Result<PreemptLockGuard<'_>, TryLockError> {
        match unsafe { try_acquire_nesting_lock_inner() } {
            Ok(restore_state) => Ok(PreemptLockGuard {
                _lock: PhantomData,
                restore_state: Some(restore_state),
            }),
            Err(err) => Err(err),
        }
    }

    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(PreemptLockKey<'_>) -> R) -> R {
        let restore_state = unsafe { acquire_nesting_lock_inner() };
        let key = unsafe { PreemptLockKey::new(CoreId::current()) };
        let result = f(key);
        unsafe { release_nesting_lock_inner(restore_state) };
        result
    }

    #[inline(always)]
    pub fn try_with<R>(f: impl FnOnce(PreemptLockKey<'_>) -> R) -> Result<R, TryLockError> {
        match unsafe { try_acquire_nesting_lock_inner() } {
            Ok(restore_state) => {
                let key = unsafe { PreemptLockKey::new(CoreId::current()) };
                let result = f(key);
                unsafe { release_nesting_lock_inner(restore_state) };
                Ok(result)
            }
            Err(err) => Err(err),
        }
    }
}

pub struct PreemptLockGuard<'lock> {
    _lock: PhantomData<&'lock PreemptLock>,
    restore_state: Option<PreemptLockRestoreState>,
}

impl<'lock> Drop for PreemptLockGuard<'lock> {
    fn drop(&mut self) {
        if let Some(state) = self.restore_state.take() {
            unsafe { release_nesting_lock_inner(state) };
        }
    }
}

unsafe impl Send for PreemptLock {}
unsafe impl Sync for PreemptLock {}

static PREEMPT_LOCK: [AtomicPtr<()>; NUM_CORES] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; NUM_CORES];

mod sealed {
    use super::CoreId;

    /// Saved preempt-lock state captured at acquire and consumed at
    /// release. Records the previous owner pointer (used to detect
    /// whether this is the outermost acquire) and the originating
    /// core (so release can index `PREEMPT_LOCK[core]` without
    /// re-reading `CoreId::current()` and can `debug_assert!` that
    /// release runs on the same core as acquire).
    pub enum PreemptLockRestoreState {
        Interrupt { prev: *const (), core: CoreId },
        Thread { prev: *const (), core: CoreId },
    }

    impl PreemptLockRestoreState {
        pub fn is_interrupt(&self) -> bool {
            matches!(self, PreemptLockRestoreState::Interrupt { .. })
        }

        pub fn is_null(&self) -> bool {
            match self {
                PreemptLockRestoreState::Interrupt { prev, .. } => prev.is_null(),
                PreemptLockRestoreState::Thread { prev, .. } => prev.is_null(),
            }
        }

        pub fn core(&self) -> CoreId {
            match self {
                PreemptLockRestoreState::Interrupt { core, .. } => *core,
                PreemptLockRestoreState::Thread { core, .. } => *core,
            }
        }
    }
}

impl NestingLock for PreemptLock {
    type Key<'a> = PreemptLockKey<'a>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { PreemptLockKey::new(CoreId::current()) }
    }
}

impl LockOps for PreemptLock {
    type Guard<'lock> = PreemptLockGuard<'lock>;

    fn lock(&self) -> Self::Guard<'_> {
        self.lock()
    }

    fn try_lock(&self) -> Result<Self::Guard<'_>, TryLockError> {
        self.try_lock()
    }
}

pub type PreemptLockRestoreState = sealed::PreemptLockRestoreState;

#[inline]
pub fn is_preempt_allowed() -> bool {
    let core = CoreId::current().as_usize();
    PREEMPT_LOCK[core].load(Ordering::SeqCst).is_null()
}

#[derive(Clone, Copy, Debug)]
pub struct CorePreemptLockKey<'lock, const CORE: CoreId = { CoreId::DEFAULT }> {
    _private: PhantomData<&'lock ()>,
}

impl<'lock, const CORE: CoreId> CorePreemptLockKey<'lock, CORE> {
    /// Creates a pre-emption lock token.
    #[inline(always)]
    pub unsafe fn new() -> Self {
        CorePreemptLockKey {
            _private: PhantomData,
        }
    }

    /// A held preempt-lock proves the holder is executing on core `CORE`.
    /// Downgrade it to a [`CoreToken`] for nested operations that only need
    /// the core-affinity proof, not the lock itself.
    #[inline(always)]
    pub fn as_core_token(&self) -> &CoreToken<'lock, CORE> {
        // SAFETY: both are zero-sized phantom types with identical layout
        // (PhantomData<*const &'k ()>-style) and the same `'lock` / `CORE`
        // parameterisation.
        unsafe { &*(self as *const Self as *const CoreToken<'lock, CORE>) }
    }

    /// Type-erase the static `CORE` into a runtime field. The returned
    /// [`PreemptLockKey`] honestly carries `core = CORE` so
    /// kernel-internal CORE-erased data structures (e.g.
    /// `RawScheduler`'s thread queues, `RawTimer`'s cells) can
    /// `debug_assert_eq!(owner.core, pkey.core)` at access without the
    /// type system claiming a different CORE.
    #[inline(always)]
    pub(crate) fn erase(self) -> PreemptLockKey<'lock> {
        PreemptLockKey {
            core: CORE,
            _private: PhantomData,
        }
    }
}

/// CORE-typed wrapper over [`PreemptLock`]. `CorePreemptLock::<CORE>::new()`
/// constructs a `PreemptLock::new(CORE)`; lock operations delegate to
/// the inner. The wrapper adds the static [`CoreToken<CORE>`]
/// wrong-core check at the API entry.
#[repr(transparent)]
pub struct CorePreemptLock<const CORE: CoreId = { CoreId::DEFAULT }> {
    inner: PreemptLock,
}

impl<const CORE: CoreId> CorePreemptLock<CORE> {
    pub const fn new() -> Self {
        Self {
            inner: PreemptLock::new(CORE),
        }
    }

    pub fn lock(&self) -> CorePreemptLockGuard<'_, CORE> {
        let _core = CoreToken::<CORE>::current();
        // SAFETY: `CoreToken::<CORE>::current()` verified
        // `CoreId::current() == CORE`, and `self.inner.core == CORE`
        // by construction (`Self::new()` reflects CORE into the inner).
        // Hence `CoreId::current() == self.inner.core`.
        let inner = unsafe { self.inner.lock_unchecked() };
        CorePreemptLockGuard { inner }
    }

    pub fn try_lock(&self) -> Result<CorePreemptLockGuard<'_, CORE>, TryLockError> {
        let _core = CoreToken::<CORE>::current();
        // SAFETY: same as `lock`.
        let inner = unsafe { self.inner.try_lock_unchecked() }?;
        Ok(CorePreemptLockGuard { inner })
    }

    /// Enter a preempt-locked section, asserting the calling core matches
    /// `CORE` once at the wrong-core check (via [`CoreToken::current`]).
    /// User-facing entry point.
    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(CorePreemptLockKey<'_, CORE>) -> R) -> R {
        let core = CoreToken::<CORE>::current();
        Self::with_core(&core, f)
    }

    #[inline(always)]
    pub fn try_with<R>(
        f: impl FnOnce(CorePreemptLockKey<'_, CORE>) -> R,
    ) -> Result<R, TryLockError> {
        let core = CoreToken::<CORE>::current();
        Self::try_with_core(&core, f)
    }

    /// Enter a preempt-locked section using a previously-acquired
    /// [`CoreToken`] as proof of core affinity. Skips the wrong-core check
    /// — the key already proved we're on `CORE`.
    #[inline(always)]
    pub fn with_core<'k, R>(
        _core: &'k CoreToken<'_, CORE>,
        f: impl FnOnce(CorePreemptLockKey<'k, CORE>) -> R,
    ) -> R {
        let restore_state = unsafe { acquire_nesting_lock_inner() };
        let key = unsafe { CorePreemptLockKey::new() };
        let result = f(key);
        unsafe { release_nesting_lock_inner(restore_state) };
        result
    }

    #[inline(always)]
    pub fn try_with_core<'k, R>(
        _core: &'k CoreToken<'_, CORE>,
        f: impl FnOnce(CorePreemptLockKey<'k, CORE>) -> R,
    ) -> Result<R, TryLockError> {
        match unsafe { try_acquire_nesting_lock_inner() } {
            Ok(restore_state) => {
                let key = unsafe { CorePreemptLockKey::new() };
                let result = f(key);
                unsafe { release_nesting_lock_inner(restore_state) };
                Ok(result)
            }
            Err(err) => Err(err),
        }
    }
}

unsafe impl<const CORE: CoreId> Send for CorePreemptLock<CORE> {}
unsafe impl<const CORE: CoreId> Sync for CorePreemptLock<CORE> {}

impl<const CORE: CoreId> LockOps for CorePreemptLock<CORE> {
    type Guard<'lock> = CorePreemptLockGuard<'lock, CORE>;

    fn lock(&self) -> Self::Guard<'_> {
        self.lock()
    }

    fn try_lock(&self) -> Result<Self::Guard<'_>, TryLockError> {
        self.try_lock()
    }
}

impl<const CORE: CoreId> ScopedLock for CorePreemptLock<CORE> {
    const DEFAULT: Self = Self::new();
}

impl<const CORE: CoreId> NestingLock for CorePreemptLock<CORE> {
    type Key<'a> = CorePreemptLockKey<'a, CORE>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { CorePreemptLockKey::new() }
    }
}

pub struct CorePreemptLockGuard<'lock, const CORE: CoreId = { CoreId::DEFAULT }> {
    // Holds the bare guard directly; Drop runs through it for the
    // actual `release_nesting_lock_inner` call. The const `CORE`
    // parameter is a type-level witness that the guard came from a
    // CORE-typed entry point.
    inner: PreemptLockGuard<'lock>,
}

// Non-generic free functions holding the actual preempt-lock logic.
// `CorePreemptLock<CORE>::with_core` delegates here, so the kernel pays exactly
// one copy of this code regardless of NUM_CORES.

unsafe fn acquire_nesting_lock_inner() -> PreemptLockRestoreState {
    match Scheduler::current_execution_context() {
        ExecutionContext::Interrupt(_current_interrupt) => {
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation)
        }
        ExecutionContext::Thread(current_thread) => {
            let core = CoreId::current();
            let previous_state = PREEMPT_LOCK[core.as_usize()].swap(
                current_thread.get_ref() as *const _ as *mut (),
                Ordering::Acquire,
            );
            PreemptLockRestoreState::Thread {
                prev: previous_state as *const _,
                core,
            }
        }
    }
}

unsafe fn try_acquire_nesting_lock_inner() -> Result<PreemptLockRestoreState, TryLockError> {
    match Scheduler::current_execution_context() {
        ExecutionContext::Interrupt(current_interrupt) => {
            let core = CoreId::current();
            // To acquire in interrupt, the lock may not be held by a thread or a
            // lower priority interrupt.
            match PREEMPT_LOCK[core.as_usize()].compare_exchange(
                core::ptr::null_mut(),
                current_interrupt.get_ref() as *const _ as *mut (),
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(previous_state) => {
                    // First lock in current interrupt handler
                    Ok(PreemptLockRestoreState::Interrupt {
                        prev: previous_state as *const _,
                        core,
                    })
                }
                Err(previous_state) => {
                    if previous_state == current_interrupt.get_ref() as *const _ as *mut () {
                        // Previous state was not null, but it points to the current interrupt context,
                        // therefore the lock was taken earlier in the current interrupt handler.
                        Ok(PreemptLockRestoreState::Interrupt {
                            prev: previous_state as *const _,
                            core,
                        })
                    } else {
                        // Lock not acquired. Thread or lower priority interrupt is holding the lock.
                        Err(TryLockError::WouldBlock)
                    }
                }
            }
        }
        ExecutionContext::Thread(current_thread) => {
            let core = CoreId::current();
            let previous_state = PREEMPT_LOCK[core.as_usize()].swap(
                current_thread.get_ref() as *const _ as *mut (),
                Ordering::Acquire,
            );
            Ok(PreemptLockRestoreState::Thread {
                prev: previous_state as *const _,
                core,
            })
        }
    }
}

unsafe fn release_nesting_lock_inner(restore_state: PreemptLockRestoreState) {
    if restore_state.is_null() {
        let core_id = restore_state.core();
        debug_assert_eq!(
            CoreId::current(),
            core_id,
            "preempt-lock released on different core than acquired"
        );
        let core = core_id.as_usize();

        // Release the preempt lock and hand off any work queued behind
        // it to the service call (PendSV). Producers conditional-pend
        // PendSV behind `is_preempt_allowed()`; while we held the lock
        // they skipped the pend, so we compensate here for any
        // deferred work or reschedule they queued. IRQs are disabled
        // for the brief window to close the race where a producer
        // checks the lock state right as we flip it.
        //
        // On Cortex-M the service call (PendSV) waits for BASEPRI to
        // drop; on the sim the SYSCALL_SIGNAL handler installs masks
        // such that `pthread_sigqueue(SYSCALL_SIGNAL)` queues until
        // the surrounding IRQ unwinds. Either way the actual
        // event-drain / context-switch happens at a safe point after
        // we return from here.
        let int_restore = acquire();
        let any_pending =
            Scheduler::is_reschedule_pending() || Scheduler::is_deferred_work_pending();
        PREEMPT_LOCK[core].store(core::ptr::null_mut(), Ordering::Release);
        if any_pending {
            pend_service_call();
        }
        restore(int_restore);
    }
}
