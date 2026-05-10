pub mod event_queue;
pub mod timers;
mod work_queue;
use crate::Instant;
use crate::cell::{LockedCell, LockedPinRefCell, LockedRefCell, PinRefMut, RefMut};
use crate::events::raw::RawEventHandler;
use crate::interrupt::{
    RawInterruptHandler, current_interrupt, in_interrupt, set_ceiling_threshold,
};
use crate::kernel::list::{LinkedList, LinkedListNode, LinkedListTag, impl_linked};
use crate::kernel::tracing;
use crate::kernel::{
    RuntimeError, Stack, ThreadPriority,
    atomic_queue::{AtomicNode, AtomicQueue},
    exception::{KernelError, handle_kernel_error},
    hal::{self, Context, set_alarm, set_current_thread_context, start_first_thread},
    handle_runtime_error, syscall,
    waiter::{
        SUSPENDABLE_PENDING_RECONFIGURE, SUSPENDABLE_PENDING_RESUME, SUSPENDABLE_PENDING_SUSPEND,
        SUSPENDABLE_PENDING_WAKEUP, WaitQueueEntry, WaitQueueHandle, WaitQueueTag,
    },
};
use crate::printkln;
use crate::priority::{AnyPriority, AtomicPriorityStatus, Priority, PriorityStatus};
use crate::sync::preempt_lock::is_preempt_allowed;
use crate::sync::{
    InterruptLock, PreemptLock, RawCeilingLock, interrupt_lock::InterruptLockKey,
    preempt_lock::PreemptLockKey,
};
use crate::thread::{
    IDLE_THREAD_ID, INVALID_THREAD_ID, RawThread, Thread, ThreadExecutionState, ThreadInfo,
};
use core::cell::SyncUnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::Ordering;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize};
use event_queue::PendingEventsQueue;
use scars_khal::{ContextInfo, FlowController};
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

const RESCHEDULE_KIND_NONE: usize = 0;
const RESCHEDULE_KIND_YIELD_TO_HIGHER: usize = 1;
const RESCHEDULE_KIND_YIELD_TO_EQUAL: usize = 2;

pub struct RawScheduler {
    // When thread execution state is one of Ready, Blocked, or Suspended, it is in
    // one of the three queues/list. Threads in Created or Started state are not yet in
    // any scheduler queue. On state Running, the thread is the `current_thread`.

    // Threads that are ready to run are in the ready_queue sorted in descending
    // priority order.
    ready_queue: LinkedList<RawThread, ExecStateTag>,

    // Threads that are blocked in wait list, or waiting for timed wakeup, are in blocked
    // list sorted in descending lock priority order.
    blocked_list: LinkedList<RawThread, ExecStateTag>,

    // Threads that are suspended do not participate in thread scheduling.
    suspended_list: LinkedList<RawThread, ExecStateTag>,

    // A wakeup-time sorted queue of timers that are waiting to be woken up at a specific time.
    timer_queue: TimerQueue,

    idle_thread: Pin<&'static RawThread>,

    // Currently running thread on state Running
    current_thread: Pin<&'static RawThread>,
}

impl RawScheduler {
    pub(crate) fn new(idle_thread: &'static RawThread) -> RawScheduler {
        RawScheduler {
            ready_queue: LinkedList::new(),
            timer_queue: LinkedList::new(),
            suspended_list: LinkedList::new(),
            blocked_list: LinkedList::new(),
            idle_thread: Pin::static_ref(idle_thread),
            current_thread: Pin::static_ref(idle_thread),
        }
    }
}

