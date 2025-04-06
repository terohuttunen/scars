use crate::abort;
use crate::kernel::hal::{Context, Fault};
use crate::printkln;
use core::panic::{Location, PanicInfo};
use scars_khal::FlowController;
use unrecoverable_error::{UnrecoverableError, unrecoverable_error_handler, UnrecoverableErrorInfo};

#[macro_export]
macro_rules! runtime_error {
    ($kind:expr) => {{
        use $crate::kernel::exception::RuntimeError;
        $crate::kernel::syscall::runtime_error(&$kind);
    }};
}

#[derive(Debug, UnrecoverableError)]
pub enum RtosError<'a> {
    /// Internal kernel error
    #[unrecoverable_error("Kernel error: {0}")]
    Kernel(&'a KernelError),

    /// Hardware error from kernel HAL
    #[unrecoverable_error("Hardware error: {0}")]
    Hardware(&'a Fault),

    /// Runtime error
    #[unrecoverable_error("Runtime error: {0}")]
    RuntimeError(&'a dyn UnrecoverableError),

    /// Panic from Rust runtime
    #[unrecoverable_error("Panic: {0}")]
    Panic(&'a PanicInfo<'a>),
}

#[derive(Debug, UnrecoverableError)]
pub enum KernelError {
    /// Stack overflow
    #[unrecoverable_error("Stack overflow in thread {thread_name} with stack size {stack_size}")]
    StackOverflow {
        thread_name: &'static str,
        stack_size: usize,
    },
}

/// Runtime errors are errors that can happen at runtime, and are not
/// related to the kernel or hardware. They are caused by incorrect usage
/// of the kernel API by the application.
#[derive(Debug, UnrecoverableError)]
pub enum RuntimeError {
    /// Idle task may not suspend, because it has to be always ready to run.
    /// Some task must always be able to run if others are suspended.
    IdleTaskSuspend,

    /// Attempt to access mutex from a task with higher than mutex ceiling
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
}

pub fn handle_runtime_error(error: &dyn UnrecoverableError) -> ! {
    let error = RtosError::RuntimeError(error);
    handle_rtos_error(&error);
}

pub fn handle_kernel_error(error: &KernelError) -> ! {
    let error = RtosError::Kernel(error);
    handle_rtos_error(&error);
}

#[unsafe(no_mangle)]
pub unsafe fn _private_hardware_exception_handler(error: &Fault) -> ! {
    let error = RtosError::Hardware(error);
    handle_rtos_error(&error);
}

pub fn handle_rtos_error(error: &RtosError) -> ! {
    crate::kernel::hal::error(error)
}

/*
#[unsafe(no_mangle)]
fn _scars_default_user_exception_handler(_exception: Exception) {}

unsafe extern "Rust" {
    fn _user_exception_handler(exception: Exception);
}
    */

#[unrecoverable_error_handler]
fn handle_unrecoverable_error(error: &UnrecoverableErrorInfo) -> ! {
    let error = RtosError::RuntimeError(error.error);
    handle_rtos_error(&error)
}

#[cfg(not(feature = "khal-sim"))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    crate::kernel::hal::kernel_hal::printkln!("{}", info);
    loop {}
    //printkln!("Panic handler");
    //printkln!("{}", info);
    //unsafe { _user_exception_handler(Exception::Panic(info)) };
    //abort()
}
