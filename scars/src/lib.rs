#![doc = include_str!("../../README.md")]
#![no_std]
#![feature(adt_const_params)]
#![feature(associated_type_defaults)]
#![feature(maybe_uninit_fill)]
#![feature(maybe_uninit_uninit_array_transpose)]
#![feature(impl_trait_in_assoc_type)]
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
pub mod priority;
pub mod sync;
pub mod task;
pub mod thread;
pub mod time;
pub mod tls;

extern crate self as scars;

pub use scars_macros::*;

pub use events::{
    AtomicEvents, EXECUTOR_WAKEUP_EVENT, EventOptions, Events, TryWaitError, WaitEvents,
    WaitTimeoutError,
};
pub use kernel::abort::abort;
pub use kernel::hal::clock_ticks;
pub use kernel::hal::kernel_hal as khal;
pub use kernel::hal::kernel_hal::{printk, printkln};
pub use kernel::hal::pac;
pub use kernel::scheduler::{EventTimer, Scheduler};
pub use kernel::stack::Stack;
pub use priority::{AnyPriority, Priority};
pub use static_cell;
pub use thread::{Thread, ThreadRef};

pub use api::*;

pub mod prelude {
    pub use crate::delay_until;
    pub use crate::make_channel;
    pub use crate::make_interrupt_handler;
    pub use crate::make_rendezvous;
    pub use crate::make_shared;
    pub use crate::make_thread;
    pub use crate::priority::{AnyPriority, Priority};
    pub use crate::thread::Thread;
}
