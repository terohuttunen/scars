use crate::kernel::hal;
use crate::priority::{AnyPriority, Priority};
use crate::sync::CoreInterruptLock;
use crate::sync::atomic::Ordering;
use crate::sync::lock::interrupt_lock::CoreInterruptLockKey;
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
        waiter::{WaitQueueEntry, WaitQueueHandle, WaitQueueTag},
    },
};
use core::cell::SyncUnsafeCell;
use core::marker::PhantomData;
use scars_khal::{CoreController, Fault};

#[cfg(feature = "multithreading")]
pub const SYSCALL_ID_YIELD: usize = 1;
#[cfg(feature = "multithreading")]
pub const SYSCALL_ID_WAIT_EVENT: usize = 3;
#[cfg(feature = "multithreading")]
pub const SYSCALL_ID_WAIT_EVENT_UNTIL: usize = 4;
#[cfg(feature = "multithreading")]
pub const SYSCALL_ID_DELAY_UNTIL: usize = 5;
pub const SYSCALL_ID_RUNTIME_ERROR: usize = 6;

#[cfg(feature = "multithreading")]
pub fn thread_yield() {
    let _ = syscall(SYSCALL_ID_YIELD, 0, 0, 0);
}

#[cfg(feature = "multithreading")]
pub(crate) fn thread_wait_event(wait_events: *mut crate::WaitEvents) {
    if in_interrupt() {
        // Error: cannot wait in an interrupt handler
        crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
    }

    syscall(SYSCALL_ID_WAIT_EVENT as usize, wait_events as usize, 0, 0);
}

#[cfg(feature = "multithreading")]
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

#[cfg(feature = "multithreading")]
pub fn delay(duration: Duration) {
    delay_until(Instant::now() + duration)
}

#[cfg(feature = "multithreading")]
pub fn delay_until(time: Instant) {
    let _ = syscall(
        SYSCALL_ID_DELAY_UNTIL,
        (time.tick >> 32) as usize,
        time.tick as u32 as usize,
        0,
    );
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

/// Per-core syscall/service-call interrupt context. Each core runs
/// its own syscall handler under its own preempt and ceiling state,
/// so the `RawInterruptHandler`'s non-`Sync` interior cells
/// (`owned_locks`, `current_event_handler`, `closure_ptr`) must not
/// be shared between cores. One handler per core; each carries its
/// own `core` field for the debug-asserts that compare key.core to
/// handler.core.
static SYSCALL_INTERRUPT_HANDLERS: [SyncUnsafeCell<RawInterruptHandler>;
    crate::kernel::hal::NUM_CORES] = {
    let arr = [const {
        SyncUnsafeCell::new(RawInterruptHandler::new(
            Priority::interrupt(0),
            crate::kernel::hal::CoreId::DEFAULT,
        ))
    }; crate::kernel::hal::NUM_CORES];
    let mut i = 0;
    while i < crate::kernel::hal::NUM_CORES {
        // SAFETY: in this const initializer `arr` is exclusive — no
        // other references exist. We patch each handler's `core` from
        // the placeholder `DEFAULT` to its real per-core id.
        unsafe {
            (*arr[i].get()).core = crate::kernel::hal::CoreId::from_u8_unchecked(i as u8);
        }
        i += 1;
    }
    arr
};

#[inline]
fn local_syscall_handler() -> *mut RawInterruptHandler {
    SYSCALL_INTERRUPT_HANDLERS[crate::kernel::hal::CoreId::current().as_usize()].get()
}

#[unsafe(no_mangle)]
unsafe fn _kernel_syscall_handler(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let rval = 0;
    // `arg1`/`arg2` are only read by the thread-only syscall arms.
    #[cfg(not(feature = "multithreading"))]
    let _ = (arg1, arg2);
    unsafe {
        interrupt_context(local_syscall_handler(), || -> () {
            match id {
                #[cfg(feature = "multithreading")]
                SYSCALL_ID_YIELD => {
                    Scheduler::yield_current_thread_isr();
                }
                #[cfg(feature = "multithreading")]
                SYSCALL_ID_WAIT_EVENT => {
                    let wait_events = arg0 as *mut crate::WaitEvents;
                    Scheduler::wait_current_thread_event_isr(wait_events, None);
                }
                #[cfg(feature = "multithreading")]
                SYSCALL_ID_WAIT_EVENT_UNTIL => {
                    let wait_events = arg0 as *mut crate::WaitEvents;
                    let time = (u64::from(arg1 as u32) << 32) + u64::from(arg2 as u32);
                    Scheduler::wait_current_thread_event_isr(wait_events, Some(time));
                }
                #[cfg(feature = "multithreading")]
                SYSCALL_ID_DELAY_UNTIL => {
                    let time = (u64::from(arg0 as u32) << 32) + u64::from(arg1 as u32);
                    Scheduler::delay_thread_until(time);
                }
                SYSCALL_ID_RUNTIME_ERROR => {
                    let wrapper = &*(arg0 as *const FaultWrapper);
                    crate::kernel::exception::handle_runtime_error(wrapper.error);
                }
                _ => panic!("Invalid syscall {:?}", id),
            };
            // No tail-drain here: producers either pended PendSV
            // directly (preempt allowed) or the preempt-lock release
            // inside the syscall body did. The service call handler
            // runs on the next safe boundary (after this SVCall trap
            // returns) and drains there.
        });
    }
    rval
}

#[unsafe(no_mangle)]
pub(crate) unsafe fn _kernel_service_call_handler() {
    unsafe {
        // Anything that needs to be done within some context, i.e. anything that calls
        // context-aware functions, must be done within an interrupt context. Service call
        // is like an asynchronous syscall without any parameters or a return value. It
        // shares the same interrupt handler as syscall.
        interrupt_context(local_syscall_handler(), || {
            Scheduler::process_pending_work();
        });
    }
}
