//! Kernel-side `FaultContext` types prepended to a propagating
//! [`scars_fault::FaultInfo`] before it reaches the platform layer.
//!
//! The kernel `#[fault_handler]` builds one of these on the stack
//! (selected by [`Scheduler::current_execution_context`] /
//! [`Scheduler::is_initialized`]) and calls [`crate::kernel::hal::fault`]
//! with the enriched `FaultInfo`.
//!
//! All fields are read with the value-copy semantics of `LockedCell::as_ptr`
//! reads or simple field loads; no locks are taken on the fault path.

use crate::priority::Priority;
use crate::thread::ThreadExecutionState;
use scars_fault::FaultContext;

/// Snapshot of the running thread at fault time.
#[derive(Debug, FaultContext)]
#[fault(
    "thread '{name}' (id {thread_id}, base priority {base_priority}, active {active_priority}, state {state:?})"
)]
pub struct ThreadContext {
    pub thread_id: u32,
    pub name: &'static str,
    pub base_priority: Priority,
    pub active_priority: Priority,
    pub state: ThreadExecutionState,
}

/// Snapshot of the running interrupt handler at fault time.
#[derive(Debug, FaultContext)]
#[fault("interrupt #{irq_number} (base priority {base_priority}, active {active_priority})")]
pub struct InterruptContext {
    pub irq_number: u16,
    pub base_priority: Priority,
    pub active_priority: Priority,
}

/// Marker context for faults raised before the scheduler is running.
#[derive(Debug, FaultContext)]
#[fault("during early init (scheduler not running)")]
pub struct BootstrapContext;
