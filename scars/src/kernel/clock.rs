use crate::kernel::{
    hal::{disable_alarm_interrupt, enable_alarm_interrupt},
    interrupt::{
        in_interrupt, interrupt_context, restore_current_interrupt, switch_current_interrupt,
        InterruptControlBlock,
    },
    scheduler::Scheduler,
};
use crate::sync::preempt_lock::PreemptLockKey;
use core::cell::SyncUnsafeCell;
use core::ops::{Add, Mul, Sub};
use core::sync::atomic::{AtomicUsize, Ordering};
use critical_section::CriticalSection;

#[no_mangle]
pub(crate) unsafe fn _private_kernel_wakeup_handler() {
    static TIMER_INTERRUPT_HANDLER: SyncUnsafeCell<InterruptControlBlock> =
        SyncUnsafeCell::new(InterruptControlBlock::new(0, 0));

    // Prevent nested wakeup interrupts. Only one wakeup interrupt
    // should be ongoing at any given time in order to not overflow the
    // ISR stack.
    //disable_alarm_interrupt();

    interrupt_context(TIMER_INTERRUPT_HANDLER.get(), || {
        Scheduler::wakeup_scheduler_isr();
    });

    // Enable wakeups only after exiting interrupt context, so that sections
    // that enable interrupts are executed without triggering another wakeup.
    //enable_alarm_interrupt();
}
