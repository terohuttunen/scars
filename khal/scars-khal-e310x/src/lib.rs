#![no_std]
#![feature(sync_unsafe_cell)]
use const_env::from_env;
use core::arch::global_asm;
use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use defmt_semihosting as _;
use riscv::register::mstatus;
use scars_arch_riscv::RISCV32;
use scars_khal::*;
#[cfg(feature = "semihosting")]
pub use semihosting::{print as printk, println as printkln};

global_asm!(include_str!("start.S"));

// Re-export the PAC at crate root
pub use e310x as pac;

// Static HAL instance
static HAL: SyncUnsafeCell<MaybeUninit<E310x>> = SyncUnsafeCell::new(MaybeUninit::uninit());

pub struct E310x {
    clint: e310x::CLINT,
    plic: e310x::PLIC,
}

impl HardwareAbstractionLayer for E310x {
    const NAME: &'static str = "e310x (RISCV32)";

    fn instance() -> &'static Self {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }

    unsafe fn init(hal: *mut Self) {
        unsafe {
            let e310x::Peripherals { PLIC, CLINT, .. } = e310x::Peripherals::steal();
            *hal = E310x {
                clint: CLINT,
                plic: PLIC,
            };

            // Enable external interrupts in PLIC
            riscv::register::mie::set_mext();
            // Enable the machine timer interrupt for alarm wakeups
            riscv::register::mie::set_mtimer();
        }
    }
}

#[from_env]
const MTIME_FREQ_HZ: u64 = 10_000_000;
pub const TIMER_FREQ_HZ: u64 = MTIME_FREQ_HZ;

pub struct InterruptClaim {
    interrupt_number: u16,
    restore_threshold: u8,
}

impl GetInterruptNumber for InterruptClaim {
    fn get_interrupt_number(&self) -> u16 {
        self.interrupt_number
    }
}

impl InterruptController for E310x {
    const MAX_INTERRUPT_PRIORITY: usize = 7;
    const MAX_INTERRUPT_NUMBER: usize = 52;
    type InterruptClaim = InterruptClaim;

    #[inline(always)]
    fn get_interrupt_priority(interrupt_number: u16) -> u8 {
        Self::instance().plic.priority[interrupt_number as usize]
            .read()
            .bits() as u8
    }

    #[inline(always)]
    fn set_interrupt_priority(interrupt_number: u16, prio: u8) -> u8 {
        let restore_state = Self::acquire();
        let previous_priority = Self::instance().plic.priority[interrupt_number as usize]
            .read()
            .bits() as u8;
        Self::instance().plic.priority[interrupt_number as usize]
            .write(|w| unsafe { w.bits(prio as u32) });
        Self::restore(restore_state);
        previous_priority
    }

    /// The scars-khal threshold convention reserves
    /// `MAX_INTERRUPT_PRIORITY` as the "no masking" sentinel (matches
    /// ARM `BASEPRI = 0`). PLIC, however, masks priorities `<=
    /// PLIC.threshold` directly — `PLIC.threshold = MAX` would mask
    /// everything. Convert at the boundary: scars `MAX` maps to PLIC
    /// `0` (no masking, because no real IRQ is at priority 0 — PLIC
    /// reserves priority 0 for "disabled"); other values pass
    /// through unchanged.
    #[inline(always)]
    fn get_interrupt_threshold() -> u8 {
        let plic = Self::instance().plic.threshold.read().bits() as u8;
        if plic == 0 {
            Self::MAX_INTERRUPT_PRIORITY as u8
        } else {
            plic
        }
    }

    #[inline(always)]
    fn set_interrupt_threshold(threshold: u8) {
        let plic_threshold = if threshold as usize >= Self::MAX_INTERRUPT_PRIORITY {
            0
        } else {
            threshold
        };
        Self::instance()
            .plic
            .threshold
            .write(|w| unsafe { w.bits(plic_threshold as u32) });
    }

    fn claim_interrupt() -> Self::InterruptClaim {
        let interrupt_number = Self::instance().plic.claim.read().bits() as u16;
        let restore_threshold = Self::get_interrupt_threshold();
        let interrupt_prio = Self::get_interrupt_priority(interrupt_number as u16);
        Self::set_interrupt_threshold(interrupt_prio);
        Self::restore(true);
        InterruptClaim {
            interrupt_number,
            restore_threshold,
        }
    }

    fn complete_interrupt(claim: Self::InterruptClaim) {
        // `claim_interrupt` enabled nesting with `restore(true)`.
        // Do the opposite here and disable interrupts again for
        // trap exit.
        Self::acquire();
        Self::set_interrupt_threshold(claim.restore_threshold);
        Self::instance()
            .plic
            .claim
            .write(|w| unsafe { w.bits(claim.interrupt_number as u32) })
    }

