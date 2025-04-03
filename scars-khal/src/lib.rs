#![no_std]
pub mod callbacks;
pub use aligned::*;
pub use callbacks::KernelCallbacks;
pub use unrecoverable_error::*;

unsafe extern "Rust" {
    pub unsafe fn start_kernel() -> !;
}

pub trait GetInterruptNumber {
    fn get_interrupt_number(&self) -> u16;
}

pub trait InterruptController {
    const MAX_INTERRUPT_PRIORITY: usize;
    const MAX_INTERRUPT_NUMBER: usize;
    type InterruptClaim: GetInterruptNumber;

    fn get_interrupt_priority(&self, interrupt_number: u16) -> u8;

    fn set_interrupt_priority(&self, interrupt_number: u16, prio: u8) -> u8;

    fn enable_interrupts(&self);

    fn disable_interrupts(&self);

    fn claim_interrupt(&self) -> Self::InterruptClaim;

    fn complete_interrupt(&self, claim: Self::InterruptClaim);

    fn enable_interrupt(&self, interrupt_number: u16);

    fn disable_interrupt(&self, interrupt_number: u16);

    fn get_interrupt_threshold(&self) -> u8;

    fn set_interrupt_threshold(&self, threshold: u8);

    fn interrupt_status(&self) -> bool;

    fn acquire(&self) -> bool;

    fn restore(&self, restore_state: bool);
}

pub type Ticks = u64;

pub trait AlarmClockController {
    /// Timer frequency as Ticks per second
    const TICK_FREQ_HZ: Ticks;

    /// Monotonously growing tick counter since some earlier epoch
    fn clock_ticks(&self) -> Ticks;

    fn set_wakeup(&self, at: Ticks);

    fn enable_wakeup(&self);

    fn disable_wakeup(&self);
}

/*
pub trait FaultInfo<Context> {
    fn code(&self) -> usize;

    fn name(&self) -> &'static str;

    fn address(&self) -> usize;

    fn context(&self) -> &Context;
}
    */

pub trait ContextInfo {
    fn stack_top_ptr(&self) -> *const u8;

    unsafe fn init(
        name: &'static str,
        main_fn: *const (),
        argument: Option<*const u8>,
        stack_ptr: *const u8,
        stack_size: usize,
        context: *mut Self,
    );
}

pub trait FlowController {
    type StackAlignment: Alignment;
    type Context: ContextInfo;
    type Fault: UnrecoverableError;

    fn start_first_thread(idle_context: *mut Self::Context) -> !;

    fn abort() -> !;

    fn breakpoint();

    fn idle();

    fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize;

    fn current_thread_context() -> *const Self::Context;

    fn set_current_thread_context(context: *const Self::Context);
}

pub trait HardwareAbstractionLayer:
    AlarmClockController + InterruptController + FlowController + Sync
{
    const NAME: &'static str;

    unsafe fn init(hal: *mut Self)
    where
        Self: Sized;
}
