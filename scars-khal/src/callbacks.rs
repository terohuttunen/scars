use super::FlowController;
use unrecoverable_error::UnrecoverableError;

mod private {
    use unrecoverable_error::UnrecoverableError;
    unsafe extern "Rust" {
        pub unsafe fn _private_kernel_wakeup_handler();

        pub unsafe fn _private_kernel_interrupt_handler();

        pub unsafe fn _private_kernel_syscall_handler(
            id: usize,
            arg0: usize,
            arg1: usize,
            arg2: usize,
        ) -> usize;

        pub unsafe fn _private_hardware_exception_handler(error: &dyn UnrecoverableError) -> !;

        pub unsafe fn _private_current_thread_context() -> *const ();
    }
}

pub trait KernelCallbacks<Context, Exception> {
    #[inline(always)]
    unsafe fn kernel_wakeup_handler() {
        unsafe { private::_private_kernel_wakeup_handler() }
    }

    /// This function is called by the kernel hardware abstraction layer when an
    /// interrupt is pending. A pending interrupt is claimed by the kernel interrupt
    /// handler and completed before returning from this function.
    #[inline(always)]
    unsafe fn kernel_interrupt_handler() {
        unsafe { private::_private_kernel_interrupt_handler() }
    }

    #[inline(always)]
    unsafe fn kernel_syscall_handler(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        unsafe { private::_private_kernel_syscall_handler(id, arg0, arg1, arg2) }
    }

    #[inline(always)]
    fn kernel_exception_handler(error: &dyn UnrecoverableError) -> ! {
        unsafe { private::_private_hardware_exception_handler(error) }
    }
}

impl<T> KernelCallbacks<T::Context, T::HardwareError> for T where T: FlowController {}
