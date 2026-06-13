pub mod event_queue;
#[cfg(feature = "multithreading")]
mod threads;
pub mod timers;
mod work_queue;
use crate::cell::{LockedCell, LockedRefCell, RefMut};
use crate::events::raw::RawEventHandler;
use crate::interrupt::{
    RawInterruptHandler, current_interrupt, in_interrupt, set_ceiling_threshold,
};
use crate::kernel::list::{LinkedList, LinkedListNode, LinkedListTag, impl_linked};
use crate::kernel::{
    RuntimeError, Stack, ThreadPriority,
    atomic_queue::{AtomicNode, AtomicQueue},
    hal::{self, Context, CoreId, set_alarm, start_first_thread},
    handle_runtime_error, syscall,
    waiter::{WaitQueueEntry, WaitQueueHandle, WaitQueueTag},
};
use crate::printkln;
use crate::priority::{AnyPriority, AtomicPriorityOpt, PriorityOpt};
use crate::sync::atomic::Ordering;
use crate::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize};
use crate::sync::lock::preempt_lock::is_preempt_allowed;
use crate::sync::lock::{interrupt_lock::CoreInterruptLockKey, preempt_lock::CorePreemptLockKey};
use crate::sync::{
    CoreInterruptLock, CorePreemptLock, PreemptLock, PreemptLockKey, Protected, RawCeilingLock,
};
use crate::thread::{
    IDLE_THREAD_ID, INVALID_THREAD_ID, RawThread, Thread, ThreadExecutionState, ThreadInfo,
};
use crate::{Duration, Instant};
use core::cell::SyncUnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr::NonNull;
use event_queue::PendingEventsQueue;
use scars_khal::{ContextInfo, CoreController};
pub(crate) use timers::RawTimer;
pub use timers::{EventTimer, Timer, TimerHandler};
use timers::{TimerQueue, TimerQueueTag};
use work_queue::WorkQueue;
pub use work_queue::{PendingWorkEntry, PendingWorkHandler, RawPendingWorkEntry};

use super::hal::current_thread_context;

pub struct ExecStateTag {}

impl LinkedListTag for ExecStateTag {}

unsafe extern "C" {
    static _isr_stack_end: u8;
}

pub(crate) enum ExecutionContext {
    Interrupt(Pin<&'static RawInterruptHandler>),
    Thread(Pin<&'static RawThread>),
}

/// Returned in the `Err` arm of `Result<_, TimedOut>` when a
/// deadlined wait expired before completing.
#[cfg(feature = "multithreading")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimedOut;

const RESCHEDULE_KIND_NONE: usize = 0;
const RESCHEDULE_KIND_YIELD_TO_HIGHER: usize = 1;
#[cfg(feature = "multithreading")]
const RESCHEDULE_KIND_YIELD_TO_EQUAL: usize = 2;
#[cfg(feature = "multithreading")]
pub(crate) const RESCHEDULE_KIND_BLOCK_CURRENT: usize = 4;

/// Minimum slack when re-arming a monitor budget alarm: a predicted deadline
/// that has already passed is moved this far ahead so it cannot retrigger
/// immediately. About 10 µs at any tick frequency.
#[cfg(feature = "execution-time-monitor")]
const MONITOR_MIN_SLACK: Duration = Duration::from_ticks({
    let s = hal::TICK_FREQ_HZ / 100_000;
    if s == 0 { 1 } else { s }
});

/// The earlier of two optional alarm deadlines.
#[cfg(feature = "execution-time-monitor")]
fn min_option(a: Option<u64>, b: Option<u64>) -> Option<u64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) => Some(x),
        (None, b) => b,
    }
}

pub struct RawScheduler {
    // When thread execution state is Ready or Blocked, it is in the ready
    // queue or the blocked list respectively. Threads in Created or Started
    // state are not yet in any scheduler queue. On state Running, the thread
    // is the `current_thread`.

    // Threads that are ready to run are in the ready_queue sorted in descending
    // priority order.
    #[cfg(feature = "multithreading")]
    ready_queue: LinkedList<RawThread, ExecStateTag>,

    // Threads that are blocked in wait list, or waiting for timed wakeup, are in blocked
    // list sorted in descending lock priority order.
    #[cfg(feature = "multithreading")]
    blocked_list: LinkedList<RawThread, ExecStateTag>,

