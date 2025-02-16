#![no_std]
use const_env::from_env;
use core::arch::global_asm;
use riscv::register::mstatus;
use scars_arch_riscv::RISCV32;
use scars_khal::*;
#[cfg(feature = "semihosting")]
pub use semihosting::{print as printk, println as printkln};

global_asm!(include_str!("start.S"));

// Re-export the PAC at crate root
pub use e310x as pac;

pub struct E310x {
    clint: e310x::CLINT,
    plic: e310x::PLIC,
}

impl HardwareAbstractionLayer for E310x {
    const NAME: &'static str = "e310x (RISCV32)";

    unsafe fn init(hal: *mut Self) {
        unsafe {
            let e310x::Peripherals { PLIC, CLINT, .. } = e310x::Peripherals::steal();
            *hal = E310x {
                clint: CLINT,
                plic: PLIC,
            };

            // Enable external interrupts in PLIC
            riscv::register::mie::set_mext();
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
    fn get_interrupt_priority(&self, interrupt_number: u16) -> u8 {
        self.plic.priority[interrupt_number as usize].read().bits() as u8
    }

    #[inline(always)]
    fn set_interrupt_priority(&self, interrupt_number: u16, prio: u8) -> u8 {
        let restore_state = self.acquire();
        let previous_priority = self.plic.priority[interrupt_number as usize].read().bits() as u8;
        self.plic.priority[interrupt_number as usize].write(|w| unsafe { w.bits(prio as u32) });
        self.restore(restore_state);
        previous_priority
    }

    #[inline(always)]
    fn enable_interrupts(&self) {
        unsafe { riscv::register::mstatus::set_mie() }
    }

    #[inline(always)]
    fn disable_interrupts(&self) {
        unsafe { riscv::register::mstatus::clear_mie() }
    }

    #[inline(always)]
    fn get_interrupt_threshold(&self) -> u8 {
        self.plic.threshold.read().bits() as u8
    }

    #[inline(always)]
    fn set_interrupt_threshold(&self, threshold: u8) {
        self.plic
            .threshold
            .write(|w| unsafe { w.bits(threshold as u32) });
    }

    fn claim_interrupt(&self) -> Self::InterruptClaim {
        let interrupt_number = self.plic.claim.read().bits() as u16;
        let restore_threshold = self.get_interrupt_threshold();
        let interrupt_prio = self.get_interrupt_priority(interrupt_number as u16);
        self.set_interrupt_threshold(interrupt_prio);
        self.enable_interrupts();
        InterruptClaim {
            interrupt_number,
            restore_threshold,
        }
    }

    fn complete_interrupt(&self, claim: Self::InterruptClaim) {
        self.disable_interrupts();
        self.set_interrupt_threshold(claim.restore_threshold);
        self.plic
            .claim
            .write(|w| unsafe { w.bits(claim.interrupt_number as u32) })
    }

    fn enable_interrupt(&self, interrupt_number: u16) {
        let restore_state = self.acquire();
        let enable_index = interrupt_number as usize / 32;
        let mut bits = self.plic.enable[enable_index].read().bits();
        bits |= 1 << (interrupt_number % 32);
        self.plic.enable[enable_index].write(|w| unsafe { w.bits(bits) });
        self.restore(restore_state);
    }

    fn disable_interrupt(&self, interrupt_number: u16) {
        let restore_state = self.acquire();
        let enable_index = interrupt_number as usize / 32;
        let mut bits = self.plic.enable[enable_index].read().bits();
        bits &= !(1 << (interrupt_number % 32));
        self.plic.enable[enable_index].write(|w| unsafe { w.bits(bits) });
        self.restore(restore_state)
    }

    #[inline(always)]
    fn interrupt_status(&self) -> bool {
        let mstatus = riscv::register::mstatus::read();
        mstatus.mie()
    }

    #[inline(always)]
    fn acquire(&self) -> bool {
        let mut mstatus: usize;
        unsafe {
            core::arch::asm!("csrrci {}, mstatus, 0b1000", out(reg) mstatus);
            core::mem::transmute::<_, mstatus::Mstatus>(mstatus).mie()
        }
    }

    #[inline(always)]
    fn restore(&self, restore_state: bool) {
        // Only re-enable interrupts if they were enabled before the critical section.
        if restore_state {
            unsafe { riscv::register::mstatus::set_mie() }
        }
    }
}

#[unsafe(no_mangle)]
fn _save_interrupt_threshold(context: &mut <E310x as FlowController>::Context) {
    let plic = unsafe { &*e310x::PLIC::ptr() };
    let threshold = plic.threshold.read().bits();
    context.interrupt_threshold = threshold as usize;
}

#[unsafe(no_mangle)]
fn _restore_interrupt_threshold(context: &mut <E310x as FlowController>::Context) {
    let plic = unsafe { &mut *(e310x::PLIC::ptr() as *mut e310x::plic::RegisterBlock) };
    plic.threshold
        .write(|w| unsafe { w.bits(context.interrupt_threshold as u32) });
}

impl AlarmClockController for E310x {
    const TICK_FREQ_HZ: u64 = TIMER_FREQ_HZ;

    #[inline(always)]
    fn clock_ticks(&self) -> u64 {
        let mut mtimeh;
        let mut mtime;
        loop {
            mtimeh = self.clint.mtimeh.read().bits();
            mtime = self.clint.mtime.read().bits();

            // Re-read high-word to detect overflow in mtime
            if mtimeh == self.clint.mtimeh.read().bits() {
                // No overflow, time read successfully
                break;
            }

            // There was an overflow of mtime after mtimeh was read the first time.
            // Therefore mtime and mtimeh must be re-read.
        }

        mtime as u64 + ((mtimeh as u64) << 32)
    }

    #[inline(always)]
    fn set_wakeup(&self, at: u64) {
        let restore_state = self.acquire();
        // First set high-word to maximum value to prevent triggering the timer
        // with old high-word and new low-word.
        self.clint.mtimecmph.write(|w| unsafe { w.bits(u32::MAX) });
        // Set new low-word
        self.clint.mtimecmp.write(|w| unsafe { w.bits(at as u32) });
        // Set new high-word
        self.clint
            .mtimecmph
            .write(|w| unsafe { w.bits((at >> 32) as u32) });
        self.restore(restore_state);
    }

    #[inline(always)]
    fn enable_wakeup(&self) {
        unsafe {
            riscv::register::mie::set_mtimer();
        }
    }

    #[inline(always)]
    fn disable_wakeup(&self) {
        unsafe {
            riscv::register::mie::clear_mtimer();
        }
    }
}

impl FlowController for E310x {
    type Context = <RISCV32 as FlowController>::Context;
    type Fault = <RISCV32 as FlowController>::Fault;
    type StackAlignment = <RISCV32 as FlowController>::StackAlignment;

    fn start_first_thread(idle_context: *mut Self::Context) -> ! {
        RISCV32::start_first_thread(idle_context)
    }

    fn abort() -> ! {
        RISCV32::abort()
    }

    fn breakpoint() {
        RISCV32::breakpoint();
    }

    fn idle() {
        RISCV32::idle();
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
}

unsafe impl Sync for E310x {}

#[unsafe(no_mangle)]
pub fn init() {
    unsafe {
        start_kernel();
    }
}

pub type HAL = E310x;
