use crate::Simulator;
use crate::context::{VirtualTrap, current_thread_context};
use crate::error::SimulatorErrorKind;
use crate::signal::{ALARM_SIGNAL, INTERRUPT_SIGNAL, SYSCALL_SIGNAL, install_handler};
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use scars_fault::*;
use scars_khal::*;

pub const MAX_INTERRUPT_PRIORITY: u8 =
    <Simulator as InterruptController>::MAX_INTERRUPT_PRIORITY as u8;
pub const MAX_INTERRUPT: usize = <Simulator as InterruptController>::MAX_INTERRUPT_NUMBER;

const INITIAL_PRIORITY: AtomicU8 = AtomicU8::new(0);
const INITIAL_ENABLE: AtomicBool = AtomicBool::new(false);

pub(crate) static INTERRUPTS_ENABLED: AtomicBool = AtomicBool::new(false);

pub struct VirtualInterruptController {
    priority: [AtomicU8; MAX_INTERRUPT + 1],
    threshold: AtomicU8,
    enable: [AtomicBool; MAX_INTERRUPT + 1],
    /// Bit `n` set means IRQ `n` is pending.
    pending: AtomicU32,
    /// Signals masked by `acquire`/`restore` to disable interrupt-like
    /// delivery during kernel critical sections.
    interrupt_sigmask: libc::sigset_t,
}

impl VirtualInterruptController {
    pub fn new() -> VirtualInterruptController {
        let mut interrupt_sigmask = MaybeUninit::uninit();
        unsafe {
            // Interrupt-like signals deferred during a kernel critical
            // section: the timer and virtual IRQs. SYSCALL_SIGNAL is
            // intentionally not included — syscalls and service calls
            // ride SIGUSR1 and continue to flow.
            if libc::sigemptyset(interrupt_sigmask.as_mut_ptr()) != 0
                || libc::sigaddset(interrupt_sigmask.as_mut_ptr(), ALARM_SIGNAL) != 0
                || libc::sigaddset(interrupt_sigmask.as_mut_ptr(), INTERRUPT_SIGNAL) != 0
            {
                fault!(SimulatorErrorKind::SignalMaskFailed {
                    signal: ALARM_SIGNAL
                });
            }

            // Syscall / service-call handler. Masks ALARM but not
            // itself — service calls must not re-enter syscalls.
            install_handler(SYSCALL_SIGNAL, &[ALARM_SIGNAL], 0);

            // Virtual-interrupt handler. SA_NODEFER lets a
            // higher-priority IRQ preempt a handler that has already
            // raised the threshold — the kernel's claim_interrupt
            // unblocks signals mid-handler to enable nesting.
            //
            // SYSCALL_SIGNAL is also masked: on Cortex-M, PendSV
            // (which SYSCALL_SIGNAL stands in for) is gated by BASEPRI
            // while any IRQ is running, so pending the service call
            // from inside an IRQ handler does not cause an immediate
            // context switch — the bit waits for the IRQ to unwind.
            // Without masking here, `pthread_sigqueue(SYSCALL_SIGNAL)`
            // from inside the user IRQ handler would deliver nested,
            // letting the inner `trap_signal_handler` tail
            // context-switch out of the IRQ stack while kernel state
            // (held locks, etc.) is still in flux.
            install_handler(
                INTERRUPT_SIGNAL,
                &[ALARM_SIGNAL, SYSCALL_SIGNAL],
                libc::SA_NODEFER,
            );
        }
        VirtualInterruptController {
            priority: [INITIAL_PRIORITY; MAX_INTERRUPT + 1],
            threshold: AtomicU8::new(0),
            enable: [INITIAL_ENABLE; MAX_INTERRUPT + 1],
            pending: AtomicU32::new(0),
            interrupt_sigmask: unsafe { interrupt_sigmask.assume_init() },
        }
    }

    pub(crate) fn handle_trap(trap: &mut VirtualTrap) {
        match trap {
            &mut VirtualTrap::Syscall {
                ref id,
                ref args,
                ref mut rval,
            } => {
                *rval =
                    unsafe { Simulator::kernel_syscall_handler(*id, args[0], args[1], args[2]) };
            }
            VirtualTrap::ServiceCall => {
                Simulator::clear_service_call();
                unsafe {
                    Simulator::kernel_service_call_handler();
                }
            }
        }
    }

    /// INTERRUPT_SIGNAL handler body. Drains runnable virtual
    /// interrupts: each `kernel_interrupt_handler` call claims one
    /// IRQ (raising the threshold), dispatches it, and completes it
    /// (lowering the threshold back). The kernel-side handler is
    /// responsible for pending a service call if the IRQ left a
    /// reschedule queued.
    pub(crate) fn handle_interrupt() {
        while find_runnable().is_some() {
            unsafe { Simulator::kernel_interrupt_handler() };
        }
    }
}

pub struct InterruptClaim {
    interrupt_number: u16,
    restore_threshold: u8,
}

impl GetInterruptNumber for InterruptClaim {
    fn get_interrupt_number(&self) -> u16 {
        self.interrupt_number
    }
}

/// Highest-priority enabled pending IRQ above the current threshold.
fn find_runnable() -> Option<u16> {
    let ic = &Simulator::instance().interrupt_controller;
    let pending = ic.pending.load(Ordering::SeqCst);
    let threshold = ic.threshold.load(Ordering::SeqCst);
    let no_mask = threshold >= MAX_INTERRUPT_PRIORITY;
    let mut best: Option<(u16, u8)> = None;
    let mut bits = pending;
    while bits != 0 {
        let i = bits.trailing_zeros() as usize;
        bits &= !(1u32 << i);
        if i > MAX_INTERRUPT {
            break;
        }
        if !ic.enable[i].load(Ordering::SeqCst) {
            continue;
        }
        let prio = ic.priority[i].load(Ordering::SeqCst);
        if !no_mask && prio <= threshold {
            continue;
        }
        if best.is_none_or(|(_, p)| prio > p) {
            best = Some((i as u16, prio));
        }
    }
    best.map(|(i, _)| i)
}

