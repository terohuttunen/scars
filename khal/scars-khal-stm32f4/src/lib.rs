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

pub use stm32f4::*;

#[cfg(feature = "stm32f429")]
pub use stm32f4::stm32f429 as pac;
#[cfg(feature = "stm32f446")]
pub use stm32f4::stm32f446 as pac;

pub use pac::{Interrupt, Peripherals};

const PRIO_BITS: u8 = pac::NVIC_PRIO_BITS;

// Static HAL instance
static HAL: SyncUnsafeCell<MaybeUninit<STM32F4>> = SyncUnsafeCell::new(MaybeUninit::uninit());

defmt::timestamp!("{=u32:us}", 0);

#[defmt::panic_handler]
fn defmt_panic() -> ! {
    cortex_m::asm::udf()
}

/// Resulting clock frequencies after `configure_default_clocks` runs.
/// Read by the kernel timer to compute prescalers; not strictly needed
/// for the current TIM2/TIM5 chain but stored for future use.
#[derive(Copy, Clone, Debug)]
pub struct ClockFrequencies {
    pub sysclk: u32,
    pub hclk: u32,
    pub pclk1: u32,
    /// APB1 timer clock = pclk1 × 2 when APB1 prescaler != 1, else pclk1.
    pub pclk1_timer: u32,
}

pub struct STM32F4 {
    _frequencies: ClockFrequencies,
}

impl STM32F4 {
    /// Set up TIM2 + TIM5 chained as a 64-bit monotonic clock at
    /// `TIMER_FREQ_HZ` Hz. Both timers are 32-bit on STM32F4.
    fn setup_clock(&mut self) {
        let tim2 = unsafe { &*pac::TIM2::ptr() };
        let tim5 = unsafe { &*pac::TIM5::ptr() };

        // Prescaler for low-word timer TIM2 targeting 10MHz timer tick.
        // tick = pclk1_timer / (PSC + 1). With pclk1_timer = 90 MHz and
        // PSC = 8 → 10 MHz tick. Computed once at boot from the actual
        // pclk1_timer to keep the math truthful if clocks ever change.
        let psc = (self._frequencies.pclk1_timer / TIMER_FREQ_HZ as u32) - 1;
        tim2.psc().write(|w| w.psc().set(psc as u16));

        // Auto-reload is 0xffff_ffff by default.
        tim2.egr().write(|w| w.ug().set_bit());

        // Set TIM2 master mode to Update.
        tim2.cr2().modify(|_r, w| w.mms().update());

        tim2.smcr().modify(|_, w| w.ts().itr0());

        // Trigger interrupt when cnt > compare.
        tim2.ccmr1_output().write(|w| w.oc1m().active_on_match());

        tim2.dier().write(|w| w.cc1ie().enabled());

        // TIM5 uses TIM2 as prescaler.
        tim5.psc().write(|w| w.psc().set(0));

        // Set TIM5 slave mode to external clock mode 1 (count on trigger).
        tim5.smcr().modify(|_r, w| w.sms().ext_clock_mode());

        // Start timers by setting CEN = 1.
        tim5.cr1().modify(|_r, w| w.cen().enabled());
        tim2.cr1().modify(|_r, w| w.cen().enabled());
    }
}

/// Default clock configuration matching the previous `stm32f4xx-hal`
/// `Config::hse(8.MHz()).sysclk(180.MHz())` path:
/// 8 MHz HSE → PLL ×360/8/2 → 180 MHz sysclk, AHB/1, APB1/4, APB2/2,
/// VOS scale 1 + over-drive, FLASH 5 wait states.
fn configure_default_clocks() -> ClockFrequencies {
    let rcc = unsafe { &*pac::RCC::ptr() };
    let pwr = unsafe { &*pac::PWR::ptr() };
    let flash = unsafe { &*pac::FLASH::ptr() };

    // Enable PWR clock so we can write VOS / over-drive bits.
    rcc.apb1enr().modify(|_, w| w.pwren().set_bit());

    // VOS = Scale 1 (0b11) — required for sysclk above 144 MHz.
    pwr.cr().modify(|_, w| unsafe { w.vos().bits(0b11) });

    // Enable HSE and wait for ready.
    rcc.cr().modify(|_, w| w.hseon().set_bit());
    while rcc.cr().read().hserdy().bit_is_clear() {}

    // Configure PLL: HSE 8 MHz / PLLM=8 → 1 MHz VCO input;
    // ×PLLN=360 → 360 MHz VCO; /PLLP=2 → 180 MHz sysclk.
    rcc.pllcfgr().write(|w| unsafe {
        w.pllm().bits(8);
        w.plln().bits(360);
        w.pllp().bits(0b00); // /2
        w.pllq().bits(7);
        w.pllsrc().hse()
    });

    // Enable PLL and wait for ready.
    rcc.cr().modify(|_, w| w.pllon().set_bit());
    while rcc.cr().read().pllrdy().bit_is_clear() {}

    // Switch on the over-drive regulator (required above 168 MHz at
    // VOS scale 1) and wait for the regulator and switch to settle.
    pwr.cr().modify(|_, w| w.oden().set_bit());
    while pwr.csr().read().odrdy().bit_is_clear() {}
    pwr.cr().modify(|_, w| w.odswen().set_bit());
    while pwr.csr().read().odswrdy().bit_is_clear() {}

    // FLASH latency: 5 wait states for sysclk ≤ 180 MHz at VOS scale 1.
    flash.acr().modify(|_, w| unsafe { w.latency().bits(5) });

    // Bus prescalers and switch SYSCLK to PLL.
    rcc.cfgr().modify(|_, w| unsafe {
        w.hpre().bits(0b0000); // /1
        w.ppre1().bits(0b101); // /4 — APB1 = 45 MHz, APB1 timer = 90 MHz
        w.ppre2().bits(0b100); // /2 — APB2 = 90 MHz
        w.sw().bits(0b10) // PLL
    });
    while rcc.cfgr().read().sws().bits() != 0b10 {}

    ClockFrequencies {
        sysclk: 180_000_000,
        hclk: 180_000_000,
        pclk1: 45_000_000,
        pclk1_timer: 90_000_000,
    }
}

