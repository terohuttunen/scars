use crate::context::TestContext;
use crate::error::TestError;
use crate::{TestHal, hal};
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use scars_fault::*;
use scars_khal::*;

impl CoreController for TestHal {
    type StackAlignment = A16;
    type Context = TestContext;
    type HardwareError = TestError;

    const NUM_CORES: usize = 1;

    #[inline(always)]
    fn current_core_id() -> u8 {
        0
    }

    #[inline(always)]
    fn pend_service_call_on(_core: u8) {
        <Self as CoreController>::pend_service_call()
    }

    fn start_first_thread(context: *mut Self::Context) -> ! {
        // Record idle as the current context, then run its body directly
        // on the host stack. With the test harness the idle body reaches
        // the test runner (`idle()` -> `test_main()` -> `test_succeed()`,
        // which exits the process). No actual context switch occurs.
        Self::set_current_thread_context(context);
        let ctx = unsafe { &*context };
        let main_fn: fn(Option<NonNull<u8>>) = unsafe { core::mem::transmute(ctx.main_fn) };
        main_fn(ctx.argument);
        unreachable!("idle thread returned in test harness");
    }

    fn on_abort() -> ! {
        std::process::abort()
    }

    fn on_exit(exit_code: i32) -> ! {
        std::process::exit(exit_code)
    }

    fn on_fault(info: &FaultInfo) -> ! {
        // A kernel fault during a unit test is a test failure: surface it
        // and exit non-zero rather than silently continuing.
        if let Some(loc) = info.location {
            std::eprintln!("Fault at {}: {}", loc, info.error);
        } else {
            std::eprintln!("Fault: {}", info.error);
        }
        for (i, frame) in info.context_iter().enumerate() {
            std::eprintln!("  {}: {}", i + 1, frame);
        }
        std::process::exit(101);
    }

    fn on_breakpoint() {}

    #[inline(always)]
    fn on_idle() {}

    fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        let rval = unsafe { Self::kernel_syscall_handler(id, arg0, arg1, arg2) };
        // Faithful tail-chain: a syscall that pended a reschedule has it
        // applied before control returns to the caller (as PendSV
        // tail-chains on hardware), unless the test opted into manual
        // pumping to observe the intermediate state.
        if !hal().manual_pump.load(Ordering::SeqCst) {
            crate::drain_service_calls();
        }
        rval
    }

    fn current_thread_context() -> *const Self::Context {
        hal().current_context.load(Ordering::SeqCst)
    }

    fn set_current_thread_context(context: *const Self::Context) {
        hal()
            .current_context
            .store(context as *mut _, Ordering::SeqCst);
        crate::record_switch(context);
    }

    fn pend_service_call() {
        hal().service_pending.store(true, Ordering::SeqCst);
    }

    fn clear_service_call() {
        hal().service_pending.store(false, Ordering::SeqCst);
    }
}
