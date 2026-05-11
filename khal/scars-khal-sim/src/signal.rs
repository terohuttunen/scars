use crate::context::{VirtualTrap, current_thread_context};
use crate::error::SimulatorErrorKind;
use crate::interrupt::{INTERRUPTS_ENABLED, VirtualInterruptController};
use crate::timer::VirtualTimer;
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;
use scars_fault::*;

/// Sent to the current RTOS thread on syscall, service call, or
/// virtual interrupt arrival.
pub(crate) const SYSCALL_SIGNAL: libc::c_int = libc::SIGUSR1;

/// Sent to the current RTOS thread on timer expiration.
pub(crate) const ALARM_SIGNAL: libc::c_int = libc::SIGALRM;

/// The clock the simulator uses for RTOS time and timeouts.
pub(crate) const SIMULATOR_CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

/// Install `trap_signal_handler` for `signal`, additionally masking
/// `also_block` while the handler runs.
pub(crate) unsafe fn install_handler(signal: libc::c_int, also_block: libc::c_int) {
    unsafe {
        let mut mask = MaybeUninit::uninit();
        if libc::sigemptyset(mask.as_mut_ptr()) != 0
            || libc::sigaddset(mask.as_mut_ptr(), also_block) != 0
        {
            fault!(SimulatorErrorKind::SignalMaskFailed { signal: also_block });
        }

        let sigaction = libc::sigaction {
            sa_sigaction: trap_signal_handler as *const () as libc::sighandler_t,
            sa_mask: mask.assume_init(),
            sa_flags: libc::SA_SIGINFO,
            sa_restorer: None,
        };
        if libc::sigaction(signal, &sigaction, core::ptr::null_mut()) != 0 {
            fault!(SimulatorErrorKind::SignalHandlerFailed { signal });
        }
    }
}

pub(crate) extern "C" fn trap_signal_handler(
    sig: libc::c_int,
    info: *const libc::siginfo_t,
    ucontext: *const libc::ucontext_t,
) {
    // thread that got interrupted by the signal
    let interrupted_context = current_thread_context();

    // Disable interrupts for the duration of the trap handling
    let restore_state = INTERRUPTS_ENABLED.swap(false, Ordering::SeqCst);

    match sig {
        // Virtual software interrupt signals
        SYSCALL_SIGNAL => {
            let trap = unsafe { &mut *((*info).si_value().sival_ptr as *mut VirtualTrap) };
            VirtualInterruptController::handle_trap(trap);
        }
        // Virtual timer interrupt signals
        ALARM_SIGNAL => {
            VirtualTimer::handle_alarm();
        }
        _ => fault!(SimulatorErrorKind::UnhandledException {
            exception_type: "Unknown signal"
        }),
    }

    // If the current thread has been changed by the trap handling,
    // resume the new current thread, and suspend the thread that was
    // interrupted.
    let context_to_resume = current_thread_context();
    if context_to_resume.thread_id != interrupted_context.thread_id {
        context_to_resume.resume();
        interrupted_context
            .stack_top_ptr
            .set(unsafe { (*ucontext).uc_stack.ss_sp as *const u8 });
        interrupted_context.suspend();
    }

    // Restore interrupts enable state
    INTERRUPTS_ENABLED.store(restore_state, Ordering::SeqCst);
}
