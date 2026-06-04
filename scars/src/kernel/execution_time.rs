//! Per-thread execution-time accounting.
//!
//! CPU time is charged at the outermost thread/interrupt boundary in
//! [`interrupt_context`](crate::interrupt::interrupt_context). A context
//! switch always runs from inside an interrupt context (a syscall or
//! service-call trap), so a thread's run-slice spans from the boundary that
//! resumes it to the next boundary that enters an interrupt. Interrupt-handler
//! time is charged to a separate per-core [`INTERRUPT_CLOCK`] and is excluded
//! from thread CPU time.
//!
//! Each core tracks the wall tick at which the current charging interval began
//! in [`ACCOUNT_START`]. On entry to the outermost interrupt the elapsed time
//! is charged to the running thread; on exit back to thread level it is charged
//! to the interrupt clock. `ACCOUNT_START` is seeded in
//! [`Scheduler::start_on`](crate::kernel::scheduler::Scheduler) before the first
//! thread runs.

use crate::kernel::hal::{CoreId, NUM_CORES, clock_ticks};
use crate::kernel::scheduler::Scheduler;
use crate::sync::atomic::{AtomicU64, Ordering};
use crate::time::Instant;

/// Per-core wall tick at which the current charging interval began. Written at
/// interrupt boundaries, and read at interrupt boundaries and by in-progress
/// thread-time queries, all on the owning core.
static ACCOUNT_START: [AtomicU64; NUM_CORES] = [const { AtomicU64::new(0) }; NUM_CORES];

/// Per-core aggregate time spent in interrupt handlers. Written by the owning
/// core, read cross-core.
static INTERRUPT_CLOCK: [AtomicU64; NUM_CORES] = [const { AtomicU64::new(0) }; NUM_CORES];

/// Seed the calling core's charging origin. Called once from `start_on` after
/// the scheduler is installed and before the first thread starts running, so
/// the first interrupt does not charge the idle thread the entire since-boot
/// tick count.
pub(crate) fn init_account_start() {
    ACCOUNT_START[CoreId::current().as_usize()].store(clock_ticks(), Ordering::Relaxed);
}

/// Outermost transition thread → interrupt: charge the running thread the time
/// it has run since the last boundary, and reset the origin to now.
///
/// Called from `interrupt_context` immediately after the current-interrupt slot
/// becomes non-null, so any nested interrupt observes `in_interrupt()` and does
/// not touch `ACCOUNT_START`.
pub(crate) fn charge_thread_boundary() {
    let core = CoreId::current().as_usize();
    let now = clock_ticks();
    let start = ACCOUNT_START[core].swap(now, Ordering::Relaxed);
    Scheduler::current_thread_raw()
        .execution_time
        .fetch_add(now.saturating_sub(start), Ordering::Relaxed);
}

/// Outermost transition interrupt → thread: charge the interrupt clock the time
/// spent in interrupt handlers since the last boundary, and reset the origin.
///
/// Must run while `in_interrupt()` is still true (before the slot is restored to
/// null) so a nested interrupt cannot race the `ACCOUNT_START` update.
pub(crate) fn charge_interrupt_boundary() {
    let core = CoreId::current().as_usize();
    let now = clock_ticks();
    let start = ACCOUNT_START[core].swap(now, Ordering::Relaxed);
    INTERRUPT_CLOCK[core].fetch_add(now.saturating_sub(start), Ordering::Relaxed);
}

/// The calling core's charging origin — the instant the current run-slice
/// began. Used by in-progress thread-time queries.
pub(crate) fn local_account_start() -> Instant {
    Instant::from_ticks(ACCOUNT_START[CoreId::current().as_usize()].load(Ordering::Relaxed))
}

/// Aggregate interrupt-handler time accumulated on `core`, in clock ticks.
pub(crate) fn interrupt_clock_ticks(core: CoreId) -> u64 {
    INTERRUPT_CLOCK[core.as_usize()].load(Ordering::Relaxed)
}
