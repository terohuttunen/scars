#![doc = include_str!("../../README.md")]
#![no_std]
#![feature(panic_info_message)]
#![feature(generic_const_exprs)]
#![feature(const_mut_refs)]
#![feature(const_maybe_uninit_assume_init)]
#![feature(maybe_uninit_slice)]
#![feature(const_slice_split_at_mut)]
#![feature(type_alias_impl_trait)]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![test_runner(crate::scars_test::test_runner)]
#![feature(sync_unsafe_cell)]
#![feature(waker_getters)]
#![feature(const_waker)]
#![feature(noop_waker)]
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
    pub use crate::kernel::priority::{any_thread_priority, AnyPriority, Priority};
    pub use crate::make_channel;
    pub use crate::make_interrupt_handler;
    pub use crate::make_rendezvous;
    pub use crate::make_shared;
    pub use crate::make_thread;
    pub use crate::thread::Thread;
}
