PROVIDE(_scars_idle_task_hook = _scars_default_idle_task_hook);

PROVIDE(_scars_trace_task_new = _scars_default_trace_task_new);
PROVIDE(_scars_trace_task_send_info = _scars_default_trace_task_send_info);
PROVIDE(_scars_trace_task_exec_begin = _scars_default_trace_task_exec_begin);
PROVIDE(_scars_trace_task_exec_end = _scars_default_trace_task_exec_end);
PROVIDE(_scars_trace_task_ready_begin = _scars_default_trace_task_ready_begin);
PROVIDE(_scars_trace_task_ready_end = _scars_default_trace_task_ready_end);
PROVIDE(_scars_trace_system_idle = _scars_default_trace_system_idle);
PROVIDE(_scars_trace_isr_enter = _scars_default_trace_isr_enter);
PROVIDE(_scars_trace_isr_exit = _scars_default_trace_isr_exit);
PROVIDE(_scars_trace_isr_exit_to_scheduler = _scars_default_trace_isr_exit_to_scheduler);