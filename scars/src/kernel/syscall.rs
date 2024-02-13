use crate::kernel::{
    hal::{clock_ticks, disable_interrupts, enable_interrupts, syscall},
    interrupt::{
        in_interrupt, interrupt_context, restore_current_interrupt, switch_current_interrupt,
        uncritical_section, CriticalSection, InterruptControlBlock,
    },
    scheduler::Scheduler,
    RuntimeError, WaitQueue,
};
use crate::sync::{interrupt_lock::InterruptLockKey, InterruptLock};
use crate::time::Instant;
use core::cell::SyncUnsafeCell;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use scars_hal::FlowController;

#[derive(Debug)]
#[repr(usize)]
pub enum SyscallId {
    StartScheduler = 0,
    Yield = 1,
    Wait = 2,
    DelayUntil = 3,
    RuntimeError = 4,
    StartTask = 5,
}

impl SyscallId {
    pub fn from_usize(u: usize) -> SyscallId {
        unsafe { core::mem::transmute(u) }
    }
}

pub fn task_yield(_ikey: InterruptLockKey<'_>) {
    let _ = syscall(SyscallId::Yield as usize, 0, 0);
}

pub(crate) fn task_wait(_ikey: InterruptLockKey<'_>) {
    if in_interrupt() {
        // Error: cannot wait in an interrupt handler
        crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
    }

    let _ = syscall(SyscallId::Wait as usize, 0, 0);
}

#[cfg(any(feature = "relative-delay", test))]
pub fn delay(ikey: InterruptLockKey<'_>, duration: crate::time::Duration) {
    delay_until(ikey, Instant::now() + duration)
}

pub fn delay_until(_ikey: InterruptLockKey<'_>, time: Instant) {
    // TODO: this does not work properly on 64bit
    let _ = syscall(
        SyscallId::DelayUntil as usize,
        (time.tick >> 32) as usize,
        time.tick as usize,
    );
}

pub fn runtime_error(
    _ikey: InterruptLockKey<'_>,
    error: RuntimeError,
    location: &'static ::core::panic::Location<'static>,
) -> ! {
    let _ = syscall(
        SyscallId::RuntimeError as usize,
        error as usize,
        location as *const ::core::panic::Location<'static> as usize,
    );
    unreachable!();
}

pub(crate) fn start_task(
    _ikey: InterruptLockKey<'_>,
    task: &mut crate::kernel::task::TaskControlBlock,
) {
    let _ = syscall(SyscallId::StartTask as usize, task as *mut _ as usize, 0);
}

#[no_mangle]
unsafe fn _private_kernel_syscall_handler(id: usize, arg0: usize, arg1: usize) -> usize {
    static SYSCALL_INTERRUPT_HANDLER: SyncUnsafeCell<InterruptControlBlock> =
        SyncUnsafeCell::new(InterruptControlBlock::new(0, 0));

    let rval = 0;
    interrupt_context(SYSCALL_INTERRUPT_HANDLER.get(), |cs| {
        if id > 5 {
            panic!("Invalid syscall id");
        }
        let syscall_id = SyscallId::from_usize(id);
        match syscall_id {
            SyscallId::StartScheduler => Scheduler::start_isr(cs),
            SyscallId::Yield => {
                Scheduler::yield_current_task_isr(cs);
            }
            SyscallId::Wait => {
                Scheduler::wait_current_task_isr(cs);
            }
            SyscallId::DelayUntil => {
                let time = (u64::from(arg0 as u32) << 32) + u64::from(arg1 as u32);
                Scheduler::delay_until_isr(cs, time);
            }
            SyscallId::RuntimeError => {
                let location = unsafe { &*(arg1 as *const ::core::panic::Location<'static>) };
                crate::kernel::exception::handle_runtime_error(
                    RuntimeError::from_id(arg0),
                    location,
                );
            }
            SyscallId::StartTask => {
                let task = unsafe { &mut *(arg0 as *mut crate::kernel::task::TaskControlBlock) };
                Scheduler::start_task_isr(cs, task);
            }
        }
    });

    rval
}
