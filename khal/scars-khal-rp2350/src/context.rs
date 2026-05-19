//! Per-core current-thread-context slot, [`CoreController`] impl, and
//! the three system exception trampolines that read the slot inline.
//!
//! Single-core Cortex-M KHALs let `impl_core_controller!` in
//! `scars-arch-cortex-m` emit SVCall/PendSV/DefaultHandler with a
//! `ldr =SLOT; ldr [...]` prologue. RP2350 has two cores, so the
//! prologue is a CPUID load + array index — handled here with the
//! load baked directly into the naked asm (`sym CURRENT_THREAD_CONTEXT`
//! resolves to the static below). No function-call indirection per
//! exception.

use core::sync::atomic::{AtomicPtr, Ordering};

use scars_arch_cortex_m::{Context as CortexMContext, CortexMFault};
use scars_khal::CoreController;

use crate::{RP2350, ipi, multicore, sio_cpuid};

/// Per-core current-thread-context slots. Indexed by `sio_cpuid()`.
/// Read inline by the per-core asm trampolines below (they bake the
/// CPUID load + array index right into the naked asm) and by the
/// [`CoreController::current_thread_context`] trait method. Written
/// only by [`CoreController::set_current_thread_context`].
#[unsafe(no_mangle)]
pub(crate) static CURRENT_THREAD_CONTEXT: [AtomicPtr<CortexMContext>; 2] = [
    AtomicPtr::new(core::ptr::null_mut()),
    AtomicPtr::new(core::ptr::null_mut()),
];

impl CoreController for RP2350 {
    type StackAlignment = scars_khal::A8;
    type Context = CortexMContext;
    type HardwareError = CortexMFault;

    /// RP2350 ships with two Cortex-M33 cores. Both run the SCARS
    /// kernel under their own per-core scheduler; cross-core ops
    /// are dispatched via [`pend_service_call_on`] + the SIO inter-core
    /// FIFO.
    const NUM_CORES: usize = 2;

    #[inline(always)]
    fn current_core_id() -> u8 {
        sio_cpuid()
    }

    #[inline(always)]
    fn pend_service_call_on(core: u8) {
        if core == sio_cpuid() {
            scars_arch_cortex_m::pend_service_call()
        } else {
            ipi::ipi_push(ipi::IPI_SENTINEL);
        }
    }

    #[inline(always)]
    fn start_first_thread(idle_context: *mut Self::Context) -> ! {
        // Populate this core's context slot before the arch asm hands
        // off; any exception fired after `start_first_thread` will
        // read the slot through the inline CPUID + array load baked
        // into the trampolines below.
        <Self as CoreController>::set_current_thread_context(idle_context);

        // On core 1, this is the last hook the kernel gives us
        // before `start_first_thread`'s asm switches to the idle
        // thread and never returns. By this point
        // `Scheduler::start_on(1)` has already published
        // `SCHEDULERS[1]` and set `SCHEDULER_INITIALIZED[1]`, so it
        // is now safe for core 0 to dispatch cross-core ops to us.
        // Tell core 0 (which is busy-polling in `launch_core1`) that
        // we're ready.
        if sio_cpuid() == 1 {
            multicore::signal_core1_alive();
        }

        scars_arch_cortex_m::start_first_thread(idle_context)
    }

    #[inline(always)]
    fn on_abort() -> ! {
        scars_arch_cortex_m::on_abort()
    }

    #[inline(always)]
    fn on_exit(exit_code: i32) -> ! {
        scars_arch_cortex_m::on_exit(exit_code)
    }

    #[inline(always)]
    fn on_fault(info: &scars_khal::FaultInfo) -> ! {
        let frame =
            CURRENT_THREAD_CONTEXT[sio_cpuid() as usize].load(Ordering::Relaxed) as *const _;
        scars_arch_cortex_m::on_fault(info, frame)
    }

    #[inline(always)]
    fn on_breakpoint() {
        scars_arch_cortex_m::on_breakpoint()
    }

    #[inline(always)]
    fn on_idle() {
        scars_arch_cortex_m::on_idle()
    }

