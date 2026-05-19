//! NVIC-based [`InterruptController`] implementation for RP2350.
//!
//! Delegates to [`scars_arch_cortex_m::nvic`] for the actual register
//! access. The only RP2350-specific bit is the priority-bits count
//! (4 bits → 16 levels) and the size of the per-IRQ kernel table
//! ([`MAX_INTERRUPT_NUMBER`]).

use scars_arch_cortex_m::nvic;
use scars_khal::{GetInterruptNumber, InterruptController};

use crate::{PRIO_BITS, RP2350};

pub struct InterruptClaim {
    interrupt_number: u8,
}

impl GetInterruptNumber for InterruptClaim {
    fn get_interrupt_number(&self) -> u16 {
        self.interrupt_number as u16
    }
}

impl InterruptController for RP2350 {
    const MAX_INTERRUPT_PRIORITY: usize = nvic::Nvic::<PRIO_BITS>::MAX_PRIO as usize;
    const MAX_INTERRUPT_NUMBER: usize = MAX_INTERRUPT_NUMBER;
    type InterruptClaim = InterruptClaim;

    #[inline]
    fn get_interrupt_priority(interrupt_number: u16) -> u8 {
        nvic::Nvic::<PRIO_BITS>::get_priority(interrupt_number)
    }

    #[inline]
    fn set_interrupt_priority(interrupt_number: u16, prio: u8) -> u8 {
        nvic::Nvic::<PRIO_BITS>::set_priority(interrupt_number, prio)
    }

    #[inline]
    fn get_interrupt_threshold() -> u8 {
        nvic::Nvic::<PRIO_BITS>::get_threshold()
    }

    #[inline]
    fn set_interrupt_threshold(threshold: u8) {
        nvic::Nvic::<PRIO_BITS>::set_threshold(threshold)
    }

    fn claim_interrupt() -> Self::InterruptClaim {
        InterruptClaim {
            interrupt_number: nvic::active_irq() as u8,
        }
    }

    fn complete_interrupt(claim: Self::InterruptClaim) {
        nvic::unpend(claim.interrupt_number as u16);
    }

    fn enable_interrupt(interrupt_number: u16) {
        nvic::enable(interrupt_number);
    }

    fn disable_interrupt(interrupt_number: u16) {
        nvic::disable(interrupt_number);
    }

    #[inline(always)]
    fn interrupt_status() -> bool {
        nvic::interrupts_enabled()
    }

    #[inline(always)]
    fn acquire() -> bool {
        nvic::acquire_critical_section()
    }

    #[inline(always)]
    fn restore(restore_state: bool) {
        nvic::restore_critical_section(restore_state)
    }
}

// RP2350 has 52 user IRQ lines (0..=51); round up to leave headroom in
// the kernel's per-IRQ table.
const MAX_INTERRUPT_NUMBER: usize = 64;
