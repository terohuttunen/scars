#![no_std]
#![feature(ptr_sub_ptr)]
#![feature(sync_unsafe_cell)]
pub mod peripherals;
use core::arch::global_asm;
use core::cell::RefCell;
use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use cortex_m_rt::entry;
use critical_section::Mutex;
pub use peripherals::Peripherals;
pub use rtt_target::debug_rprint as debug_printk;
pub use rtt_target::debug_rprintln as debug_printkln;
pub use rtt_target::rprint as printk;
pub use rtt_target::rprintln as printkln;
use rtt_target::rtt_init_print;
use scars_arch_cortex_m::*;
use scars_khal::*;
pub use stm32f4xx_hal::pac::Interrupt;
pub use stm32f4xx_hal::{
    pac,
    prelude::*,
    rcc::{APB1, Clocks, Enable, Rcc, RccBus},
    timer::{Event, Timer},
};

const NVIC_PRIO_MAX: u8 = 2u8.pow(pac::NVIC_PRIO_BITS as u32) - 1;
const NVIC_PRIO_SHIFT: u8 = 8 - pac::NVIC_PRIO_BITS;

// Static HAL instance
static HAL: SyncUnsafeCell<MaybeUninit<STM32F4>> = SyncUnsafeCell::new(MaybeUninit::uninit());

pub struct STM32F4 {
    nvic: Mutex<RefCell<cortex_m::peripheral::NVIC>>,
    fpu: cortex_m::peripheral::FPU,
    syst: cortex_m::peripheral::SYST,
    tim2: pac::TIM2,
    tim5: pac::TIM5,
    scb: cortex_m::peripheral::SCB,
    clocks: Clocks,
}

impl STM32F4 {
    /// Setup monotonous clock and alarm
    fn setup_clock(&mut self) {
        // Monotonous 64bit clock is setup by chaining two 32bit timers TIM2 and TIM5

        // Prescaler for low-word timer TIM2 targeting 10MHz timer tick
        //  prescaler = core clock frequency / timer clock frequency - 1
        //  prescaler = 90MHz / 10MHz - 1 = 8
        self.tim2.psc.write(|w| w.psc().bits(8));
        // Auto-reload is 0xffff_ffff by default
        self.tim2.egr.write(|w| w.ug().set_bit());

        // Set TIM2 master mode to Update
        self.tim2.cr2.modify(|_r, w| w.mms().update());

        self.tim2.smcr.modify(|_, w| w.ts().itr0());

        // Trigger interrupt when cnt > compare
        self.tim2
            .ccmr1_output()
            .write(|w| w.oc1m().active_on_match());

        self.tim2.dier.write(|w| w.cc1ie().enabled());

        // TIM5 uses TIM2 as prescaler
        self.tim5.psc.write(|w| w.psc().bits(0));

        // Set TIM5 slave mode to Encoder mode 1
        self.tim5.smcr.modify(|_r, w| w.sms().ext_clock_mode());

        // Start timers by settings CEN = 1
        self.tim5.cr1.modify(|_r, w| w.cen().enabled());
        self.tim2.cr1.modify(|_r, w| w.cen().enabled());
    }
}

impl HardwareAbstractionLayer for STM32F4 {
    const NAME: &'static str = "STM32F4";

    fn instance() -> &'static Self {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }

