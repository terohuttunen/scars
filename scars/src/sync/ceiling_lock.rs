use super::TryLockError;
use crate::cell::LockedCell;
use crate::kernel::{
    hal::get_interrupt_threshold,
    interrupt::InterruptControlBlock,
    list::{impl_linked, Link},
    scheduler::{ExecutionContext, Scheduler},
    task::{LockListTag, TaskControlBlock, IDLE_TASK_ID, INVALID_TASK_ID},
    AnyPriority, Priority,
};
use crate::runtime_error;
use crate::sync::{KeyToken, Lock, PreemptLock};
use core::marker::PhantomData;

/// CeilingLock is a locking primitive that allows raising the priority
/// of a task to a ceiling priority while the lock is being held.
///
/// At interrupt priorities, the lock will prevent interrupts up to the
/// ceiling priority when it is being held by a task. Only one task at
/// a time can own the lock, and it must be released by the same task
/// that acquired it. Acquiring the lock recursively from the owner
/// is not allowed. Acquiring the lock in an interrupt handler, raises
/// the current interrupt threshold to the ceiling to prevent access
/// from nested interrupts.
pub struct RawCeilingLock {
    // Ceiling priority
    pub ceiling_priority: Priority,

    // The owning task id, or INVALID_TASK_ID if free
    pub(crate) owner: LockedCell<u32, PreemptLock>,

    // Link for task lock list.
    // Only one tasks owns the lock at any given time, and
    // the task maintains a list of locks it holds.
    lock_list_link: Link<Self, LockListTag>,
}

impl_linked!(lock_list_link, RawCeilingLock, LockListTag);

unsafe impl Send for RawCeilingLock {}
unsafe impl Sync for RawCeilingLock {}

impl RawCeilingLock {
    pub const fn new(ceiling_priority: AnyPriority) -> RawCeilingLock {
        RawCeilingLock {
            ceiling_priority: Priority::any(ceiling_priority),
            owner: LockedCell::new(INVALID_TASK_ID),
            lock_list_link: Link::new(),
        }
    }

    fn lock_in_isr(&self, current_interrupt: &'static InterruptControlBlock) {
        // Ceiling check: If locking interrupt has priority higher than the
        // mutex ceiling, then it violates the priority ceiling protocol.
        if current_interrupt.active_priority() > self.ceiling_priority {
            runtime_error!(RuntimeError::CeilingPriorityViolation);
        }

        current_interrupt.acquire_lock(self);
    }

    fn lock_in_task(&self, current_task: &'static TaskControlBlock) {
        PreemptLock::with(|pkey| {
            if current_task.task_id == IDLE_TASK_ID {
                runtime_error!(RuntimeError::IdleTaskCeilingLock);
            }

            if current_task.task_id == self.owner.get(pkey) {
                runtime_error!(RuntimeError::RecursiveLock);
            }

            // Ceiling check: If locking task has priority higher than the
            // mutex ceiling, then it violates the priority ceiling protocol.
            if current_task.active_priority() > self.ceiling_priority {
                runtime_error!(RuntimeError::CeilingPriorityViolation);
            }

            // Acquisition of the lock raises the task priority to the lock ceiling
            current_task.acquire_lock(pkey, self);
        })
    }

    pub fn lock(&self) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => self.lock_in_isr(current_interrupt),
            ExecutionContext::Task(current_task) => self.lock_in_task(current_task),
        }
    }

    fn unlock_in_isr(&self, current_interrupt: &'static InterruptControlBlock) {
        current_interrupt.release_lock(self);
    }

    fn unlock_in_task(&self, current_task: &'static TaskControlBlock) {
        PreemptLock::with(|pkey| {
            // If lock has not been acquired by any task
            if self.owner.get(pkey) == INVALID_TASK_ID {
                return;
            }

            if self.owner.get(pkey) != current_task.task_id {
                runtime_error!(RuntimeError::LockOwnerViolation);
            }

            current_task.release_lock(pkey, self);
        });
    }

    pub fn unlock(&self) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => self.unlock_in_isr(current_interrupt),
            ExecutionContext::Task(current_task) => self.unlock_in_task(current_task),
        }
    }
}

pub struct CeilingLock<const CEILING: AnyPriority> {
    raw: RawCeilingLock,
}

impl<const CEILING: AnyPriority> CeilingLock<CEILING> {
    pub const fn new() -> CeilingLock<CEILING> {
        CeilingLock {
            raw: RawCeilingLock::new(CEILING),
        }
    }

    pub fn lock(&self) {
        self.raw.lock();
    }

    pub fn unlock(&self) {
        self.raw.unlock();
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
    saved_priority: Priority,
}

impl<const CEILING: AnyPriority> Lock for CeilingLock<CEILING> {
    type RestoreState = CeilingLockRestoreState;
    type Key<'lock> = CeilingLockKey<'lock, CEILING>;

    unsafe fn section_start() -> Self::RestoreState {
        let ceiling = Priority::any(CEILING);
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                if !ceiling.is_interrupt() || ceiling.get_value() < get_interrupt_threshold() {
                    runtime_error!(RuntimeError::CeilingPriorityViolation);
                }

                let saved_priority = current_interrupt.raise_section_lock_priority(ceiling);

                CeilingLockRestoreState { saved_priority }
            }
            ExecutionContext::Task(current_task) => {
                let saved_priority = current_task.raise_section_lock_priority(ceiling);

                CeilingLockRestoreState { saved_priority }
            }
        }
    }

    unsafe fn try_section_start() -> Result<Self::RestoreState, TryLockError> {
        Ok(Self::section_start())
    }

    unsafe fn section_end(restore_state: Self::RestoreState) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                current_interrupt.set_section_lock_priority(restore_state.saved_priority);
            }
            ExecutionContext::Task(current_task) => {
                current_task.set_section_lock_priority(restore_state.saved_priority);
                PreemptLock::with(|pkey| {
                    Scheduler::cond_reschedule(pkey);
                });
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CeilingLockKey<'lock, const CEILING: AnyPriority> {
    _private: PhantomData<&'lock ()>,
}

impl<'key, const CEILING: AnyPriority> CeilingLockKey<'key, CEILING> {
    #[inline(always)]
    pub unsafe fn new() -> Self {
        CeilingLockKey {
            _private: PhantomData,
        }
    }
}

impl<'lock, const CEILING: AnyPriority> KeyToken<'lock> for CeilingLockKey<'lock, CEILING> {
    unsafe fn new() -> CeilingLockKey<'lock, CEILING>
    where
        Self: Sized,
    {
        CeilingLockKey::new()
    }
}
