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

#[macro_export]
macro_rules! make_interrupt_handler {
    ($intnum: expr, $prio : expr, executor = true $(, core = $core:expr)?) => {{
        let mut handler = $crate::make_interrupt_handler!($intnum, $prio $(, core = $core)?);
        let executor = $crate::make_interrupt_executor!();
        handler.start_executor(executor);
        // Automatically enable default interrupt event when executor is used
        handler.with_default_interrupt_event()
    }};
    ($intnum: expr, $prio : expr $(, core = $core:expr)?) => {{
        type T = impl ::core::marker::Sized + ::core::marker::Send + FnMut();
        static HANDLER: $crate::interrupt::InterruptHandler<
            { $prio },
            T,
            { $crate::make_interrupt_handler!(@core $($core)?) },
        > = $crate::interrupt::InterruptHandler::new();
        HANDLER.init($intnum)
    }};
    (@core) => {$crate::CoreId::DEFAULT};
    (@core $core:expr) => { $core };
}

// Re-export HAL constants that users need
pub use crate::kernel::hal::MAX_INTERRUPT_NUMBER;
