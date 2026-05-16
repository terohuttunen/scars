#![no_std]
#![feature(sync_unsafe_cell)]

use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use cortex_m_rt::entry;
pub use defmt::println as printk;
pub use defmt::println as printkln;
use defmt_rtt as _;
use scars_arch_cortex_m::{
    CURRENT_THREAD_CONTEXT, impl_core_controller, init_pendsv_priority, nvic,
};
use scars_khal::*;

pub use stm32h7::*;

#[cfg(feature = "stm32h753v")]
pub use stm32h7::stm32h753v as pac;

pub use pac::{Interrupt, Peripherals};

const PRIO_BITS: u8 = pac::NVIC_PRIO_BITS;

static HAL: SyncUnsafeCell<MaybeUninit<STM32H7>> = SyncUnsafeCell::new(MaybeUninit::uninit());

defmt::timestamp!("{=u32:us}", 0);

#[defmt::panic_handler]
fn defmt_panic() -> ! {
    cortex_m::asm::udf()
}

#[derive(Copy, Clone, Debug)]
pub struct ClockFrequencies {
    pub sysclk: u32,
    pub hclk: u32,
    pub pclk1: u32,
    pub pclk1_timer: u32,
}

pub struct STM32H7 {
    _frequencies: ClockFrequencies,
}

impl STM32H7 {
    /// Chain TIM2 and TIM5 (both 32-bit on H7) as a 64-bit monotonic
    /// counter, identical in spirit to the F4 path.
    fn setup_clock(&mut self) {
        let tim2 = unsafe { &*pac::TIM2::ptr() };
        let tim5 = unsafe { &*pac::TIM5::ptr() };

        let psc = (self._frequencies.pclk1_timer / TIMER_FREQ_HZ as u32) - 1;
        tim2.psc().write(|w| unsafe { w.psc().bits(psc as u16) });
        tim2.egr().write(|w| w.ug().set_bit());

        tim2.cr2().modify(|_r, w| w.mms().update());
        tim2.smcr().modify(|_, w| w.ts().itr0());
        tim2.ccmr1_output()
            .write(|w| unsafe { w.oc1m().bits(0b001) }); // active on match
        tim2.dier().write(|w| w.cc1ie().set_bit());

        tim5.psc().write(|w| unsafe { w.psc().bits(0) });
        tim5.smcr().modify(|_r, w| unsafe { w.sms().bits(0b111) }); // external clock mode 1
        tim5.cr1().modify(|_r, w| w.cen().set_bit());
        tim2.cr1().modify(|_r, w| w.cen().set_bit());
    }
}

/// Default clock config: HSI 64 MHz, no PLL, VOS scale 3. Keeps the
/// board minimally configured so we can boot and run the kernel timer
/// deterministically. Apps targeting full 400/480 MHz operation must
/// replace this (PLL1, voltage scaling, SMPS — all H7-specific).
fn configure_default_clocks() -> ClockFrequencies {
    // HSI is on by default after reset and clocks SYSCLK at 64 MHz.
    // We don't touch RCC.CR / RCC.CFGR — the reset state already
    // gives us HSI → SYSCLK → HCLK → CPU. The flash latency default
    // of 7 wait states is conservative but works at all defaults.
    //
    // APB1 prescaler defaults to /2 (D2PPRE1), so APB1 timer clock =
    // pclk1 × 2 = 64 MHz.
    ClockFrequencies {
        sysclk: 64_000_000,
        hclk: 32_000_000,
        pclk1: 32_000_000,
        pclk1_timer: 64_000_000,
    }
}

impl HardwareAbstractionLayer for STM32H7 {
    const NAME: &'static str = "STM32H7";

    fn instance() -> &'static Self {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }

