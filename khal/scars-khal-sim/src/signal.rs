use crate::context::{VirtualTrap, current_thread_context};
use crate::error::SimulatorErrorKind;
use crate::interrupt::{INTERRUPTS_ENABLED, VirtualInterruptController};
use crate::timer::VirtualTimer;
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;
use scars_fault::*;

/// Sent to the current RTOS thread on syscall or service call. The
/// `sival_ptr` carries a `&mut VirtualTrap`.
pub(crate) const SYSCALL_SIGNAL: libc::c_int = libc::SIGUSR1;

/// Sent to the current RTOS thread on virtual interrupt arrival. Has
/// no payload — the handler drains the pending bitmap. Carried on a
/// distinct signal so it can run with `SA_NODEFER` (nested IRQs)
/// without affecting reentry semantics for syscalls / service calls.
pub(crate) const INTERRUPT_SIGNAL: libc::c_int = libc::SIGUSR2;

/// Sent to the current RTOS thread on timer expiration.
pub(crate) const ALARM_SIGNAL: libc::c_int = libc::SIGALRM;

/// The clock the simulator uses for RTOS time and timeouts.
pub(crate) const SIMULATOR_CLOCK: libc::clockid_t = libc::CLOCK_MONOTONIC;

/// Install `trap_signal_handler` for `signal`, additionally masking
/// every signal in `also_block` while the handler runs. `extra_flags`
/// is OR'd into `sa_flags` on top of the always-required
/// `SA_SIGINFO`. Pass `SA_NODEFER` on signals whose handler may
/// re-enter itself (virtual-interrupt nesting).
pub(crate) unsafe fn install_handler(
    signal: libc::c_int,
    also_block: &[libc::c_int],
    extra_flags: libc::c_int,
) {
    unsafe {
        let mut mask = MaybeUninit::uninit();
        if libc::sigemptyset(mask.as_mut_ptr()) != 0 {
            fault!(SimulatorErrorKind::SignalMaskFailed { signal });
        }
        for &s in also_block {
            if libc::sigaddset(mask.as_mut_ptr(), s) != 0 {
                fault!(SimulatorErrorKind::SignalMaskFailed { signal: s });
            }
        }

        let sigaction = libc::sigaction {
            sa_sigaction: trap_signal_handler as *const () as libc::sighandler_t,
            sa_mask: mask.assume_init(),
            sa_flags: libc::SA_SIGINFO | extra_flags,
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
        // Syscall or service-call trap.
        SYSCALL_SIGNAL => {
            let trap = unsafe { &mut *((*info).si_value().sival_ptr as *mut VirtualTrap) };
            VirtualInterruptController::handle_trap(trap);
        }
        // Virtual interrupt arrival. Drain the pending bitmap; the
        // payload (if any) is irrelevant.
        INTERRUPT_SIGNAL => {
            VirtualInterruptController::handle_interrupt();
        }
        // Virtual timer.
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
