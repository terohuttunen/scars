#![no_std]
use core::arch::{asm, global_asm};
use core::sync::atomic::AtomicPtr;
use cortex_m::register::basepri;
use cortex_m_rt::exception;
use scars_khal::*;
use unrecoverable_error::*;
pub use rtt_target::rprint as print;
pub use rtt_target::rprintln as println;
use rtt_target::rtt_init_print;

#[unsafe(no_mangle)]
pub static CURRENT_THREAD_CONTEXT: AtomicPtr<Context> = AtomicPtr::new(core::ptr::null_mut());

#[repr(C)]
#[derive(Debug)]
pub struct Context {
    // Caller saved registers r0, r1, r2, r3, r12, lr, pc are pushed
    // into thread stack on interrupt. Thread context stores all callee
    // saved registers.

    // Register
    // r4-r11     Local variables
    r4: u32,
    r5: u32,
    r6: u32,
    r7: u32,
    r8: u32,
    r9: u32,
    r10: u32,
    r11: u32,
    // Link register at the interrupt handler.
    // Contains information of what was stored in thread stack.
    lr: u32,

    // Stack top (r13) at offset 9 * 4
    sp: u32,

    // Thread current interrupt priority threshold at offset 10 * 4
    basepri: u32,

    // Callee saved FPU registers starting from offset 11 * 4
    s16: f32,
    s17: f32,
    s18: f32,
    s19: f32,
    s20: f32,
    s21: f32,
    s22: f32,
    s23: f32,
    s24: f32,
    s25: f32,
    s26: f32,
    s27: f32,
    s28: f32,
    s29: f32,
    s30: f32,
    s31: f32,
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
            (*context).lr = 0xFFFFFFFD;

            (*context).s16 = 0.0f32;
            (*context).s17 = 0.0f32;
            (*context).s18 = 0.0f32;
            (*context).s19 = 0.0f32;
            (*context).s20 = 0.0f32;
            (*context).s21 = 0.0f32;
            (*context).s22 = 0.0f32;
            (*context).s23 = 0.0f32;
            (*context).s24 = 0.0f32;
            (*context).s25 = 0.0f32;
            (*context).s26 = 0.0f32;
            (*context).s27 = 0.0f32;
            (*context).s28 = 0.0f32;
            (*context).s29 = 0.0f32;
            (*context).s30 = 0.0f32;
            (*context).s31 = 0.0f32;

            // Allocate exception frame from thread stack
            let frame_ptr = stack_ptr.sub(core::mem::size_of::<cortex_m_rt::ExceptionFrame>())
                as *mut cortex_m_rt::ExceptionFrame;
            (*context).sp = frame_ptr as u32;
            // TODO: initial priority
            (*context).basepri = basepri::read() as u32;

            (*frame_ptr).set_r0(argument.unwrap_or(core::ptr::null()) as u32);
            (*frame_ptr).set_r1(0);
            (*frame_ptr).set_r2(0);
            (*frame_ptr).set_r3(0);
            (*frame_ptr).set_r12(0);
            (*frame_ptr).set_lr(on_abort as u32);
            (*frame_ptr).set_pc(main_fn as u32);
            // TODO: disable FPU at thread startup because not pushing FPU context
            (*frame_ptr).set_xpsr(0x01000000);
        }
    }
}

#[repr(C)]
#[derive(PartialEq, Eq, Copy, Clone, Debug)]
pub enum FaultKind {
    HardFault = 3,
    MemManage = 4,
    BusFault = 5,
    UsageFault = 6,
}

#[derive(PartialEq, Eq, Copy, Clone, Debug, UnrecoverableError)]
#[unrecoverable_error("Cortex-M fault: {kind:?}")]
pub struct Fault {
    kind: FaultKind,
    frame: *const Context,
}