    unsafe fn init(hal: *mut Self) {
        let cortex_m::Peripherals { mut SCB, .. } = unsafe { cortex_m::Peripherals::steal() };

        let frequencies = configure_default_clocks();

        let rcc = unsafe { &*pac::RCC::ptr() };
        // APB1 LOW enable register — H7 splits APB1 into low/high.
        rcc.apb1lenr()
            .modify(|_, w| w.tim2en().set_bit().tim5en().set_bit());

        nvic::enable(pac::Interrupt::TIM2 as u16);

        init_pendsv_priority(&mut SCB);

        unsafe {
            *hal = STM32H7 {
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

impl InterruptController for STM32H7 {
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

// STM32H753 has up to ~166 interrupts; round up.
const MAX_INTERRUPT_NUMBER: usize = 168;

impl AlarmClockController for STM32H7 {
    const TICK_FREQ_HZ: u64 = TIMER_FREQ_HZ;

    #[inline(always)]
    fn clock_ticks() -> u64 {
        let tim2 = unsafe { &*pac::TIM2::ptr() };
        let tim5 = unsafe { &*pac::TIM5::ptr() };
        let restore_state = Self::acquire();
        loop {
            let high = tim5.cnt().read().bits();
            let low = tim2.cnt().read().bits();
            let new_high = tim5.cnt().read().bits();
            if new_high == high {
                Self::restore(restore_state);
                return ((high as u64) << 32) + low as u64;
            }
        }
    }

    #[inline(always)]
    fn set_wakeup(at: Option<u64>) {
        let restore_state = Self::acquire();
        let tim2 = unsafe { &*pac::TIM2::ptr() };
        let tim5 = unsafe { &*pac::TIM5::ptr() };
        match at {
            None => {
                tim2.dier().modify(|_, w| w.cc1ie().clear_bit());
                tim2.sr().modify(|_, w| w.cc1if().clear_bit());
            }
            Some(target) => {
                let compare_high = (target >> 32) as u32;
                let compare_low = (target & 0xffff_ffff) as u32;

                tim2.ccr1().write(|w| unsafe { w.bits(compare_low) });
                tim5.ccr1().write(|w| unsafe { w.bits(compare_high) });
                // Drop any stale CC1IF latch from a match that fired
                // while CC1IE was masked, so re-enabling below does not
                // immediately dispatch a non-current wakeup.
                tim2.sr().modify(|_, w| w.cc1if().clear_bit());
                tim2.dier().modify(|_, w| w.cc1ie().set_bit());

                let high_cnt = tim5.cnt().read().bits();
                if (high_cnt == compare_high && tim2.cnt().read().bits() > compare_low)
                    || high_cnt > compare_high
                {
                    tim2.egr().write(|w| w.cc1g().set_bit());
                }
            }
        }
        Self::restore(restore_state);
    }
}

impl_core_controller!(STM32H7);

unsafe impl Sync for STM32H7 {}

/// TIM2 IRQ — see scars-khal-stm32f4 for the algorithm. The hard-coded
/// addresses (TIM2 SR @ 0x40000010, TIM5 CNT @ 0x40000C24, TIM5 CCR1 @
/// 0x40000C34) live in the APB1 peripheral window which is mapped at
/// the same base on every Cortex-M STM32 family this crate handles.
#[unsafe(naked)]
#[unsafe(export_name = "TIM2")]
#[unsafe(link_section = ".TIM2.user")]
pub unsafe extern "C" fn tim2() {
    core::arch::naked_asm!(
        "ldr    r0, =CURRENT_THREAD_CONTEXT",
        "ldr    r0, [r0]",
        "movw   r1, 0x0010",
        "movt   r1, 0x4000",
        "mov    r2, #0",
        "str    r2, [r1]",
        "movw   r1, 0x0C24",
        "movt   r1, 0x4000",
        "ldr    r2, [r1]",
        "movw   r1, 0x0C34",
        "movt   r1, 0x4000",
        "ldr    r3, [r1]",
        "cmp    r2, r3",
        "it     lt",
        "bxlt   lr",
        "push   {{r0, lr}}",
        "bl     _kernel_wakeup_handler",
        "pop    {{r0, lr}}",
        "ldr    r1, =CURRENT_THREAD_CONTEXT",
        "ldr    r1, [r1]",
        "b      _switch_context",
    );
}

#[entry]
fn init() -> ! {
    unsafe {
        start_kernel();
    }
}

pub type HAL = STM32H7;