// Pin projections from Pin<&RawScheduler> to pinned fields
impl RawScheduler {
    fn ready_queue(self: Pin<&Self>) -> Pin<&LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked(|s| &s.ready_queue) }
    }

    fn ready_queue_mut(self: Pin<&mut Self>) -> Pin<&mut LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.ready_queue) }
    }

    fn blocked_list(self: Pin<&Self>) -> Pin<&LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked(|s| &s.blocked_list) }
    }

    fn blocked_list_mut(self: Pin<&mut Self>) -> Pin<&mut LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.blocked_list) }
    }

    fn suspended_list(self: Pin<&Self>) -> Pin<&LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked(|s| &s.suspended_list) }
    }

    fn suspended_list_mut(self: Pin<&mut Self>) -> Pin<&mut LinkedList<RawThread, ExecStateTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.suspended_list) }
    }

    fn timer_queue(self: Pin<&Self>) -> Pin<&LinkedList<RawTimer, TimerQueueTag>> {
        unsafe { self.map_unchecked(|s| &s.timer_queue) }
    }

    pub(crate) fn timer_queue_mut(
        self: Pin<&mut Self>,
    ) -> Pin<&mut LinkedList<RawTimer, TimerQueueTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.timer_queue) }
    }

    fn current_thread_mut(self: Pin<&mut Self>) -> &mut Pin<&'static RawThread> {
        unsafe { &mut self.get_unchecked_mut().current_thread }
    }
}

// Queue operations
impl RawScheduler {
    fn insert_to_ready_queue(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        thread.state.set(pkey, ThreadExecutionState::Ready);
        if thread.thread_id == self.idle_thread.thread_id {
            // Idle thread is always ready, but it is never inserted to ready queue.
            return;
        }
        tracing::thread_ready_begin(thread.as_thread_ref());
        let thread_priority = thread.priority(pkey);
        if !thread.ceiling_lock_priority(pkey).is_valid() {
            // If thread is not holding any locks, then thread goes to the back of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.priority.get(pkey) >= thread_priority
            });
        } else {
            // If thread is holding any locks, then it goes to the front of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.priority.get(pkey) > thread_priority
            });
        }
    }

    // Reinsert when priority changes
    fn reinsert_to_ready_queue(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        // Reinsert to ready queue
        self.as_mut().ready_queue_mut().remove(thread);
        let thread_priority = thread.priority(pkey);
        if !thread.ceiling_lock_priority(pkey).is_valid() {
            // If thread is not holding any locks, then thread goes to the back of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.priority.get(pkey) >= thread_priority
            });
        } else {
            // If thread is holding any locks, then it goes to the front of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.priority.get(pkey) > thread_priority
            });
        }
    }

    fn blocked_list_order(
        pkey: PreemptLockKey<'_>,
        thread: &RawThread,
        thread_priority: PriorityStatus,
    ) -> bool {
        let queue_thread_priority =
            unsafe { Pin::new_unchecked(thread).ceiling_lock_priority(pkey) };

        if queue_thread_priority.is_valid() && thread_priority.is_valid() {
            queue_thread_priority >= thread_priority
        } else if queue_thread_priority.is_valid() {
            true
        } else if thread_priority.is_valid() {
            false
        } else {
            false
        }
    }

    fn insert_to_blocked_queue(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        thread.state.set(pkey, ThreadExecutionState::Blocked);
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not block");
        }

        tracing::thread_ready_end(thread.as_thread_ref());
        let thread_priority = thread.ceiling_lock_priority(pkey);
        self.blocked_list_mut()
            .insert_after(thread, |queue_thread| {
                Self::blocked_list_order(pkey, queue_thread, thread_priority)
            });
    }

    fn reinsert_to_blocked_queue(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        // Reinsert to blocked list
        self.as_mut().blocked_list_mut().remove(thread);
        let thread_priority = thread.ceiling_lock_priority(pkey);
        self.blocked_list_mut()
            .insert_after(thread, |queue_thread| {
                Self::blocked_list_order(pkey, queue_thread, thread_priority)
            });

        // If thread is waiting in a queue, reinsert to the wait queue
        match thread.wait_queue.get(pkey) {
            Some(wait_queue_handle) => unsafe {
                wait_queue_handle.reinsert(pkey, thread.get_wait_entry());
            },
            None => (),
        }
    }

    fn insert_to_suspended_list(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        thread.state.set(pkey, ThreadExecutionState::Suspended);
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not suspend");
        }

        tracing::thread_ready_end(thread.as_thread_ref());
        self.suspended_list_mut().push_back(thread);
    }
}