pub fn start_first_thread(idle_context: *mut Context) -> ! {
    unsafe {
        CURRENT_THREAD_CONTEXT.store(idle_context, core::sync::atomic::Ordering::SeqCst);
        asm!(
            // Read the thread stack pointer from the context
            "ldr r4, [r0, #9*4]",

            // Read PC from stack
            "ldr r5, [r4, #6*4]",

            // Read LR from stack
            "ldr lr, [r4, #5*4]",

            "ldmia r4, {{r0-r3, r12}}",

            // Pop exception frame from the stack
            "add r4, r4, #8*4",

            // Set process stack pointer to thread stack bottom
            "msr psp, r4",
            // Make sure that stack pointer is set before enabling use of it
            "isb",

            // Enable process stack pointer
            // FPU context is not active, because FPU registers were not stored in context init.
            "mov r4, #2",
            "msr control, r4",

            // Restore basepri register
            "ldr r4, [r0, #10 * 4]",
            "msr basepri, r4",
            "dsb",
            "isb",

            // Jump to thread main
            "bx r5",
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

pub fn on_error(error: &dyn UnrecoverableError) -> ! {
    println!("{}", error);

    on_exit(1);
}

pub fn on_breakpoint() {
    cortex_m::asm::bkpt()
}

pub fn on_idle() {
    cortex_m::asm::wfi();
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

#[macro_export]
macro_rules! impl_flow_controller {
    ($struct_name:ident) => {
        impl FlowController for $struct_name {
            type StackAlignment = scars_khal::A8;
            type Context = $crate::Context;
            type HardwareError = $crate::Fault;

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
            fn on_error(error: &dyn UnrecoverableError) -> ! {
                $crate::on_error(error)
            }

            #[inline(always)]
            fn on_breakpoint() {
                $crate::on_breakpoint()
            }

            #[inline(always)]
            fn on_idle() {
                $crate::on_idle();
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
        }
    };
}

global_asm!(
    ".cfi_sections .debug_frame
     .section .SVCall.user, \"ax\"
     .global SVCall
     .type SVCall,%function
     .thumb_func",
    ".cfi_startproc
    SVCall:",
    "push   {{r0, lr}}",
    "ldr    lr,=CURRENT_THREAD_CONTEXT",
    "ldr    lr, [lr]",
    "str    lr, [sp]",
    "bl     _private_kernel_syscall_handler",
    // Copy syscall return value in r0 to thread stack
    "mrs    r1, psp",
    "str    r0, [r1]",
    "pop    {{r0, lr}}",
    "ldr    r1,=CURRENT_THREAD_CONTEXT",
    "ldr    r1, [r1]",
    "b      _switch_context",
    ".cfi_endproc
     .size SVCall, . - SVCall",
);

global_asm!(
    ".cfi_sections .debug_frame
     .section ._switch_context.user, \"ax\"
     .global _switch_context
     .type _switch_context,%function
     .thumb_func",
    ".cfi_startproc
    _switch_context:",
    "cmp    r0, r1",
    "it     eq",
    "beq    0f",
    // Save callee saved registers
    "stmia  r0, {{r4-r11, lr}}",
    //"add    r2, r0, #11*4",
    //"tst    lr, #0x10",
    //"it     eq",
    //"vstmiaeq r2, {{s16-s31}}",

    // Store process stack pointer to context
    "mrs    r2, psp",
    "str    r2, [r0, #9 * 4]",
    // Store basepri register to context
    "mrs    r2, basepri",
    "str    r2, [r0, #10 * 4]",
    // Restore new context
    // Restore psp from context 'sp'
    "ldr    r2, [r1, #9 * 4]",
    "msr    psp, r2",
    // Restore callee saved registers
    "ldmia  r1, {{r4-r11, lr}}",
    //"add    r2, r1, #11*4",
    //"tst    lr, #0x10",
    //"it     eq",
    //"vldmiaeq r2, {{s16-s31}}",

    // Restore basepri
    "ldr    r2, [r1, #10 * 4]",
    "msr    basepri, r2",
    "dsb",
    "isb",
    "0:",
    "bx     lr",
    ".cfi_endproc
     .size _switch_context, . - _switch_context",
);

// All interrupts are by default handled by the common interrupt handler
global_asm!(
    ".cfi_sections .debug_frame
     .section .DefaultHandler.user, \"ax\"
     .global DefaultHandler
     .type DefaultHandler,%function
     .thumb_func",
    ".cfi_startproc
    DefaultHandler:",
    "ldr    r0,=CURRENT_THREAD_CONTEXT",
    "ldr    r0, [r0]",
    "push   {{r0, lr}}",
    "bl     _private_kernel_interrupt_handler",
    "pop    {{r0, lr}}",
    "ldr    r1,=CURRENT_THREAD_CONTEXT",
    "ldr    r1, [r1]",
    "b      _switch_context",
    ".cfi_endproc
     .size DefaultHandler, . - DefaultHandler",
);

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

#[exception]
unsafe fn MemoryManagement() -> ! {
    loop {}
}

#[exception]
unsafe fn BusFault() -> ! {
    loop {}
}

#[exception]
unsafe fn UsageFault() -> ! {
    loop {}
}

#[exception]
unsafe fn DebugMonitor() -> ! {
    loop {}
}

#[exception]
unsafe fn PendSV() {
    loop {}
}
