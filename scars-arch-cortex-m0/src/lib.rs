#![no_std]
pub mod nvic;

use core::arch::{asm, naked_asm};
use core::sync::atomic::AtomicPtr;
use cortex_m_rt::exception;
use scars_fault::*;
use scars_khal::*;

#[unsafe(no_mangle)]
pub static CURRENT_THREAD_CONTEXT: AtomicPtr<Context> = AtomicPtr::new(core::ptr::null_mut());

/// ARMv6-M thread context. Smaller than the ARMv7-M cousin — no FPU
/// register slots, and the per-thread interrupt-mask state is just
/// the saved PRIMASK bit (M0 has no BASEPRI; PRIMASK is the whole
/// masking state).
///
/// Layout is referenced by offset from the inline asm below:
/// r4..r11 at 0..32, lr at 32, sp at 36, primask at 40.
#[repr(C)]
#[derive(Debug)]
pub struct Context {
    r4: u32,
    r5: u32,
    r6: u32,
    r7: u32,
    r8: u32,
    r9: u32,
    r10: u32,
    r11: u32,
    lr: u32,
    sp: u32,
    primask: u32,
}

impl ContextInfo for Context {
    fn stack_top_ptr(&self) -> *const u8 {
        self.sp as *const _
    }

    unsafe fn init(
        _name: &'static str,
        main_fn: *const (),
        argument: Option<*const u8>,
        stack_ptr: *const u8,
        _stack_size: usize,
        context: *mut Self,
    ) {
        unsafe {
            (*context).r4 = 0;
            (*context).r5 = 0;
            (*context).r6 = 0;
            (*context).r7 = 0;
            (*context).r8 = 0;
            (*context).r9 = 0;
            (*context).r10 = 0;
            (*context).r11 = 0;
            // EXC_RETURN cookie: thread mode, PSP, basic frame, no FP
            // (bit 4 set ⇒ no FP frame; ARMv6-M has no FPU so always
            // basic). Same encoding as on M3.
            (*context).lr = 0xFFFFFFFD;

            // Allocate a basic exception frame at the top of the
            // thread stack; the CPU will pop it on the first
            // exception-return into this thread.
            let frame_ptr = stack_ptr.sub(core::mem::size_of::<cortex_m_rt::ExceptionFrame>())
                as *mut cortex_m_rt::ExceptionFrame;
            (*context).sp = frame_ptr as u32;
            // Capture current PRIMASK so the new thread starts with
            // the same mask state as its creator (same shape as the
            // M3+ crate capturing BASEPRI at thread init). 0 = IRQs
            // enabled, 1 = disabled.
            (*context).primask = !cortex_m::register::primask::read().is_active() as u32;

            (*frame_ptr).set_r0(argument.unwrap_or(core::ptr::null()) as u32);
            (*frame_ptr).set_r1(0);
            (*frame_ptr).set_r2(0);
            (*frame_ptr).set_r3(0);
            (*frame_ptr).set_r12(0);
            (*frame_ptr).set_lr(on_abort as u32);
            (*frame_ptr).set_pc(main_fn as u32);
            (*frame_ptr).set_xpsr(0x01000000);
        }
    }
}

#[repr(C)]
#[derive(PartialEq, Eq, Copy, Clone, Debug, defmt::Format)]
pub enum FaultKind {
    HardFault = 3,
}

#[derive(PartialEq, Eq, Copy, Clone, Fault)]
#[fault("Cortex-M0 fault: {kind:?}")]
pub struct CortexMFault {
    kind: FaultKind,
    frame: *const Context,
}

#[derive(FaultContext)]
#[fault("cortex-m0 frame at {frame:?}")]
pub struct CortexMContext {
    pub frame: *const Context,
}