    unsafe fn init(hal: *mut Self) {
        let pac::Peripherals {
            TIM2, TIM5, RCC, ..
        } = unsafe { pac::Peripherals::steal() };

        let cortex_m::Peripherals {
            FPU,
            NVIC,
            SYST,
            SCB,
            ..
        } = unsafe { cortex_m::Peripherals::steal() };

        pac::TIM2::enable(&RCC);
        pac::TIM5::enable(&RCC);

        unsafe { pac::NVIC::unmask(pac::Interrupt::TIM2) };

        let rcc = RCC.constrain();

        let clocks = rcc.cfgr.use_hse(8.MHz()).sysclk(180.MHz()).freeze();

        unsafe {
            *hal = STM32F4 {
                nvic: Mutex::new(RefCell::new(NVIC)),
                fpu: FPU,
                syst: SYST,
                tim2: TIM2,
                tim5: TIM5,
                scb: SCB,
                clocks,
            };

            (*hal).setup_clock();
            Self::set_interrupt_priority(pac::Interrupt::TIM2 as u16, 0);
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
    const MAX_INTERRUPT_PRIORITY: usize = NVIC_PRIO_MAX as usize;
    const MAX_INTERRUPT_NUMBER: usize = pac::Interrupt::DMA2D as usize;
    type InterruptClaim = InterruptClaim;

    fn get_interrupt_priority(interrupt_number: u16) -> u8 {
        let interrupt: pac::Interrupt = unsafe { core::mem::transmute(interrupt_number) };
        NVIC_PRIO_MAX - (pac::NVIC::get_priority(interrupt) >> NVIC_PRIO_SHIFT)
    }

    fn set_interrupt_priority(interrupt_number: u16, prio: u8) -> u8 {
        let cortex_prio = NVIC_PRIO_MAX - prio;
        let interrupt: pac::Interrupt = unsafe { core::mem::transmute(interrupt_number) };
        let old_prio = Self::get_interrupt_priority(interrupt_number);
        critical_section::with(|cs| unsafe {
            Self::instance().nvic
                .borrow(cs)
                .borrow_mut()
                .set_priority(interrupt, cortex_prio);
        });
        old_prio
    }

    //#[inline(always)]
    fn get_interrupt_threshold() -> u8 {
        NVIC_PRIO_MAX - (cortex_m::register::basepri::read() >> NVIC_PRIO_SHIFT)
    }

    //#[inline(always)]
    fn set_interrupt_threshold(threshold: u8) {
        unsafe {
            cortex_m::register::basepri::write((NVIC_PRIO_MAX - threshold) << NVIC_PRIO_SHIFT);
        }
    }

    fn claim_interrupt() -> Self::InterruptClaim {
        let icsr = Self::instance().scb.icsr.read();
        let interrupt_number = icsr as u8 - 16;
        InterruptClaim { interrupt_number }
    }

    fn complete_interrupt(claim: Self::InterruptClaim) {
        let interrupt: pac::Interrupt =
            unsafe { core::mem::transmute(claim.interrupt_number as u16) };
        pac::NVIC::unpend(interrupt);
    }

    fn enable_interrupt(interrupt_number: u16) {
        let interrupt: pac::Interrupt = unsafe { core::mem::transmute(interrupt_number) };
        unsafe { pac::NVIC::unmask(interrupt) };
    }

    fn disable_interrupt(interrupt_number: u16) {
        let interrupt: pac::Interrupt = unsafe { core::mem::transmute(interrupt_number) };
        pac::NVIC::mask(interrupt);
    }

    #[inline(always)]
    fn interrupt_status() -> bool {
        let primask = cortex_m::register::primask::read();
        primask.is_active()
    }

    #[inline(always)]
    fn acquire() -> bool {
        let primask = cortex_m::register::primask::read();
        let restore_state = primask.is_active();
        cortex_m::interrupt::disable();
        restore_state
    }

    #[inline(always)]
    fn restore(restore_state: bool) {
        // Only re-enable interrupts if they were enabled before the critical section.
        if restore_state {
            unsafe { cortex_m::interrupt::enable() }
        }
    }
}

impl AlarmClockController for STM32F4 {
    const TICK_FREQ_HZ: u64 = TIMER_FREQ_HZ;

    #[inline(always)]
    fn clock_ticks() -> u64 {
        let restore_state = Self::acquire();
        loop {
            let high = Self::instance().tim5.cnt.read().bits();
            let low = Self::instance().tim2.cnt.read().bits();
            let new_high = Self::instance().tim5.cnt.read().bits();
            if new_high == high {
                Self::restore(restore_state);
                return ((high as u64) << 32) + low as u64;
            }
            // There was an overflow in tim2, re-read
        }
    }

    #[inline(always)]
    fn set_wakeup(at: Option<u64>) {
        let at = at.unwrap_or(u64::MAX);
        let restore_state = Self::acquire();
        let compare_high = (at >> 32) as u32;
        let compare_low = (at & 0xffff_ffff) as u32;

        Self::instance().tim2.ccr1().write(|w| w.bits(compare_low));
        Self::instance().tim5.ccr1().write(|w| w.bits(compare_high));

        let low_cnt = Self::instance().tim2.cnt.read().bits();
        let high_cnt = Self::instance().tim5.cnt.read().bits();
        if high_cnt == compare_high && Self::instance().tim2.cnt.read().bits() > compare_low {
            // If there was no overflow in tim5, but tim2 cnt is greater than compare_low,
            // trigger interrupt.
            Self::instance().tim2.egr.write(|w| w.cc1g().set_bit());
        } else if high_cnt > compare_high {
            // If there was overflow of low count to high count in tim5,
            // or if tim5 cnt was already greater than compare_high,
            // trigger interrupt.
            Self::instance().tim2.egr.write(|w| w.cc1g().set_bit());
        }
        Self::restore(restore_state);
    }
}

impl_flow_controller!(STM32F4);

unsafe impl Sync for STM32F4 {}

// Timer TIM2 interrupt handler
global_asm!(
    ".cfi_sections .debug_frame
     .section .TIM2.user, \"ax\"
     .global TIM2
     .type TIM2,%function
     .thumb_func",
    ".cfi_startproc
    TIM2:",
    "ldr    r0,=CURRENT_THREAD_CONTEXT",
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
    // If TIM5 CNT < TIM5 CCR1 (compare register), then the timer has not yet reached the
    // 64bit compare value, and this interrupt from TIM2 can be ignored.
    "cmp    r2, r3",
    "it     lt",
    "bxlt   lr",
    "push   {{r0, lr}}",
    "bl     _private_kernel_wakeup_handler",
    "pop    {{r0, lr}}",
    "ldr    r1,=CURRENT_THREAD_CONTEXT",
    "ldr    r1, [r1]",
    "b      _switch_context",
    ".cfi_endproc
     .size TIM2, . - TIM2",
);

#[entry]
fn init() -> ! {
    unsafe {
        rtt_init_print!();

        start_kernel();
    }
}

pub type HAL = STM32F4;