// Thread scheduling
impl RawScheduler {
    // Resume thread due to wakeup while rescheduling
    pub(crate) fn wakeup_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not be woken up");
        }

        // Remove thread from a wait queue if it is waiting in one, or postpone the operation,
        // if removal is not safe to do from the current context.
        if let Some(wait_queue) = thread.wait_queue.get(pkey) {
            let wait_entry = thread.get_wait_entry();

            // If the wait queue lock is a ceiling lock, and it is not safe to acquire
            // the lock from the current context, then the resuming operation is postponed
            // until it is safe to do so.
            if let Some(required_ceiling) = wait_queue.required_ceiling() {
                if Scheduler::current_priority(pkey) > required_ceiling {
                    Scheduler::schedule_deferred_operation(
                        thread.get_pending_work(),
                        SUSPENDABLE_PENDING_WAKEUP,
                        Some(required_ceiling),
                    );
                    return;
                }
            }

            unsafe {
                // `remove` will use the queue lock
                wait_queue.remove(pkey, wait_entry);
            }
            thread.set_wait_queue(None, pkey);
        }

        // Set timeout flag if thread has current wait events (indicating it timed out)
        let wait_events_ptr = thread
            .current_wait_events
            .load(core::sync::atomic::Ordering::SeqCst);
        if !wait_events_ptr.is_null() {
            let wait_events = unsafe { &*wait_events_ptr };
            wait_events.set_timed_out();
        }

        match thread.state.get(pkey) {
            ThreadExecutionState::Blocked => {
                self.as_mut().blocked_list_mut().remove(thread);
                self.insert_to_ready_queue(pkey, thread);
            }
            _ => (),
        }

        // Note: does not check for need to reschedule, as this is called from reschedule.
    }

    // Resume thread due to notification. Will set pending reschedule flag if the resumed thread has
    // higher priority than the current thread.
    fn resume_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        // Remove thread from a wait queue if it is waiting in one, or postpone the operation,
        // if removal is not safe to do from the current context.
        if let Some(wait_queue) = thread.wait_queue.get(pkey) {
            let wait_entry = thread.get_wait_entry();

            // If the wait queue lock is a ceiling lock, and it is not safe to acquire
            // the lock from the current context, then the resuming operation is postponed
            // until it is safe to do so.
            if let Some(required_ceiling) = wait_queue.required_ceiling() {
                if Scheduler::current_priority(pkey) > required_ceiling {
                    Scheduler::schedule_deferred_operation(
                        thread.get_pending_work(),
                        SUSPENDABLE_PENDING_RESUME,
                        Some(required_ceiling),
                    );
                    return;
                }
            }

            unsafe {
                // `remove` will use the queue lock
                wait_queue.remove(pkey, wait_entry);
            }
            thread.set_wait_queue(None, pkey);
        }

        match thread.state.get(pkey) {
            ThreadExecutionState::Ready | ThreadExecutionState::Running => {
                // Thread is already in ready queue or running
            }
            ThreadExecutionState::Blocked => {
                // Clear any wakeup deadline; this also removes the timer from
                // the timer queue and reprograms the alarm.
                thread.set_wakeup_deadline(pkey, self.as_mut(), None);
                self.as_mut().blocked_list_mut().remove(thread);
                self.as_mut().insert_to_ready_queue(pkey, thread);
            }
            ThreadExecutionState::Suspended => {
                self.as_mut().suspended_list_mut().remove(thread);
                self.as_mut().insert_to_ready_queue(pkey, thread);
            }
            ThreadExecutionState::Created => {
                // Created thread does not yet have a closure, so it cannot be resumed
                // until it becomes Started.
            }
            ThreadExecutionState::Started => {
                self.as_mut().insert_to_ready_queue(pkey, thread);
            }
        }

        let current_priority = self.current_thread.priority(pkey);
        let locks_priority = self.as_ref().locks_priority_ceiling(pkey);
        let min_priority = current_priority.max_valid(locks_priority);
        if min_priority < thread.priority(pkey) {
            Scheduler::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER)
        }
    }

    fn block_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
        timeout_opt: Option<u64>,
    ) {
        let deadline = timeout_opt.map(|tick| crate::Instant { tick });
        thread.set_wakeup_deadline(pkey, self.as_mut(), deadline);

        self.insert_to_blocked_queue(pkey, thread);
    }

    fn suspend_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        maybe_thread: Option<Pin<&'static RawThread>>,
    ) {
        let thread = maybe_thread.unwrap_or(self.current_thread);

        match thread.state.get(pkey) {
            ThreadExecutionState::Ready => {
                self.as_mut().ready_queue_mut().remove(thread);
                self.insert_to_suspended_list(pkey, thread);
            }
            ThreadExecutionState::Running => {
                // Highest priority of any locks held by the current or blocked threads.
                // Any ready thread above lock ceiling can run next. No lock should have
                // the minimum priority, so default lock priority to MIN.
                let locks_ceiling = self
                    .as_ref()
                    .locks_priority_ceiling(pkey)
                    .unwrap_or_default(Priority::MIN);

                // Highest priority ready thread that is above the lock ceiling
                // will be the next to run. Or if there is no ready thread above
                // the lock ceiling, then the idle thread will be the next to run.
                let next = self
                    .as_mut()
                    .ready_queue_mut()
                    .pop_front_if(|ready| ready.priority.get(pkey) > locks_ceiling)
                    .unwrap_or(self.as_ref().idle_thread);

                let previous = self.as_mut().switch_thread(pkey, next);
                self.insert_to_suspended_list(pkey, previous);
            }
            ThreadExecutionState::Blocked => {
                // A blocked thread holds its locks and prevents tasks below its priority
                // from running until it releases the locks, even when suspended.
                self.insert_to_suspended_list(pkey, thread);
            }
            ThreadExecutionState::Suspended => {
                // Thread is already suspended
            }
            ThreadExecutionState::Created => {
                // Created thread does not yet have a closure, so it cannot be suspended
                // until it becomes Started.
                panic!("Cannot suspend a thread that has not been started");
            }
            ThreadExecutionState::Started => {
                self.insert_to_suspended_list(pkey, thread);
            }
        }
    }

    fn check_stack_overflow(&self) {
        if !unsafe { self.current_thread.stack.assume_init_ref() }.is_alive() {
            let error = KernelError::StackOverflow {
                thread_name: self.current_thread.name,
                stack_size: unsafe { self.current_thread.stack.assume_init_ref().size() },
            };
            handle_kernel_error(&error);
        }
    }

    fn switch_thread(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        new: Pin<&'static RawThread>,
    ) -> Pin<&'static RawThread> {
        // Whenever current thread is switched out, check its stack canary for
        // stack overflow that could have occurred during the thread execution.
        self.check_stack_overflow();
        //printkln!("[scheduler] switching to thread {}", new.name);
        new.state.set(pkey, ThreadExecutionState::Running);

        let new_thread_ref = new.as_thread_ref();
        if new.thread_id == IDLE_THREAD_ID {
            tracing::system_idle();
        }

        set_current_thread_context(new.context.as_ptr());
        let old = core::mem::replace(self.current_thread_mut(), new);

        tracing::thread_exec_end(old.as_thread_ref());
        tracing::thread_exec_begin(new_thread_ref);
        old.state.set(pkey, ThreadExecutionState::Ready);

        old
    }

    pub(crate) fn reprogram_alarm(self: Pin<&Self>, pkey: PreemptLockKey<'_>) {
        match self.timer_queue().head() {
            Some(sleeping_thread) => {
                set_alarm(sleeping_thread.get_deadline(pkey).map(|d| d.tick));
            }
            None => {
                // Disable wakeup
                set_alarm(None);
            }
        }
    }

    fn locks_priority_ceiling(self: Pin<&Self>, pkey: PreemptLockKey<'_>) -> PriorityStatus {
        if let Some(blocked_thread) = self.blocked_list().head() {
            let blocked_prio = blocked_thread.ceiling_lock_priority(pkey);
            let current_prio = self.current_thread.ceiling_lock_priority(pkey);
            blocked_prio.max(current_prio)
        } else {
            self.current_thread.ceiling_lock_priority(pkey)
        }
    }

    fn reschedule<'key>(mut self: Pin<&mut Self>, pkey: PreemptLockKey<'key>, kind: usize) {
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

        let current_priority = self.current_thread.priority(pkey);

        let next = if (kind & RESCHEDULE_KIND_YIELD_TO_EQUAL) != 0 {
            // Any thread that has equal or higher priority than the current thread
            self.as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| ready.priority.get(pkey) >= current_priority)
                .unwrap_or(self.current_thread)
        } else if (kind & RESCHEDULE_KIND_YIELD_TO_HIGHER) != 0 {
            // Any thread that has higher priority than the current thread
            self.as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| ready.priority.get(pkey) > current_priority)
                .unwrap_or(self.current_thread)
        } else {
            unreachable!();
        };

        if next.thread_id != self.current_thread.thread_id {
            let previous = self.as_mut().switch_thread(pkey, next);
            self.insert_to_ready_queue(pkey, previous);
        }
    }

    fn delay_thread_until(mut self: Pin<&mut Self>, pkey: PreemptLockKey<'_>, wakeup_time: u64) {
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread cannot block");
        }

        // Highest priority of any locks held by the current or blocked threads.
        // Any ready thread above lock ceiling can run next.
        let locks_ceiling = self
            .as_ref()
            .locks_priority_ceiling(pkey)
            .unwrap_or_default(Priority::MIN);

        // Highest priority ready thread that is above the lock ceiling
        // will be the next to run. Or if there is no ready thread above
        // the lock ceiling, then the idle thread will be the next to run.
        let next = self
            .as_mut()
            .ready_queue_mut()
            .pop_front_if(|ready| ready.priority.get(pkey) > locks_ceiling)
            .unwrap_or(self.idle_thread);

        let previous = self.as_mut().switch_thread(pkey, next);
        self.as_mut()
            .block_thread(pkey, previous, Some(wakeup_time));
    }

    pub(crate) fn wait_current_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        _wait_queue: WaitQueueHandle,
    ) {
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread cannot block");
        }

        if self.current_thread.wait_queue.get(pkey).is_none() {
            // Thread has been removed from the wait queue before it could be blocked.
            // Blocking is cancelled.
            return;
        }

        // Highest priority of any locks held by the current or blocked threads.
        let locks_ceiling = self
            .as_ref()
            .locks_priority_ceiling(pkey)
            .unwrap_or_default(Priority::MIN);

        let next = self
            .as_mut()
            .ready_queue_mut()
            .pop_front_if(|ready| ready.priority.get(pkey) > locks_ceiling)
            .unwrap_or(self.idle_thread);

        let blocked_thread = self.as_mut().switch_thread(pkey, next);

        self.block_thread(pkey, blocked_thread, None);
    }

    pub(crate) fn wait_current_thread_event(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        wait_events: *mut crate::WaitEvents,
        deadline: Option<u64>,
    ) {
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread cannot block");
        }

        let wait_events = unsafe { &*wait_events };
        let waiting_thread = self.current_thread;

        // Set thread's current WaitEvents
        waiting_thread
            .current_wait_events
            .store(wait_events as *const _ as *mut _, Ordering::SeqCst);

        // Step 1: Read all currently pending events
        let all_pending = waiting_thread.pending_events.load(Ordering::SeqCst);

        // Step 2: Let WaitEvents process and store events atomically
        let (received_events, events_to_clear) =
            wait_events.process_and_store_pending_events(all_pending);

        // Step 3: Atomically clear the determined events
        waiting_thread
            .pending_events
            .fetch_and(!events_to_clear, Ordering::SeqCst);

        // Note: If there is a send_events call from ISR between the above operations
        // and blocking of the thread, then the ISR will put the thread into pending resume
        // queue, and the thread will be unblocked when the preemption lock is released.

        // Determine if thread should block based on WaitEvents configuration
        let should_block = wait_events.should_block(received_events);

        if should_block {
            // Highest priority of any locks held by the current or blocked threads.
            let locks_ceiling = self
                .as_ref()
                .locks_priority_ceiling(pkey)
                .unwrap_or_default(Priority::MIN);

            let next = self
                .as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| ready.priority.get(pkey) > locks_ceiling)
                .unwrap_or(self.idle_thread);

            let blocked_thread = self.as_mut().switch_thread(pkey, next);
            self.as_mut().block_thread(pkey, blocked_thread, deadline);
        }
    }

    pub(crate) fn threads(self: Pin<&Self>) -> impl Iterator<Item = Pin<&RawThread>> {
        Some(self.idle_thread)
            .into_iter()
            .chain(Some(self.current_thread).into_iter())
            .chain(self.ready_queue().cursor_front())
            .chain(self.blocked_list().cursor_front())
            .chain(self.suspended_list().cursor_front())
    }

    pub fn thread_info<'a, 'key: 'a>(
        self: Pin<&'a Self>,
        pkey: PreemptLockKey<'key>,
    ) -> impl Iterator<Item = ThreadInfo> {
        self.threads().map(move |thread| thread.get_info(pkey))
    }
}

