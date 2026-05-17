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

use crate::kernel::hal::{
    CoreId, NUM_CORES, claim_interrupt, complete_interrupt, pend_service_call,
    set_interrupt_threshold,
};
use crate::kernel::scheduler::Scheduler;
use crate::priority::PriorityStatus;
use crate::sync::atomic::{AtomicPtr, Ordering};
use core::ptr::NonNull;
use scars_khal::GetInterruptNumber;

/// Per-core pointer to the in-flight interrupt's handler. Each core
/// only ever reads or writes its own slot — entered and unwound by
/// nested `interrupt_context` calls — so cross-core access never
/// aliases.
static CURRENT_INTERRUPT_CONTEXT: [AtomicPtr<RawInterruptHandler>; NUM_CORES] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; NUM_CORES];

#[inline]
fn local_context_slot() -> &'static AtomicPtr<RawInterruptHandler> {
    &CURRENT_INTERRUPT_CONTEXT[CoreId::current().as_usize()]
}

unsafe extern "C" {
    static mut _isr_stack_start: u8;
}

/// Switch the current interrupt context
pub(crate) fn switch_current_interrupt(
    context_ptr: *mut RawInterruptHandler,
) -> *mut RawInterruptHandler {
    local_context_slot().swap(context_ptr, Ordering::SeqCst)
}

/// Restore the current interrupt context
pub(crate) fn restore_current_interrupt(context_ptr: *mut RawInterruptHandler) {
    local_context_slot().store(context_ptr, Ordering::SeqCst)
}

/// Get the current interrupt handler (if in interrupt context)
pub(crate) fn current_interrupt() -> Option<NonNull<RawInterruptHandler>> {
    NonNull::new(local_context_slot().load(Ordering::SeqCst))
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
    !local_context_slot().load(Ordering::SeqCst).is_null()
}

/// Execute code in interrupt context with proper context switching
#[inline]
pub(crate) unsafe fn interrupt_context<R>(
    context_ptr: *mut RawInterruptHandler,
    f: impl FnOnce() -> R,
) -> R {
    let prev_context = switch_current_interrupt(context_ptr);

    // Run the handler first
    let rval = f();

    // Restore the previous interrupt context
    restore_current_interrupt(prev_context);
    rval
}

/// Private kernel interrupt handler entry point
#[unsafe(no_mangle)]
pub(crate) unsafe fn _kernel_interrupt_handler() {
    let claim = claim_interrupt();
    let interrupt_number = claim.get_interrupt_number();

    if interrupt_number > MAX_INTERRUPT_NUMBER as u16 {
        panic!("unexpected interrupt (IRQn={})", interrupt_number);
    }

    let cs = unsafe { crate::sync::interrupt_lock::InterruptLockKey::new(CoreId::current()) };

    let vector = get_interrupt_vector(interrupt_number as u16, cs);
    let handler_fn: fn(*const RawInterruptHandler) =
        unsafe { core::mem::transmute(vector.handler_ptr) };
    let context_ptr = vector.context_ptr as *mut _;

    unsafe {
        interrupt_context(context_ptr, || {
            handler_fn(context_ptr);
        });
    }

    complete_interrupt(claim);

    if Scheduler::is_reschedule_pending() {
        pend_service_call();
    }
}
