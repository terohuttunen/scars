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
use scars_arch_cortex_m::{impl_core_controller, init_pendsv_priority, nvic};
use scars_khal::*;

pub use stm32f1::*;

#[cfg(feature = "stm32f103")]
pub use stm32f1::stm32f103 as pac;

pub use pac::{Interrupt, Peripherals};

const PRIO_BITS: u8 = pac::NVIC_PRIO_BITS;

static HAL: SyncUnsafeCell<MaybeUninit<STM32F1>> = SyncUnsafeCell::new(MaybeUninit::uninit());

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
    /// APB1 timer clock: pclk1 * 2 when APB1 prescaler != 1, else pclk1.
    pub pclk1_timer: u32,
}

/// Software-extended upper bits of the 64-bit monotonic counter.
/// Incremented in the TIM2 update ISR on each 16-bit wrap; combined
/// with TIM2.CNT to form the 64-bit tick value.
static OVERFLOW_HIGH: AtomicU64 = AtomicU64::new(0);

/// 64-bit compare target requested via `set_wakeup`. The TIM2 ISR
/// drives the hardware CC1 register to land at this value across
/// overflow boundaries.
static WAKEUP_TARGET: AtomicU64 = AtomicU64::new(u64::MAX);

pub struct STM32F1 {
    _frequencies: ClockFrequencies,
}

/// HSI (8 MHz, internal RC) → /2 → PLL ×16 → 64 MHz sysclk.
/// AHB /1, APB1 /2 → PCLK1 = 32 MHz, APB1 timer clock = 64 MHz.
///
/// HSI is used in preference to HSE so the KHAL works on Nucleo F1
/// boards regardless of the ST-LINK-MCO-to-HSE solder bridge config
/// (SB17/SB37 etc.). Apps wanting HSE-sourced clocks should replace
/// this function.
fn configure_default_clocks() -> ClockFrequencies {
    let rcc = unsafe { &*pac::RCC::ptr() };
    let flash = unsafe { &*pac::FLASH::ptr() };

    // Two flash wait states for 48..72 MHz, enable prefetch.
    flash
        .acr()
        .modify(|_, w| unsafe { w.latency().bits(0b010).prftbe().set_bit() });

    // HSI is on after reset.
    rcc.cr().modify(|_, w| w.hsion().set_bit());
    while rcc.cr().read().hsirdy().bit_is_clear() {}

    // PLL: PLLSRC=0 selects HSI/2 = 4 MHz; PLLMUL ×16 = 64 MHz sysclk.
    rcc.cfgr().modify(|_, w| unsafe {
        w.pllsrc().clear_bit();
        w.pllmul().bits(0b1110);
        w.hpre().bits(0b0000);
        w.ppre1().bits(0b100);
        w.ppre2().bits(0b000)
    });

    rcc.cr().modify(|_, w| w.pllon().set_bit());
    while rcc.cr().read().pllrdy().bit_is_clear() {}

    rcc.cfgr().modify(|_, w| unsafe { w.sw().bits(0b10) });
    while rcc.cfgr().read().sws().bits() != 0b10 {}

    ClockFrequencies {
        sysclk: 64_000_000,
        hclk: 64_000_000,
        pclk1: 32_000_000,
        pclk1_timer: 64_000_000,
    }
}

impl STM32F1 {
    /// Configure TIM2 as a 16-bit up-counter wrapping at 0xFFFF,
    /// prescaled to a 1 MHz tick. Software extends the upper 48 bits
    /// via the update-interrupt path in the TIM2 ISR.
    fn setup_clock(&mut self) {
        let tim2 = unsafe { &*pac::TIM2::ptr() };

        let psc = (self._frequencies.pclk1_timer / TIMER_FREQ_HZ as u32) - 1;
        tim2.psc().write(|w| w.psc().set(psc as u16));

        // ARR default is 0xFFFF on F1's 16-bit timers.
        tim2.arr().write(|w| w.arr().set(0xFFFF));

        // Reset count, push prescaler.
        tim2.egr().write(|w| w.ug().set_bit());

        // Enable update + compare-1 interrupts. Compare arms on demand
        // from set_wakeup; update fires on every 16-bit wrap.
        tim2.dier()
            .modify(|_, w| w.uie().set_bit().cc1ie().set_bit());

        // Start counting.
        tim2.cr1().modify(|_r, w| w.cen().set_bit());
    }
}

impl HardwareAbstractionLayer for STM32F1 {
    const NAME: &'static str = "STM32F1";

    fn instance() -> &'static Self {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }

