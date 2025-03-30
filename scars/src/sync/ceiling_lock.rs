use super::TryLockError;
use crate::kernel::{
    Priority,
    interrupt::RawInterruptHandler,
    list::{Node, impl_linked},
    priority::PriorityStatus,
    scheduler::{ExecutionContext, Scheduler},
};
use crate::runtime_error;
use crate::sync::{NestingLock, PreemptLock, ScopedLock, TryLockResult, Unlock};
use crate::thread::{IDLE_THREAD_ID, LockListTag, RawThread};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::pin::Pin;
use core::sync::atomic::{AtomicPtr, Ordering};
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

    // The owning thread or interrupt ptr, or null if free
    pub(crate) owner: AtomicPtr<()>,

    // Node for thread lock list.
    // Only one thread owns the lock at any given time, and
    // the thread maintains a list of locks it holds.
    lock_list_node: Node<Self, LockListTag>,
}

impl_linked!(lock_list_node, RawCeilingLock, LockListTag);

unsafe impl Send for RawCeilingLock {}
unsafe impl Sync for RawCeilingLock {}

impl RawCeilingLock {
    pub const fn new(ceiling_priority: Priority) -> RawCeilingLock {
        RawCeilingLock {
            ceiling_priority,
            owner: AtomicPtr::new(core::ptr::null_mut()),
            lock_list_node: Node::new(),
        }
    }

    unsafe fn acquire_scoped_lock_in_interrupt(
        self: Pin<&Self>,
        current_interrupt: Pin<&'static RawInterruptHandler>,
    ) {
        // Ceiling check: If locking interrupt has priority higher than the
        // mutex ceiling, then it violates the priority ceiling protocol.
        if current_interrupt.base_priority() > self.ceiling_priority {
            runtime_error!(RuntimeError::CeilingPriorityViolation);
        }

        if current_interrupt.as_ptr() as *const () == self.owner.load(Ordering::Relaxed) {
            runtime_error!(RuntimeError::RecursiveLock);
        }

        unsafe { current_interrupt.acquire_lock(self) };
    }

    unsafe fn acquire_scoped_lock_in_thread(
        self: Pin<&Self>,
        current_thread: Pin<&'static RawThread>,
    ) {
        PreemptLock::with(|pkey| {
            if current_thread.thread_id == IDLE_THREAD_ID {
                runtime_error!(RuntimeError::IdleThreadCeilingLock);
            }

            if current_thread.get_ref() as *const _ as *const ()
                == self.owner.load(Ordering::Relaxed)
            {
                runtime_error!(RuntimeError::RecursiveLock);
            }

            // Ceiling check: If locking thread has priority higher than the
            // mutex ceiling, then it violates the priority ceiling protocol.
            if current_thread.base_priority > self.ceiling_priority {
                runtime_error!(RuntimeError::CeilingPriorityViolation);
            }

            // Acquisition of the lock raises the thread priority to the lock ceiling
            unsafe {
                current_thread.acquire_scoped_lock(pkey, self);
            }
        })
    }

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

    pub fn lock(self: Pin<&Self>) -> RawCeilingLockGuard<'_> {
        unsafe {
            self.acquire_scoped_lock();
        }
        RawCeilingLockGuard { lock: self }
    }

    unsafe fn release_scoped_lock_in_interrupt(
        self: Pin<&Self>,
        current_interrupt: Pin<&'static RawInterruptHandler>,
    ) {
        unsafe {
            current_interrupt.release_lock(self);
        }
    }

    unsafe fn release_scoped_lock_in_thread(
        self: Pin<&Self>,
        current_thread: Pin<&'static RawThread>,
    ) {
        let owner = self.owner.load(Ordering::Relaxed);

        // If lock has not been acquired by any thread. Most likely
        // an attempt to release a lock twice. For example, guard is
        // used to unlock the lock, and then the guard is dropped.
        if owner.is_null() {
            return;
        }

        if owner != current_thread.get_ref() as *const _ as *mut () {
            runtime_error!(RuntimeError::LockOwnerViolation);
        }

        PreemptLock::with(|pkey| unsafe {
            current_thread.release_scoped_lock(pkey, self);
        });
    }

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
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                let saved_priority = current_interrupt.raise_nesting_lock_priority(ceiling);

                CeilingLockRestoreState { saved_priority }
            }
            ExecutionContext::Thread(current_thread) => {
                let saved_priority = current_thread.raise_nesting_lock_priority(ceiling);

                CeilingLockRestoreState { saved_priority }
            }
        }
    }

    pub(crate) unsafe fn release_nesting_lock(restore_state: CeilingLockRestoreState) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                current_interrupt.set_nesting_lock_priority(restore_state.saved_priority);
            }
            ExecutionContext::Thread(current_thread) => {
                current_thread.set_nesting_lock_priority(restore_state.saved_priority);
                PreemptLock::with(|pkey| {
                    Scheduler::cond_reschedule(pkey);
                });
            }
        }
    }
}

