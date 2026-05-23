use crate::priority::Priority;
use crate::sync::atomic::{AtomicUsize, Ordering};
use crate::sync::lock::preempt_lock::CorePreemptLockKey;
use crate::{
    interrupt::{
        RawInterruptHandler, in_interrupt, interrupt_context, restore_current_interrupt,
        switch_current_interrupt,
    },
    kernel::scheduler::Scheduler,
};
use core::cell::SyncUnsafeCell;
use core::ops::{Add, Mul, Sub};
use critical_section::CriticalSection;

#[unsafe(no_mangle)]
pub(crate) unsafe fn _kernel_wakeup_handler() {
    static TIMER_INTERRUPT_HANDLER: SyncUnsafeCell<RawInterruptHandler> = SyncUnsafeCell::new(
        RawInterruptHandler::new(Priority::interrupt(0), crate::kernel::hal::CoreId::DEFAULT),
    );

    unsafe {
        interrupt_context(TIMER_INTERRUPT_HANDLER.get(), || {
            Scheduler::wakeup_scheduler_isr();
        });
    }
}
