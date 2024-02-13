use super::FlowController;

mod private {
    extern "Rust" {
        pub fn _private_kernel_wakeup_handler();

        pub fn _private_kernel_interrupt_handler();

        pub fn _private_kernel_syscall_handler(id: usize, arg0: usize, arg1: usize) -> usize;

        pub fn _private_hardware_exception_handler(exception: *const u8) -> !;

        pub fn _private_current_task_context() -> *const ();

        pub fn _save_additional_context();

        pub fn _restore_additional_context();
    }
}

pub trait KernelCallbacks<Context, Exception> {
    #[inline(always)]
    unsafe fn kernel_wakeup_handler() {
        private::_private_kernel_wakeup_handler()
    }

    /// SAFETY: Must be called from interrupt handler with interrupts disabled.
    #[inline(always)]
    unsafe fn kernel_interrupt_handler() {
        private::_private_kernel_interrupt_handler()
    }

    /// SAFETY: Must be called from interrupt handler with interrupts disabled.
    #[inline(always)]
    unsafe fn kernel_syscall_handler(id: usize, arg0: usize, arg1: usize) -> usize {
        private::_private_kernel_syscall_handler(id, arg0, arg1)
    }

    #[inline(always)]
    fn kernel_exception_handler(exception: &Exception) -> ! {
        unsafe {
            private::_private_hardware_exception_handler(exception as *const Exception as *const u8)
        }
    }

    #[inline(always)]
    fn current_task_context() -> &'static Context {
        unsafe { &*(private::_private_current_task_context() as *const Context) }
    }

    #[inline(always)]
    fn save_additional_context() {
        unsafe { private::_save_additional_context() }
    }

    #[inline(always)]
    fn restore_additional_context() {
        unsafe { private::_restore_additional_context() }
    }
}

impl<T> KernelCallbacks<T::Context, T::Exception> for T where T: FlowController {}