pub struct RawCeilingLockGuard<'lock> {
    lock: Pin<&'lock RawCeilingLock>,
}

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

impl<'lock> Deref for RawCeilingLockGuard<'lock> {
    type Target = Pin<&'lock RawCeilingLock>;

    fn deref(&self) -> &Self::Target {
        &self.lock
    }
}

impl<'lock> DerefMut for RawCeilingLockGuard<'lock> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.lock
    }
}

impl Drop for RawCeilingLockGuard<'_> {
    fn drop(&mut self) {
        unsafe {
            self.unlock();
        }
    }
}

impl ScopedLock for RawCeilingLock {
    type Guard<'guard> = RawCeilingLockGuard<'guard>;

    fn lock(&self) -> Self::Guard<'_> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.lock()
    }

    fn try_lock(&self) -> TryLockResult<Self::Guard<'_>> {
        Ok(self.lock())
    }
}

#[pin_project]
pub struct CeilingLock<const CEILING: Priority> {
    #[pin]
    raw: RawCeilingLock,
}

impl<const CEILING: Priority> CeilingLock<CEILING> {
    pub const fn new() -> CeilingLock<CEILING> {
        CeilingLock {
            raw: RawCeilingLock::new(CEILING),
        }
    }

    pub fn lock(self: Pin<&Self>) -> CeilingLockGuard<'_, CEILING> {
        let this = self.project_ref();
        let raw_guard = this.raw.lock();
        CeilingLockGuard { raw: raw_guard }
    }

    pub unsafe fn unlock(self: Pin<&Self>) {
        let this = self.project_ref();
        unsafe {
            this.raw.release_scoped_lock();
        }
    }

    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(<Self as NestingLock>::Key<'_>) -> R) -> R {
        let restore_state = unsafe { RawCeilingLock::acquire_nesting_lock(CEILING) };
        let key = unsafe { CeilingLockKey::new() };

        let result = f(key);

        unsafe { RawCeilingLock::release_nesting_lock(restore_state) };
        result
    }

    #[inline(always)]
    pub fn try_with<R>(
        f: impl FnOnce(<Self as NestingLock>::Key<'_>) -> R,
    ) -> Result<R, TryLockError> {
        Ok(Self::with(f))
    }
}

pub struct CeilingLockRestoreState {
    saved_priority: PriorityStatus,
}

pub struct CeilingLockGuard<'lock, const CEILING: Priority> {
    raw: RawCeilingLockGuard<'lock>,
}

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

impl<const CEILING: Priority> ScopedLock for CeilingLock<CEILING> {
    type Guard<'guard> = CeilingLockGuard<'guard, CEILING>;

    fn lock(&self) -> Self::Guard<'_> {
        let this = unsafe { Pin::new_unchecked(self) };
        this.lock()
    }

    fn try_lock(&self) -> TryLockResult<Self::Guard<'_>> {
        Ok(self.lock())
    }
}

impl<const CEILING: Priority> NestingLock for CeilingLock<CEILING> {
    type Key<'guard> = CeilingLockKey<'guard, CEILING>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Ok(Self::with(f))
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
