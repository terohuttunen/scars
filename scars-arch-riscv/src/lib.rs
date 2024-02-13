#![no_std]
use bit_field::BitField;
use core::arch::{asm, global_asm};
use scars_hal::{ContextInfo, ExceptionInfo, FlowController, KernelCallbacks};

global_asm!(include_str!("trap.S"));

extern "C" {
    static _global_pointer: usize;
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RISCVTrapFrame {
    // Register ABI-Name Description
    // x0       zero     Hard-wired zero
    // x1       ra       Return address
    // x2       sp       Stack pointer
    // x3       gp       Global pointer
    // x4       tp       Thread pointer
    // x5-7     t0-2     Temporaries
    // x8       s0/fp    Saved register/frame pointer
    // x9       s1       Saved register
    // x10-11   a0-1     Function arguments/return values
    // x12-17   a2-7     Function arguments
    // x18-27   s2-11    Saved registers
    // x28-31   t3-6     Temporaries
    pub gp_regs: [usize; 32], // 0..31
    // f0-7     ft0-7    FP temporaries
    // f8-9     fs0-1    FP saved registers
    // f10-11   fa0-1    FP arguments/return values
    // f12-17   fa2-7    FP arguments
    // f18-27   fs2-11   FP saved registers
    // f28-31   ft8-11   FP temporaries
    pub fpu_regs: [usize; 32], // 32..63
    pub pc: usize,             // 64
    pub mstatus: usize,        // 65
}

impl RISCVTrapFrame {
    pub const fn new() -> RISCVTrapFrame {
        RISCVTrapFrame {
            gp_regs: [0; 32],
            fpu_regs: [0; 32],
            pc: 0,
            mstatus: 0,
        }
    }

    pub const fn new_with_init(
        ra: usize,
        sp: usize,
        pc: usize,
        gp: usize,
        arg: usize,
        mstatus: usize,
    ) -> RISCVTrapFrame {
        let mut context = RISCVTrapFrame {
            gp_regs: [0; 32],
            fpu_regs: [0; 32],
            pc,
            mstatus,
        };
        context.gp_regs[1] = ra;
        context.gp_regs[2] = sp;
        context.gp_regs[3] = gp;
        context.gp_regs[10] = arg;
        context
    }
}

impl ContextInfo for RISCVTrapFrame {
    fn stack_top_ptr(&self) -> *const u8 {
        self.gp_regs[2] as *const u8
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
        // when entering the user mode task.
        mstatus.set_bit(7, true);
        // Set machine previous privilege bits to stay in machine mode
        mstatus.set_bits(11..13, riscv::register::mstatus::MPP::Machine as usize);

        *context = RISCVTrapFrame::new_with_init(
            main_fn as usize,
            stack_ptr as usize,
            main_fn as usize,
            unsafe { &_global_pointer as *const usize as usize },
            argument.map(|a| a as usize).unwrap_or(0),
            mstatus,
        )
    }
}

#[no_mangle]
// 'a not 'static because we might have context in stack from nested interrupt
extern "C" fn kernel_trap_handler<'a>(
    mepc: usize,
    mtval: usize,
    mcause: usize,
    _hartid: usize,
    _mstatus: usize,
    context: &'a mut RISCVTrapFrame,
    _nested: u32,
) -> &'a RISCVTrapFrame {
    RISCV32::save_additional_context();
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
                context.pc = mepc + 4;
                // SAFETY: Called from interrupt handler with interrupts disabled
                let rval = unsafe {
                    RISCV32::kernel_syscall_handler(
                        context.gp_regs[17],
                        context.gp_regs[10],
                        context.gp_regs[11],
                    )
                };
                context.gp_regs[10] = rval;
            }
            code => {
                let kind = ExceptionKind::try_from(code).unwrap_or(ExceptionKind::Unknown);
                let exception = RISCVException::new(kind, mtval, context);
                RISCV32::kernel_exception_handler(&exception);
            }
        }
    };
    RISCV32::restore_additional_context();
    RISCV32::current_task_context()
}

#[derive(PartialEq, Eq, Copy, Clone)]
pub enum ExceptionKind {
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

impl TryFrom<usize> for ExceptionKind {
    type Error = ();
    fn try_from(value: usize) -> Result<ExceptionKind, ()> {
        match value {
            0 => Ok(ExceptionKind::InstructionAddressMisaligned),
            1 => Ok(ExceptionKind::InstructionAddressFault),
            2 => Ok(ExceptionKind::IllegalInstruction),
            3 => Ok(ExceptionKind::Breakpoint),
            4 => Ok(ExceptionKind::LoadAddressMisaligned),
            5 => Ok(ExceptionKind::LoadAccessFault),
            6 => Ok(ExceptionKind::StoreAddressMisaligned),
            7 => Ok(ExceptionKind::StoreAccessFault),
            _ => Err(()),
        }
    }
}

impl ExceptionKind {
    pub fn name(&self) -> &'static str {
        match self {
            ExceptionKind::InstructionAddressMisaligned => "InstructionAddressMisaligned",
            ExceptionKind::InstructionAddressFault => "InstructionAddressFault",
            ExceptionKind::IllegalInstruction => "IllegalInstruction",
            ExceptionKind::Breakpoint => "Breakpoint",
            ExceptionKind::LoadAddressMisaligned => "LoadAddressMisaligned",
            ExceptionKind::LoadAccessFault => "LoadAccessFault",
            ExceptionKind::StoreAddressMisaligned => "StoreAddressMisaligned",
            ExceptionKind::StoreAccessFault => "StoreAccessFault",
            ExceptionKind::Unknown => "Unknown",
        }
    }
}

pub struct RISCVException {
    kind: ExceptionKind,
    mtval: usize,
    frame: *const RISCVTrapFrame,
}

impl RISCVException {
    pub fn new(kind: ExceptionKind, mtval: usize, frame: &RISCVTrapFrame) -> RISCVException {
        RISCVException {
            kind,
            mtval,
            frame: frame as *const RISCVTrapFrame,
        }
    }
}

impl ExceptionInfo<RISCVTrapFrame> for RISCVException {
    fn code(&self) -> usize {
        self.kind as usize
    }

    fn name(&self) -> &'static str {
        self.kind.name()
    }

    fn address(&self) -> usize {
        if self.kind == ExceptionKind::Breakpoint {
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
    fn _start_first_task(idle_context: *mut ()) -> !;
}

pub struct RISCV32 {}

impl FlowController for RISCV32 {
    type Context = RISCVTrapFrame;
    type Exception = RISCVException;

    fn start_first_task(idle_context: *mut Self::Context) -> ! {
        unsafe { _start_first_task(idle_context as *mut _) }
    }

    fn abort() -> ! {
        // Abort by disabling interrupts and then waiting for an interrupt
        unsafe {
            riscv::interrupt::disable();
            loop {
                riscv::asm::wfi();
            }
        }
    }

    fn breakpoint() {
        unsafe { riscv::asm::ebreak() };
    }

    #[inline(always)]
    fn idle() {
        unsafe { riscv::asm::wfi() };
    }

    fn syscall(id: usize, arg0: usize, arg1: usize) -> usize {
        let rval: usize;
        unsafe {
            asm!(
                "ecall",
                in("a7") id,
                inout("a0") arg0 => rval,
                in("a1") arg1,
                options(nostack)
            );
        }
        rval
    }
}
