use crate::Simulator;
use crate::context::VirtualTrap;
use crate::error::SimulatorErrorKind;
use crate::signal::{ALARM_SIGNAL, SYSCALL_SIGNAL, install_handler};
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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
    interrupt_sigmask: libc::sigset_t,
}

impl VirtualInterruptController {
    pub fn new() -> VirtualInterruptController {
        let mut interrupt_sigmask = MaybeUninit::uninit();
        unsafe {
            // Signals to mask while interrupts are disabled.
            if libc::sigemptyset(interrupt_sigmask.as_mut_ptr()) != 0
                || libc::sigaddset(interrupt_sigmask.as_mut_ptr(), ALARM_SIGNAL) != 0
            {
                fault!(SimulatorErrorKind::SignalMaskFailed {
                    signal: ALARM_SIGNAL
                });
            }

            // SYSCALL_SIGNAL handler additionally masks ALARM_SIGNAL while running.
            install_handler(SYSCALL_SIGNAL, ALARM_SIGNAL);
        }
        VirtualInterruptController {
            priority: [INITIAL_PRIORITY; MAX_INTERRUPT + 1],
            threshold: AtomicU8::new(0),
            enable: [INITIAL_ENABLE; MAX_INTERRUPT + 1],
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
                // Clear the pending flag and call the kernel service call handler
                Simulator::clear_service_call();
                unsafe {
                    Simulator::kernel_service_call_handler();
                }
            }
            _ => fault!(SimulatorErrorKind::UnhandledException {
                exception_type: "Unhandled trap"
            }),
        }
    }
}

pub struct InterruptClaim {
    interrupt_number: u16,
}

impl GetInterruptNumber for InterruptClaim {
    fn get_interrupt_number(&self) -> u16 {
        self.interrupt_number
    }
}

impl InterruptController for Simulator {
    const MAX_INTERRUPT_PRIORITY: usize = 7;
    const MAX_INTERRUPT_NUMBER: usize = 0;
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
            .store(threshold, Ordering::SeqCst)
        // TODO: if threshold was decreased, pending interrupts might
        // become executable, if they are enabled.
    }

    fn claim_interrupt() -> InterruptClaim {
        unimplemented!()
    }

    fn complete_interrupt(_claim: InterruptClaim) {
        unimplemented!()
    }

    fn enable_interrupt(interrupt_number: u16) {
        Self::instance().interrupt_controller.enable[interrupt_number as usize]
            .store(true, Ordering::SeqCst);
        // TODO: if interrupt is pending, it must be executed
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
        // Only re-enable interrupts if they were enabled before the critical section.
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
