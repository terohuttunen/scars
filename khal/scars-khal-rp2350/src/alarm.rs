//! TIMER0 alarm driver and [`AlarmClockController`] implementation.
//!
//! TIMER0 has four alarm channels (ALARM_0..3), each backed by its
//! own NVIC line. We split per-core:
//!
//! - **core 0** owns ALARM_0 + TIMER0_IRQ_0 + slot 0
//! - **core 1** owns ALARM_1 + TIMER0_IRQ_1 + slot 1
//!
//! Each core's NVIC only enables its own IRQ. `set_wakeup` flips the
//! caller-core's ALARM_<n>.INTE bit via the RP2350 SET/CLR alias
//! addresses so the sibling core's bit is never disturbed by an
//! RMW race.
//!
//! ALARM_<n> matches only the low 32 bits of TIMER0. The Rust half
//! of each IRQ ([`handle_alarm`]) checks the high half against
//! `TARGET_HI[core]` on every firing and re-arms ALARM if the high
//! word hasn't reached the target yet.

use core::sync::atomic::{AtomicU32, Ordering};

use scars_khal::{AlarmClockController, InterruptController};

use crate::{RP2350, pac, sio_cpuid};

/// TIMER0 tick frequency in Hz; `setup_clock` programs the TICKS
/// divider to deliver this rate from `clk_ref`.
pub const TIMER_FREQ_HZ: u64 = 1_000_000;

/// Software half of the 64-bit ALARM_N compare. ALARM_N only matches
/// the low 32 bits of TIMER0, so the ISR re-checks the high half
/// against TARGET_HI[N] on every firing and re-arms if the high half
/// has not yet caught up.
///
/// Indexed by core id: core 0 owns ALARM_0 + TIMER0_IRQ_0 + slot 0;
/// core 1 owns ALARM_1 + TIMER0_IRQ_1 + slot 1.
static TARGET_HI: [AtomicU32; 2] = [AtomicU32::new(u32::MAX), AtomicU32::new(u32::MAX)];
static TARGET_LO: [AtomicU32; 2] = [AtomicU32::new(u32::MAX), AtomicU32::new(u32::MAX)];

/// Atomic SET alias offset for RP2350 peripheral registers. A write of
/// `mask` to `base + 0x2000` sets the named bits without disturbing
/// the rest of the register; the hardware does the OR. Used here to
/// flip individual ALARM bits in `TIMER0.INTE` from either core
/// without a cross-core RMW race.
const PERI_SET_ALIAS: u32 = 0x2000;
/// Atomic CLEAR alias offset — write-1-to-clear semantics over an
/// AND-NOT mask.
const PERI_CLR_ALIAS: u32 = 0x3000;

#[inline]
unsafe fn peri_atomic_set(reg: *mut u32, mask: u32) {
    unsafe { core::ptr::write_volatile((reg as u32 + PERI_SET_ALIAS) as *mut u32, mask) }
}

#[inline]
unsafe fn peri_atomic_clear(reg: *mut u32, mask: u32) {
    unsafe { core::ptr::write_volatile((reg as u32 + PERI_CLR_ALIAS) as *mut u32, mask) }
}

#[inline]
fn read_ticks() -> u64 {
    let timer = unsafe { &*pac::TIMER0::ptr() };
    loop {
        let hi = timer.timerawh().read().bits();
        let lo = timer.timerawl().read().bits();
        let hi2 = timer.timerawh().read().bits();
        if hi == hi2 {
            return ((hi as u64) << 32) | lo as u64;
        }
    }
}

impl AlarmClockController for RP2350 {
    const TICK_FREQ_HZ: u64 = TIMER_FREQ_HZ;

    #[inline(always)]
    fn clock_ticks() -> u64 {
        read_ticks()
    }

    #[inline(always)]
    fn set_wakeup(at: Option<u64>) {
        let restore_state = Self::acquire();
        let timer = unsafe { &*pac::TIMER0::ptr() };
        // Pick the per-core alarm slot. Each core owns one alarm
        // (ALARM_<cpuid>) and one IRQ (TIMER0_IRQ_<cpuid>); INTE
        // bits are flipped atomically via SET/CLR aliases so the
        // sibling core's bit is never disturbed.
        let core = sio_cpuid() as usize;
        let inte = timer.inte().as_ptr();
        let mask: u32 = 1 << core;
        match at {
            None => {
                timer
                    .armed()
                    .write(|w| unsafe { w.armed().bits(mask as u8) });
                unsafe { peri_atomic_clear(inte, mask) };
                TARGET_HI[core].store(u32::MAX, Ordering::Relaxed);
                TARGET_LO[core].store(u32::MAX, Ordering::Relaxed);
            }
            Some(target) => {
                let target_hi = (target >> 32) as u32;
                let target_lo = target as u32;
                TARGET_HI[core].store(target_hi, Ordering::Relaxed);
                TARGET_LO[core].store(target_lo, Ordering::Relaxed);

                // Writing ALARM_<n> arms it; ARMED bit <n> reads as 1
                // while armed.
                match core {
                    0 => timer.alarm0().write(|w| unsafe { w.bits(target_lo) }),
                    1 => timer.alarm1().write(|w| unsafe { w.bits(target_lo) }),
                    _ => unreachable!(),
                }
                // Drop any stale INTR latch (W1C) before re-enabling
                // INTE, so a previous masked match doesn't fire now.
                timer.intr().write(|w| unsafe { w.bits(mask) });
                unsafe { peri_atomic_set(inte, mask) };

                if read_ticks() >= target {
                    let irq = if core == 0 {
                        pac::Interrupt::TIMER0_IRQ_0
                    } else {
                        pac::Interrupt::TIMER0_IRQ_1
                    };
                    cortex_m::peripheral::NVIC::pend(irq);
                }
            }
        }
        Self::restore(restore_state);
    }
}