pub fn start_first_thread(idle_context: *mut Context) -> ! {
    unsafe {
        CURRENT_THREAD_CONTEXT.store(idle_context, core::sync::atomic::Ordering::SeqCst);
        asm!(
            // r0 = idle_context on entry; preserve it in r6 since the
            // exception-frame pop below clobbers r0–r3.
            "mov    r6, r0",

            // r4 = context.sp = prebuilt exception frame
            "ldr    r4, [r6, #9*4]",

            // r5 = frame.pc (idle main_fn)
            "ldr    r5, [r4, #6*4]",

            // Pop r0–r3 from the frame so the thread sees its argument
            // in r0 on entry. (No ldmia with high registers on ARMv6-M.)
            "ldr    r0, [r4, #0]",
            "ldr    r1, [r4, #4]",
            "ldr    r2, [r4, #8]",
            "ldr    r3, [r4, #12]",

            // Discard the rest of the frame (r12, lr, pc, xpsr) by
            // advancing PSP past the whole frame.
            "adds   r4, r4, #8*4",
            "msr    psp, r4",
            "isb",

            // Switch to PSP, thread mode.
            "movs   r4, #2",
            "msr    control, r4",
            "isb",

            // Restore the thread's saved PRIMASK.
            "ldr    r2, [r6, #10*4]",
            "msr    primask, r2",

            // Jump to thread main.
            "bx     r5",
            in("r0") idle_context,
            options(noreturn)
        )
    }
}

pub fn on_abort() -> ! {
    #[cfg(feature = "semihosting")]
    semihosting::process::abort();

    #[cfg(not(feature = "semihosting"))]
    on_exit(1)
}

pub fn on_exit(exit_code: i32) -> ! {
    #[cfg(feature = "semihosting")]
    semihosting::process::exit(exit_code);

    #[cfg(not(feature = "semihosting"))]
    loop {
        cortex_m::asm::wfi();
    }
}

pub fn on_fault(info: &FaultInfo) -> ! {
    let plat = CortexMContext {
        frame: CURRENT_THREAD_CONTEXT.load(core::sync::atomic::Ordering::SeqCst) as *const _,
    };
    let plat_node = FaultContextNode {
        frame: &plat,
        next: info.context,
    };
    let info = info.with_context(&plat_node);

    if let Some(loc) = info.location {
        defmt::error!("Fault at {}:{}: {}", loc.file(), loc.line(), info.error);
    } else {
        defmt::error!("Fault: {}", info.error);
    }
    for (i, frame) in info.context_iter().enumerate() {
        defmt::error!("  {}: {}", i + 1, frame);
    }
    defmt::panic!()
}

pub fn on_breakpoint() {
    cortex_m::asm::bkpt()
}

pub fn on_idle() {
    cortex_m::asm::wfi();
}

/// See `scars_arch_cortex_m::on_idle_active`. STM32F0 (Cortex-M0)
/// will likely run into the same probe-rs-RTT streaming issue as F1
/// when wfi sleeps the core; KHAL can opt into this if needed.
pub fn on_idle_active() {
    cortex_m::asm::nop();
}

pub fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let rval: usize;
    unsafe {
        asm!(
            "svc #0",
            inout("r0") id => rval,
            in("r1") arg0,
            in("r2") arg1,
            in("r3") arg2,
            options(nostack)
        );
    }
    rval
}

pub fn pend_service_call() {
    const SCB_ICSR_PENDSVSET: u32 = 1 << 28;
    unsafe {
        let scb = &*cortex_m::peripheral::SCB::PTR;
        scb.icsr.modify(|r| r | SCB_ICSR_PENDSVSET);
    }
}

pub fn clear_service_call() {
    const SCB_ICSR_PENDSVCLR: u32 = 1 << 27;
    unsafe {
        let scb = &*cortex_m::peripheral::SCB::PTR;
        scb.icsr.modify(|r| r | SCB_ICSR_PENDSVCLR);
    }
}

/// Pin every kernel-owned exception (SVCall, PendSV) to the lowest
/// NVIC priority. The kernel-priority-is-lowest invariant is a
/// scars-wide assumption; on ARMv6-M with no BASEPRI it's also
/// load-bearing — see `nvic::Nvic::set_threshold`.
pub fn init_kernel_priorities(scb: &mut cortex_m::peripheral::SCB) {
    unsafe {
        scb.set_priority(cortex_m::peripheral::scb::SystemHandler::SVCall, 0xFF);
        scb.set_priority(cortex_m::peripheral::scb::SystemHandler::PendSV, 0xFF);
    }
}

