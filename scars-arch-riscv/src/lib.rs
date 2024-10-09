#![no_std]
use bit_field::BitField;
use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicPtr, Ordering};
use scars_khal::*;

global_asm!(include_str!("trap.S"));

extern "C" {
    static CURRENT_THREAD_CONTEXT: AtomicPtr<RISCVTrapFrame>;
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RISCVTrapFrame {
    // Offset Register ABI-Name Description                         Saver
    // --     x0       zero     Hard-wired zero                     --
    //          x0 is not saved in the context
    // 0      x1       ra       Return address                      Caller
    // 1      x2       sp       Stack pointer                       Callee
    // --     x3       gp       Global pointer                      --
    // --     x4       tp       Thread pointer                      --
    //         x3 and x4 are not saved in the context
    // 2-4    x5-7     t0-2     Temporaries                         Caller
    // 5      x8       s0/fp    Saved register/frame pointer        Callee
    // 6      x9       s1       Saved register                      Callee
    // 7-8    x10-11   a0-1     Function arguments/return values    Caller
    // 9-14   x12-17   a2-7     Function arguments                  Caller
    // 15-24  x18-27   s2-11    Saved registers                     Callee
    // 25-28  x28-31   t3-6     Temporaries                         Caller
    gp_regs: [usize; 29],
    pub pc: usize,      // Offset 29
    pub mstatus: usize, // Offset 30
    pub interrupt_threshold: usize,
    // f0-7     ft0-7    FP temporaries
    // f8-9     fs0-1    FP saved registers
    // f10-11   fa0-1    FP arguments/return values
    // f12-17   fa2-7    FP arguments
    // f18-27   fs2-11   FP saved registers
    // f28-31   ft8-11   FP temporaries
    //pub fpu_regs: [usize; 32], // 32..63
}

impl ContextInfo for RISCVTrapFrame {
    fn stack_top_ptr(&self) -> *const u8 {
        self.gp_regs[1] as *const u8
    }

    unsafe fn init(
        _name: &'static str,
        main_fn: *const (),
        argument: Option<*const u8>,
        stack_ptr: *const u8,
        _stack_size: usize,
        context: *mut Self,
    ) {
        let mut mstatus: usize = 0;
        // Set machine previous interrupt enable to enable user mode interrupts
        // when entering the user mode thread.
        mstatus.set_bit(7, true);
        // Set machine previous privilege bits to stay in machine mode
        mstatus.set_bits(11..13, riscv::register::mstatus::MPP::Machine as usize);

        (*context).gp_regs = [0; 29];
        (*context).gp_regs[0] = abort as usize;
        (*context).gp_regs[1] = stack_ptr as usize;
        (*context).gp_regs[7] = argument.map(|a| a as usize).unwrap_or(0);
        (*context).pc = main_fn as usize;
        (*context).mstatus = mstatus;
        (*context).interrupt_threshold = 0;
    }
}

#[no_mangle]
// 'a not 'static because we might have context in stack from nested interrupt
extern "C" fn kernel_trap_handler<'a>(mepc: usize, mtval: usize, mcause: usize) {
    let is_async: bool = (mcause >> 31) & 1 == 1;
    let exception_code: usize = mcause & ((1 << 30) - 1);
    if is_async {
        // Interrupt
        match exception_code {
            // Machine timer interrupt
            // SAFETY: Called from interrupt handler with interrupts disabled
            7 => unsafe { RISCV32::kernel_wakeup_handler() },
            // Machine external interrupt
            // SAFETY: Called from interrupt handler with interrupts disabled
            11 => unsafe { RISCV32::kernel_interrupt_handler() },
            _ => RISCV32::abort(),
        }
    } else {
        // Exception
        match exception_code {
            // Environment call from U|S|M-mode
            8 | 9 | 11 => {
                let context = unsafe { &mut *CURRENT_THREAD_CONTEXT.load(Ordering::SeqCst) };
                context.pc = mepc + 4;
                // SAFETY: Called from interrupt handler with interrupts disabled
                let rval = unsafe {
                    RISCV32::kernel_syscall_handler(
                        context.gp_regs[14], // a7
                        context.gp_regs[7],  // a0
                        context.gp_regs[8],  // a1
                    )
                };
                context.gp_regs[10] = rval;
            }
            code => {
                let kind = FaultKind::try_from(code).unwrap_or(FaultKind::Unknown);
                let context = unsafe { &mut *CURRENT_THREAD_CONTEXT.load(Ordering::SeqCst) };
                let exception = RISCFault::new(kind, mtval, context);
                RISCV32::kernel_exception_handler(&exception);
            }
        }
    };
}