/// Queue an `INTERRUPT_SIGNAL` to the current RTOS thread so the
/// signal handler drains the pending bitmap. Safe to call from any
/// pthread (RTOS thread, peripheral host thread, kernel context).
fn dispatch() {
    unsafe {
        let context = current_thread_context();
        libc::pthread_sigqueue(
            context.thread_id,
            INTERRUPT_SIGNAL,
            libc::sigval {
                sival_ptr: core::ptr::null_mut(),
            },
        );
    }
}

/// Pend virtual interrupt `n`. If it's enabled and its priority is
/// above the current threshold, signal the running RTOS thread to
/// drain the pending bitmap. A spurious signal (race against threshold
/// rising) is harmless — the trap handler re-checks.
pub fn pend_interrupt(n: u16) {
    let ic = &Simulator::instance().interrupt_controller;
    let bit = 1u32 << (n as u32);
    ic.pending.fetch_or(bit, Ordering::SeqCst);
    if find_runnable().is_some() {
        dispatch();
    }
}

impl InterruptController for Simulator {
    const MAX_INTERRUPT_PRIORITY: usize = 7;
    const MAX_INTERRUPT_NUMBER: usize = 31;
    type InterruptClaim = InterruptClaim;

    fn get_interrupt_priority(interrupt_number: u16) -> u8 {
        Self::instance().interrupt_controller.priority[interrupt_number as usize]
            .load(Ordering::SeqCst)
    }

    fn set_interrupt_priority(interrupt_number: u16, prio: u8) -> u8 {
        Self::instance().interrupt_controller.priority[interrupt_number as usize]
            .swap(prio, Ordering::SeqCst)
    }

    #[inline(always)]
    fn get_interrupt_threshold() -> u8 {
        Self::instance()
            .interrupt_controller
            .threshold
            .load(Ordering::SeqCst)
    }

    #[inline(always)]
    fn set_interrupt_threshold(threshold: u8) {
        Self::instance()
            .interrupt_controller
            .threshold
            .store(threshold, Ordering::SeqCst);
        // Lowering the threshold can expose previously masked pending
        // interrupts; re-dispatch if anything became runnable.
        if find_runnable().is_some() {
            dispatch();
        }
    }

    fn claim_interrupt() -> InterruptClaim {
        // Called from the trap handler with signals disabled. Pick the
        // highest-priority runnable IRQ, clear its pending bit, raise
        // the threshold to that IRQ's priority, and re-enable signals
        // so a higher-priority IRQ can preempt this handler.
        let interrupt_number = find_runnable().expect("claim_interrupt with no runnable IRQ");
        let ic = &Self::instance().interrupt_controller;
        ic.pending
            .fetch_and(!(1u32 << (interrupt_number as u32)), Ordering::SeqCst);
        let interrupt_prio = ic.priority[interrupt_number as usize].load(Ordering::SeqCst);
        let restore_threshold = ic.threshold.swap(interrupt_prio, Ordering::SeqCst);
        Self::restore(true);
        InterruptClaim {
            interrupt_number,
            restore_threshold,
        }
    }

    fn complete_interrupt(claim: InterruptClaim) {
        // No signal-mask change here. `claim_interrupt` unblocked the
        // IRQ signals to allow nesting *during* the handler body;
        // between drain-loop iterations we don't need them blocked —
        // the pending bitmap is the source of truth for what to
        // dispatch next, and the outermost `trap_signal_handler`'s
        // tail (plus the kernel's automatic mask restore on handler
        // return) re-establishes the pre-trap state. Just drop the
        // threshold back to the caller's saved level.
        Self::instance()
            .interrupt_controller
            .threshold
            .store(claim.restore_threshold, Ordering::SeqCst);
    }

    fn enable_interrupt(interrupt_number: u16) {
        Self::instance().interrupt_controller.enable[interrupt_number as usize]
            .store(true, Ordering::SeqCst);
        // Newly-enabled IRQ may already be pending.
        if find_runnable().is_some() {
            dispatch();
        }
    }

    fn disable_interrupt(interrupt_number: u16) {
        Self::instance().interrupt_controller.enable[interrupt_number as usize]
            .store(false, Ordering::SeqCst);
    }

    #[inline(always)]
    fn interrupt_status() -> bool {
        INTERRUPTS_ENABLED.load(Ordering::SeqCst)
    }

    fn acquire() -> bool {
        let old_state = INTERRUPTS_ENABLED.swap(false, Ordering::SeqCst);
        if old_state {
            unsafe {
                libc::pthread_sigmask(
                    libc::SIG_BLOCK,
                    &Self::instance().interrupt_controller.interrupt_sigmask,
                    core::ptr::null_mut(),
                );
            }
        }
        old_state
    }

    fn restore(restore_state: bool) {
        if restore_state {
            INTERRUPTS_ENABLED.store(true, Ordering::SeqCst);
            unsafe {
                libc::pthread_sigmask(
                    libc::SIG_UNBLOCK,
                    &Self::instance().interrupt_controller.interrupt_sigmask,
                    core::ptr::null_mut(),
                );
            }
        }
    }
}
