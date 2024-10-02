use crate::abort;
use crate::kernel::hal::{Context, Fault};
use crate::printkln;
use core::panic::{Location, PanicInfo};
use scars_khal::{FaultInfo, FlowController};

#[macro_export]
macro_rules! runtime_error {
    ($kind:expr) => {{
        use $crate::kernel::exception::RuntimeError;
        $crate::kernel::syscall::runtime_error($kind, ::core::panic::Location::caller());
    }};
}

pub enum Exception<'a> {
    Panic(&'a PanicInfo<'a>),
    Fault(&'a Fault),
    RuntimeError(&'a RuntimeError),
}

#[derive(Debug)]
#[repr(usize)]
pub enum RuntimeError {
    /// Idle task may not suspend, because it has to be always ready to run.
    /// Some task must always be able to run if others are suspended.
    IdleTaskSuspend = 1, // TODO: is this even needed any more?

    /// Attempt to access mutex from a task with higher than mutex ceiling
    /// priority.
    CeilingPriorityViolation = 2,

    /// Attempt to release lock from different task than from where it was
    /// acquired.
    LockOwnerViolation = 3,

    /// Tasks should never terminate
    TaskTerminated = 4,

    /// Locks cannot be locked recursively
    RecursiveLock = 5,

    /// Forbidden operation in interrupt handler
    InterruptHandlerViolation = 6,

    BlockingForbidden = 7,

    /// Attempt to use ceiling locking in idle task
    IdleThreadCeilingLock = 8,

    Unknown,
}

impl RuntimeError {
    pub fn from_id(id: usize) -> RuntimeError {
        match id {
            1 => RuntimeError::IdleTaskSuspend,
            2 => RuntimeError::CeilingPriorityViolation,
            3 => RuntimeError::LockOwnerViolation,
            4 => RuntimeError::TaskTerminated,
            5 => RuntimeError::RecursiveLock,
            6 => RuntimeError::InterruptHandlerViolation,
            7 => RuntimeError::IdleThreadCeilingLock,
            _ => RuntimeError::Unknown,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RuntimeError::IdleTaskSuspend => "IdleTaskSuspend",
            RuntimeError::CeilingPriorityViolation => "CeilingPriorityViolation",
            RuntimeError::LockOwnerViolation => "MutexOwnerViolation",
            RuntimeError::TaskTerminated => "TaskTerminated",
            RuntimeError::RecursiveLock => "RecursiveMutex",
            RuntimeError::InterruptHandlerViolation => "InterruptHandlerViolation",
            RuntimeError::BlockingForbidden => "BlockingForbidden",
            RuntimeError::IdleThreadCeilingLock => "IdleTaskCeilingLock",
            RuntimeError::Unknown => "Unknown",
        }
    }
}

pub fn handle_runtime_error(error: RuntimeError, location: &Location<'static>) -> ! {
    #[cfg(not(feature = "khal-sim"))]
    unsafe {
        _user_exception_handler(Exception::RuntimeError(&error))
    };
    printkln!("Runtime error: {} at {}", error.as_str(), location);
    abort()
}

#[no_mangle]
pub unsafe fn _private_hardware_exception_handler(fault: *const u8) -> ! {
    let fault = unsafe { &*(fault as *const Fault) };

    #[cfg(not(feature = "khal-sim"))]
    unsafe {
        _user_exception_handler(Exception::Fault(fault))
    };

    printkln!(
        "Unrecoverable fault: {} at 0x{:x?}",
        fault.name(),
        fault.address(),
    );
    printkln!("{:?}", fault.context());
    abort()
}

#[no_mangle]
fn _scars_default_user_exception_handler(_exception: Exception) {}

extern "Rust" {
    fn _user_exception_handler(exception: Exception);
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
