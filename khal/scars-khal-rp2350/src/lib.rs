#![no_std]
#![feature(sync_unsafe_cell)]

use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use cortex_m_rt::entry;
pub use defmt::println as printk;
pub use defmt::println as printkln;
use defmt_rtt as _;
use scars_arch_cortex_m::init_pendsv_priority;
use scars_arch_cortex_m::nvic;
use scars_khal::*;

pub use pac::{Interrupt, Peripherals};
pub use rp235x_pac as pac;

mod alarm;
mod context;
mod interrupt;
mod ipi;
mod multicore;

pub use alarm::TIMER_FREQ_HZ;
pub use interrupt::InterruptClaim;

/// RP2350 M33 NVIC: 4 bits → 16 priority levels. Exposed at crate
/// root so `interrupt.rs` can parameterise `nvic::Nvic<PRIO_BITS>`.
pub(crate) const PRIO_BITS: u8 = pac::NVIC_PRIO_BITS;

/// SIO peripheral base. SIO is a single-cycle IO peripheral aliased
/// per-core: reading the same address from each core gives that core's
/// own view of `CPUID`, the inter-core FIFOs, etc.
pub(crate) const SIO_BASE: u32 = 0xD000_0000;

/// `SIO->CPUID` — returns `0` on core 0, `1` on core 1. The only
/// architectural way to identify the calling core on RP2350.
#[inline(always)]
pub(crate) fn sio_cpuid() -> u8 {
    unsafe { core::ptr::read_volatile(SIO_BASE as *const u32) as u8 }
}

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
        // Note: `SIO_IRQ_FIFO` is NOT enabled here. The boot-ROM
        // handshake in `launch_core1` below uses the same FIFO to
        // exchange the launch sequence with core 1; if our cross-core
        // IPI handler were armed during the handshake, it would
        // drain core 1's echoed replies before `launch_core1`'s
        // blocking pop could read them, and the launch loop would
        // hang. We enable the IPI after the handshake completes.
        init_pendsv_priority(&mut SCB);

        unsafe {
            *hal = RP2350 { frequencies };

            (*hal).setup_clock();
            <Self as InterruptController>::set_interrupt_priority(
                pac::Interrupt::TIMER0_IRQ_0 as u16,
                0,
            );
        }

        // Bring up core 1. Returns once core 1's
        // `CoreController::start_first_thread` has posted
        // `CORE1_ALIVE_SENTINEL` — which only happens after
        // `Scheduler::start_on(1)` has fully published `SCHEDULERS[1]`
        // and set `SCHEDULER_INITIALIZED[1]`. The `dmb` inside
        // `launch_core1` synchronises core 0's view of core 1's
        // normal-memory writes after the MMIO FIFO read. Together
        // these guarantee the first cross-core dispatch on core 0
        // (after `init_hal` returns) sees a valid core-1 scheduler.
        multicore::launch_core1();

        // Arm core 0's cross-core IPI handler only AFTER the launch
        // handshake completes — otherwise the IPI's drain handler
        // would consume core 1's echoed launch replies before
        // `launch_core1` could read them.
        nvic::enable(pac::Interrupt::SIO_IRQ_FIFO as u16);
        <Self as InterruptController>::set_interrupt_priority(
            pac::Interrupt::SIO_IRQ_FIFO as u16,
            0,
        );
    }
}

unsafe impl Sync for RP2350 {}

#[entry]
fn init() -> ! {
    unsafe {
        start_kernel();
    }
}

pub type HAL = RP2350;