unsafe extern "Rust" {
    fn _kernel_wakeup_handler();
}

/// Shared body for both ALARM_N IRQs. `core` selects which alarm /
/// per-core target slot to operate on; the two IRQ trampolines just
/// thread their static core id through here.
#[inline]
fn handle_alarm(core: usize) {
    let timer = unsafe { &*pac::TIMER0::ptr() };
    let mask: u32 = 1 << core;

    // Clear ALARM_<core> INTR latch (W1C).
    timer.intr().write(|w| unsafe { w.bits(mask) });

    let now_hi = timer.timerawh().read().bits();
    let target_hi = TARGET_HI[core].load(Ordering::Relaxed);

    if now_hi >= target_hi {
        // Mask this alarm in INTE before invoking the kernel; the
        // kernel re-arms via `set_wakeup` if more timers are pending.
        let inte = timer.inte().as_ptr();
        unsafe { peri_atomic_clear(inte, mask) };
        TARGET_HI[core].store(u32::MAX, Ordering::Relaxed);
        TARGET_LO[core].store(u32::MAX, Ordering::Relaxed);
        unsafe { _kernel_wakeup_handler() };
    } else {
        // HI still behind — re-arm so we fire again when LO wraps
        // and reaches target_lo, then re-check HI.
        let target_lo = TARGET_LO[core].load(Ordering::Relaxed);
        match core {
            0 => timer.alarm0().write(|w| unsafe { w.bits(target_lo) }),
            1 => timer.alarm1().write(|w| unsafe { w.bits(target_lo) }),
            _ => unreachable!(),
        }
    }
}

/// Rust half of TIMER0_IRQ_0 (core 0's alarm).
#[unsafe(no_mangle)]
extern "C" fn _scars_rp2350_timer0_irq_0() {
    handle_alarm(0)
}

/// Rust half of TIMER0_IRQ_1 (core 1's alarm).
#[unsafe(no_mangle)]
extern "C" fn _scars_rp2350_timer0_irq_1() {
    handle_alarm(1)
}

/// TIMER0_IRQ_0 — naked trampoline (core 0's alarm IRQ). Inline CPUID
/// + array load to fetch old / new Context*. r4 caches the old
/// pointer (callee-saved, AAPCS-preserved) across the Rust handler.
#[unsafe(naked)]
#[unsafe(export_name = "TIMER0_IRQ_0")]
#[unsafe(link_section = ".TIMER0_IRQ_0.user")]
pub unsafe extern "C" fn timer0_irq_0() {
    core::arch::naked_asm!(
        "push   {{r4, r5, r6, lr}}",
        "ldr    r5, =0xd0000000",           // SIO->CPUID
        "ldr    r4, [r5]",
        "ldr    r5, ={array}",
        "ldr    r4, [r5, r4, lsl #2]",      // r4 = old Context*
        "bl     _scars_rp2350_timer0_irq_0",
        "ldr    r5, =0xd0000000",
        "ldr    r1, [r5]",
        "ldr    r5, ={array}",
        "ldr    r1, [r5, r1, lsl #2]",      // r1 = new Context*
        "mov    r0, r4",
        "pop    {{r4, r5, r6, lr}}",
        "b      _switch_context",
        array = sym crate::context::CURRENT_THREAD_CONTEXT,
    );
}

/// TIMER0_IRQ_1 — naked trampoline (core 1's alarm IRQ). Same shape
/// as IRQ_0; the inline CPUID load resolves to whichever core is
/// executing this trampoline (always core 1 in practice, since only
/// core 1's NVIC enables TIMER0_IRQ_1).
#[unsafe(naked)]
#[unsafe(export_name = "TIMER0_IRQ_1")]
#[unsafe(link_section = ".TIMER0_IRQ_1.user")]
pub unsafe extern "C" fn timer0_irq_1() {
    core::arch::naked_asm!(
        "push   {{r4, r5, r6, lr}}",
        "ldr    r5, =0xd0000000",
        "ldr    r4, [r5]",
        "ldr    r5, ={array}",
        "ldr    r4, [r5, r4, lsl #2]",
        "bl     _scars_rp2350_timer0_irq_1",
        "ldr    r5, =0xd0000000",
        "ldr    r1, [r5]",
        "ldr    r5, ={array}",
        "ldr    r1, [r5, r1, lsl #2]",
        "mov    r0, r4",
        "pop    {{r4, r5, r6, lr}}",
        "b      _switch_context",
        array = sym crate::context::CURRENT_THREAD_CONTEXT,
    );
}