#[macro_export]
macro_rules! impl_core_controller {
    ($struct_name:ident) => {
        $crate::impl_core_controller!($struct_name, on_idle = $crate::on_idle());
    };
    ($struct_name:ident, on_idle = $on_idle:expr) => {
        impl ::scars_khal::CoreController for $struct_name {
            type StackAlignment = ::scars_khal::A8;
            type Context = $crate::Context;
            type HardwareError = $crate::CortexMFault;

            const NUM_CORES: usize = 1;

            #[inline(always)]
            fn current_core_id() -> u8 {
                0
            }

            #[inline(always)]
            fn pend_service_call_on(_core: u8) {
                $crate::pend_service_call()
            }

            #[inline(always)]
            fn start_first_thread(idle_context: *mut Self::Context) -> ! {
                $crate::start_first_thread(idle_context)
            }

            #[inline(always)]
            fn on_abort() -> ! {
                $crate::on_abort()
            }

            #[inline(always)]
            fn on_exit(exit_code: i32) -> ! {
                $crate::on_exit(exit_code)
            }

            #[inline(always)]
            fn on_fault(info: &::scars_khal::FaultInfo) -> ! {
                $crate::on_fault(info)
            }

            #[inline(always)]
            fn on_breakpoint() {
                $crate::on_breakpoint()
            }

            #[inline(always)]
            fn on_idle() {
                $on_idle;
            }

            #[inline(always)]
            fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
                $crate::syscall(id, arg0, arg1, arg2)
            }

            #[inline(always)]
            fn current_thread_context() -> *const Self::Context {
                CURRENT_THREAD_CONTEXT.load(core::sync::atomic::Ordering::Relaxed)
            }

            #[inline(always)]
            fn set_current_thread_context(context: *const Self::Context) {
                CURRENT_THREAD_CONTEXT
                    .store(context as *mut _, core::sync::atomic::Ordering::Relaxed);
            }

            #[inline(always)]
            fn pend_service_call() {
                $crate::pend_service_call()
            }

            #[inline(always)]
            fn clear_service_call() {
                $crate::clear_service_call()
            }
        }
    };
}

/// SVC exception handler — ARMv6-M version. The handler must call
/// `_kernel_syscall_handler(id, arg0, arg1, arg2)` with the original
/// r0–r3 from the SVC site, and on return tail-jump to `_switch_context`
/// with r0 = OLD context, r1 = NEW context.
///
/// The M3+ analogue achieves this by `push {r0, lr}` and then
/// `str lr, [sp]` (overwriting the saved-r0 slot with the OLD context
/// pointer, while keeping r0-the-register intact for the handler).
/// ARMv6-M cannot encode `str` with `lr` as the source register, so
/// the trick here is the same shape but uses `r4` as the staging
/// register, with the thread's original `r4` saved/restored on the
/// stack around the use.
#[unsafe(naked)]
#[unsafe(export_name = "SVCall")]
#[unsafe(link_section = ".SVCall.user")]
pub unsafe extern "C" fn svcall() {
    naked_asm!(
        // Stack the syscall id and EXC_RETURN cookie. After this:
        //   SP[0] = r0_saved (= syscall id), SP[4] = lr_saved.
        // r0 (the register) still holds the syscall id for the handler.
        "push   {{r0, lr}}",

        // Use r4 as a scratch to overwrite the SP[0] slot with the
        // OLD context pointer. Save r4 first so we don't lose the
        // thread's value of it.
        "push   {{r4}}",
        "ldr    r4, ={ctx}",
        "ldr    r4, [r4]",
        "str    r4, [sp, #4]",
        "pop    {{r4}}",

        // r0–r3 still hold the original syscall args; call the handler.
        "bl     _kernel_syscall_handler",
        // r0 = syscall return value; r4–r11 preserved by AAPCS.

        // Write the return value into the SVC exception frame's r0
        // slot so the thread observes it after EXC_RETURN.
        "mrs    r1, psp",
        "str    r0, [r1]",

        // Recover OLD context and EXC_RETURN from the stack.
        "pop    {{r0, r3}}",
        "mov    lr, r3",

        // r1 = currently-installed thread context (may differ from
        // OLD if the syscall switched threads).
        "ldr    r1, ={ctx}",
        "ldr    r1, [r1]",

        "b      _switch_context",
        ctx = sym CURRENT_THREAD_CONTEXT,
    );
}

