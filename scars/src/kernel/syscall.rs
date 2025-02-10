use crate::kernel::{
    RuntimeError,
    hal::{clock_ticks, disable_interrupts, enable_interrupts, syscall},
    interrupt::{
        CriticalSection, RawInterruptHandler, in_interrupt, interrupt_context,
        restore_current_interrupt, switch_current_interrupt,
    },
    list::LinkedList,
    priority::{AnyPriority, Priority},
    scheduler::Scheduler,
    waiter::{Suspendable, WaitQueueTag},
};
use crate::sync::{InterruptLock, interrupt_lock::InterruptLockKey};
use crate::thread::RawThread;
use crate::time::{Duration, Instant};
use core::cell::SyncUnsafeCell;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use scars_khal::FlowController;

pub const SYSCALL_ID_YIELD: usize = 1;
pub const SYSCALL_ID_WAIT: usize = 2;
pub const SYSCALL_ID_WAIT_EVENT: usize = 3;
pub const SYSCALL_ID_WAIT_EVENT_UNTIL: usize = 4;
pub const SYSCALL_ID_DELAY_UNTIL: usize = 5;
pub const SYSCALL_ID_RUNTIME_ERROR: usize = 6;
pub const SYSCALL_ID_START_THREAD: usize = 7;
pub const SYSCALL_ID_SUSPEND: usize = 8;
pub const SYSCALL_ID_POLL_INTERRUPT_EXECUTOR: usize = 9;

pub fn thread_yield() {
    let _ = syscall(SYSCALL_ID_YIELD, 0, 0, 0);
}

pub(crate) fn thread_wait(
    waiter_queue: *mut LinkedList<Suspendable, WaitQueueTag>,
    ceiling: AnyPriority,
) {
    if in_interrupt() {
        // Error: cannot wait in an interrupt handler
        crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
    }

    let _ = syscall(SYSCALL_ID_WAIT, waiter_queue as usize, ceiling as usize, 0);
}

pub(crate) fn thread_wait_event(events: u32) -> u32 {
    if in_interrupt() {
        // Error: cannot wait in an interrupt handler
        crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
    }

    syscall(SYSCALL_ID_WAIT_EVENT as usize, events as usize, 0, 0) as u32
}

pub(crate) fn thread_wait_event_until(events: u32, deadline: Instant) -> u32 {
    if in_interrupt() {
        // Error: cannot wait in an interrupt handler
        crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
    }
    syscall(
        SYSCALL_ID_WAIT_EVENT_UNTIL,
        events as usize,
        (deadline.tick >> 32) as usize,
        deadline.tick as u32 as usize,
    ) as u32
}

#[cfg(any(feature = "relative-delay", test))]
pub fn delay(duration: Duration) {
    delay_until(Instant::now() + duration)
}

pub fn delay_until(time: Instant) {
    let _ = syscall(
        SYSCALL_ID_DELAY_UNTIL,
        (time.tick >> 32) as usize,
        time.tick as u32 as usize,
        0,
    );
}

pub fn thread_suspend(thread: Option<&RawThread>) {
    let thread_ptr = thread.map(|t| t as *const _ as usize).unwrap_or(0);
    let _ = syscall(SYSCALL_ID_SUSPEND, thread_ptr, 0, 0);
}

pub fn poll_interrupt_executor(interrupt: &RawInterruptHandler) {
    syscall(
        SYSCALL_ID_POLL_INTERRUPT_EXECUTOR,
        interrupt as *const _ as usize,
        0,
        0,
    );
}

pub fn runtime_error(
    error: RuntimeError,
    location: &'static ::core::panic::Location<'static>,
) -> ! {
    let _ = syscall(
        SYSCALL_ID_RUNTIME_ERROR,
        error as usize,
        location as *const ::core::panic::Location<'static> as usize,
        0,
    );
    unreachable!();
}

pub(crate) fn start_thread(thread: &mut RawThread) {
    let _ = syscall(SYSCALL_ID_START_THREAD, thread as *mut _ as usize, 0, 0);
}

#[unsafe(no_mangle)]
unsafe fn _private_kernel_syscall_handler(
    id: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
) -> usize {
    static SYSCALL_INTERRUPT_HANDLER: SyncUnsafeCell<RawInterruptHandler> =
        SyncUnsafeCell::new(RawInterruptHandler::new(0, Priority::interrupt(0)));

    let mut rval = 0;
    unsafe {
        interrupt_context(SYSCALL_INTERRUPT_HANDLER.get(), || match id {
            SYSCALL_ID_YIELD => {
                Scheduler::yield_current_thread_isr();
            }
            SYSCALL_ID_WAIT => {
                Scheduler::wait_current_thread_isr(
                    arg0 as *mut _,
                    Priority::from_any(arg1 as AnyPriority),
                );
            }
            SYSCALL_ID_WAIT_EVENT => {
                rval = Scheduler::wait_current_thread_event_isr(arg0 as u32, None) as usize;
            }
            SYSCALL_ID_WAIT_EVENT_UNTIL => {
                let time = (u64::from(arg1 as u32) << 32) + u64::from(arg2 as u32);
                rval = Scheduler::wait_current_thread_event_isr(arg0 as u32, Some(time)) as usize;
            }
            SYSCALL_ID_DELAY_UNTIL => {
                let time = (u64::from(arg0 as u32) << 32) + u64::from(arg1 as u32);
                Scheduler::delay_task_until(time);
            }
            SYSCALL_ID_RUNTIME_ERROR => {
                let location = &*(arg1 as *const ::core::panic::Location<'static>);
                crate::kernel::exception::handle_runtime_error(
                    RuntimeError::from_id(arg0),
                    location,
                );
            }
            SYSCALL_ID_START_THREAD => {
                let thread: &'static mut RawThread = &mut *(arg0 as *mut RawThread);
                Scheduler::start_thread_isr(Pin::static_mut(thread));
            }
            SYSCALL_ID_SUSPEND => {
                let maybe_thread =
                    NonNull::new(arg0 as *mut RawThread).map(|p| Pin::new_unchecked(p.as_ref()));
                Scheduler::suspend_thread_isr(maybe_thread);
            }
            SYSCALL_ID_POLL_INTERRUPT_EXECUTOR => {
                let interrupt = &*(arg0 as *const RawInterruptHandler);
                interrupt.poll_executor();
            }
            _ => panic!("Invalid syscall {:?}", id),
        });
    }
    rval
}
