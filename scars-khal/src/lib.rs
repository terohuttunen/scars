#![no_std]
pub mod callbacks;
pub use aligned::*;
pub use callbacks::KernelCallbacks;
pub use unrecoverable_error::UnrecoverableError;

unsafe extern "Rust" {
    pub unsafe fn start_kernel() -> !;
}

pub trait GetInterruptNumber {
    fn get_interrupt_number(&self) -> u16;
}

pub trait InterruptController: Sync {
    const MAX_INTERRUPT_PRIORITY: usize;
    const MAX_INTERRUPT_NUMBER: usize;
    type InterruptClaim: GetInterruptNumber;

    fn get_interrupt_priority(&self, interrupt_number: u16) -> u8;

    fn set_interrupt_priority(&self, interrupt_number: u16, prio: u8) -> u8;

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

pub trait AlarmClockController: Sync {
    /// Timer frequency as Ticks per second
    const TICK_FREQ_HZ: Ticks;

    /// Monotonously growing tick counter since some earlier epoch
    fn clock_ticks(&self) -> Ticks;

    /// Set the wakeup time for the alarm clock.
    ///
    /// If `at` is `None`, the wakeup is disabled.
    fn set_wakeup(&self, at: Option<Ticks>);
}

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

pub trait FlowController: Sync {
    type StackAlignment: Alignment;
    type Context: ContextInfo;
    type HardwareError: UnrecoverableError;

    fn start_first_thread(idle_context: *mut Self::Context) -> !;

    fn on_abort() -> !;

    fn on_exit(exit_code: i32) -> !;

    fn on_error(error: &dyn UnrecoverableError) -> !;

    fn on_breakpoint();

    fn on_idle();

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
