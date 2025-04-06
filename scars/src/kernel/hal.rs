use crate::kernel::priority::InterruptPriority;
use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use scars_khal::*;
use unrecoverable_error::UnrecoverableError;
#[cfg(feature = "khal-e310x")]
pub use scars_khal_e310x as kernel_hal;
#[cfg(feature = "khal-sim")]
pub use scars_khal_sim as kernel_hal;
#[cfg(feature = "khal-stm32f4")]
pub use scars_khal_stm32f4 as kernel_hal;

pub use kernel_hal::pac;

pub type Context = <kernel_hal::HAL as FlowController>::Context;
pub type Fault = <kernel_hal::HAL as FlowController>::HardwareError;

#[allow(dead_code)]
pub const MAX_INTERRUPT_NUMBER: usize =
    <kernel_hal::HAL as InterruptController>::MAX_INTERRUPT_NUMBER;
#[allow(dead_code)]
pub const MAX_INTERRUPT_PRIORITY: usize =
    <kernel_hal::HAL as InterruptController>::MAX_INTERRUPT_PRIORITY;
#[allow(dead_code)]
pub(crate) const TICK_FREQ_HZ: u64 = <kernel_hal::HAL as AlarmClockController>::TICK_FREQ_HZ;

pub(crate) type StackAlignment = <kernel_hal::HAL as FlowController>::StackAlignment;

pub(crate) fn init_hal() {
    unsafe {
        kernel_hal::HAL::init(kernel_hal::HAL::instance() as *const _ as *mut _);
    }
}


#[allow(dead_code)]
#[inline(always)]
pub fn clock_ticks() -> u64 {
    <kernel_hal::HAL as AlarmClockController>::clock_ticks()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn set_alarm(at: Option<u64>) {
    <kernel_hal::HAL as AlarmClockController>::set_wakeup(at)
}

#[allow(dead_code)]
#[inline(always)]
pub fn get_interrupt_priority(interrupt_number: u16) -> u8 {
    <kernel_hal::HAL as InterruptController>::get_interrupt_priority(interrupt_number)
}

#[allow(dead_code)]
#[inline(always)]
pub fn set_interrupt_priority(interrupt_number: u16, prio: InterruptPriority) -> InterruptPriority {
    <kernel_hal::HAL as InterruptController>::set_interrupt_priority(interrupt_number, prio)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn claim_interrupt() -> <kernel_hal::HAL as InterruptController>::InterruptClaim {
    <kernel_hal::HAL as InterruptController>::claim_interrupt()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn complete_interrupt(claim: <kernel_hal::HAL as InterruptController>::InterruptClaim) {
    <kernel_hal::HAL as InterruptController>::complete_interrupt(claim)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn enable_interrupt(interrupt_number: u16) {
    <kernel_hal::HAL as InterruptController>::enable_interrupt(interrupt_number)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn disable_interrupt(interrupt_number: u16) {
    <kernel_hal::HAL as InterruptController>::disable_interrupt(interrupt_number)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn get_interrupt_threshold() -> u8 {
    <kernel_hal::HAL as InterruptController>::get_interrupt_threshold()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn set_interrupt_threshold(threshold: u8) {
    <kernel_hal::HAL as InterruptController>::set_interrupt_threshold(threshold);
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn interrupt_status() -> bool {
    <kernel_hal::HAL as InterruptController>::interrupt_status()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn acquire() -> bool {
    <kernel_hal::HAL as InterruptController>::acquire()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn restore(restore_state: bool) {
    <kernel_hal::HAL as InterruptController>::restore(restore_state)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn start_first_thread(idle_context: *mut Context) -> ! {
    <kernel_hal::HAL as FlowController>::start_first_thread(idle_context)
}

#[allow(dead_code)]
#[inline(always)]
#[unsafe(export_name = "exit_scars")]
pub fn exit(exit_code: i32) -> ! {
    <kernel_hal::HAL as FlowController>::on_exit(exit_code)
}

#[allow(dead_code)]
#[inline(always)]
pub fn error(error: &dyn UnrecoverableError) -> ! {
    <kernel_hal::HAL as FlowController>::on_error(error)
}

#[allow(dead_code)]
#[inline(always)]
pub fn breakpoint() {
    <kernel_hal::HAL as FlowController>::on_breakpoint()
}

#[allow(dead_code)]
#[inline(always)]
pub fn idle() {
    <kernel_hal::HAL as FlowController>::on_idle()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    <kernel_hal::HAL as FlowController>::syscall(id, arg0, arg1, arg2)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn current_thread_context() -> *const Context {
    <kernel_hal::HAL as FlowController>::current_thread_context()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn set_current_thread_context(context: *const Context) {
    <kernel_hal::HAL as FlowController>::set_current_thread_context(context)
}
