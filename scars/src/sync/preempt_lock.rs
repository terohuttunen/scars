use super::{NestingLock, TryLockError};
use crate::kernel::scheduler::{ExecutionContext, Scheduler};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicPtr, Ordering};

#[derive(Clone, Copy, Debug)]
pub struct PreemptLockKey<'lock> {
    _private: PhantomData<&'lock ()>,
}

impl<'lock> PreemptLockKey<'lock> {
    /// Creates a pre-emption lock token.
    #[inline(always)]
    pub unsafe fn new() -> Self {
        PreemptLockKey {
            _private: PhantomData,
        }
    }
}

pub struct PreemptLock {}

impl PreemptLock {
    #[inline(always)]
    pub fn with<R>(f: impl FnOnce(PreemptLockKey<'_>) -> R) -> R {
        let restore_state = unsafe { Self::acquire_nesting_lock() };
        let key = unsafe { PreemptLockKey::new() };
        let result = f(key);
        unsafe { Self::release_nesting_lock(restore_state) };
        result
    }

    #[inline(always)]
    pub fn try_with<R>(f: impl FnOnce(PreemptLockKey<'_>) -> R) -> Result<R, TryLockError> {
        match unsafe { Self::try_acquire_nesting_lock() } {
            Ok(restore_state) => {
                let key = unsafe { PreemptLockKey::new() };
                let result = f(key);
                unsafe { Self::release_nesting_lock(restore_state) };
                Ok(result)
            }
            Err(err) => Err(err),
        }
    }

    unsafe fn acquire_nesting_lock() -> PreemptLockRestoreState {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(_current_interrupt) => {
                crate::runtime_error!(RuntimeError::InterruptHandlerViolation)
            }
            ExecutionContext::Thread(current_thread) => {
                let previous_state =
                    PREEMPT_LOCK.swap(current_thread as *const _ as *mut (), Ordering::Acquire);
                PreemptLockRestoreState::Thread(previous_state as *const _)
            }
        }
    }

    unsafe fn try_acquire_nesting_lock() -> Result<PreemptLockRestoreState, TryLockError> {
        match Scheduler::current_execution_context() {
            ExecutionContext::Interrupt(current_interrupt) => {
                // To acquire in interrupt, the lock may not be held by a thread or a
                // lower priority interrupt.
                match PREEMPT_LOCK.compare_exchange(
                    core::ptr::null_mut(),
                    current_interrupt as *const _ as *mut (),
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(previous_state) => {
                        // First lock in current interrupt handler
                        Ok(PreemptLockRestoreState::Interrupt(
                            previous_state as *const _,
                        ))
                    }
                    Err(previous_state) => {
                        if previous_state == current_interrupt as *const _ as *mut () {
                            // Previous state was not null, but it points to the current interrupt context,
                            // therefore the lock was taken earlier in the current interrupt handler.
                            Ok(PreemptLockRestoreState::Interrupt(
                                previous_state as *const _,
                            ))
                        } else {
                            // Lock not acquired. Thread or lower priority interrupt is holding the lock.
                            Err(TryLockError::WouldBlock)
                        }
                    }
                }
            }
            ExecutionContext::Thread(current_thread) => {
                let previous_state =
                    PREEMPT_LOCK.swap(current_thread as *const _ as *mut (), Ordering::Acquire);
                Ok(PreemptLockRestoreState::Thread(previous_state as *const _))
            }
        }
    }

    unsafe fn release_nesting_lock(restore_state: PreemptLockRestoreState) {
        if restore_state.is_null() {
            let key = unsafe { PreemptLockKey::new() };

            // This is the lock that was first acquired, and now released last.
            // Move any pending ready threads to ready queue while we still have
            // the ownership of the lock.
            Scheduler::complete_pending(key);

            // Release the lock
            PREEMPT_LOCK.store(core::ptr::null_mut(), Ordering::Release);

            // Rescheduling was not allowed while the preemption lock was held.
            // If any operations were made that require rescheduling, a pending
            // reschedule flag is set, and the pending reschedule is executed
            // as soon as it is again possible, which is after the preemption lock
            // is released.
            //
            // Execution of any pending reschedule is done differently in threads
            // and interrupts:
            //  - Threads: Pending reschedules are executed when preemption lock
            //    is released. (below)
            //
            //  - Interrupts: Pending reschedule is executed only once when exiting
            //    the interrupt context, regardless of how many preemption
            //    locks were acquired during the interrupt.
            if !restore_state.is_interrupt() {
                if Scheduler::is_reschedule_pending() {
                    crate::thread_yield();
                }
            }
        }
    }
}

unsafe impl Send for PreemptLock {}
unsafe impl Sync for PreemptLock {}

static PREEMPT_LOCK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

mod sealed {
    use crate::kernel::interrupt::RawInterruptHandler;
    use crate::thread::RawThread;

    pub enum PreemptLockRestoreState {
        Interrupt(*const RawInterruptHandler),
        Thread(*const RawThread),
    }

    impl PreemptLockRestoreState {
        pub fn is_interrupt(&self) -> bool {
            matches!(self, PreemptLockRestoreState::Interrupt(_))
        }

        pub fn is_null(&self) -> bool {
            match self {
                PreemptLockRestoreState::Interrupt(current_interrupt) => {
                    current_interrupt.is_null()
                }
                PreemptLockRestoreState::Thread(current_thread) => current_thread.is_null(),
            }
        }
    }
}

pub type PreemptLockRestoreState = sealed::PreemptLockRestoreState;

impl NestingLock for PreemptLock {
    type Key<'a> = PreemptLockKey<'a>;

    fn with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> R {
        Self::with(f)
    }

    fn try_with<R>(f: impl FnOnce(Self::Key<'_>) -> R) -> Result<R, TryLockError> {
        Self::try_with(f)
    }

    unsafe fn get_key_unchecked<'a>() -> Self::Key<'a> {
        unsafe { PreemptLockKey::new() }
    }
}

#[inline]
pub fn is_preempt_allowed() -> bool {
    PREEMPT_LOCK.load(Ordering::SeqCst).is_null()
}
