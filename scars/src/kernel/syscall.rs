use crate::kernel::hal;
use crate::priority::{AnyPriority, Priority};
use crate::sync::{InterruptLock, NestingLock, interrupt_lock::InterruptLockKey};
use crate::thread::RawThread;
use crate::time::{Duration, Instant};
use crate::{
    interrupt::{
        CriticalSection, RawInterruptHandler, in_interrupt, interrupt_context,
        restore_current_interrupt, switch_current_interrupt,
    },
    kernel::{
        RuntimeError,
        hal::{clock_ticks, syscall},
        list::LinkedList,
        scheduler::Scheduler,
        waiter::{WaitQueue, WaitQueueEntry, WaitQueueHandle, WaitQueueTag},
    },
};
use core::cell::SyncUnsafeCell;
use core::marker::PhantomData;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use scars_khal::{FlowController, Fault};

pub const SYSCALL_ID_YIELD: usize = 1;
pub const SYSCALL_ID_WAIT: usize = 2;
pub const SYSCALL_ID_WAIT_EVENT: usize = 3;
pub const SYSCALL_ID_WAIT_EVENT_UNTIL: usize = 4;
pub const SYSCALL_ID_DELAY_UNTIL: usize = 5;
pub const SYSCALL_ID_RUNTIME_ERROR: usize = 6;
pub const SYSCALL_ID_START_THREAD: usize = 7;
pub const SYSCALL_ID_SUSPEND: usize = 8;

pub fn thread_yield() {
    let _ = syscall(SYSCALL_ID_YIELD, 0, 0, 0);
}

pub(crate) fn thread_wait<'a, L: NestingLock>(wait_queue: &WaitQueue<L>) {
    if in_interrupt() {
        // Error: cannot wait in an interrupt handler
        crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
    }

    let (queue, vtable) = wait_queue.to_raw();

    let _ = syscall(SYSCALL_ID_WAIT, queue as usize, vtable as usize, 0);
}

pub(crate) fn thread_wait_event(wait_events: *mut crate::WaitEvents) {
    if in_interrupt() {
        // Error: cannot wait in an interrupt handler
        crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
    }

    syscall(SYSCALL_ID_WAIT_EVENT as usize, wait_events as usize, 0, 0);
}

pub(crate) fn thread_wait_event_until(wait_events: *mut crate::WaitEvents, deadline: Instant) {
    if in_interrupt() {
        // Error: cannot wait in an interrupt handler
        crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
    }
    syscall(
        SYSCALL_ID_WAIT_EVENT_UNTIL,
        wait_events as usize,
        (deadline.tick >> 32) as usize,
        deadline.tick as u32 as usize,
    );
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

pub(crate) fn thread_suspend(thread: Option<&RawThread>) {
    let thread_ptr = thread.map(|t| t as *const _ as usize).unwrap_or(0);
    let _ = syscall(SYSCALL_ID_SUSPEND, thread_ptr, 0, 0);
}

struct FaultWrapper<'a> {
    error: &'a dyn Fault,
}

pub fn runtime_error(error: &dyn Fault) -> ! {
    let wrapper = FaultWrapper { error };
    let _ = syscall(
        SYSCALL_ID_RUNTIME_ERROR,
        &wrapper as *const _ as usize,
        0,
        0,
    );
    unreachable!();
}

pub(crate) fn start_thread(thread: &mut RawThread) {
    let _ = syscall(SYSCALL_ID_START_THREAD, thread as *mut _ as usize, 0, 0);
}

static SYSCALL_INTERRUPT_HANDLER: SyncUnsafeCell<RawInterruptHandler> =
    SyncUnsafeCell::new(RawInterruptHandler::new(Priority::interrupt(0)));

#[unsafe(no_mangle)]
unsafe fn _private_kernel_syscall_handler(
    id: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
) -> usize {
    let rval = 0;
    unsafe {
        interrupt_context(SYSCALL_INTERRUPT_HANDLER.get(), || {
            match id {
                SYSCALL_ID_YIELD => {
                    Scheduler::yield_current_thread_isr();
                }
                SYSCALL_ID_WAIT => {
                    let wait_queue = WaitQueueHandle::from_raw(arg0 as *const (), arg1 as *const _);
                    Scheduler::wait_current_thread_isr(wait_queue);
                }
                SYSCALL_ID_WAIT_EVENT => {
                    let wait_events = arg0 as *mut crate::WaitEvents;
                    Scheduler::wait_current_thread_event_isr(wait_events, None);
                }
                SYSCALL_ID_WAIT_EVENT_UNTIL => {
                    let wait_events = arg0 as *mut crate::WaitEvents;
                    let time = (u64::from(arg1 as u32) << 32) + u64::from(arg2 as u32);
                    Scheduler::wait_current_thread_event_isr(wait_events, Some(time));
                }
                SYSCALL_ID_DELAY_UNTIL => {
                    let time = (u64::from(arg0 as u32) << 32) + u64::from(arg1 as u32);
                    Scheduler::delay_thread_until(time);
                }
                SYSCALL_ID_RUNTIME_ERROR => {
                    let wrapper = &*(arg0 as *const FaultWrapper);
                    crate::kernel::exception::handle_runtime_error(wrapper.error);
                }
                SYSCALL_ID_START_THREAD => {
                    let thread: &'static mut RawThread = &mut *(arg0 as *mut RawThread);
                    Scheduler::start_thread(Pin::static_mut(thread));
                }
                SYSCALL_ID_SUSPEND => {
                    let maybe_thread = NonNull::new(arg0 as *mut RawThread)
                        .map(|p| Pin::new_unchecked(p.as_ref()));
                    Scheduler::suspend_thread(maybe_thread);
                }
                _ => panic!("Invalid syscall {:?}", id),
            }
            Scheduler::execute_pending_reschedule();
        });
    }
    rval
}

#[unsafe(no_mangle)]
pub(crate) unsafe fn _private_kernel_service_call_handler() {
    unsafe {
        // Anything that needs to be done within some context, i.e. anything that calls
        // context-aware functions, must be done within an interrupt context. Service call
        // is like an asynchronous syscall without any parameters or a return value. It
        // shares the same interrupt handler as syscall.
        interrupt_context(SYSCALL_INTERRUPT_HANDLER.get(), || {
            Scheduler::process_all_pending_events();
            Scheduler::execute_pending_reschedule();
        });
    }
}
