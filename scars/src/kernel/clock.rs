use crate::kernel::{
    interrupt::{
        RawInterruptHandler, in_interrupt, interrupt_context, restore_current_interrupt,
        switch_current_interrupt,
    },
    scheduler::Scheduler,
};
use crate::priority::Priority;
use crate::sync::preempt_lock::PreemptLockKey;
use core::cell::SyncUnsafeCell;
use core::ops::{Add, Mul, Sub};
use core::sync::atomic::{AtomicUsize, Ordering};
use critical_section::CriticalSection;

#[unsafe(no_mangle)]
pub(crate) unsafe fn _private_kernel_wakeup_handler() {
    static TIMER_INTERRUPT_HANDLER: SyncUnsafeCell<RawInterruptHandler> =
        SyncUnsafeCell::new(RawInterruptHandler::new(0, Priority::interrupt(0)));

    unsafe {
        interrupt_context(TIMER_INTERRUPT_HANDLER.get(), || {
            Scheduler::wakeup_scheduler_isr();
        });
    }
}
