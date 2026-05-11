use super::FlowController;
use scars_fault::Fault;

mod private {
    use scars_fault::Fault;
    unsafe extern "Rust" {
        pub unsafe fn _kernel_wakeup_handler();

        pub unsafe fn _kernel_interrupt_handler();

        pub unsafe fn _kernel_syscall_handler(
            id: usize,
            arg0: usize,
            arg1: usize,
            arg2: usize,
        ) -> usize;

        pub unsafe fn _hardware_exception_handler(error: &dyn Fault) -> !;

        pub unsafe fn _current_thread_context() -> *const ();

        pub unsafe fn _kernel_service_call_handler();
    }
}

pub trait KernelCallbacks<Context, Exception> {
    #[inline(always)]
    unsafe fn kernel_wakeup_handler() {
        unsafe { private::_kernel_wakeup_handler() }
    }

    /// This function is called by the kernel hardware abstraction layer when an
    /// interrupt is pending. A pending interrupt is claimed by the kernel interrupt
    /// handler and completed before returning from this function.
    #[inline(always)]
    unsafe fn kernel_interrupt_handler() {
        unsafe { private::_kernel_interrupt_handler() }
    }

    #[inline(always)]
    unsafe fn kernel_syscall_handler(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
        unsafe { private::_kernel_syscall_handler(id, arg0, arg1, arg2) }
    }

    #[inline(always)]
    fn kernel_exception_handler(error: &dyn Fault) -> ! {
        unsafe { private::_hardware_exception_handler(error) }
    }

    /// Called by the HAL when the service call executes at lowest interrupt priority.
    /// Used for deferred kernel operations like event processing and context switching.
    #[inline(always)]
    unsafe fn kernel_service_call_handler() {
        unsafe { private::_kernel_service_call_handler() }
    }
}

impl<T> KernelCallbacks<T::Context, T::HardwareError> for T where T: FlowController {}
