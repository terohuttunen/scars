#![doc = include_str!("../../README.md")]
#![no_std]
#![feature(adt_const_params)]
#![feature(associated_type_defaults)]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(scars_test::test_runner)]
#![feature(sync_unsafe_cell)]
#![feature(type_alias_impl_trait)]
#![feature(never_type)]
#![reexport_test_harness_main = "test_main"]
#[macro_use]
pub mod kernel;
pub mod api;
pub mod cell;
pub mod events;
pub mod interrupt;
pub mod local;
pub mod priority;
pub mod sync;
pub mod task;
pub mod thread;
pub mod time;

// `raii-locks` and `priority-inheritance` each enable `multithreading`
// via a Cargo dependency edge (see `Cargo.toml`), so they can never be
// active without it. `multi-core` is independent (per-core idle threads
// do not context-switch), so it does not imply `multithreading`.
extern crate self as scars;

pub use scars_fault::{
    Fault, FaultContext, FaultContextNode, FaultInfo, fault, fault_handler, handle_fault,
};
pub use scars_macros::*;

pub use events::{AtomicEvents, EXECUTOR_WAKEUP_EVENT, EventOptions, Events};
#[cfg(feature = "multithreading")]
pub use events::{TryWaitError, WaitEvents, WaitTimeoutError};
pub use kernel::abort::abort;
pub use kernel::hal::kernel_hal as khal;
pub use kernel::hal::kernel_hal::{printk, printkln};
pub use kernel::hal::pac;
pub use kernel::hal::{CoreId, CoreToken, NUM_CORES, clock_ticks};
pub use kernel::scheduler::{EventTimer, Scheduler};
pub use kernel::stack::Stack;
pub use priority::{AnyPriority, Priority};
pub use static_cell;
#[cfg(feature = "multithreading")]
pub use thread::Thread;
pub use thread::ThreadRef;

pub use api::*;

pub mod prelude {
    pub use crate::priority::{AnyPriority, Priority};
    #[cfg(feature = "multithreading")]
    pub use crate::{delay_until, make_channel, make_rendezvous, thread::Thread};
}
