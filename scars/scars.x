PROVIDE(_scars_idle_thread_hook = _scars_default_idle_thread_hook);
PROVIDE(_user_exception_handler = _scars_default_user_exception_handler);

PROVIDE(_scars_trace_thread_new = _scars_default_trace_thread_new);
PROVIDE(_scars_trace_thread_send_info = _scars_default_trace_thread_send_info);
PROVIDE(_scars_trace_thread_exec_begin = _scars_default_trace_thread_exec_begin);
PROVIDE(_scars_trace_thread_exec_end = _scars_default_trace_thread_exec_end);
PROVIDE(_scars_trace_thread_ready_begin = _scars_default_trace_thread_ready_begin);
PROVIDE(_scars_trace_thread_ready_end = _scars_default_trace_thread_ready_end);
PROVIDE(_scars_trace_system_idle = _scars_default_trace_system_idle);
PROVIDE(_scars_trace_isr_enter = _scars_default_trace_isr_enter);
PROVIDE(_scars_trace_isr_exit = _scars_default_trace_isr_exit);
PROVIDE(_scars_trace_isr_exit_to_scheduler = _scars_default_trace_isr_exit_to_scheduler);