    fn enable_interrupt(interrupt_number: u16) {
        let restore_state = Self::acquire();
        let enable_index = interrupt_number as usize / 32;
        let mut bits = Self::instance().plic.enable[enable_index].read().bits();
        bits |= 1 << (interrupt_number % 32);
        Self::instance().plic.enable[enable_index].write(|w| unsafe { w.bits(bits) });
        Self::restore(restore_state);
    }

    fn disable_interrupt(interrupt_number: u16) {
        let restore_state = Self::acquire();
        let enable_index = interrupt_number as usize / 32;
        let mut bits = Self::instance().plic.enable[enable_index].read().bits();
        bits &= !(1 << (interrupt_number % 32));
        Self::instance().plic.enable[enable_index].write(|w| unsafe { w.bits(bits) });
        Self::restore(restore_state)
    }

    #[inline(always)]
    fn interrupt_status() -> bool {
        let mstatus = riscv::register::mstatus::read();
        mstatus.mie()
    }

    #[inline(always)]
    fn acquire() -> bool {
        let mut mstatus: usize;
        unsafe {
            core::arch::asm!("csrrci {}, mstatus, 0b1000", out(reg) mstatus);
            core::mem::transmute::<_, mstatus::Mstatus>(mstatus).mie()
        }
    }

    #[inline(always)]
    fn restore(restore_state: bool) {
        // Only re-enable interrupts if they were enabled before the critical section.
        if restore_state {
            unsafe { riscv::register::mstatus::set_mie() }
        }
    }
}

impl AlarmClockController for E310x {
    const TICK_FREQ_HZ: u64 = TIMER_FREQ_HZ;

    #[inline(always)]
    fn clock_ticks() -> u64 {
        let mut mtimeh;
        let mut mtime;
        loop {
            mtimeh = Self::instance().clint.mtimeh.read().bits();
            mtime = Self::instance().clint.mtime.read().bits();

            // Re-read high-word to detect overflow in mtime
            if mtimeh == Self::instance().clint.mtimeh.read().bits() {
                // No overflow, time read successfully
                break;
            }

            // There was an overflow of mtime after mtimeh was read the first time.
            // Therefore mtime and mtimeh must be re-read.
        }

        mtime as u64 + ((mtimeh as u64) << 32)
    }

    #[inline(always)]
    fn set_wakeup(at: Option<u64>) {
        let at = at.unwrap_or(u64::MAX);
        let restore_state = Self::acquire();
        // First set high-word to maximum value to prevent triggering the timer
        // with old high-word and new low-word.
        Self::instance()
            .clint
            .mtimecmph
            .write(|w| unsafe { w.bits(u32::MAX) });
        // Set new low-word
        Self::instance()
            .clint
            .mtimecmp
            .write(|w| unsafe { w.bits(at as u32) });
        // Set new high-word
        Self::instance()
            .clint
            .mtimecmph
            .write(|w| unsafe { w.bits((at >> 32) as u32) });
        Self::restore(restore_state);
    }
}

impl CoreController for E310x {
    type Context = <RISCV32 as CoreController>::Context;
    type HardwareError = <RISCV32 as CoreController>::HardwareError;
    type StackAlignment = <RISCV32 as CoreController>::StackAlignment;

    const NUM_CORES: usize = 1;

    #[inline(always)]
    fn current_core_id() -> u8 {
        0
    }

    #[inline(always)]
    fn pend_service_call_on(_core: u8) {
        <Self as CoreController>::pend_service_call()
    }

    fn start_first_thread(idle_context: *mut Self::Context) -> ! {
        RISCV32::start_first_thread(idle_context)
    }

    fn on_abort() -> ! {
        RISCV32::on_abort()
    }

    fn on_exit(exit_code: i32) -> ! {
        RISCV32::on_exit(exit_code)
    }

    fn on_fault(info: &::scars_khal::FaultInfo) -> ! {
        RISCV32::on_fault(info)
    }

    fn on_breakpoint() {
        RISCV32::on_breakpoint();
    }

    fn on_idle() {
        RISCV32::on_idle();
    }

    fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        RISCV32::syscall(id, arg0, arg1, arg2)
    }

    fn current_thread_context() -> *const Self::Context {
        RISCV32::current_thread_context()
    }

    fn set_current_thread_context(context: *const Self::Context) {
        RISCV32::set_current_thread_context(context)
    }

    fn pend_service_call() {
        // Use E310x CLINT for machine software interrupt
        unsafe {
            Self::instance().clint.msip.write(|w| w.bits(1));
        }
    }

    fn clear_service_call() {
        // Clear E310x CLINT machine software interrupt
        unsafe {
            Self::instance().clint.msip.write(|w| w.bits(0));
        }
    }
}

unsafe impl Sync for E310x {}

#[unsafe(no_mangle)]
pub fn init() {
    unsafe {
        riscv::register::mie::set_msoft();
        start_kernel();
    }
}

pub type HAL = E310x;
