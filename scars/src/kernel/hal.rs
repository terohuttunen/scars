use crate::kernel::priority::InterruptPriority;
use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use scars_khal::*;

#[cfg(feature = "khal-e310x")]
pub(crate) use scars_khal_e310x as kernel_hal;
#[cfg(feature = "khal-sim")]
pub(crate) use scars_khal_sim as kernel_hal;
#[cfg(feature = "khal-stm32f4")]
pub(crate) use scars_khal_stm32f4 as kernel_hal;

pub use kernel_hal::pac;

pub type Context = <kernel_hal::HAL as FlowController>::Context;
pub type Fault = <kernel_hal::HAL as FlowController>::Fault;

pub const MAX_INTERRUPT_NUMBER: usize =
    <kernel_hal::HAL as InterruptController>::MAX_INTERRUPT_NUMBER;
pub const MAX_INTERRUPT_PRIORITY: usize =
    <kernel_hal::HAL as InterruptController>::MAX_INTERRUPT_PRIORITY;
pub(crate) const TICK_FREQ_HZ: u64 = <kernel_hal::HAL as AlarmClockController>::TICK_FREQ_HZ;

pub struct Hal {
    hal: SyncUnsafeCell<MaybeUninit<kernel_hal::HAL>>,
}

impl Hal {
    fn get(&self) -> *mut MaybeUninit<kernel_hal::HAL> {
        self.hal.get()
    }

    fn instance() -> &'static kernel_hal::HAL {
        unsafe { (&*HAL.get()).assume_init_ref() }
    }
}

static HAL: Hal = Hal {
    hal: SyncUnsafeCell::new(MaybeUninit::uninit()),
};

pub(crate) fn init_hal(hal: kernel_hal::HAL) {
    unsafe {
        (&mut *HAL.get()).write(hal);
    }
}

#[allow(dead_code)]
#[inline(always)]
pub fn clock_ticks() -> u64 {
    Hal::instance().clock_ticks()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn set_alarm(at: u64) {
    Hal::instance().set_wakeup(at)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn enable_alarm_interrupt() {
    Hal::instance().enable_wakeup();
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn disable_alarm_interrupt() {
    Hal::instance().disable_wakeup();
}

#[allow(dead_code)]
#[inline(always)]
pub fn get_interrupt_priority(interrupt_number: u16) -> u8 {
    Hal::instance().get_interrupt_priority(interrupt_number)
}

#[allow(dead_code)]
#[inline(always)]
pub fn set_interrupt_priority(interrupt_number: u16, prio: InterruptPriority) -> InterruptPriority {
    Hal::instance().set_interrupt_priority(interrupt_number, prio)
}

#[allow(dead_code)]
#[inline(always)]
pub fn enable_interrupts() {
    Hal::instance().enable_interrupts();
}

#[allow(dead_code)]
#[inline(always)]
pub fn disable_interrupts() {
    Hal::instance().disable_interrupts();
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn claim_interrupt() -> <kernel_hal::HAL as InterruptController>::InterruptClaim {
    Hal::instance().claim_interrupt()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn complete_interrupt(claim: <kernel_hal::HAL as InterruptController>::InterruptClaim) {
    Hal::instance().complete_interrupt(claim)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn enable_interrupt(interrupt_number: u16) {
    Hal::instance().enable_interrupt(interrupt_number)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn disable_interrupt(interrupt_number: u16) {
    Hal::instance().disable_interrupt(interrupt_number)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn get_interrupt_threshold() -> u8 {
    Hal::instance().get_interrupt_threshold()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn set_interrupt_threshold(threshold: u8) {
    Hal::instance().set_interrupt_threshold(threshold);
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn interrupt_status() -> bool {
    Hal::instance().interrupt_status()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn acquire() -> bool {
    Hal::instance().acquire()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn restore(restore_state: bool) {
    Hal::instance().restore(restore_state)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn start_first_task(idle_context: *mut Context) -> ! {
    <kernel_hal::HAL as FlowController>::start_first_task(idle_context)
}

#[allow(dead_code)]
#[inline(always)]
pub fn abort() -> ! {
    <kernel_hal::HAL as FlowController>::abort()
}

#[allow(dead_code)]
#[inline(always)]
pub fn breakpoint() {
    <kernel_hal::HAL as FlowController>::breakpoint()
}

#[allow(dead_code)]
#[inline(always)]
pub fn idle() {
    <kernel_hal::HAL as FlowController>::idle()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn syscall(id: usize, arg0: usize, arg1: usize) -> usize {
    <kernel_hal::HAL as FlowController>::syscall(id, arg0, arg1)
}
