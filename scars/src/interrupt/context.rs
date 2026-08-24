//! Per-core tracking of the in-flight interrupt handler.

use super::RawInterruptHandler;
use crate::kernel::hal::{CoreId, NUM_CORES};
use crate::kernel::scheduler::Scheduler;
use crate::sync::atomic::{AtomicPtr, Ordering};
use core::ptr::NonNull;

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
    // Restored below regardless of what ceiling state `f` leaves behind.
    let prev_ceiling = Scheduler::get_ceiling();

    // Outermost thread -> interrupt transition: charge the preempted
    // thread the time it ran before this handler executes. Nested
    // interrupts (prev non-null) leave thread accounting untouched.
    #[cfg(feature = "execution-time")]
    if prev_context.is_null() {
        crate::kernel::execution_time::charge_thread_boundary();
    }

    let rval = f();

    // Outermost interrupt -> thread transition: charge the interrupt
    // clock before restoring the slot to thread level, so a nested
    // interrupt cannot race the accounting origin.
    #[cfg(feature = "execution-time")]
    if prev_context.is_null() {
        crate::kernel::execution_time::charge_interrupt_boundary();
    }

    // Restore the previous interrupt context
    restore_current_interrupt(prev_context);
    Scheduler::set_ceiling(prev_ceiling);
    rval
}
