use super::TryLockError;
use crate::cell::LockedCell;
use crate::kernel::{
    Priority,
    interrupt::RawInterruptHandler,
    list::{Node, impl_linked},
    priority::PriorityStatus,
    scheduler::{ExecutionContext, Scheduler},
};
use crate::runtime_error;
use crate::sync::{KeyToken, Lock, PreemptLock};
use crate::thread::{IDLE_THREAD_ID, INVALID_THREAD_ID, LockListTag, RawThread};
use core::marker::PhantomData;
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

    // The owning thread id, or INVALID_THREAD_ID if free
    pub(crate) owner: LockedCell<u32, PreemptLock>,

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
            owner: LockedCell::new(INVALID_THREAD_ID),
            lock_list_node: Node::new(),
        }
    }

    unsafe fn lock_in_isr(self: Pin<&Self>, current_interrupt: &'static RawInterruptHandler) {
        // Ceiling check: If locking interrupt has priority higher than the
        // mutex ceiling, then it violates the priority ceiling protocol.
        if current_interrupt.base_priority() > self.ceiling_priority {
            runtime_error!(RuntimeError::CeilingPriorityViolation);
        }

        unsafe { current_interrupt.acquire_lock(self) };
    }

    unsafe fn lock_in_thread(self: Pin<&Self>, current_thread: &'static RawThread) {
        PreemptLock::with(|pkey| {
            if current_thread.thread_id == IDLE_THREAD_ID {
                runtime_error!(RuntimeError::IdleThreadCeilingLock);
            }

            if current_thread.thread_id == self.owner.get(pkey) {
                runtime_error!(RuntimeError::RecursiveLock);
            }

            // Ceiling check: If locking thread has priority higher than the
            // mutex ceiling, then it violates the priority ceiling protocol.
            if current_thread.base_priority > self.ceiling_priority {
                runtime_error!(RuntimeError::CeilingPriorityViolation);
            }

            // Acquisition of the lock raises the thread priority to the lock ceiling
            unsafe {
                current_thread.acquire_lock(pkey, self);
            }
        })
    }

    pub unsafe fn lock(self: Pin<&Self>) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => unsafe {
                self.lock_in_isr(current_interrupt)
            },
            ExecutionContext::Thread(current_thread) => unsafe {
                self.lock_in_thread(current_thread)
            },
        }
    }

    unsafe fn unlock_in_isr(self: Pin<&Self>, current_interrupt: &'static RawInterruptHandler) {
        unsafe {
            current_interrupt.release_lock(self);
        }
    }

    unsafe fn unlock_in_thread(self: Pin<&Self>, current_thread: &'static RawThread) {
        PreemptLock::with(|pkey| {
            // If lock has not been acquired by any thread
            if self.owner.get(pkey) == INVALID_THREAD_ID {
                return;
            }

            if self.owner.get(pkey) != current_thread.thread_id {
                runtime_error!(RuntimeError::LockOwnerViolation);
            }

            unsafe {
                current_thread.release_lock(pkey, self);
            }
        });
    }

    pub unsafe fn unlock(self: Pin<&Self>) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => unsafe {
                self.unlock_in_isr(current_interrupt)
            },
            ExecutionContext::Thread(current_thread) => unsafe {
                self.unlock_in_thread(current_thread)
            },
        }
    }

    pub(crate) unsafe fn section_start(ceiling: Priority) -> CeilingLockRestoreState {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                let saved_priority = current_interrupt.raise_section_lock_priority(ceiling);

                CeilingLockRestoreState { saved_priority }
            }
            ExecutionContext::Thread(current_thread) => {
                let saved_priority = current_thread.raise_section_lock_priority(ceiling);

                CeilingLockRestoreState { saved_priority }
            }
        }
    }

    pub(crate) unsafe fn section_end(restore_state: CeilingLockRestoreState) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                current_interrupt.set_section_lock_priority(restore_state.saved_priority);
            }
            ExecutionContext::Thread(current_thread) => {
                current_thread.set_section_lock_priority(restore_state.saved_priority);
                PreemptLock::with(|pkey| {
                    Scheduler::cond_reschedule(pkey);
                });
            }
        }
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

    pub unsafe fn lock(self: Pin<&Self>) {
        let this = self.project_ref();
        unsafe {
            this.raw.lock();
        }
    }

    pub unsafe fn unlock(self: Pin<&Self>) {
        let this = self.project_ref();
        unsafe {
            this.raw.unlock();
        }
    }

    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(<Self as Lock>::Key<'_>) -> R) -> R
    where
        Self: Sized,
    {
        <Self as Lock>::with(f)
    }

    #[inline(always)]
    pub fn try_with<R>(f: impl FnOnce(<Self as Lock>::Key<'_>) -> R) -> Result<R, TryLockError> {
        <Self as Lock>::try_with(f)
    }
}

pub struct CeilingLockRestoreState {
    saved_priority: PriorityStatus,
}

impl<const CEILING: Priority> Lock for CeilingLock<CEILING> {
    type RestoreState = CeilingLockRestoreState;
    type Key<'lock> = CeilingLockKey<'lock, CEILING>;

    unsafe fn section_start() -> Self::RestoreState {
        unsafe { RawCeilingLock::section_start(CEILING) }
    }

    unsafe fn try_section_start() -> Result<Self::RestoreState, TryLockError> {
        Ok(unsafe { Self::section_start() })
    }

    unsafe fn section_end(restore_state: Self::RestoreState) {
        unsafe { RawCeilingLock::section_end(restore_state) }
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

impl<'lock, const CEILING: Priority> KeyToken<'lock> for CeilingLockKey<'lock, CEILING> {
    unsafe fn new() -> CeilingLockKey<'lock, CEILING>
    where
        Self: Sized,
    {
        unsafe { CeilingLockKey::new() }
    }
}