/// Context-switch primitive. Entered via `b _switch_context` from the
/// tail of an exception handler. r0 = outgoing Context*, r1 = incoming
/// Context*. Returns via `bx lr` which performs the parent exception's
/// EXC_RETURN.
///
/// ARMv6-M caveats that drive the shape:
/// - `stmia`/`ldmia` register lists are limited to {r0–r7} and cannot
///   include `lr`; r8–r11 and lr are moved through low registers.
/// - No IT blocks; the threshold-application branch uses explicit
///   `beq`/`b` instead of conditional execution.
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = "._switch_context.user")]
pub unsafe extern "C" fn _switch_context(_old: *mut Context, _new: *const Context) {
    naked_asm!(
        // Fast path: same context, just return.
        "cmp    r0, r1",
        "beq    9f",
        // --- Save outgoing context (r0 points to outgoing Context) ---
        // r4–r7 first.
        "str    r4, [r0, #0]",
        "str    r5, [r0, #4]",
        "str    r6, [r0, #8]",
        "str    r7, [r0, #12]",
        // r8–r11 via low-register staging.
        "mov    r4, r8",
        "mov    r5, r9",
        "mov    r6, r10",
        "mov    r7, r11",
        "str    r4, [r0, #16]",
        "str    r5, [r0, #20]",
        "str    r6, [r0, #24]",
        "str    r7, [r0, #28]",
        // lr (EXC_RETURN cookie) via r4.
        "mov    r4, lr",
        "str    r4, [r0, #32]",
        // psp into context.sp.
        "mrs    r4, psp",
        "str    r4, [r0, #36]",
        // primask into context.primask.
        "mrs    r4, primask",
        "str    r4, [r0, #40]",
        // --- Restore incoming context (r1 points to incoming Context) ---
        // primask from context.primask.
        "ldr    r4, [r1, #40]",
        "msr    primask, r4",
        // psp from context.sp.
        "ldr    r4, [r1, #36]",
        "msr    psp, r4",
        // r8–r11 first, while we can still use r4–r7 as scratch.
        "ldr    r4, [r1, #16]",
        "mov    r8, r4",
        "ldr    r4, [r1, #20]",
        "mov    r9, r4",
        "ldr    r4, [r1, #24]",
        "mov    r10, r4",
        "ldr    r4, [r1, #28]",
        "mov    r11, r4",
        // lr.
        "ldr    r4, [r1, #32]",
        "mov    lr, r4",
        // r4–r7 last, since we used them as scratch above.
        "ldr    r4, [r1, #0]",
        "ldr    r5, [r1, #4]",
        "ldr    r6, [r1, #8]",
        "ldr    r7, [r1, #12]",
        "dsb",
        "isb",
        "9:",
        "bx     lr",
    );
}

/// Common entry for any IRQ that doesn't have its own dedicated
/// handler. Routes through `_kernel_interrupt_handler` which looks up
/// the per-IRQ closure registered via `InterruptHandler`.
#[unsafe(naked)]
#[unsafe(export_name = "DefaultHandler")]
#[unsafe(link_section = ".DefaultHandler.user")]
pub unsafe extern "C" fn default_handler() {
    naked_asm!(
        // Push lr via low register; ARMv6-M cannot push lr+r0 in one
        // shot.
        "mov    r1, lr",
        "push   {{r0, r1}}",
        "bl     _kernel_interrupt_handler",
        "pop    {{r0, r1}}",
        "mov    lr, r1",
        "bx     lr",
    );
}

#[exception]
unsafe fn HardFault(_frame: &::cortex_m_rt::ExceptionFrame) -> ! {
    loop {}
}

#[exception]
unsafe fn SysTick() -> ! {
    loop {}
}

#[exception]
unsafe fn NonMaskableInt() -> ! {
    loop {}
}

/// PendSV handler — kernel service-call / context-switch trampoline.
#[unsafe(naked)]
#[unsafe(export_name = "PendSV")]
#[unsafe(link_section = ".PendSV.user")]
pub unsafe extern "C" fn pendsv() {
    naked_asm!(
        "ldr    r0, ={ctx}",
        "ldr    r0, [r0]",
        "mov    r1, lr",
        "push   {{r0, r1}}",
        "bl     _kernel_service_call_handler",
        "pop    {{r0, r1}}",
        "mov    lr, r1",
        "ldr    r1, ={ctx}",
        "ldr    r1, [r1]",
        "b      _switch_context",
        ctx = sym CURRENT_THREAD_CONTEXT,
    );
}