#[derive(PartialEq, Eq, Copy, Clone)]
pub enum FaultKind {
    InstructionAddressMisaligned = 0,
    InstructionAddressFault = 1,
    IllegalInstruction = 2,
    Breakpoint = 3,
    LoadAddressMisaligned = 4,
    LoadAccessFault = 5,
    StoreAddressMisaligned = 6,
    StoreAccessFault = 7,
    // EnvironmentCallFromUMode = 8,
    // EnvironmentCallFromSMode = 9,
    // Reserved = 10,
    // EnvironmentCallFromMMode = 11,
    //InstructionPageFault = 12,
    //LoadPageFault = 13,
    // Reserved2 = 14,
    //StorePageFault = 15,
    Unknown = 255,
}

impl TryFrom<usize> for FaultKind {
    type Error = ();
    fn try_from(value: usize) -> Result<FaultKind, ()> {
        match value {
            0 => Ok(FaultKind::InstructionAddressMisaligned),
            1 => Ok(FaultKind::InstructionAddressFault),
            2 => Ok(FaultKind::IllegalInstruction),
            3 => Ok(FaultKind::Breakpoint),
            4 => Ok(FaultKind::LoadAddressMisaligned),
            5 => Ok(FaultKind::LoadAccessFault),
            6 => Ok(FaultKind::StoreAddressMisaligned),
            7 => Ok(FaultKind::StoreAccessFault),
            _ => Err(()),
        }
    }
}

impl FaultKind {
    pub fn name(&self) -> &'static str {
        match self {
            FaultKind::InstructionAddressMisaligned => "InstructionAddressMisaligned",
            FaultKind::InstructionAddressFault => "InstructionAddressFault",
            FaultKind::IllegalInstruction => "IllegalInstruction",
            FaultKind::Breakpoint => "Breakpoint",
            FaultKind::LoadAddressMisaligned => "LoadAddressMisaligned",
            FaultKind::LoadAccessFault => "LoadAccessFault",
            FaultKind::StoreAddressMisaligned => "StoreAddressMisaligned",
            FaultKind::StoreAccessFault => "StoreAccessFault",
            FaultKind::Unknown => "Unknown",
        }
    }
}

pub struct RISCFault {
    kind: FaultKind,
    mtval: usize,
    frame: *const RISCVTrapFrame,
}

impl RISCFault {
    pub fn new(kind: FaultKind, mtval: usize, frame: &RISCVTrapFrame) -> RISCFault {
        RISCFault {
            kind,
            mtval,
            frame: frame as *const RISCVTrapFrame,
        }
    }
}

impl FaultInfo<RISCVTrapFrame> for RISCFault {
    fn code(&self) -> usize {
        self.kind as usize
    }

    fn name(&self) -> &'static str {
        self.kind.name()
    }

    fn address(&self) -> usize {
        if self.kind == FaultKind::Breakpoint {
            // Breakpoint sets mtval to zero, but pc is the address of interest
            unsafe { *self.frame }.pc
        } else {
            self.mtval
        }
    }

    fn context(&self) -> &RISCVTrapFrame {
        unsafe { &*self.frame }
    }
}

extern "C" {
    fn _start_first_thread(idle_context: *mut ()) -> !;
}

pub struct RISCV32 {}

fn abort() -> ! {
    // Abort by disabling interrupts and then waiting for an interrupt
    unsafe {
        riscv::interrupt::disable();
        loop {
            riscv::asm::wfi();
        }
    }
}

impl FlowController for RISCV32 {
    type StackAlignment = A16;
    type Context = RISCVTrapFrame;
    type Fault = RISCFault;

    fn start_first_thread(idle_context: *mut Self::Context) -> ! {
        unsafe { _start_first_thread(idle_context as *mut _) }
    }

    #[inline(always)]
    fn abort() -> ! {
        crate::abort()
    }

    fn breakpoint() {
        unsafe { riscv::asm::ebreak() };
    }

    #[inline(always)]
    fn idle() {
        unsafe { riscv::asm::wfi() };
    }

    fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        let rval: usize;
        unsafe {
            asm!(
                "ecall",
                in("a7") id,
                inout("a0") arg0 => rval,
                in("a1") arg1,
                in("a2") arg2,
                options(nostack)
            );
        }
        rval
    }
}
