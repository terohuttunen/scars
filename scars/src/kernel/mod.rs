#![allow(unused_imports)]
pub(crate) mod clock;
pub mod exception;
pub(crate) mod idle;
pub mod interrupt;
pub mod priority;
pub(crate) mod scheduler;
pub(crate) mod stack;
pub(crate) mod task;
pub mod wait_queue;
use crate::cell::LockedCell;
use crate::printkln;
use crate::sync::interrupt_lock::InterruptLockKey;
use core::cell::UnsafeCell;
pub(crate) use exception::{handle_runtime_error, RuntimeError};
pub use priority::{any_interrupt_priority, any_task_priority};
pub(crate) use priority::{AnyPriority, AtomicPriority, InterruptPriority, Priority, TaskPriority};
use scars_khal::{ContextInfo, FlowController};
pub use scheduler::print_tasks;
pub(crate) use scheduler::Scheduler;
pub(crate) use stack::Stack;
pub(crate) use task::{Task, TaskControlBlock, TaskState, IDLE_TASK_ID};
pub(crate) use wait_queue::WaitQueue;
pub mod abort;
pub mod atomic_list;
pub(crate) mod hal;
pub mod list;
pub mod syscall;
pub(crate) mod tracing;

#[no_mangle]
pub fn start_kernel(hal: crate::kernel::hal::kernel_hal::HAL) -> ! {
    //#[cfg(not(feature = "arch-std"))]
    //init_isr_stack_canary();
    crate::kernel::hal::init_hal(hal);

    Scheduler::start();
}