    // Currently running thread on state Running. Without multithreading the idle
    // thread is the only context, so `current_thread()` returns it instead.
    #[cfg(feature = "multithreading")]
    current_thread: Pin<&'static RawThread>,

    // A wakeup-time sorted queue of timers that are waiting to be woken up at a specific time.
    timer_queue: TimerQueue,

    idle_thread: Pin<&'static RawThread>,
}

unsafe impl Send for RawScheduler {}

impl RawScheduler {
    pub(crate) fn new(idle_thread: &'static RawThread) -> RawScheduler {
        RawScheduler {
            #[cfg(feature = "multithreading")]
            ready_queue: LinkedList::new(),
            #[cfg(feature = "multithreading")]
            blocked_list: LinkedList::new(),
            #[cfg(feature = "multithreading")]
            current_thread: Pin::static_ref(idle_thread),
            timer_queue: LinkedList::new(),
            idle_thread: Pin::static_ref(idle_thread),
        }
    }

    /// The currently running thread. Without multithreading this is always the
    /// idle thread (the sole execution context).
    #[cfg(feature = "multithreading")]
    fn current_thread(self: Pin<&Self>) -> Pin<&'static RawThread> {
        self.current_thread
    }

    #[cfg(not(feature = "multithreading"))]
    fn current_thread(self: Pin<&Self>) -> Pin<&'static RawThread> {
        self.idle_thread
    }

    fn timer_queue(self: Pin<&Self>) -> Pin<&LinkedList<RawTimer, TimerQueueTag>> {
        unsafe { self.map_unchecked(|s| &s.timer_queue) }
    }

    pub(crate) fn timer_queue_mut(
        self: Pin<&mut Self>,
    ) -> Pin<&mut LinkedList<RawTimer, TimerQueueTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.timer_queue) }
    }

    pub(crate) fn reprogram_alarm(self: Pin<&Self>, pkey: PreemptLockKey<'_>) {
        let wall = self
            .timer_queue()
            .head()
            .and_then(|t| t.get_deadline(pkey))
            .map(|d| d.tick);
        // Merge the running thread's CPU-budget deadline (if any) so the
        // single hardware alarm fires for whichever comes first.
        #[cfg(feature = "execution-time-monitor")]
        let deadline = min_option(wall, self.current_monitor_deadline(pkey));
        #[cfg(not(feature = "execution-time-monitor"))]
        let deadline = wall;
        set_alarm(deadline);
    }

    /// Predicted wall tick at which the currently-running thread's open monitor
    /// window will reach its budget, or `None` if there is no monitor, no open
    /// window, or it has already fired this window. Only the running thread is
    /// tracked; after a context switch this reflects the new current thread.
    #[cfg(feature = "execution-time-monitor")]
    fn current_monitor_deadline(self: Pin<&Self>, pkey: PreemptLockKey<'_>) -> Option<u64> {
        let config = self.current_thread.monitor?;
        let window = self.current_thread.monitor_state.get(pkey).window?;
        if window.fired {
            return None;
        }
        let target = window.start.saturating_add(config.budget);
        let consumed =
            Duration::from_ticks(self.current_thread.execution_time.load(Ordering::Relaxed));
        let remaining = target.saturating_sub(consumed);
        // While `current` runs, its CPU clock advances from `account_start`
        // at wall rate and reaches `target` at `account_start + remaining`.
        // Interrupts push the true crossing later, so an early alarm is
        // corrected by re-arming. Clamp into the future to bound the re-arm
        // rate.
        let deadline = crate::kernel::execution_time::local_account_start() + remaining;
        let floor = Instant::now() + MONITOR_MIN_SLACK;
        Some(deadline.max(floor).tick)
    }

    /// Deliver the running thread's monitor event if its CPU consumption in the
    /// open window has reached the budget, then mark the window fired so it
    /// fires only once per window. Level-triggered on `execution_time >=
    /// start + budget`, so it is correct whether the alarm fired for this
    /// budget or for an unrelated wall-clock timer.
    #[cfg(feature = "execution-time-monitor")]
    fn check_monitor(self: Pin<&Self>, pkey: PreemptLockKey<'_>) {
        let current = self.current_thread;
        let Some(config) = current.monitor else {
            return;
        };
        let mut state = current.monitor_state.get(pkey);
        let Some(mut window) = state.window else {
            return;
        };
        let consumed = Duration::from_ticks(current.execution_time.load(Ordering::Relaxed));
        if !window.fired && consumed >= window.start.saturating_add(config.budget) {
            config.sender.send_events(config.events);
            window.fired = true;
            state.window = Some(window);
            current.monitor_state.set(pkey, state);
        }
    }
}

