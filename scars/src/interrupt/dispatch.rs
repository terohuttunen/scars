//! Hardware interrupt entry point and interrupt-threshold control.

use super::{MAX_INTERRUPT_NUMBER, RawInterruptHandler, get_interrupt_vector, interrupt_context};
use crate::kernel::hal::{
    CoreId, MAX_INTERRUPT_PRIORITY, claim_interrupt, complete_interrupt, set_interrupt_threshold,
};
use crate::priority::PriorityOpt;
use scars_khal::GetInterruptNumber;

unsafe extern "C" {
    static mut _isr_stack_start: u8;
}

/// Set the interrupt priority threshold based on ceiling priority.
///
/// For an interrupt-level ceiling, mask interrupts at that priority and
/// below. For a thread-level or absent ceiling, leave all interrupt
/// priorities deliverable.
pub(crate) fn set_ceiling_threshold(ceiling: PriorityOpt) {
    match ceiling {
        PriorityOpt::Interrupt(prio) => set_interrupt_threshold(prio),
        PriorityOpt::Thread(_) | PriorityOpt::None => {
            set_interrupt_threshold(MAX_INTERRUPT_PRIORITY as u8)
        }
    }
}

/// Private kernel interrupt handler entry point
#[unsafe(no_mangle)]
pub(crate) unsafe fn _kernel_interrupt_handler() {
    let claim = claim_interrupt();
    let interrupt_number = claim.get_interrupt_number();

    if interrupt_number > MAX_INTERRUPT_NUMBER as u16 {
        panic!("unexpected interrupt (IRQn={})", interrupt_number);
    }

    let cs = unsafe { crate::sync::lock::interrupt_lock::InterruptLockKey::new(CoreId::current()) };

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
}
