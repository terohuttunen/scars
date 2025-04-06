#![allow(unused_imports)]
pub(crate) mod clock;
pub mod exception;
pub(crate) mod idle;
pub mod interrupt;
pub mod priority;
pub(crate) mod scheduler;
pub(crate) mod stack;
use crate::cell::LockedCell;
use crate::printkln;
use crate::sync::interrupt_lock::InterruptLockKey;
use core::cell::UnsafeCell;
pub(crate) use exception::{RuntimeError, handle_runtime_error};
pub(crate) use priority::{
    AnyPriority, AtomicPriority, InterruptPriority, Priority, ThreadPriority,
};
use scars_khal::{ContextInfo, FlowController, HardwareAbstractionLayer};
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
    //#[cfg(not(feature = "arch-std"))]
    //init_isr_stack_canary();
    crate::kernel::hal::init_hal();

    Scheduler::start();
}