impl RawScheduler {
    /// Drains the timer queue, firing every expired timer (which delivers
    /// its events to the registered handler). With `multithreading`, then
    /// runs `reschedule_threads` to apply any thread block, yield, or
    /// context switch indicated by `kind`.
    fn reschedule<'key>(mut self: Pin<&mut Self>, pkey: PreemptLockKey<'key>, kind: usize) {
        // Fire the running thread's monitor budget if it has been reached,
        // before the wakeup drain or `reschedule_threads` can change
        // `current_thread`. Its CPU clock was refreshed at this interrupt's
        // entry boundary.
        #[cfg(feature = "execution-time-monitor")]
        self.as_ref().check_monitor(pkey);

        // Wakeup sleeping threads that should have been woken up
        let now = Instant::now();
        loop {
            if let Some(sleeping_thread) = self.as_ref().timer_queue().head() {
                if let Some(wakeup_time) = sleeping_thread.get_deadline(pkey) {
                    if wakeup_time > now {
                        // No more threads to wake up
                        set_alarm(Some(wakeup_time.tick));
                        break;
                    }
                }
            } else {
                // Wakeup queue is empty, disable wakeup
                set_alarm(None);
                break;
            }

            if let Some(suspended) = self.as_mut().timer_queue_mut().pop_front() {
                self.as_mut().wakeup_timer(pkey, suspended)
            }
        }

        // Without multithreading there is no thread to block, yield, or
        // switch, so `kind` is unused.
        #[cfg(not(feature = "multithreading"))]
        let _ = kind;

        #[cfg(feature = "multithreading")]
        self.as_mut().reschedule_threads(pkey, kind);

        // Re-merge the alarm with the current thread's CPU budget and the next
        // wall-clock wakeup, reflecting any context switch just made.
        #[cfg(feature = "execution-time-monitor")]
        self.as_ref().reprogram_alarm(pkey);
    }

    #[cfg(feature = "multithreading")]
    pub(crate) fn threads(self: Pin<&Self>) -> impl Iterator<Item = Pin<&RawThread>> {
        Some(self.idle_thread)
            .into_iter()
            .chain(Some(self.current_thread).into_iter())
            .chain(self.ready_queue().cursor_front())
            .chain(self.blocked_list().cursor_front())
    }

    // Without multithreading the idle thread is the only context.
    #[cfg(not(feature = "multithreading"))]
    pub(crate) fn threads(self: Pin<&Self>) -> impl Iterator<Item = Pin<&RawThread>> {
        Some(self.idle_thread).into_iter()
    }

    pub fn thread_info<'a, 'key: 'a>(
        self: Pin<&'a Self>,
        pkey: PreemptLockKey<'key>,
    ) -> impl Iterator<Item = ThreadInfo> {
        self.threads().map(move |thread| thread.get_info(pkey))
    }
}

use crate::kernel::hal::NUM_CORES;

static SCHEDULER_INITIALIZED: [AtomicBool; NUM_CORES] =
    [const { AtomicBool::new(false) }; NUM_CORES];

static SCHEDULERS: [SyncUnsafeCell<MaybeUninit<Scheduler>>; NUM_CORES] =
    [const { SyncUnsafeCell::new(MaybeUninit::uninit()) }; NUM_CORES];

/// Per-core scheduler.The per-core instance is selected via the SCHEDULERS
/// static array indexed by `CoreId::current()`.
pub struct Scheduler {
    // Core this scheduler instance belongs to. Set at construction in
    // `start_on`. Used for cross-core dispatch (e.g. waking a thread on
    // its own core's scheduler) and debug-asserts.
    pub core: CoreId,

    /// Work that could not be completed because of a lock.
    /// The operations will be completed when the preemption lock is released.
    deferred_work_queue: WorkQueue,

