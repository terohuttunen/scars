#![no_std]
#![feature(sync_unsafe_cell)]

use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;
use cortex_m_rt::entry;
pub use defmt::println as printk;
pub use defmt::println as printkln;
use defmt_rtt as _;
use portable_atomic::AtomicU64;
use scars_arch_cortex_m0::{
    CURRENT_THREAD_CONTEXT, impl_core_controller, init_kernel_priorities, nvic,
};
use scars_khal::*;

pub use stm32f0::*;

#[cfg(feature = "stm32f0x1")]
pub use stm32f0::stm32f0x1 as pac;

pub use pac::{Interrupt, Peripherals};

const PRIO_BITS: u8 = pac::NVIC_PRIO_BITS;

static HAL: SyncUnsafeCell<MaybeUninit<STM32F0>> = SyncUnsafeCell::new(MaybeUninit::uninit());

defmt::timestamp!("{=u32:us}", 0);

#[defmt::panic_handler]
fn defmt_panic() -> ! {
    cortex_m::asm::udf()
}

#[derive(Copy, Clone, Debug)]
pub struct ClockFrequencies {
    pub sysclk: u32,
    pub hclk: u32,
    pub pclk: u32,
    /// APB timer clock: equals `pclk` when APB prescaler is /1, else
    /// `pclk * 2`. The F0 family has a single APB (no APB1/APB2 split).
    pub pclk_timer: u32,
}

/// Software-extended upper bits of the 64-bit monotonic counter.
/// Incremented in the TIM2 update ISR on every 16-bit wrap; combined
/// with TIM2.CNT to form the 64-bit tick value.
static OVERFLOW_HIGH: AtomicU64 = AtomicU64::new(0);

/// 64-bit compare target requested via `set_wakeup`. The TIM2 ISR
/// drives the hardware CC1 register to land at this value across
/// overflow boundaries.
static WAKEUP_TARGET: AtomicU64 = AtomicU64::new(u64::MAX);

pub struct STM32F0 {
    _frequencies: ClockFrequencies,
}

/// Bring-up minimal default: leave the chip on HSI 8 MHz post-reset,
/// no PLL. APB prescaler /1 → PCLK = 8 MHz, APB timer clock = 8 MHz.
/// Applications wanting 48 MHz can replace this function with one that
/// configures the PLL.
fn configure_default_clocks() -> ClockFrequencies {
    ClockFrequencies {
        sysclk: 8_000_000,
        hclk: 8_000_000,
        pclk: 8_000_000,
        pclk_timer: 8_000_000,
    }
}

impl STM32F0 {
    /// Configure TIM2 as a 16-bit up-counter wrapping at 0xFFFF,
    /// prescaled to a 1 MHz tick. Software extends the upper 48 bits
    /// via the update-interrupt path in the TIM2 ISR — same scheme
    /// as the F1 KHAL.
    fn setup_clock(&mut self) {
        let tim2 = unsafe { &*pac::TIM2::ptr() };

        let psc = (self._frequencies.pclk_timer / TIMER_FREQ_HZ as u32) - 1;
        tim2.psc().write(|w| w.psc().set(psc as u16));

        tim2.arr().write(|w| w.arr().set(0xFFFF));

        tim2.egr().write(|w| w.ug().set_bit());

        tim2.dier()
            .modify(|_, w| w.uie().set_bit().cc1ie().set_bit());

        tim2.cr1().modify(|_r, w| w.cen().set_bit());
    }
}

impl HardwareAbstractionLayer for STM32F0 {
    const NAME: &'static str = "STM32F0";

    fn instance() -> &'static Self {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }

