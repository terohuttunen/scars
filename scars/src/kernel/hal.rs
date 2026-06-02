use crate::priority::InterruptPriority;
use core::cell::SyncUnsafeCell;
use core::mem::MaybeUninit;
use scars_fault::FaultInfo;
use scars_khal::*;
#[cfg(feature = "khal-e310x")]
pub use scars_khal_e310x as kernel_hal;
#[cfg(feature = "khal-rp2350")]
pub use scars_khal_rp2350 as kernel_hal;
#[cfg(feature = "khal-sim")]
pub use scars_khal_sim as kernel_hal;
#[cfg(feature = "khal-stm32f0")]
pub use scars_khal_stm32f0 as kernel_hal;
#[cfg(feature = "khal-stm32f1")]
pub use scars_khal_stm32f1 as kernel_hal;
#[cfg(feature = "khal-stm32f4")]
pub use scars_khal_stm32f4 as kernel_hal;
#[cfg(feature = "khal-stm32h7")]
pub use scars_khal_stm32h7 as kernel_hal;
#[cfg(feature = "khal-test")]
pub use scars_khal_test as kernel_hal;

pub use kernel_hal::pac;

pub type Context = <kernel_hal::HAL as CoreController>::Context;
pub type HardwareFault = <kernel_hal::HAL as CoreController>::HardwareError;

#[allow(dead_code)]
pub const MAX_INTERRUPT_NUMBER: usize =
    <kernel_hal::HAL as InterruptController>::MAX_INTERRUPT_NUMBER;
#[allow(dead_code)]
pub const MAX_INTERRUPT_PRIORITY: usize =
    <kernel_hal::HAL as InterruptController>::MAX_INTERRUPT_PRIORITY;
#[allow(dead_code)]
pub(crate) const TICK_FREQ_HZ: u64 = <kernel_hal::HAL as AlarmClockController>::TICK_FREQ_HZ;

pub(crate) type StackAlignment = <kernel_hal::HAL as CoreController>::StackAlignment;

#[allow(dead_code)]
pub const NUM_CORES: usize = <kernel_hal::HAL as CoreController>::NUM_CORES;

mod core_id;
pub use core_id::CoreId;

#[cfg(all(test, feature = "khal-test"))]
mod harness_tests;

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn pend_service_call_on(core: CoreId) {
    <kernel_hal::HAL as CoreController>::pend_service_call_on(core.as_u8())
}

/// Zero-sized proof that the bearer is executing on core `CORE`.
///
/// `CoreToken` is `!Send + !Sync`, so it cannot escape its producing thread or
/// interrupt handler. Within a single execution context the kernel does not
/// migrate execution between cores, so a key obtained from `current()` is
/// valid for the rest of that context.
///
/// Lock primitives accept `&CoreToken<'_, CORE>` on their `*_core` methods to
/// skip the wrong-core check, since the key already proves we are on the
/// right core.
pub struct CoreToken<'k, const CORE: CoreId> {
    _phantom: core::marker::PhantomData<*const &'k ()>,
}

impl<const CORE: CoreId> CoreToken<'_, CORE> {
    /// Acquire a `CoreToken` proving we are on core `CORE`.
    ///
    /// Triggers [`RuntimeError::WrongCore`] if `CoreId::current() != CORE`.
    #[inline]
    pub fn current<'k>() -> CoreToken<'k, CORE> {
        if CoreId::current() != CORE {
            crate::runtime_error!(crate::kernel::RuntimeError::WrongCore);
        }
        CoreToken {
            _phantom: core::marker::PhantomData,
        }
    }

    /// Construct a `CoreToken` without checking the current core.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the calling context is executing on
    /// core `CORE`. Used at kernel callback boundaries after the dispatcher
    /// has matched `CoreId::current()`.
    #[inline(always)]
    pub unsafe fn new_unchecked<'k>() -> CoreToken<'k, CORE> {
        CoreToken {
            _phantom: core::marker::PhantomData,
        }
    }
}

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
    <kernel_hal::HAL as CoreController>::start_first_thread(idle_context)
}

#[allow(dead_code)]
#[unsafe(export_name = "exit_scars")]
pub fn exit(exit_code: i32) -> ! {
    <kernel_hal::HAL as CoreController>::on_exit(exit_code)
}

#[allow(dead_code)]
#[inline(always)]
pub fn fault(info: &FaultInfo) -> ! {
    <kernel_hal::HAL as CoreController>::on_fault(info)
}

#[allow(dead_code)]
#[inline(always)]
pub fn breakpoint() {
    <kernel_hal::HAL as CoreController>::on_breakpoint()
}

#[allow(dead_code)]
#[inline(always)]
pub fn idle() {
    <kernel_hal::HAL as CoreController>::on_idle()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    <kernel_hal::HAL as CoreController>::syscall(id, arg0, arg1, arg2)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn current_thread_context() -> *const Context {
    <kernel_hal::HAL as CoreController>::current_thread_context()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn set_current_thread_context(context: *const Context) {
    <kernel_hal::HAL as CoreController>::set_current_thread_context(context)
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn pend_service_call() {
    <kernel_hal::HAL as CoreController>::pend_service_call()
}

#[allow(dead_code)]
#[inline(always)]
pub(crate) fn clear_service_call() {
    <kernel_hal::HAL as CoreController>::clear_service_call()
}
