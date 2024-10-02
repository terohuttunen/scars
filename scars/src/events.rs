use crate::kernel::{scheduler::ExecutionContext, Scheduler};
pub use crate::thread::{RawThread, TryWaitEventsError};
use crate::time::Instant;

pub const EXECUTOR_WAKEUP_EVENT: u32 = 1u32 << 28;
pub const SCHEDULER_WAKEUP_EVENT: u32 = 1u32 << 29;
pub const SCHEDULER_NOTIFY_EVENT: u32 = 1u32 << 30;
pub const REQUIRE_ALL_EVENTS: u32 = 1u32 << 31;

#[derive(Copy, Clone, Debug)]
pub enum WaitEventsUntilError {
    Timeout(u32),
}

pub fn wait_events(events: u32) -> u32 {
    match Scheduler::current_execution_context() {
        ExecutionContext::Interrupt(_) => {
            panic!("wait_events is not available in interrupt context")
        }
        ExecutionContext::Thread(thread) => thread.wait_events(events),
    }
}

pub fn wait_events_until(
    events: u32,
    deadline_opt: Option<Instant>,
) -> Result<u32, WaitEventsUntilError> {
    match Scheduler::current_execution_context() {
        ExecutionContext::Interrupt(_) => {
            panic!("wait_events is not available in interrupt context")
        }
        ExecutionContext::Thread(thread) => {
            if let Some(deadline) = deadline_opt {
                thread.wait_events_until(events, deadline)
            } else {
                Ok(thread.wait_events(events))
            }
        }
    }
}
