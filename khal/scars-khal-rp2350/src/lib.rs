#![no_std]
#![feature(sync_unsafe_cell)]

use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};
use cortex_m_rt::entry;
pub use defmt::println as printk;
pub use defmt::println as printkln;
use defmt_rtt as _;
use scars_arch_cortex_m::{
    CURRENT_THREAD_CONTEXT, impl_core_controller, init_pendsv_priority, nvic,
};
use scars_khal::*;

pub use pac::{Interrupt, Peripherals};
pub use rp235x_pac as pac;

const PRIO_BITS: u8 = pac::NVIC_PRIO_BITS; // RP2350 M33 NVIC: 4 bits → 16 levels

static HAL: SyncUnsafeCell<MaybeUninit<RP2350>> = SyncUnsafeCell::new(MaybeUninit::uninit());

defmt::timestamp!("{=u32:us}", 0);

#[defmt::panic_handler]
fn defmt_panic() -> ! {
    cortex_m::asm::udf()
}

/// RP2350 boot ROM Image Definition block. The boot ROM searches the
/// first 4 KiB of flash for `BLOCK_MARKER_START`, validates the items
/// between it and `BLOCK_MARKER_END`, and only then jumps to the reset
/// vector. Five words encode the minimum viable secure-Arm executable:
/// marker, IMAGE_TYPE item (EXE | SECURE | ARM | RP2350), LAST item
/// terminator with a zero "next-block" offset, marker.
#[used]
#[unsafe(link_section = ".start_block")]
static IMAGE_DEF: [u32; 5] = [
    0xffff_ded3, // BLOCK_MARKER_START
    0x1021_0142, // ITEM_IMAGE_TYPE: tag=0x42, size=1, flags=EXE|SECURE|ARM|RP2350
    0x0000_01ff, // ITEM_LAST: tag=0xff, size=1
    0x0000_0000, // LAST.offset = 0 (no further blocks)
    0xab12_3579, // BLOCK_MARKER_END
];

#[derive(Copy, Clone, Debug)]
pub struct ClockFrequencies {
    /// Reference clock feeding the TICKS divider. The RP2350 boot ROM
    /// starts XOSC at 12 MHz before handing control to user code for
    /// IMAGE_DEF-tagged secure images, so we treat clk_ref = 12 MHz as
    /// the bring-up baseline.
    pub clk_ref: u32,
}

pub struct RP2350 {
    frequencies: ClockFrequencies,
}

/// Leaves system clocks at their post-bootrom state — clk_ref on XOSC
/// at 12 MHz, clk_sys on ROSC — and drives TIMER0 from the 1 MHz tick
/// produced by the TICKS divider. Replace this function to bring the
/// chip up at 150 MHz from the PLL.
fn configure_default_clocks() -> ClockFrequencies {
    ClockFrequencies {
        clk_ref: 12_000_000,
    }
}

impl RP2350 {
    /// Bring TIMER0 out of reset and program the TICKS divider so the
    /// counter increments at 1 MHz. CYCLES is the number of clk_ref
    /// cycles per tick, so for clk_ref = 12 MHz, CYCLES = 12.
    fn setup_clock(&mut self) {
        let resets = unsafe { &*pac::RESETS::ptr() };
        resets.reset().modify(|_, w| w.timer0().clear_bit());
        while resets.reset_done().read().timer0().bit_is_clear() {}

        let ticks = unsafe { &*pac::TICKS::ptr() };
        let timer0_ticks = ticks.ticktimer0();
        timer0_ticks.ctrl().write(|w| w.enable().clear_bit());
        timer0_ticks
            .cycles()
            .write(|w| unsafe { w.bits(self.frequencies.clk_ref / TIMER_FREQ_HZ as u32) });
        timer0_ticks.ctrl().write(|w| w.enable().set_bit());

        let timer = unsafe { &*pac::TIMER0::ptr() };
        // ARMED is W1C per bit; 0b1111 disarms all four alarms.
        timer.armed().write(|w| unsafe { w.armed().bits(0b1111) });
        timer.inte().write(|w| unsafe { w.bits(0) }); // mask everything
    }
}

impl HardwareAbstractionLayer for RP2350 {
    const NAME: &'static str = "RP2350";

    fn instance() -> &'static Self {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }

    unsafe fn init(hal: *mut Self) {
        let cortex_m::Peripherals { mut SCB, .. } = unsafe { cortex_m::Peripherals::steal() };

        let frequencies = configure_default_clocks();

        nvic::enable(pac::Interrupt::TIMER0_IRQ_0 as u16);
        init_pendsv_priority(&mut SCB);

        unsafe {
            *hal = RP2350 { frequencies };

            (*hal).setup_clock();
            <Self as InterruptController>::set_interrupt_priority(
                pac::Interrupt::TIMER0_IRQ_0 as u16,
                0,
            );
        }
    }
}

