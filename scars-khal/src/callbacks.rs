use super::FlowController;

mod private {
    unsafe extern "Rust" {
        pub unsafe fn _private_kernel_wakeup_handler();

        pub unsafe fn _private_kernel_interrupt_handler();

        pub unsafe fn _private_kernel_syscall_handler(
            id: usize,
            arg0: usize,
            arg1: usize,
            arg2: usize,
        ) -> usize;

        pub unsafe fn _private_hardware_exception_handler(exception: *const u8) -> !;

        pub unsafe fn _private_current_thread_context() -> *const ();
    }
}

pub trait KernelCallbacks<Context, Exception> {
    #[inline(always)]
    unsafe fn kernel_wakeup_handler() {
        unsafe { private::_private_kernel_wakeup_handler() }
    }

    /// SAFETY: Must be called from interrupt handler with interrupts disabled.
    #[inline(always)]
    unsafe fn kernel_interrupt_handler() {
        unsafe { private::_private_kernel_interrupt_handler() }
    }

    /// SAFETY: Must be called from interrupt handler with interrupts disabled.
    #[inline(always)]
    unsafe fn kernel_syscall_handler(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        unsafe { private::_private_kernel_syscall_handler(id, arg0, arg1, arg2) }
    }

    #[inline(always)]
    fn kernel_exception_handler(exception: &Exception) -> ! {
        unsafe {
            private::_private_hardware_exception_handler(exception as *const Exception as *const u8)
        }
    }
}

impl<T> KernelCallbacks<T::Context, T::Fault> for T where T: FlowController {}