impl HardwareAbstractionLayer for STM32F4 {
    const NAME: &'static str = "STM32F4";

    fn instance() -> &'static Self {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }

    unsafe fn init(hal: *mut Self) {
        let cortex_m::Peripherals { mut SCB, .. } = unsafe { cortex_m::Peripherals::steal() };

        let frequencies = configure_default_clocks();

        let rcc = unsafe { &*pac::RCC::ptr() };
        // Enable TIM2 and TIM5 on APB1.
        rcc.apb1enr()
            .modify(|_, w| w.tim2en().set_bit().tim5en().set_bit());

        // Make TIM2 IRQ deliverable. (Priority set below.)
        nvic::enable(pac::Interrupt::TIM2 as u16);

        init_pendsv_priority(&mut SCB);

        unsafe {
            *hal = STM32F4 {
                _frequencies: frequencies,
            };

            (*hal).setup_clock();
            <Self as InterruptController>::set_interrupt_priority(pac::Interrupt::TIM2 as u16, 0);
        }
    }
}

pub const TIMER_FREQ_HZ: u64 = 10_000_000;

pub struct InterruptClaim {
    interrupt_number: u8,
}

impl GetInterruptNumber for InterruptClaim {
    fn get_interrupt_number(&self) -> u16 {
        self.interrupt_number as u16
    }
}

impl InterruptController for STM32F4 {
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

// Upper bound on the IRQ number for kernel data-structure sizing.
// Both stm32f429 (DMA2D = 90) and stm32f446 (FMPI2C1_ERR = 96) fit
// under 97. Keep one number that covers every chip the crate selects;
// over-sizing slightly costs a few entries in the kernel's interrupt
// table.
const MAX_INTERRUPT_NUMBER: usize = 97;

impl AlarmClockController for STM32F4 {
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
            // There was an overflow in tim2, re-read
        }
    }

    #[inline(always)]
    fn set_wakeup(at: Option<u64>) {
        let restore_state = Self::acquire();
        let tim2 = unsafe { &*pac::TIM2::ptr() };
        let tim5 = unsafe { &*pac::TIM5::ptr() };
        match at {
            None => {
                tim2.dier().modify(|_, w| w.cc1ie().disabled());
                tim2.sr().modify(|_, w| w.cc1if().clear_bit());
            }
            Some(target) => {
                let compare_high = (target >> 32) as u32;
                let compare_low = (target & 0xffff_ffff) as u32;

                tim2.ccr1().write(|w| w.set(compare_low));
                tim5.ccr1().write(|w| w.set(compare_high));
                // Drop any stale CC1IF latch from a match that fired
                // while CC1IE was masked, so re-enabling below does not
                // immediately dispatch a non-current wakeup.
                tim2.sr().modify(|_, w| w.cc1if().clear_bit());
                tim2.dier().modify(|_, w| w.cc1ie().enabled());

                let _low_cnt = tim2.cnt().read().bits();
                let high_cnt = tim5.cnt().read().bits();
                if high_cnt == compare_high && tim2.cnt().read().bits() > compare_low {
                    // Low-word already past compare on the current high
                    // word — pend the compare interrupt manually so the
                    // kernel timer does not miss the edge.
                    tim2.egr().write(|w| w.cc1g().set_bit());
                } else if high_cnt > compare_high {
                    tim2.egr().write(|w| w.cc1g().set_bit());
                }
            }
        }
        Self::restore(restore_state);
    }
}

impl_core_controller!(STM32F4);

unsafe impl Sync for STM32F4 {}

/// TIM2 IRQ — STM32F4's monotonic-clock alarm. Reads the chained
/// TIM2/TIM5 64-bit counter against the 64-bit compare and either
/// returns early (low-word fired but high word not yet at compare) or
/// enters `_kernel_wakeup_handler` to fire expired kernel timers.
///
/// The hard-coded addresses (0x40000010 = TIM2_SR, 0x40000C24 = TIM5_CNT,
/// 0x40000C34 = TIM5_CCR1) are part of the APB1 peripheral map and
/// identical across every STM32F4 part this crate supports.
#[unsafe(naked)]
#[unsafe(export_name = "TIM2")]
#[unsafe(link_section = ".TIM2.user")]
pub unsafe extern "C" fn tim2() {
    core::arch::naked_asm!(
        "ldr    r0, =CURRENT_THREAD_CONTEXT",
        "ldr    r0, [r0]",
        // Clear TIM2 interrupt bits in SR register
        "movw   r1, 0x0010",
        "movt   r1, 0x4000",
        "mov    r2, #0",
        "str    r2, [r1]",
        // Read TIM5 CNT register
        "movw   r1, 0x0C24",
        "movt   r1, 0x4000",
        "ldr    r2, [r1]",
        // Read TIM5 CCR1
        "movw   r1, 0x0C34",
        "movt   r1, 0x4000",
        "ldr    r3, [r1]",
        // If TIM5 CNT < TIM5 CCR1 (compare register), then the timer has
        // not yet reached the 64bit compare value, and this interrupt
        // from TIM2 can be ignored.
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

pub type HAL = STM32F4;