    /// Event handlers with pending events that need processing
    pending_events: PendingEventsQueue,

    // The kind of pending reschedule. Rescheduling may be triggered by different
    // events, but they are always executed either at the end of interrupt handling,
    // or when the preemption lock is released.
    pending_reschedule_kind: AtomicUsize,

    // Current ceiling priority from all held ceiling locks
    current_ceiling_priority: AtomicPriorityOpt,

    raw: Protected<RawScheduler, PreemptLock>,
}

impl Scheduler {
    fn new(core: crate::kernel::hal::CoreId, idle_thread: &'static RawThread) -> Scheduler {
        Scheduler {
            core,
            deferred_work_queue: WorkQueue::new(),
            pending_events: PendingEventsQueue::new(),
            pending_reschedule_kind: AtomicUsize::new(RESCHEDULE_KIND_NONE),
            current_ceiling_priority: AtomicPriorityOpt::new(PriorityOpt::none()),
            raw: Protected::new(RawScheduler::new(idle_thread)),
        }
    }

    pub(super) fn start_on(core: crate::kernel::hal::CoreId) -> ! {
        let idle_thread = crate::kernel::idle::init_idle_thread(core);
        unsafe {
            let _ =
                (&mut *SCHEDULERS[core.as_usize()].get()).write(Scheduler::new(core, idle_thread));
        }
        SCHEDULER_INITIALIZED[core.as_usize()].store(true, Ordering::Release);
        // Seed the execution-time charging origin before the first thread
        // runs, so the first interrupt charges the idle thread only the
        // time it actually ran, not the entire since-boot tick count.
        #[cfg(feature = "execution-time")]
        crate::kernel::execution_time::init_account_start();
        let idle_context = idle_thread.context.as_ptr() as *mut _;
        start_first_thread(idle_context)
    }

    /// True once `start_on` has finished installing the calling core's
    /// scheduler instance. Reading scheduler state on a core whose
    /// scheduler has not yet been started is undefined behaviour, so
    /// the kernel fault handler uses it to dispatch to a
    /// `BootstrapContext` instead.
    pub(crate) fn is_initialized() -> bool {
        let core = CoreId::current().as_usize();
        SCHEDULER_INITIALIZED[core].load(Ordering::Acquire)
    }

