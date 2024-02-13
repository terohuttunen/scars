use crate::kernel::task::{TaskInfo, TaskRef};

mod internal {
    use crate::kernel::task::{TaskInfo, TaskRef};
    #[allow(dead_code)]
    extern "Rust" {
        #[link_name = "_scars_trace_task_new"]
        pub(super) fn task_new(task: TaskRef);

        #[link_name = "_scars_trace_task_exec_begin"]
        pub(super) fn task_exec_begin(task: TaskRef);

        #[link_name = "_scars_trace_task_exec_end"]
        pub(super) fn task_exec_end(task: TaskRef);

        #[link_name = "_scars_trace_task_ready_begin"]
        pub(super) fn task_ready_begin(task: TaskRef);

        #[link_name = "_scars_trace_task_ready_end"]
        pub(super) fn task_ready_end(task: TaskRef);

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
pub(crate) fn task_new(task: TaskRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::task_new(task)
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn task_exec_begin(task: TaskRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::task_exec_begin(task)
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn task_exec_end(task: TaskRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::task_exec_end(task)
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn task_ready_begin(task: TaskRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::task_ready_begin(task)
    }
}

#[allow(unused)]
#[inline(always)]
pub(crate) fn task_ready_end(task: TaskRef) {
    #[cfg(feature = "tracing")]
    unsafe {
        internal::task_ready_end(task)
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

#[export_name = "_scars_default_trace_task_new"]
fn default_task_new(_task: TaskRef) {}

#[export_name = "_scars_default_trace_task_exec_begin"]
fn default_task_exec_begin(_task: TaskRef) {}

#[export_name = "_scars_default_trace_task_exec_end"]
fn default_task_exec_end(_task: TaskRef) {}

#[export_name = "_scars_default_trace_task_ready_begin"]
fn default_task_ready_begin(_task: TaskRef) {}

#[export_name = "_scars_default_trace_task_ready_end"]
fn default_task_ready_end(_task: TaskRef) {}

#[export_name = "_scars_default_trace_system_idle"]
fn default_system_idle() {}

#[export_name = "_scars_default_trace_isr_enter"]
fn default_isr_enter() {}

#[export_name = "_scars_default_trace_isr_exit"]
fn default_isr_exit() {}

#[export_name = "_scars_default_trace_isr_exit_to_scheduler"]
fn default_isr_exit_to_scheduler() {}
