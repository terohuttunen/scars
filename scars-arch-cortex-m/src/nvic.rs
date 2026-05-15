//! NVIC helpers usable by any Cortex-M KHAL.
//!
//! The helpers operate on raw `u16` interrupt numbers and read/write
//! the NVIC register block directly. They are parameterised on
//! `PRIO_BITS` (the implementation's `NVIC_PRIO_BITS` constant from
//! the PAC) and convert between the SCARS priority convention
//! (higher value = higher priority, range `0..=(2^PRIO_BITS - 1)`)
//! and the Cortex-M hardware convention (lower value = higher
//! priority, MSB-aligned in an 8-bit register).

use cortex_m::peripheral::{NVIC, SCB};
use cortex_m::register::{basepri, primask};

/// Priority/threshold helpers parameterised on the implementation's
/// `NVIC_PRIO_BITS`.
pub struct Nvic<const PRIO_BITS: u8>;

impl<const PRIO_BITS: u8> Nvic<PRIO_BITS> {
    pub const MAX_PRIO: u8 = (1u8 << PRIO_BITS) - 1;
    const PRIO_SHIFT: u8 = 8 - PRIO_BITS;

    #[inline]
    pub fn get_priority(interrupt_number: u16) -> u8 {
        let raw = unsafe { (*NVIC::PTR).ipr[interrupt_number as usize].read() };
        Self::MAX_PRIO - (raw >> Self::PRIO_SHIFT)
    }

    #[inline]
    pub fn set_priority(interrupt_number: u16, prio: u8) -> u8 {
        let old = Self::get_priority(interrupt_number);
        let raw = (Self::MAX_PRIO - prio) << Self::PRIO_SHIFT;
        unsafe {
            (*NVIC::PTR).ipr[interrupt_number as usize].write(raw);
        }
        old
    }

    #[inline]
    pub fn get_threshold() -> u8 {
        Self::MAX_PRIO - (basepri::read() >> Self::PRIO_SHIFT)
    }

    #[inline]
    pub fn set_threshold(threshold: u8) {
        unsafe {
            basepri::write((Self::MAX_PRIO - threshold) << Self::PRIO_SHIFT);
        }
    }
}

#[inline]
pub fn enable(interrupt_number: u16) {
    unsafe {
        (*NVIC::PTR).iser[(interrupt_number >> 5) as usize].write(1 << (interrupt_number & 31));
    }
}

#[inline]
pub fn disable(interrupt_number: u16) {
    unsafe {
        (*NVIC::PTR).icer[(interrupt_number >> 5) as usize].write(1 << (interrupt_number & 31));
    }
}

#[inline]
pub fn unpend(interrupt_number: u16) {
    unsafe {
        (*NVIC::PTR).icpr[(interrupt_number >> 5) as usize].write(1 << (interrupt_number & 31));
    }
}

/// Returns the currently-active exception number minus the 16-entry
/// system-handler offset, i.e. the IRQ number that fired. Read from
/// ICSR.VECTACTIVE.
#[inline]
pub fn active_irq() -> u16 {
    let icsr = unsafe { (*SCB::PTR).icsr.read() };
    (icsr & 0x1FF) as u16 - 16
}

#[inline]
pub fn interrupts_enabled() -> bool {
    primask::read().is_active()
}

#[inline]
pub fn acquire_critical_section() -> bool {
    let was = interrupts_enabled();
    cortex_m::interrupt::disable();
    was
}

#[inline]
pub fn restore_critical_section(restore_state: bool) {
    if restore_state {
        unsafe { cortex_m::interrupt::enable() }
    }
}
