use crate::thread::{ThreadInfo, ThreadRef};

mod internal {
    use crate::thread::{ThreadInfo, ThreadRef};
    #[allow(dead_code)]
    unsafe extern "Rust" {
        #[link_name = "_scars_trace_thread_new"]
        pub(super) fn thread_new(thread: ThreadRef);

        #[link_name = "_scars_trace_thread_exec_begin"]
        pub(super) fn thread_exec_begin(thread: ThreadRef);

        #[link_name = "_scars_trace_thread_exec_end"]
        pub(super) fn thread_exec_end(thread: ThreadRef);

        #[link_name = "_scars_trace_thread_ready_begin"]
        pub(super) fn thread_ready_begin(thread: ThreadRef);

        #[link_name = "_scars_trace_thread_ready_end"]
        pub(super) fn thread_ready_end(thread: ThreadRef);

        #[link_name = "_scars_trace_system_idle"]
        pub(super) fn system_idle();

        #[link_name = "_scars_trace_isr_enter"]
        pub(super) fn isr_enter();

        #[link_name = "_scars_trace_isr_exit"]
        pub(super) fn isr_exit();

        #[link_name = "_scars_trace_isr_exit_to_scheduler"]
        pub(super) fn isr_exit_to_scheduler();
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn thread_new(thread: ThreadRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::thread_new(thread)
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn thread_exec_begin(thread: ThreadRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::thread_exec_begin(thread)
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn thread_exec_end(thread: ThreadRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::thread_exec_end(thread)
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn thread_ready_begin(thread: ThreadRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::thread_ready_begin(thread)
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn thread_ready_end(thread: ThreadRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::thread_ready_end(thread)
    }
}

#[inline(always)]
pub(crate) fn system_idle() {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::system_idle()
    }
}

#[inline(always)]
pub(crate) fn isr_enter() {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::isr_enter()
    }
}

#[inline(always)]
pub(crate) fn isr_exit() {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::isr_exit()
    }
}

#[inline(always)]
pub(crate) fn isr_exit_to_scheduler() {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::isr_exit_to_scheduler()
    }
}

#[unsafe(export_name = "_scars_default_trace_thread_new")]
fn default_thread_new(_thread: ThreadRef) {}

#[unsafe(export_name = "_scars_default_trace_thread_exec_begin")]
fn default_thread_exec_begin(_thread: ThreadRef) {}

#[unsafe(export_name = "_scars_default_trace_thread_exec_end")]
fn default_thread_exec_end(_thread: ThreadRef) {}

#[unsafe(export_name = "_scars_default_trace_thread_ready_begin")]
fn default_thread_ready_begin(_thread: ThreadRef) {}

#[unsafe(export_name = "_scars_default_trace_thread_ready_end")]
fn default_thread_ready_end(_thread: ThreadRef) {}

#[unsafe(export_name = "_scars_default_trace_system_idle")]
fn default_system_idle() {}

#[unsafe(export_name = "_scars_default_trace_isr_enter")]
fn default_isr_enter() {}

#[unsafe(export_name = "_scars_default_trace_isr_exit")]
fn default_isr_exit() {}

#[unsafe(export_name = "_scars_default_trace_isr_exit_to_scheduler")]
fn default_isr_exit_to_scheduler() {}
