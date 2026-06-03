//! Interrupt handling
//!
mod builder;
mod context;
mod dispatch;
mod handler;
mod raw;
mod reference;
pub mod vector;

pub use builder::*;
pub use context::in_interrupt;
pub(crate) use context::{
    current_interrupt, interrupt_context, restore_current_interrupt, switch_current_interrupt,
};
pub(crate) use dispatch::set_ceiling_threshold;
pub use handler::{InterruptHandler, InterruptHandlerFn};
pub(crate) use raw::*;
pub use reference::*;
pub use vector::*;

pub use critical_section::CriticalSection;

/// Interrupt number type
pub type InterruptNumber = u16;

// Re-export HAL constants that users need
pub use crate::kernel::hal::MAX_INTERRUPT_NUMBER;