static SCHEDULER_INITIALIZED: AtomicBool = AtomicBool::new(false);

static SCHEDULER: SyncUnsafeCell<MaybeUninit<Scheduler>> =
    SyncUnsafeCell::new(MaybeUninit::uninit());

pub struct Scheduler {
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
    current_ceiling_priority: AtomicPriorityStatus,

    raw: LockedPinRefCell<RawScheduler, PreemptLock>,
}

impl Scheduler {
    fn new(idle_thread: &'static RawThread) -> Scheduler {
        Scheduler {
            deferred_work_queue: WorkQueue::new(),
            pending_events: PendingEventsQueue::new(),
            pending_reschedule_kind: AtomicUsize::new(RESCHEDULE_KIND_NONE),
            current_ceiling_priority: AtomicPriorityStatus::new(PriorityStatus::invalid()),
            raw: LockedPinRefCell::new(RawScheduler::new(idle_thread)),
        }
    }

    pub(super) fn start() -> ! {
        let idle_thread = crate::kernel::idle::init_idle_thread();
        unsafe {
            let _ = (&mut *SCHEDULER.get()).write(Scheduler::new(idle_thread));
        }
        SCHEDULER_INITIALIZED.store(true, Ordering::Release);
        let idle_context = idle_thread.context.as_ptr() as *mut _;
        start_first_thread(idle_context)
    }