    fn instance() -> &'static Scheduler {
        // SAFETY: The scheduler for this core is initialized in `start_on`.
        let core = CoreId::current().as_usize();
        unsafe { (&*SCHEDULERS[core].get()).assume_init_ref() }
    }

    fn pin_instance() -> Pin<&'static Scheduler> {
        Pin::static_ref(Scheduler::instance())
    }

    /// Borrow the scheduler instance belonging to `core`. The caller is
    /// responsible for ensuring that core's scheduler has been started;
    /// this is used by cross-core dispatch paths where the scheduler is
    /// known to be live.
    #[allow(dead_code)]
    fn instance_for(core: crate::kernel::hal::CoreId) -> &'static Scheduler {
        unsafe { (&*SCHEDULERS[core.as_usize()].get()).assume_init_ref() }
    }

    fn raw_pin(self: Pin<&'static Self>) -> Pin<&'static Protected<RawScheduler, PreemptLock>> {
        // SAFETY: `raw` is a field of the pinned per-core Scheduler.
        unsafe { self.map_unchecked(|s| &s.raw) }
    }

    /// The calling core's currently-running thread, as a raw static
    /// reference. Used by execution-time accounting at interrupt boundaries,
    /// where the preempt lock is not held. The read is sound on the local core
    /// only: a context switch always runs from inside an interrupt context, so
    /// none is in flight at an outermost boundary.
    #[cfg(feature = "execution-time")]
    pub(crate) fn current_thread_raw() -> &'static RawThread {
        unsafe { Pin::new_unchecked(&*Scheduler::instance().raw.as_ptr()) }
            .current_thread()
            .get_ref()
    }

    /// Reprogram the local-core alarm given the preempt lock, from outside a
    /// scheduler borrow. Used by the execution-time monitor controls, which run
    /// synchronously in thread context under [`PreemptLock`]. The shared read of
    /// the scheduler is sound on the local core: `timer_queue` is mutated only
    /// under the preempt lock (held here), and no `&mut RawScheduler` is live in
    /// thread context.
    #[cfg(feature = "execution-time-monitor")]
    pub(crate) fn reprogram_alarm_local(pkey: PreemptLockKey<'_>) {
        unsafe { Pin::new_unchecked(&*Scheduler::instance().raw.as_ptr()) }.reprogram_alarm(pkey);
    }

    pub(crate) fn current_execution_context() -> ExecutionContext {
        match current_interrupt() {
            Some(interrupt_context) => ExecutionContext::Interrupt(unsafe {
                Pin::new_unchecked(interrupt_context.as_ref())
            }),
            None => ExecutionContext::Thread(
                unsafe { Pin::new_unchecked(&*Scheduler::instance().raw.as_ptr()) }
                    .current_thread(),
            ),
        }
    }

    /// Get current ceiling priority from all held ceiling locks
    #[allow(dead_code)]
    pub(crate) fn current_ceiling_priority() -> PriorityOpt {
        Scheduler::instance()
            .current_ceiling_priority
            .load(Ordering::Acquire)
    }

    /// Update the current ceiling priority atomically
    pub(crate) fn update_ceiling_priority(new_ceiling: PriorityOpt) {
        Scheduler::instance()
            .current_ceiling_priority
            .store(new_ceiling, Ordering::Release);
    }

    /// Set ceiling priority, updating both global tracking and hardware threshold
    pub(crate) fn set_ceiling(ceiling: PriorityOpt) {
        // Update global ceiling priority tracking
        Self::update_ceiling_priority(ceiling);

        // Update hardware interrupt threshold
        crate::interrupt::set_ceiling_threshold(ceiling);
    }

    #[cfg(feature = "priority-inheritance")]
    pub(crate) fn thread_priority_changed<'key>(
        pkey: PreemptLockKey<'key>,
        thread: Pin<&'static RawThread>,
    ) {
        Self::pin_instance()
            .raw_pin()
            .with_pin_key(pkey, |pkey, mut pin_scheduler| {
                match thread.state.get(pkey) {
                    ThreadExecutionState::Ready => {
                        pin_scheduler.as_mut().reinsert_to_ready_queue(pkey, thread);

                        if let Some(ready_thread) = pin_scheduler.as_ref().ready_queue().head() {
                            if pin_scheduler.current_thread.priority(pkey)
                                < ready_thread.priority(pkey)
                            {
                                // Rescheduling will be executed when preemption lock is released
                                Self::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER);
                            }
                        }
                    }
                    ThreadExecutionState::Blocked => {
                        pin_scheduler.reinsert_to_blocked_queue(pkey, thread);
                    }
                    _ => (),
                }
            })
    }

    #[cfg(feature = "multithreading")]
    pub(crate) fn cond_reschedule<'key>(pkey: PreemptLockKey<'key>) {
        Self::pin_instance()
            .raw_pin()
            .with_pin_key(pkey, |pkey, scheduler| {
                let pin_scheduler = scheduler.as_ref();

                if let Some(ready_thread) = pin_scheduler.ready_queue().head() {
                    if pin_scheduler.current_thread.priority(pkey) < ready_thread.priority(pkey) {
                        // Rescheduling will be executed when preemption lock is released
                        Self::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER);
                    }
                }
            })
    }

    // Without multithreading there is no ready queue to yield to; a ceiling-lock
    // release can never make a higher-priority thread runnable.
    #[cfg(not(feature = "multithreading"))]
    pub(crate) fn cond_reschedule(_pkey: PreemptLockKey<'_>) {}

    pub(crate) fn set_pending_reschedule(kind: usize) {
        let scheduler = Scheduler::instance();
        scheduler
            .pending_reschedule_kind
            .fetch_or(kind, Ordering::Relaxed);

        // If preemption lock is being held, context switch is not allowed until
        // preemption lock is released. Pending reschedule will be handled at
        // that time.
        if is_preempt_allowed() {
            hal::pend_service_call();
        }
    }

    pub(crate) fn is_reschedule_pending() -> bool {
        let scheduler = Scheduler::instance();
        scheduler.pending_reschedule_kind.load(Ordering::Relaxed) != RESCHEDULE_KIND_NONE
    }

    /// Post `ops` to `target`'s `deferred_work_queue`. Producer-safe
    /// from any core; [`AtomicWorkQueue`] is MPSC.
    ///
    /// The service call is pended on `target` whenever the post might
    /// otherwise sit: remote targets always need the IPI; same-core
    /// posts made outside a held preempt lock need a local pend so a
    /// drain runs without waiting for an unrelated lock release. A
    /// same-core post inside a held preempt lock skips the pend — the
    /// lock's release path drains the queue.
    ///
    /// The handler's [`PendingWorkHandler::complete`] runs under
    /// `target`'s preempt lock and owns any ceiling-based re-deferral,
    /// which it expresses by calling this function again from inside
    /// the dispatch.
    #[cfg(feature = "multithreading")]
    pub(crate) fn schedule_deferred_operation_on(
        target: CoreId,
        pending_work: Pin<&RawPendingWorkEntry>,
        ops: u32,
    ) {
        Scheduler::instance_for(target)
            .deferred_work_queue
            .queue_work(pending_work, ops);
        if target != CoreId::current() {
            hal::pend_service_call_on(target);
        } else if is_preempt_allowed() {
            hal::pend_service_call();
        }
    }

    pub(crate) fn is_deferred_work_pending() -> bool {
        let scheduler = Scheduler::instance();
        scheduler.deferred_work_queue.work_pending()
    }

    /// Drain everything the local kernel has queued for dispatch:
    /// pending event handlers, deferred-work queue, and any pending
    /// reschedule. Single entry point for both `_kernel_syscall_handler`
    /// and `_kernel_service_call_handler`. Resolves the local scheduler
    /// once and reuses it across all three sweeps.
    pub(crate) fn process_pending_work() {
        let scheduler: &'static Scheduler = Scheduler::instance();
        let raw_pin = Pin::static_ref(scheduler).raw_pin();

        // Pending event-handler queue. No preempt lock needed; handlers
        // run in their own event-handler context.
        scheduler.pending_events.process_pending_events();

        // Deferred-work queue. Each handler dispatch needs the preempt
        // lock; bail when someone else holds it (their release path
        // will pick up the rest).
        while scheduler.deferred_work_queue.work_pending() {
            if raw_pin
                .try_with_pin(|pkey, raw| scheduler.deferred_work_queue.complete_work(pkey, raw))
                .is_err()
            {
                break;
            }
        }

        // Pending reschedule. Same shape as `execute_pending_reschedule`,
        // but reuses the resolved `scheduler` instead of looking it up
        // again.
        let kind = scheduler
            .pending_reschedule_kind
            .swap(RESCHEDULE_KIND_NONE, Ordering::AcqRel);
        if kind != RESCHEDULE_KIND_NONE {
            if raw_pin
                .try_with_pin(|pkey, raw| raw.reschedule(pkey, kind))
                .is_err()
            {
                // Re-pend without going through `set_pending_reschedule`,
                // which would re-resolve the instance.
                scheduler
                    .pending_reschedule_kind
                    .fetch_or(kind, Ordering::Relaxed);
                if is_preempt_allowed() {
                    hal::pend_service_call();
                }
            }
        }
    }
}

