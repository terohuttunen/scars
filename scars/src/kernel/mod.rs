#![allow(unused_imports)]
pub(crate) mod clock;
pub mod exception;
pub mod fault_context;
pub(crate) mod idle;
pub(crate) mod scheduler;
pub(crate) mod stack;

use crate::cell::LockedCell;
use crate::printkln;
pub(crate) use crate::priority::{
    AnyPriority, AtomicPriority, InterruptPriority, Priority, ThreadPriority,
};
use crate::sync::interrupt_lock::CoreInterruptLockKey;
use core::cell::UnsafeCell;
pub(crate) use exception::{RuntimeError, handle_runtime_error};
use scars_khal::{ContextInfo, CoreController, HardwareAbstractionLayer};
//pub use scheduler::print_threads;
pub(crate) use scheduler::Scheduler;
pub(crate) use stack::Stack;
pub mod abort;
pub mod atomic_queue;
pub(crate) mod hal;
pub mod list;
pub mod syscall;
pub(crate) mod tracing;
pub mod waiter;

#[unsafe(no_mangle)]
pub fn start_kernel() -> ! {
    crate::kernel::hal::init_hal();

    Scheduler::start_on(crate::kernel::hal::CoreId::DEFAULT);
}