    unsafe fn init(hal: *mut Self) {
        let cortex_m::Peripherals { mut SCB, .. } = unsafe { cortex_m::Peripherals::steal() };

        let frequencies = configure_default_clocks();

        let rcc = unsafe { &*pac::RCC::ptr() };
        rcc.apb1enr().modify(|_, w| w.tim2en().set_bit());

        nvic::enable(pac::Interrupt::TIM2 as u16);

        // Pin every kernel-owned exception (SVCall, PendSV) and the
        // kernel-timer IRQ to the lowest NVIC priority. Higher levels
        // are reserved for user IRQs.
        init_kernel_priorities(&mut SCB);

        unsafe {
            *hal = STM32F0 {
                _frequencies: frequencies,
            };

            (*hal).setup_clock();
            <Self as InterruptController>::set_interrupt_priority(pac::Interrupt::TIM2 as u16, 0);
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

impl InterruptController for STM32F0 {
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

// STM32F051 interrupt count fits under 32; round up for safety.
const MAX_INTERRUPT_NUMBER: usize = 32;

#[inline]
fn read_ticks() -> u64 {
    let tim2 = unsafe { &*pac::TIM2::ptr() };
    loop {
        let high1 = OVERFLOW_HIGH.load(Ordering::Acquire);
        let low = tim2.cnt().read().cnt().bits() as u64;
        let high2 = OVERFLOW_HIGH.load(Ordering::Acquire);
        if high1 == high2 {
            return (high1 << 16) | low;
        }
    }
}

fn arm_compare(target: u64) {
    let tim2 = unsafe { &*pac::TIM2::ptr() };
    let now_high = OVERFLOW_HIGH.load(Ordering::Acquire);
    if (target >> 16) == now_high {
        tim2.ccr1().write(|w| w.ccr().set((target & 0xFFFF) as u32));
    } else {
        tim2.ccr1().write(|w| w.ccr().set(0));
    }
}

impl AlarmClockController for STM32F0 {
    const TICK_FREQ_HZ: u64 = TIMER_FREQ_HZ;

    #[inline(always)]
    fn clock_ticks() -> u64 {
        read_ticks()
    }

    #[inline(always)]
    fn set_wakeup(at: Option<u64>) {
        let restore_state = Self::acquire();
        let tim2 = unsafe { &*pac::TIM2::ptr() };
        match at {
            None => {
                tim2.dier().modify(|_, w| w.cc1ie().clear_bit());
                tim2.sr().modify(|_, w| w.cc1if().clear_bit());
                WAKEUP_TARGET.store(u64::MAX, Ordering::Release);
            }
            Some(target) => {
                WAKEUP_TARGET.store(target, Ordering::Release);
                arm_compare(target);
                // Drop any stale CC1IF latch from a match that fired
                // while CC1IE was masked, so re-enabling below does not
                // immediately dispatch a non-current wakeup.
                tim2.sr().modify(|_, w| w.cc1if().clear_bit());
                tim2.dier().modify(|_, w| w.cc1ie().set_bit());
                if read_ticks() >= target {
                    tim2.egr().write(|w| w.cc1g().set_bit());
                }
            }
        }
        Self::restore(restore_state);
    }
}

impl_core_controller!(STM32F0, on_idle = scars_arch_cortex_m0::on_idle_active());

unsafe impl Sync for STM32F0 {}

unsafe extern "Rust" {
    fn _kernel_wakeup_handler();
}

/// Rust half of the TIM2 IRQ. Handles UIF (overflow → bump
/// `OVERFLOW_HIGH` and re-arm CC1 if the target lies in the new
/// window) and CC1IF (target reached → kernel wakeup).
#[unsafe(no_mangle)]
extern "C" fn _scars_stm32f0_tim2_irq() {
    let tim2 = unsafe { &*pac::TIM2::ptr() };
    let sr = tim2.sr().read();

    if sr.uif().bit_is_set() {
        tim2.sr().modify(|_, w| w.uif().clear_bit());
        OVERFLOW_HIGH.fetch_add(1, Ordering::AcqRel);
        let target = WAKEUP_TARGET.load(Ordering::Acquire);
        arm_compare(target);
    }

    if sr.cc1if().bit_is_set() {
        tim2.sr().modify(|_, w| w.cc1if().clear_bit());
        let now = read_ticks();
        let target = WAKEUP_TARGET.load(Ordering::Acquire);
        if now >= target {
            WAKEUP_TARGET.store(u64::MAX, Ordering::Release);
            unsafe { _kernel_wakeup_handler() };
        }
    }
}

/// TIM2 IRQ trampoline — ARMv6-M variant. Calls the Rust handler and
/// then tail-jumps to `_switch_context` with the (possibly-updated)
/// CURRENT_THREAD_CONTEXT so a freshly-readied thread can take over
/// without bouncing through PendSV.
#[unsafe(naked)]
#[unsafe(export_name = "TIM2")]
#[unsafe(link_section = ".TIM2.user")]
pub unsafe extern "C" fn tim2() {
    core::arch::naked_asm!(
        "ldr    r0, ={ctx}",
        "ldr    r0, [r0]",
        "mov    r1, lr",
        "push   {{r0, r1}}",
        "bl     _scars_stm32f0_tim2_irq",
        "pop    {{r0, r1}}",
        "mov    lr, r1",
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

pub type HAL = STM32F0;
