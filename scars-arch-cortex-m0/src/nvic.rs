//! NVIC helpers for Cortex-M0 (ARMv6-M).
//!
//! ARMv6-M has no `BASEPRI`, only `PRIMASK` (global enable/disable),
//! so the kernel's "interrupt threshold" abstraction degrades to a
//! binary mask on this architecture:
//!
//! - `set_threshold(MAX_PRIO)` clears `PRIMASK`. The sentinel value
//!   that the kernel uses for "no masking".
//! - `set_threshold(_)` sets `PRIMASK`. All maskable IRQs are
//!   suppressed for the duration of the critical section.
//!
//! The kernel's invariant is that scars-internal exceptions (SVCall,
//! PendSV, TIM2) all run at the lowest NVIC priority, and higher
//! priorities are reserved for user IRQs. Because we cannot
//! selectively mask by priority, kernel critical sections briefly
//! suppress every IRQ — over-restrictive but correct.
//!
//! PRIMASK is the sole source of truth: `get_threshold` reads it and
//! reports either `MAX_PRIO` (unmasked) or `0` (masked). The kernel
//! only uses get/set as a save/restore pair, so `set(get())` is
//! idempotent and that's all the round-trip we need. The per-thread
//! `Context.primask` field saves/restores the bit across thread
//! switches, mirroring how the M3+ crate stores `basepri`.

use cortex_m::peripheral::{NVIC, SCB};

/// 2-priority-bit NVIC math. Cortex-M0 reserves the upper 2 bits of
/// the 8-bit priority byte; we keep the same scars convention
/// (higher numeric value = higher priority) as the M3+ helper.
pub struct Nvic<const PRIO_BITS: u8>;

impl<const PRIO_BITS: u8> Nvic<PRIO_BITS> {
    pub const MAX_PRIO: u8 = (1u8 << PRIO_BITS) - 1;
    const PRIO_SHIFT: u8 = 8 - PRIO_BITS;

    #[inline]
    pub fn get_priority(interrupt_number: u16) -> u8 {
        // ARMv6-M: NVIC IPR is `[u32; 8]` with 4 IRQ priority bytes
        // packed per word.
        let n = interrupt_number as usize;
        let word = n >> 2;
        let shift = (n & 3) * 8;
        let raw_byte = unsafe { ((*NVIC::PTR).ipr[word].read() >> shift) as u8 };
        Self::MAX_PRIO - (raw_byte >> Self::PRIO_SHIFT)
    }

    #[inline]
    pub fn set_priority(interrupt_number: u16, prio: u8) -> u8 {
        let old = Self::get_priority(interrupt_number);
        let raw_byte = (Self::MAX_PRIO - prio) << Self::PRIO_SHIFT;
        let n = interrupt_number as usize;
        let word = n >> 2;
        let shift = (n & 3) * 8;
        unsafe {
            let cur = (*NVIC::PTR).ipr[word].read();
            let new = (cur & !(0xFFu32 << shift)) | ((raw_byte as u32) << shift);
            (*NVIC::PTR).ipr[word].write(new);
        }
        old
    }

    #[inline]
    pub fn get_threshold() -> u8 {
        if cortex_m::register::primask::read().is_active() {
            Self::MAX_PRIO
        } else {
            0
        }
    }

    #[inline]
    pub fn set_threshold(threshold: u8) {
        if threshold == Self::MAX_PRIO {
            unsafe { cortex_m::interrupt::enable() };
        } else {
            cortex_m::interrupt::disable();
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

#[inline]
pub fn active_irq() -> u16 {
    let icsr = unsafe { (*SCB::PTR).icsr.read() };
    (icsr & 0x1FF) as u16 - 16
}

#[inline]
pub fn interrupts_enabled() -> bool {
    cortex_m::register::primask::read().is_active()
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
