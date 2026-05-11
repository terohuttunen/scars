use crate::abort;
use crate::kernel::fault_context::{BootstrapContext, InterruptContext, ThreadContext};
use crate::kernel::hal::{self, Context, HardwareFault};
use crate::kernel::scheduler::{ExecutionContext, Scheduler};
use crate::printkln;
use core::panic::{Location, PanicInfo};
use scars_fault::{Fault, FaultContextNode, FaultInfo, fault_handler};
use scars_khal::FlowController;

#[macro_export]
macro_rules! runtime_error {
    ($kind:expr) => {{
        #[allow(unused)]
        use $crate::kernel::exception::RuntimeError;
        $crate::kernel::syscall::runtime_error(&$kind);
    }};
}

#[derive(Debug, Fault)]
pub enum KernelError {
    /// Stack overflow
    #[fault("Stack overflow in thread {thread_name} with stack size {stack_size}")]
    StackOverflow {
        thread_name: &'static str,
        stack_size: usize,
    },
}

/// Runtime errors are errors that can happen at runtime, and are not
/// related to the kernel or hardware. They are caused by incorrect usage
/// of the kernel API by the application.
#[derive(Debug, Fault)]
pub enum RuntimeError {
    /// Idle task may not suspend, because it has to be always ready to run.
    /// Some task must always be able to run if others are suspended.
    IdleTaskSuspend,

    /// Attempt to access mutex from a thread with higher than mutex ceiling
    /// priority.
    CeilingPriorityViolation,

    /// Attempt to release lock from different task than from where it was
    /// acquired.
    LockOwnerViolation,

    /// Tasks should never terminate
    TaskTerminated,

    /// Locks cannot be locked recursively
    RecursiveLock,

    /// Forbidden operation in interrupt handler
    InterruptHandlerViolation,

    BlockingForbidden,

    /// Attempt to use ceiling locking in idle task
    IdleThreadCeilingLock,

    /// Inheritance locks may not be acquired while holding any ceiling locks.
    InheritanceLockNotAllowed,
}

#[track_caller]
pub fn handle_runtime_error(error: &dyn Fault) -> ! {
    let info = FaultInfo {
        error,
        location: Some(Location::caller()),
        context: None,
    };
    dispatch_fault(&info)
}

#[track_caller]
pub fn handle_kernel_error(error: &KernelError) -> ! {
    let info = FaultInfo {
        error,
        location: Some(Location::caller()),
        context: None,
    };
    dispatch_fault(&info)
}

#[unsafe(no_mangle)]
pub unsafe fn _private_hardware_exception_handler(error: &HardwareFault) -> ! {
    let info = FaultInfo {
        error,
        location: None,
        context: None,
    };
    dispatch_fault(&info)
}

/// Build the running-context frame for the current execution context
/// (thread / interrupt / pre-scheduler), prepend it to `info`, and
/// hand off to the platform fault entry.
///
/// Reads `LockedCell` thread fields via `as_ptr` because the fault
/// path must not take any preempt locks — the cells are written under
/// `PreemptLock`, but here we are heading to `-> !` and a torn read is
/// preferable to a deadlock.
fn dispatch_fault(info: &FaultInfo) -> ! {
    if !Scheduler::is_initialized() {
        let ctx = BootstrapContext;
        let node = FaultContextNode {
            frame: &ctx,
            next: info.context,
        };
        let info = info.with_context(&node);
        hal::fault(&info)
    } else {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(thread) => {
                let ctx = ThreadContext {
                    thread_id: thread.thread_id,
                    name: thread.name,
                    base_priority: thread.base_priority,
                    active_priority: unsafe { *thread.priority.as_ptr() },
                    state: unsafe { *thread.state.as_ptr() },
                };
                let node = FaultContextNode {
                    frame: &ctx,
                    next: info.context,
                };
                let info = info.with_context(&node);
                hal::fault(&info)
            }
            ExecutionContext::Interrupt(irq) => {
                let ctx = InterruptContext {
                    irq_number: irq.interrupt_number(),
                    base_priority: irq.base_priority(),
                    active_priority: irq.priority(),
                };
                let node = FaultContextNode {
                    frame: &ctx,
                    next: info.context,
                };
                let info = info.with_context(&node);
                hal::fault(&info)
            }
        }
    }
}

/*
#[unsafe(no_mangle)]
fn _scars_default_user_exception_handler(_exception: Exception) {}

unsafe extern "Rust" {
    fn _user_exception_handler(exception: Exception);
}
    */

#[fault_handler]
fn handle_fault(info: &FaultInfo) -> ! {
    dispatch_fault(info)
}

#[cfg(not(feature = "khal-sim"))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::kernel::hal::kernel_hal::printkln!("{}", info);
    loop {}
}
