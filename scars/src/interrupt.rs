//! Interrupt handling
//!
mod builder;
mod handler;
mod raw;
mod reference;
pub mod vector;

pub use builder::*;
pub use handler::{InterruptHandler, InterruptHandlerFn};
pub(crate) use raw::*;
pub use reference::*;
pub use vector::*;

pub use critical_section::CriticalSection;

/// Interrupt number type
pub type InterruptNumber = u16;

#[macro_export]
macro_rules! make_interrupt_handler {
    ($intnum: expr, $prio : expr, executor = true) => {{
        let mut handler = $crate::make_interrupt_handler!($intnum, $prio);
        let executor = $crate::make_interrupt_executor!();
        handler.start_executor(executor);
        // Automatically enable default interrupt event when executor is used
        handler.with_default_interrupt_event()
    }};
    ($intnum: expr, $prio : expr) => {{
        type T = impl ::core::marker::Sized + ::core::marker::Send + FnMut();
        static HANDLER: $crate::interrupt::InterruptHandler<{ $prio }, T> =
            $crate::interrupt::InterruptHandler::new();
        HANDLER.init($intnum)
    }};
}

// Re-export HAL constants that users need
pub use crate::kernel::hal::MAX_INTERRUPT_NUMBER;

use crate::kernel::hal::{claim_interrupt, complete_interrupt, set_interrupt_threshold};
use crate::priority::PriorityStatus;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicPtr, Ordering};
use scars_khal::GetInterruptNumber;

static CURRENT_INTERRUPT_CONTROL_BLOCK: AtomicPtr<RawInterruptHandler> =
    AtomicPtr::new(core::ptr::null_mut());

unsafe extern "C" {
    static mut _isr_stack_start: u8;
}

/// Switch the current interrupt context
pub(crate) fn switch_current_interrupt(
    icb_ptr: *mut RawInterruptHandler,
) -> *mut RawInterruptHandler {
    CURRENT_INTERRUPT_CONTROL_BLOCK.swap(icb_ptr, Ordering::SeqCst)
}

/// Restore the current interrupt context
pub(crate) fn restore_current_interrupt(icb_ptr: *mut RawInterruptHandler) {
    CURRENT_INTERRUPT_CONTROL_BLOCK.store(icb_ptr, Ordering::SeqCst)
}

/// Get the current interrupt handler (if in interrupt context)
pub(crate) fn current_interrupt() -> Option<NonNull<RawInterruptHandler>> {
    NonNull::new(CURRENT_INTERRUPT_CONTROL_BLOCK.load(Ordering::SeqCst))
}

/// Set the interrupt priority threshold based on ceiling priority.
///
/// For an interrupt-level ceiling, mask interrupts at that priority and
/// below. For a thread-level or absent ceiling, leave all interrupt
/// priorities deliverable.
pub(crate) fn set_ceiling_threshold(ceiling: PriorityStatus) {
    match ceiling {
        PriorityStatus::Interrupt(prio) => set_interrupt_threshold(prio),
        PriorityStatus::Thread(_) | PriorityStatus::Invalid => {
            set_interrupt_threshold(crate::kernel::hal::MAX_INTERRUPT_PRIORITY as u8)
        }
    }
}

/// Check if currently executing in an interrupt context
#[inline(always)]
pub fn in_interrupt() -> bool {
    !CURRENT_INTERRUPT_CONTROL_BLOCK
        .load(Ordering::SeqCst)
        .is_null()
}

/// Execute code in interrupt context with proper context switching
#[inline]
pub(crate) unsafe fn interrupt_context<R>(
    icb_ptr: *mut RawInterruptHandler,
    f: impl FnOnce() -> R,
) -> R {
    let prev_icb = switch_current_interrupt(icb_ptr);

    // Run the handler first
    let rval = f();

    // Restore the previous interrupt context
    restore_current_interrupt(prev_icb);
    rval
}

/// Private kernel interrupt handler entry point
#[unsafe(no_mangle)]
pub(crate) unsafe fn _private_kernel_interrupt_handler() {
    let claim = claim_interrupt();
    let interrupt_number = claim.get_interrupt_number();

    if interrupt_number > MAX_INTERRUPT_NUMBER as u16 {
        panic!("unexpected interrupt (IRQn={})", interrupt_number);
    }

    let cs = unsafe { crate::sync::interrupt_lock::InterruptLockKey::new() };

    let vector = get_interrupt_vector(interrupt_number as u16, cs);
    let handler_fn: fn(*const RawInterruptHandler) =
        unsafe { core::mem::transmute(vector.handler_ptr) };
    let icb_ptr = vector.icb_ptr as *mut _;

    unsafe {
        interrupt_context(icb_ptr, || {
            handler_fn(icb_ptr);
        });
    }

    complete_interrupt(claim);
}