    #[inline(always)]
    fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        scars_arch_cortex_m::syscall(id, arg0, arg1, arg2)
    }

    #[inline(always)]
    fn current_thread_context() -> *const Self::Context {
        CURRENT_THREAD_CONTEXT[sio_cpuid() as usize].load(Ordering::Relaxed) as *const _
    }

    #[inline(always)]
    fn set_current_thread_context(context: *const Self::Context) {
        CURRENT_THREAD_CONTEXT[sio_cpuid() as usize].store(context as *mut _, Ordering::Relaxed);
    }

    #[inline(always)]
    fn pend_service_call() {
        scars_arch_cortex_m::pend_service_call()
    }

    #[inline(always)]
    fn clear_service_call() {
        scars_arch_cortex_m::clear_service_call()
    }
}

// ── System exception trampolines (multi-core variant) ─────────────────
//
// These three handlers (SVCall, PendSV, DefaultHandler) are normally
// supplied by `scars_arch_cortex_m::impl_core_controller!` for
// single-core KHALs, but RP2350 needs the slot load to read the
// per-core `CURRENT_THREAD_CONTEXT[cpuid]` array.

/// SVCall handler. r0-r3 hold the syscall id + args at entry; the
/// inline slot load targets r4 (and r5 as scratch for the CPUID +
/// array dance), so r0-r3 are never disturbed before the kernel
/// handler call.
#[unsafe(naked)]
#[unsafe(export_name = "SVCall")]
#[unsafe(link_section = ".SVCall.user")]
pub unsafe extern "C" fn _scars_rp2350_svcall() {
    core::arch::naked_asm!(
        "push   {{r4, r5, r6, lr}}",        // r6 alignment padding
        // r4 = old Context* = CURRENT_THREAD_CONTEXT[CPUID].load()
        "ldr    r5, =0xd0000000",           // SIO->CPUID
        "ldr    r4, [r5]",                  // r4 = cpuid (0 or 1)
        "ldr    r5, ={array}",              // r5 = &CURRENT_THREAD_CONTEXT
        "ldr    r4, [r5, r4, lsl #2]",      // r4 = ARRAY[cpuid] (AtomicPtr value)
        "bl     _kernel_syscall_handler",
        "mrs    r1, psp",
        "str    r0, [r1]",                  // write return value to PSP[0]
        // r1 = new Context* (re-read; kernel handler may have updated it)
        "ldr    r5, =0xd0000000",
        "ldr    r1, [r5]",
        "ldr    r5, ={array}",
        "ldr    r1, [r5, r1, lsl #2]",
        "mov    r0, r4",                    // r0 = old Context*
        "pop    {{r4, r5, r6, lr}}",
        "b      _switch_context",
        array = sym CURRENT_THREAD_CONTEXT,
    );
}

/// PendSV handler. Same multi-core slot-load shape as SVCall, minus
/// the syscall return-value write.
#[unsafe(naked)]
#[unsafe(export_name = "PendSV")]
#[unsafe(link_section = ".PendSV.user")]
pub unsafe extern "C" fn _scars_rp2350_pendsv() {
    core::arch::naked_asm!(
        "push   {{r4, r5, r6, lr}}",
        "ldr    r5, =0xd0000000",
        "ldr    r4, [r5]",
        "ldr    r5, ={array}",
        "ldr    r4, [r5, r4, lsl #2]",      // r4 = old Context*
        "bl     _kernel_service_call_handler",
        "ldr    r5, =0xd0000000",
        "ldr    r1, [r5]",
        "ldr    r5, ={array}",
        "ldr    r1, [r5, r1, lsl #2]",      // r1 = new Context*
        "mov    r0, r4",
        "pop    {{r4, r5, r6, lr}}",
        "b      _switch_context",
        array = sym CURRENT_THREAD_CONTEXT,
    );
}

/// DefaultHandler. No context switch needed — just preserve lr
/// (EXC_RETURN) across the kernel handler call.
#[unsafe(naked)]
#[unsafe(export_name = "DefaultHandler")]
#[unsafe(link_section = ".DefaultHandler.user")]
pub unsafe extern "C" fn _scars_rp2350_default_handler() {
    core::arch::naked_asm!(
        "push   {{r4, lr}}", // r4 alignment padding
        "bl     _kernel_interrupt_handler",
        "pop    {{r4, lr}}",
        "bx     lr",
    );
}
