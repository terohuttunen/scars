#![doc = include_str!("../../README.md")]
#![no_std]
#![feature(adt_const_params)]
#![feature(maybe_uninit_slice)]
#![feature(maybe_uninit_fill)]
#![feature(maybe_uninit_uninit_array_transpose)]
#![feature(impl_trait_in_assoc_type)]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::scars_test::test_runner)]
#![feature(sync_unsafe_cell)]
#![reexport_test_harness_main = "test_main"]
#[macro_use]
pub mod kernel;
pub mod api;
pub mod cell;
pub mod events;
pub mod sync;
pub mod task;
pub mod thread;
pub mod time;
pub mod tls;

extern crate self as scars;

pub use scars_macros::*;

pub use kernel::abort::abort;
pub use kernel::hal::clock_ticks;
pub use kernel::hal::kernel_hal as khal;
pub use kernel::hal::kernel_hal::{printk, printkln};
pub use kernel::hal::pac;
pub use kernel::priority::{AnyPriority, Priority};
pub use kernel::scheduler::{ExecutionContext, Scheduler};
pub use kernel::stack::Stack;
pub use static_cell;
pub use thread::{Thread, ThreadRef};

#[cfg(feature = "semihosting")]
#[macro_use]
pub use semihosting;

use scars_test;

pub use api::*;

pub mod prelude {
    pub use crate::delay_until;
    pub use crate::kernel::priority::{AnyPriority, Priority};
    pub use crate::make_channel;
    pub use crate::make_interrupt_handler;
    pub use crate::make_rendezvous;
    pub use crate::make_shared;
    pub use crate::make_thread;
    pub use crate::thread::Thread;
}