#[cfg(feature = "multithreading")]
impl Scheduler {
    // Thread or ISR context
    pub(crate) fn resume_thread(thread: Pin<&'static RawThread>) {
        if thread.core != CoreId::current() {
            // Foreign-core target: drop the op onto the owning core's
            // deferred-work queue. The target's
            // `RawThread::PendingWorkHandler::complete` will run
            // `sched.try_resume_thread(pkey, this)` against its own
            // scheduler instance, honoring every per-core invariant.
            thread.schedule_deferred_op(RawThread::OP_RESUME);
            return;
        }
        let result = Self::pin_instance()
            .raw_pin()
            .try_with_pin(|pkey, raw| raw.try_resume_thread(pkey, thread));
        match result {
            // Lock acquired and operation completed inline.
            Ok(Ok(())) => (),
            // Lock acquired but the ceiling blocked us; defer through
            // the work queue.
            Ok(Err(())) => {
                thread.schedule_deferred_op(RawThread::OP_RESUME);
            }
            // Lock held by another context — defer; the lock-release
            // path will drain.
            Err(_) => {
                thread.schedule_deferred_op(RawThread::OP_RESUME);
            }
        }
    }

    // Delay
    // ISR context
    /// Puts the current thread into scheduler sleep queue to be woken up later at given time.
    pub(crate) fn delay_thread_until(wakeup_time: u64) {
        match Self::pin_instance()
            .raw_pin()
            .try_with_pin(|pkey, raw| raw.delay_thread_until(pkey, wakeup_time))
        {
            Ok(()) => (),
            Err(_) => {
                // Error: Thread is trying to sleep while it holds the preempt lock.
                panic!("Thread is trying to sleep while it holds the preempt lock");
            }
        };
    }