pub const TIMER_FREQ_HZ: u64 = 1_000_000;

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

/// Software half of the 64-bit ALARM_0 compare. ALARM_0 only matches the
/// low 32 bits of TIMER0, so the ISR re-checks the high half against
/// TARGET_HI on every firing and re-arms if the high half has not yet
/// caught up.
static TARGET_HI: AtomicU32 = AtomicU32::new(u32::MAX);
static TARGET_LO: AtomicU32 = AtomicU32::new(u32::MAX);

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
        match at {
            None => {
                timer.armed().write(|w| unsafe { w.armed().bits(0b0001) }); // disarm ALARM_0
                timer.inte().modify(|_, w| w.alarm_0().clear_bit());
                TARGET_HI.store(u32::MAX, Ordering::Relaxed);
                TARGET_LO.store(u32::MAX, Ordering::Relaxed);
            }
            Some(target) => {
                let target_hi = (target >> 32) as u32;
                let target_lo = target as u32;
                TARGET_HI.store(target_hi, Ordering::Relaxed);
                TARGET_LO.store(target_lo, Ordering::Relaxed);

                // Writing ALARM_0 arms it; ARMED bit 0 reads as 1 while armed.
                timer.alarm0().write(|w| unsafe { w.bits(target_lo) });
                // Drop any stale latch from a previous match that fired
                // while INTE was masked, so re-enabling INTE below does
                // not immediately dispatch a non-current wakeup.
                timer.intr().write(|w| w.alarm_0().clear_bit_by_one());
                timer.inte().modify(|_, w| w.alarm_0().set_bit());

                if read_ticks() >= target {
                    cortex_m::peripheral::NVIC::pend(pac::Interrupt::TIMER0_IRQ_0);
                }
            }
        }
        Self::restore(restore_state);
    }
}

impl_core_controller!(RP2350);

unsafe impl Sync for RP2350 {}

unsafe extern "Rust" {
    fn _kernel_wakeup_handler();
}

/// Rust half of TIMER0_IRQ_0. Clears the ALARM_0 latch and either
/// calls into the kernel wakeup path (when the high half has reached
/// TARGET_HI) or re-arms ALARM_0 for the next low-half wrap.
#[unsafe(no_mangle)]
extern "C" fn _scars_rp2350_timer0_irq() {
    let timer = unsafe { &*pac::TIMER0::ptr() };

    // Clear ALARM_0 INTR (write-1-to-clear, bit 0).
    timer.intr().write(|w| unsafe { w.bits(1) });

    let now_hi = timer.timerawh().read().bits();
    let target_hi = TARGET_HI.load(Ordering::Relaxed);

    if now_hi >= target_hi {
        // Disable ALARM_0 IRQ before invoking the kernel; the kernel
        // will re-arm via `set_wakeup` if more timers are pending.
        timer.inte().modify(|_, w| w.alarm_0().clear_bit());
        TARGET_HI.store(u32::MAX, Ordering::Relaxed);
        TARGET_LO.store(u32::MAX, Ordering::Relaxed);
        unsafe { _kernel_wakeup_handler() };
    } else {
        // HI still behind — re-arm ALARM_0 so it fires again when LO
        // wraps and reaches target_lo, then we re-check HI.
        let target_lo = TARGET_LO.load(Ordering::Relaxed);
        timer.alarm0().write(|w| unsafe { w.bits(target_lo) });
    }
}

/// TIMER0_IRQ_0 — naked trampoline. Calls the Rust handler and tail-jumps
/// to `_switch_context` so a freshly-readied higher-priority thread can
/// take over without bouncing through PendSV.
#[unsafe(naked)]
#[unsafe(export_name = "TIMER0_IRQ_0")]
#[unsafe(link_section = ".TIMER0_IRQ_0.user")]
pub unsafe extern "C" fn timer0_irq_0() {
    core::arch::naked_asm!(
        "ldr    r0, ={ctx}",
        "ldr    r0, [r0]",
        "push   {{r0, lr}}",
        "bl     _scars_rp2350_timer0_irq",
        "pop    {{r0, lr}}",
        "ldr    r1, ={ctx}",
        "ldr    r1, [r1]",
        "b      _switch_context",
        ctx = sym CURRENT_THREAD_CONTEXT,
    );
}

#[entry]
fn init() -> ! {
    unsafe {
        start_kernel();
    }
}

pub type HAL = RP2350;