    unsafe fn init(hal: *mut Self) {
        let cortex_m::Peripherals { mut SCB, .. } = unsafe { cortex_m::Peripherals::steal() };

        let frequencies = configure_default_clocks();

        let rcc = unsafe { &*pac::RCC::ptr() };
        rcc.apb1enr().modify(|_, w| w.tim2en().set_bit());

        nvic::enable(pac::Interrupt::TIM2 as u16);

        init_pendsv_priority(&mut SCB);

        unsafe {
            *hal = STM32F1 {
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

impl InterruptController for STM32F1 {
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

// STM32F103xx interrupt count fits under 60. Size the table at 64 to
// cover the family with a small safety margin.
const MAX_INTERRUPT_NUMBER: usize = 64;

/// Read 64-bit monotonic tick, handling the OVERFLOW_HIGH/TIM2.CNT
/// race the same way the F4 path handles the TIM5/TIM2 chain: re-read
/// the upper word after the lower and discard the sample if a wrap
/// snuck in between.
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

/// Program TIM2 CC1 with the low 16 bits of `target` if it lies in
/// the current overflow window; otherwise leave CC1 set to wait for a
/// later wrap and let the update-IRQ path re-arm it.
fn arm_compare(target: u64) {
    let tim2 = unsafe { &*pac::TIM2::ptr() };
    let now_high = OVERFLOW_HIGH.load(Ordering::Acquire);
    if (target >> 16) == now_high {
        tim2.ccr1().write(|w| w.ccr().set((target & 0xFFFF) as u16));
    } else {
        // Sentinel that won't match in this window; we'll re-evaluate
        // on the next update IRQ.
        tim2.ccr1().write(|w| w.ccr().set(0));
    }
}

impl AlarmClockController for STM32F1 {
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
                // If we missed the boat (now already past target), pend
                // CC1 manually so the kernel timer fires immediately.
                if read_ticks() >= target {
                    tim2.egr().write(|w| w.cc1g().set_bit());
                }
            }
        }
        Self::restore(restore_state);
    }
}

// F1 uses the active-idle hook instead of the default `wfi` so that
// probe-rs's SWD-driven RTT polling can observe kernel-timer output
// between thread ticks. See `scars_arch_cortex_m::on_idle_active`.
impl_core_controller!(STM32F1, on_idle = scars_arch_cortex_m::on_idle_active());

unsafe impl Sync for STM32F1 {}

unsafe extern "Rust" {
    /// Kernel callback fired when the monotonic clock reaches the
    /// last `set_wakeup` target. Re-armed on each call.
    fn _kernel_wakeup_handler();
}

/// Rust half of the TIM2 IRQ. Handles UIF (overflow → bump
/// `OVERFLOW_HIGH` and re-arm CC1 if the target lies in the new
/// window) and CC1IF (target reached → kernel wakeup).
///
/// The naked-ASM wrapper below loads `CURRENT_THREAD_CONTEXT` before
/// and after this returns and tail-jumps to `_switch_context`, so any
/// thread the wakeup made runnable can take over immediately.
#[unsafe(no_mangle)]
extern "C" fn _scars_stm32f1_tim2_irq() {
    let tim2 = unsafe { &*pac::TIM2::ptr() };
    let sr = tim2.sr().read();

    if sr.uif().bit_is_set() {
        // Clear UIF before incrementing — if a new UIF fires while we
        // process this one, NVIC will re-pend us, and we'll re-enter.
        tim2.sr().modify(|_, w| w.uif().clear_bit());
        OVERFLOW_HIGH.fetch_add(1, Ordering::AcqRel);
        // Re-arm compare if target now sits in the (new) current
        // overflow window.
        let target = WAKEUP_TARGET.load(Ordering::Acquire);
        arm_compare(target);
    }

    if sr.cc1if().bit_is_set() {
        tim2.sr().modify(|_, w| w.cc1if().clear_bit());
        // Defensive check: only fire the kernel wakeup if the 64-bit
        // counter actually reached the target. Compare hits inside
        // earlier overflow windows would be spurious.
        let now = read_ticks();
        let target = WAKEUP_TARGET.load(Ordering::Acquire);
        if now >= target {
            WAKEUP_TARGET.store(u64::MAX, Ordering::Release);
            unsafe { _kernel_wakeup_handler() };
        }
    }
}

/// TIM2 IRQ entry — matches the F4 KHAL's pattern. Loads the
/// outgoing thread context, dispatches to the Rust handler, then
/// reads the incoming context and tail-jumps into `_switch_context`
/// so any newly-ready higher-priority thread runs without bouncing
/// through PendSV.
#[unsafe(naked)]
#[unsafe(export_name = "TIM2")]
#[unsafe(link_section = ".TIM2.user")]
pub unsafe extern "C" fn tim2() {
    core::arch::naked_asm!(
        // Inline the slot load instead of bl-ing into the macro-
        // generated getter. The slot is the `CURRENT_THREAD_CONTEXT`
        // static emitted by `impl_core_controller!` in this crate;
        // `sym` resolves it to its linker name. r4 caches the old
        // Context pointer across the handler (callee-saved by AAPCS);
        // r5/r6 pad the push to a 4-register / 16-byte aligned shape.
        "push   {{r4, r5, r6, lr}}",
        "ldr    r4, ={slot}",
        "ldr    r4, [r4]",
        "bl     _scars_stm32f1_tim2_irq",
        "ldr    r0, ={slot}",
        "ldr    r1, [r0]",
        "mov    r0, r4",
        "pop    {{r4, r5, r6, lr}}",
        "b      _switch_context",
        slot = sym crate::CURRENT_THREAD_CONTEXT,
    );
}

#[entry]
fn init() -> ! {
    unsafe {
        start_kernel();
    }
}

pub type HAL = STM32F1;