    pub(crate) fn suspend_thread(maybe_thread: Option<Pin<&'static RawThread>>) {
        if let Some(thread) = maybe_thread {
            if thread.core != CoreId::current() {
                // Foreign-core target: dispatch via owning core's
                // deferred-work queue. `maybe_thread == None` (suspend
                // current thread) is intrinsically same-core, so the
                // foreign branch only fires when the caller named a
                // specific target.
                thread.schedule_deferred_op(RawThread::OP_SUSPEND);
                return;
            }
        }
        match Self::pin_instance()
            .raw_pin()
            .try_with_pin(|pkey, raw| raw.suspend_thread(pkey, maybe_thread))
        {
            Ok(()) => (),
            Err(_) => match maybe_thread {
                Some(thread) => {
                    thread.schedule_deferred_op(RawThread::OP_SUSPEND);
                }
                None => {
                    panic!("Cannot suspend the current thread while holding the preemption lock")
                }
            },
        }
    }
}

impl Scheduler {
    // Pre-emption
    // ISR context
    pub(crate) fn wakeup_scheduler_isr() {
        Scheduler::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER);
    }
}

#[cfg(feature = "multithreading")]
impl Scheduler {
    // Yield current thread
    // ISR context
    pub(crate) fn yield_current_thread_isr() {
        // Yielding can switch to another thread with equal priority
        Scheduler::set_pending_reschedule(
            RESCHEDULE_KIND_YIELD_TO_EQUAL | RESCHEDULE_KIND_YIELD_TO_HIGHER,
        );
    }

    /// Commit-site helper: stash `deadline` on the current thread so
    /// `block_current`'s `take_pending_block_deadline` picks it up.
    /// Must be called from thread context.
    pub(crate) fn set_current_pending_block_deadline(deadline: Option<Instant>) {
        PreemptLock::with(|pkey| match Scheduler::current_execution_context() {
            ExecutionContext::Thread(t) => t.set_pending_block_deadline(pkey, deadline),
            ExecutionContext::Interrupt(_) => {
                crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
            }
        });
    }

    pub(crate) fn take_last_wait_timed_out() -> Result<(), TimedOut> {
        PreemptLock::with(|pkey| match Scheduler::current_execution_context() {
            ExecutionContext::Thread(t) => {
                if t.take_wait_timed_out(pkey) {
                    Err(TimedOut)
                } else {
                    Ok(())
                }
            }
            ExecutionContext::Interrupt(_) => Ok(()),
        })
    }

    pub(crate) fn wait_current_thread_event_isr(
        wait_events: *mut crate::WaitEvents,
        deadline: Option<u64>,
    ) {
        match Self::pin_instance()
            .raw_pin()
            .try_with_pin(|pkey, raw| raw.wait_current_thread_event(pkey, wait_events, deadline))
        {
            Ok(()) => (),
            Err(_) => {
                // Error: Thread is blocking to wait for events while it holds the preempt lock.
                unimplemented!()
            }
        }
    }
}

#[allow(dead_code)]
pub fn print_threads() {
    printkln!("NAME       PRI  STATUS ENTRY");
    Scheduler::pin_instance()
        .raw_pin()
        .with_pin(|pkey, scheduler| {
            let pin_scheduler = scheduler.as_ref();
            for info in pin_scheduler.thread_info(pkey) {
                printkln!(
                    "{} {}   {} {:x}",
                    info.name,
                    info.base_priority,
                    match info.state {
                        ThreadExecutionState::Running => "Exec ",
                        ThreadExecutionState::Ready => "Ready",
                        ThreadExecutionState::Blocked => "Block",
                        _ => "?",
                    },
                    info.entry as usize,
                );
            }
        });
}