    /// True once `Scheduler::start` has finished installing the
    /// scheduler instance. Reading scheduler state before this returns
    /// `true` is undefined behaviour, so the kernel fault handler uses
    /// it to dispatch to a `BootstrapContext` instead.
    pub(crate) fn is_initialized() -> bool {
        SCHEDULER_INITIALIZED.load(Ordering::Acquire)
    }

    fn instance() -> &'static Scheduler {
        // SAFETY: The scheduler is initialized in the start function.
        unsafe { (&*SCHEDULER.get()).assume_init_ref() }
    }

    fn pin_instance() -> Pin<&'static Scheduler> {
        Pin::static_ref(Scheduler::instance())
    }

    fn borrow_mut<'lock, 'a: 'lock>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'lock>,
    ) -> PinRefMut<'lock, RawScheduler> {
        let raw = unsafe { self.map_unchecked(|s| &s.raw) };
        raw.borrow_mut(pkey)
    }

    pub(crate) fn current_execution_context() -> ExecutionContext {
        match current_interrupt() {
            Some(interrupt_context) => ExecutionContext::Interrupt(unsafe {
                Pin::new_unchecked(interrupt_context.as_ref())
            }),
            None => ExecutionContext::Thread(
                unsafe { &*Scheduler::instance().raw.as_ptr() }
                    .current_thread
                    .as_ref(),
            ),
        }
    }

    pub(crate) fn current_priority(pkey: PreemptLockKey<'_>) -> Priority {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(thread) => thread.priority(pkey),
            ExecutionContext::Interrupt(interrupt) => interrupt.priority(),
        }
    }

    /// Get current ceiling priority from all held ceiling locks
    #[allow(dead_code)]
    pub(crate) fn current_ceiling_priority() -> PriorityStatus {
        Scheduler::instance()
            .current_ceiling_priority
            .load(Ordering::Acquire)
    }

    /// Update the current ceiling priority atomically
    pub(crate) fn update_ceiling_priority(new_ceiling: PriorityStatus) {
        Scheduler::instance()
            .current_ceiling_priority
            .store(new_ceiling, Ordering::Release);
    }

    /// Set ceiling priority, updating both global tracking and hardware threshold
    pub(crate) fn set_ceiling(ceiling: PriorityStatus) {
        // Update global ceiling priority tracking
        Self::update_ceiling_priority(ceiling);

        // Update hardware interrupt threshold
        crate::interrupt::set_ceiling_threshold(ceiling);
    }

    pub(crate) fn thread_priority_changed<'key>(
        pkey: PreemptLockKey<'key>,
        thread: Pin<&'static RawThread>,
    ) {
        let mut scheduler = Scheduler::pin_instance().borrow_mut(pkey);
        let mut pin_scheduler = scheduler.as_mut();

        match thread.state.get(pkey) {
            ThreadExecutionState::Ready => {
                // Reorder in the ready queue
                pin_scheduler.as_mut().reinsert_to_ready_queue(pkey, thread);

                if let Some(ready_thread) = pin_scheduler.as_ref().ready_queue().head() {
                    if pin_scheduler.current_thread.priority(pkey) < ready_thread.priority(pkey) {
                        // Rescheduling will be executed when preemption lock is released
                        Scheduler::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER);
                    }
                }
            }
            ThreadExecutionState::Blocked => {
                // Reorder in the blocked list, and wakeup queues if needed
                pin_scheduler.reinsert_to_blocked_queue(pkey, thread);
            }
            _ => (),
        }
    }

    pub(crate) fn cond_reschedule<'key>(pkey: PreemptLockKey<'key>) {
        let scheduler = Scheduler::pin_instance().borrow_mut(pkey);
        let pin_scheduler = scheduler.as_ref();

        if let Some(ready_thread) = pin_scheduler.ready_queue().head() {
            if pin_scheduler.current_thread.priority(pkey) < ready_thread.priority(pkey) {
                // Rescheduling will be executed when preemption lock is released
                Scheduler::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER);
            }
        }
    }

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

    pub(crate) fn schedule_deferred_operation(
        suspendable: Pin<&RawPendingWorkEntry>,
        mask: u32,
        required_ceiling: Option<Priority>,
    ) {
        let _ = Scheduler::instance().deferred_work_queue.queue_work(
            suspendable,
            mask,
            required_ceiling,
        );
    }

    pub(crate) fn complete_deferred_work(pkey: PreemptLockKey<'_>) {
        let scheduler = Scheduler::pin_instance();
        let mut raw_scheduler = scheduler.borrow_mut(pkey);
        Scheduler::instance()
            .deferred_work_queue
            .complete_work(pkey, raw_scheduler.as_mut());
    }

    pub(crate) fn is_deferred_work_pending() -> bool {
        let scheduler = Scheduler::instance();
        scheduler.deferred_work_queue.work_pending()
    }

    // ISR context
    pub(crate) fn execute_pending_reschedule() {
        match Scheduler::instance()
            .pending_reschedule_kind
            .swap(RESCHEDULE_KIND_NONE, Ordering::AcqRel)
        {
            RESCHEDULE_KIND_NONE => (),
            kind => {
                if let Err(_) = PreemptLock::try_with(|pkey| {
                    Scheduler::pin_instance()
                        .borrow_mut(pkey)
                        .as_mut()
                        .reschedule(pkey, kind);
                }) {
                    // Thread or lower priority interrupt handler is holding the lock.
                    // Postpone thread switch execution to lock release.
                    Scheduler::set_pending_reschedule(kind);
                };
            }
        }
    }

    // Thread or ISR context
    pub(crate) fn resume_thread(thread: Pin<&'static RawThread>) {
        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .resume_thread(pkey, thread);
        }) {
            Ok(()) => (),
            Err(_) => {
                // Could not acquire pre-emption lock, because some thread or ongoing lower
                // priority ISR holds the lock.
                // Store unblocked thread in pending ready list instead, from which it will be
                // moved to ready list when the preempt lock is released.
                let _ = Scheduler::schedule_deferred_operation(
                    thread.get_pending_work(),
                    SUSPENDABLE_PENDING_RESUME,
                    None,
                );
            }
        };
    }

    // Delay
    // ISR context
    /// Puts the current thread into scheduler sleep queue to be woken up later at given time.
    pub(crate) fn delay_thread_until(wakeup_time: u64) {
        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .delay_thread_until(pkey, wakeup_time);
        }) {
            Ok(()) => (),
            Err(_) => {
                // Error: Thread is trying to sleep while it holds the preempt lock.
                panic!("Thread is trying to sleep while it holds the preempt lock");
            }
        };
    }

    pub(crate) fn suspend_thread(maybe_thread: Option<Pin<&'static RawThread>>) {
        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .suspend_thread(pkey, maybe_thread);
        }) {
            Ok(()) => (),
            Err(_) => match maybe_thread {
                Some(thread) => {
                    Scheduler::schedule_deferred_operation(
                        thread.get_pending_work(),
                        SUSPENDABLE_PENDING_SUSPEND,
                        None,
                    );
                }
                None => {
                    panic!("Cannot suspend the current thread while holding the preemption lock")
                }
            },
        }
    }

    pub(crate) fn start_thread(mut thread: Pin<&'static mut RawThread>) {
        unsafe {
            let ptr: *mut RawThread = thread.as_mut().get_unchecked_mut();
            RawThread::init_at(ptr);
            //thread.as_mut().init_at();
        }

        crate::printkln!("Starting thread {}", thread.name);

        // Thread mutability ends
        let thread = thread.into_ref();
        tracing::thread_new(thread.as_thread_ref());
        Scheduler::resume_thread(thread);
    }

    // Pre-emption
    // ISR context
    pub(crate) fn wakeup_scheduler_isr() {
        Scheduler::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER);
    }

    // Yield current thread
    // ISR context
    pub(crate) fn yield_current_thread_isr() {
        // Yielding can switch to another thread with equal priority
        Scheduler::set_pending_reschedule(
            RESCHEDULE_KIND_YIELD_TO_EQUAL | RESCHEDULE_KIND_YIELD_TO_HIGHER,
        );
    }

    // Blocking
    // ISR context
    pub(crate) fn wait_current_thread_isr(wait_list: WaitQueueHandle) {
        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .wait_current_thread(pkey, wait_list);
        }) {
            Ok(()) => (),
            Err(_) => {
                // Error: Thread is blocking in a wait list while it holds the preempt lock.
                unreachable!()
            }
        }
    }

    pub(crate) fn wait_current_thread_event_isr(
        wait_events: *mut crate::WaitEvents,
        deadline: Option<u64>,
    ) {
        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .wait_current_thread_event(pkey, wait_events, deadline)
        }) {
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
    PreemptLock::with(|pkey| {
        let scheduler = Scheduler::pin_instance().borrow_mut(pkey);
        let pin_scheduler = scheduler.as_ref();
        for info in pin_scheduler.thread_info(pkey) {
            printkln!(
                "{:<10} {:<4}   {:<4} {:x?}",
                info.name,
                info.base_priority,
                match info.state {
                    ThreadExecutionState::Running => "Exec ",
                    ThreadExecutionState::Ready => "Ready",
                    ThreadExecutionState::Blocked => "Block",
                    _ => "?",
                },
                info.entry,
            );
        }
    });
}
