use crate::Simulator;
use crate::context::{CURRENT_THREAD_CONTEXT, VirtualContext, VirtualTrap, current_thread_context};
use crate::error::{SimContext, SimulatorErrorKind};
use crate::signal::{ALARM_SIGNAL, INTERRUPT_SIGNAL, SYSCALL_SIGNAL};
use core::mem::MaybeUninit;
use core::sync::atomic::Ordering;
use scars_fault::*;
use scars_khal::*;

impl FlowController for Simulator {
    type StackAlignment = A16;
    type Context = VirtualContext;
    type HardwareError = crate::error::SimulatorError;

    fn start_first_thread(context: *mut Self::Context) -> ! {
        let mut wait_set = MaybeUninit::uninit();

        // Only the currently active RTOS thread should receive the
        // trap, virtual-interrupt, or alarm signals.
        unsafe {
            libc::sigemptyset(wait_set.as_mut_ptr());
            libc::sigaddset(wait_set.as_mut_ptr(), SYSCALL_SIGNAL);
            libc::sigaddset(wait_set.as_mut_ptr(), INTERRUPT_SIGNAL);
            libc::sigaddset(wait_set.as_mut_ptr(), ALARM_SIGNAL);
            libc::pthread_sigmask(
                libc::SIG_BLOCK,
                wait_set.as_mut_ptr(),
                core::ptr::null_mut(),
            );
        }

        CURRENT_THREAD_CONTEXT.store(context, Ordering::SeqCst);

        unsafe { &*context }.resume();

        let mut sig = MaybeUninit::uninit();
        loop {
            // Signals SIGUSR1 and SIGALRM, which the RTOS threads use for traps and alarm
            // clock are blocked from this thread, and should always be handled by the
            // currently running RTOS thread.
            unsafe {
                libc::sigwait(wait_set.as_ptr(), sig.as_mut_ptr());
            }
        }
    }

    fn on_abort() -> ! {
        unsafe {
            libc::abort();
        }
    }

    fn on_exit(exit_code: i32) -> ! {
        unsafe {
            libc::exit(exit_code);
        }
    }

    fn on_fault(info: &FaultInfo) -> ! {
        let plat = SimContext {
            pid: unsafe { libc::getpid() },
        };
        let plat_node = FaultContextNode {
            frame: &plat,
            next: info.context,
        };
        let info = info.with_context(&plat_node);

        if let Some(loc) = info.location {
            eprintln!("Fault at {}: {}", loc, info.error);
        } else {
            eprintln!("Fault: {}", info.error);
        }
        for (i, frame) in info.context_iter().enumerate() {
            eprintln!("  {}: {}", i + 1, frame);
        }

        unsafe {
            libc::exit(1);
        }
    }

    fn on_breakpoint() {
        unimplemented!()
    }

    #[inline(always)]
    fn on_idle() {
        unsafe {
            libc::sched_yield();
        }
    }

    fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        let mut trap = VirtualTrap::Syscall {
            id,
            args: [arg0, arg1, arg2],
            rval: 0,
        };

        let context = current_thread_context();

        if unsafe {
            libc::pthread_sigqueue(
                context.thread_id,
                SYSCALL_SIGNAL,
                libc::sigval {
                    sival_ptr: &mut trap as *mut VirtualTrap as *mut std::ffi::c_void,
                },
            )
        } != 0
        {
            fault!(SimulatorErrorKind::Unknown);
        }

        if let VirtualTrap::Syscall { rval, .. } = trap {
            rval
        } else {
            unreachable!()
        }
    }

    fn current_thread_context() -> *const VirtualContext {
        CURRENT_THREAD_CONTEXT.load(Ordering::SeqCst)
    }

    fn set_current_thread_context(context: *const VirtualContext) {
        CURRENT_THREAD_CONTEXT.store(context as *mut _, Ordering::SeqCst);
    }

    fn pend_service_call() {
        // Mark service call as pending using compare_exchange to avoid duplicate signals
        if Self::instance()
            .service_call_pending
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            // Only send signal if not already pending
            static TRAP: VirtualTrap = VirtualTrap::ServiceCall;
            unsafe {
                let context = current_thread_context();
                libc::pthread_sigqueue(
                    context.thread_id,
                    SYSCALL_SIGNAL,
                    libc::sigval {
                        sival_ptr: &TRAP as *const _ as *mut VirtualTrap as *mut std::ffi::c_void,
                    },
                );
            }
        }
    }

    fn clear_service_call() {
        Self::instance()
            .service_call_pending
            .store(false, Ordering::Release);
    }
}
