use crate::cell::{LockedCell, LockedRefCell, RefMut};
use crate::events::REQUIRE_ALL_EVENTS;
use crate::kernel::list::{impl_linked, LinkedList, LinkedListNode, LinkedListTag};
use crate::kernel::tracing;
use crate::kernel::{
    atomic_list::AtomicQueue,
    hal::{
        disable_alarm_interrupt, disable_interrupts, enable_alarm_interrupt, enable_interrupts,
        set_alarm, start_first_thread, Context,
    },
    handle_runtime_error,
    interrupt::{current_interrupt, in_interrupt, set_ceiling_threshold, RawInterruptHandler},
    priority::{AnyPriority, Priority, PriorityStatus},
    syscall,
    waiter::{SleepQueueTag, Suspendable, SuspendableKind, WaitQueueTag},
    RuntimeError, Stack, ThreadPriority,
};
use crate::printkln;
use crate::sync::{
    interrupt_lock::InterruptLockKey, preempt_lock::PreemptLockKey, InterruptLock, KeyToken, Lock,
    PreemptLock, RawCeilingLock,
};
use crate::thread::{
    RawThread, Thread, ThreadExecutionState, ThreadInfo, IDLE_THREAD_ID, INVALID_THREAD_ID,
};
use core::cell::SyncUnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use scars_khal::{ContextInfo, FlowController};

pub struct ExecStateTag {}

impl LinkedListTag for ExecStateTag {}

extern "C" {
    static _isr_stack_end: u8;
}

pub enum ExecutionContext {
    Interrupt(&'static RawInterruptHandler),
    Thread(&'static RawThread),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum RescheduleKind {
    /// No rescheduling
    None = 0,

    /// Yield to thread with higher priority
    YieldToHigher = 1,

    /// Yield to thread with equal or higher priority.
    /// YieldToHigher is a subset of YieldToEqualOrHigher.
    YieldToEqualOrHigher = 2,
}

pub struct AtomicRescheduleKind(AtomicUsize);

impl AtomicRescheduleKind {
    pub(crate) const fn new(kind: RescheduleKind) -> AtomicRescheduleKind {
        AtomicRescheduleKind(AtomicUsize::new(kind as usize))
    }

    pub(crate) fn take(&self) -> RescheduleKind {
        unsafe {
            core::mem::transmute(self.0.swap(RescheduleKind::None as usize, Ordering::SeqCst))
        }
    }

    pub(crate) fn update(&self, value: RescheduleKind) {
        match value {
            RescheduleKind::None => (),
            RescheduleKind::YieldToHigher => {
                let _ = self.0.compare_exchange(
                    RescheduleKind::None as usize,
                    RescheduleKind::YieldToHigher as usize,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
            }
            RescheduleKind::YieldToEqualOrHigher => {
                self.0.store(
                    RescheduleKind::YieldToEqualOrHigher as usize,
                    Ordering::SeqCst,
                );
            }
        }
    }

    pub(crate) fn is_some(&self) -> bool {
        let kind = unsafe { core::mem::transmute(self.0.load(Ordering::SeqCst)) };
        match kind {
            RescheduleKind::None => false,
            _ => true,
        }
    }
}

/// Schedulables waiting to be resumed when preemption lock is released
static PENDING_RESUME: AtomicQueue<Suspendable, ExecStateTag> = AtomicQueue::new();

/// Schedulables waiting to go to sleep when preemption lock is released.
/// Since tasks cannot sleep while holding the pre-emption lock, these are
/// always interrupt handlers waiting to be polled at given deadline.
static PENDING_SLEEP: AtomicQueue<Suspendable, ExecStateTag> = AtomicQueue::new();

// The kind of pending reschedule. Rescheduling may be triggered by different
// events, but they are always executed either at the end of interrupt handling,
// or when the preemption lock is released.
static PENDING_RESCHEDULE_KIND: AtomicRescheduleKind =
    AtomicRescheduleKind::new(RescheduleKind::None);

// Scheduler is accessible only while holding the preemption lock
static SCHEDULER: SyncUnsafeCell<MaybeUninit<LockedRefCell<Scheduler, PreemptLock>>> =
    SyncUnsafeCell::new(MaybeUninit::uninit());

#[no_mangle]
static CURRENT_THREAD_CONTEXT: AtomicPtr<Context> = AtomicPtr::new(core::ptr::null_mut());

pub struct Scheduler {
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

    // A wakeup-time sorted queue of suspendables that are waiting to be woken up at a specific time.
    sleep_queue: LinkedList<Suspendable, SleepQueueTag>,

    idle_thread: &'static RawThread,

    // Currently running thread on state Running
    current_thread: &'static RawThread,
}

impl Scheduler {
    pub(crate) fn new(idle_thread: &'static RawThread) -> Scheduler {
        Scheduler {
            ready_queue: LinkedList::new(),
            sleep_queue: LinkedList::new(),
            suspended_list: LinkedList::new(),
            blocked_list: LinkedList::new(),
            idle_thread,
            current_thread: idle_thread,
        }
    }

    pub(super) fn start() -> ! {
        let idle_thread = crate::kernel::idle::init_idle_thread();
        unsafe {
            let _ = (&mut *SCHEDULER.get()).write(LockedRefCell::new(Scheduler::new(idle_thread)));
        }
        enable_alarm_interrupt();
        let idle_context = idle_thread.context.as_ptr() as *mut _;
        start_first_thread(idle_context)
    }

    fn borrow_mut(pkey: PreemptLockKey<'_>) -> RefMut<'_, Scheduler> {
        unsafe { (&*SCHEDULER.get()).assume_init_ref().borrow_mut(pkey) }
    }

    pub(crate) fn current_thread_context() -> *mut Context {
        unsafe { &*(&*SCHEDULER.get()).assume_init_ref().as_ptr() }
            .current_thread
            .context
            .as_ptr() as *mut _
    }

    pub(crate) fn current_execution_context() -> ExecutionContext {
        match current_interrupt() {
            Some(interrupt_context) => {
                ExecutionContext::Interrupt(unsafe { interrupt_context.as_ref() })
            }
            None => ExecutionContext::Thread(
                unsafe { &*(&*SCHEDULER.get()).assume_init_ref().as_ptr() }.current_thread,
            ),
        }
    }
}

// Queue operations
impl Scheduler {
    fn insert_to_ready_queue(&mut self, pkey: PreemptLockKey<'_>, thread: &'static RawThread) {
        thread.state.set(pkey, ThreadExecutionState::Ready);
        if thread.thread_id == self.idle_thread.thread_id {
            // Idle thread is always ready, but it is never inserted to ready queue.
            return;
        }
        tracing::thread_ready_begin(thread.as_ref());
        let thread_priority = thread.active_priority();
        if !thread.lock_priority().is_valid() {
            // If thread is not holding any locks, then thread goes to the back of its priority queue
            self.ready_queue
                .insert_after(Pin::static_ref(thread), |queue_thread| {
                    queue_thread.active_priority() >= thread_priority
                });
        } else {
            // If thread is holding any locks, then it goes to the front of its priority queue
            self.ready_queue
                .insert_after(Pin::static_ref(thread), |queue_thread| {
                    queue_thread.active_priority() > thread_priority
                });
        }
    }

    fn insert_to_blocked_queue(&mut self, pkey: PreemptLockKey<'_>, thread: &'static RawThread) {
        thread.state.set(pkey, ThreadExecutionState::Blocked);
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not block");
        }

        tracing::thread_ready_end(thread.as_ref());
        let thread_priority = thread.lock_priority();
        self.blocked_list
            .insert_after(Pin::static_ref(thread), |queue_thread| {
                let queue_thread_priority = queue_thread.lock_priority();

                if queue_thread_priority.is_valid() && thread_priority.is_valid() {
                    queue_thread_priority >= thread_priority
                } else if queue_thread_priority.is_valid() {
                    true
                } else if thread_priority.is_valid() {
                    false
                } else {
                    false
                }
            });
    }

    fn insert_to_suspended_list(&mut self, pkey: PreemptLockKey<'_>, thread: &'static RawThread) {
        thread.state.set(pkey, ThreadExecutionState::Suspended);
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not suspend");
        }

        tracing::thread_ready_end(thread.as_ref());
        self.suspended_list.push_back(Pin::static_ref(thread));
    }

    fn insert_to_wakeup_queue(&mut self, pkey: PreemptLockKey<'_>, thread: &'static RawThread) {
        thread.state.set(pkey, ThreadExecutionState::Blocked);
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not block");
        }

        tracing::thread_ready_end(thread.as_ref());
        let deadline = thread.suspendable.deadline();
        self.sleep_queue.insert_after(
            unsafe { Pin::new_unchecked(&thread.suspendable) },
            |queue_thread| queue_thread.deadline() <= deadline,
        );
    }
}

// Suspendable operations. Suspendable is an abstraction for operations that can be suspended
// with optional deadline, and resumed later.
impl Scheduler {
    // Resume thread due to wakeup while rescheduling
    fn wakeup_thread(&mut self, pkey: PreemptLockKey<'_>, thread: &'static RawThread) {
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not be woken up");
        }

        match thread.state.get(pkey) {
            ThreadExecutionState::Blocked => {
                thread.set_wakeup_event();
                self.blocked_list.remove(Pin::static_ref(thread));
                self.insert_to_ready_queue(pkey, thread);
            }
            _ => (),
        }

        // Note: does not check for need to reschedule, as this is called from reschedule.
    }

    fn wakeup_interrupt(
        &mut self,
        _pkey: PreemptLockKey<'_>,
        interrupt: &'static RawInterruptHandler,
    ) {
        // Interrupts cannot go to sleep, so they cannot be woken up.
        // Asynchronous tasks associated with the interrupt can go to sleep, and be woken up.
        // Do not poll executor here, as we are holding the preemption lock.
        interrupt.set_pending_executor_poll();
    }

    fn wakeup_suspendable(&mut self, pkey: PreemptLockKey<'_>, suspendable: &Suspendable) {
        match suspendable.kind {
            SuspendableKind::Thread(thread_ptr) => {
                let thread = unsafe { &*thread_ptr };
                self.wakeup_thread(pkey, thread)
            }
            SuspendableKind::Interrupt(interrupt_ptr) => {
                let interrupt = unsafe { &*interrupt_ptr };
                self.wakeup_interrupt(pkey, interrupt);
            }
            SuspendableKind::Async(_, ref waker) => {
                waker.wake_by_ref();
            }
            SuspendableKind::None => (),
        }

        // Note: does not check for need to reschedule, as this is called from reschedule.
    }

    fn resume_suspendable_now(&mut self, pkey: PreemptLockKey<'_>, suspendable: &Suspendable) {
        match suspendable.kind {
            SuspendableKind::Thread(thread_ptr) => {
                let thread = unsafe { &*thread_ptr };
                self.resume_thread_now(pkey, thread);
            }
            SuspendableKind::Interrupt(interrupt) => {
                let interrupt = unsafe { &*interrupt };
                self.resume_interrupt_now(pkey, interrupt);
            }
            SuspendableKind::Async(_, ref waker) => {
                waker.wake_by_ref();
            }
            SuspendableKind::None => (),
        }
    }

    fn suspend_suspendable_now(&mut self, pkey: PreemptLockKey<'_>, suspendable: &Suspendable) {
        match suspendable.kind {
            SuspendableKind::Thread(_thread_ptr) => {
                // No threads are put to the pending sleep queue yet.
                unimplemented!();
            }
            SuspendableKind::Interrupt(interrupt) => {
                let interrupt = unsafe { &*interrupt };
                self.schedule_interrupt_wakeup_now(pkey, interrupt);
            }
            SuspendableKind::Async(_, _) => {
                // Asynchronous tasks cannot be suspended yet
                unimplemented!()
            }
            SuspendableKind::None => (),
        }

        self.reprogram_alarm(pkey);
    }
}

// Interrupt scheduling
impl Scheduler {
    fn schedule_interrupt_wakeup_now(
        &mut self,
        pkey: PreemptLockKey<'_>,
        interrupt: &RawInterruptHandler,
    ) {
        if interrupt.suspendable.in_sleep_queue() {
            self.sleep_queue
                .remove(unsafe { Pin::new_unchecked(&interrupt.suspendable) });
        }
        let deadline = interrupt.suspendable.deadline();
        self.sleep_queue.insert_after(
            unsafe { Pin::new_unchecked(&interrupt.suspendable) },
            |queue_interrupt| queue_interrupt.deadline() <= deadline,
        );
        self.reprogram_alarm(pkey);
    }

    /// Puts the interrupt handler into scheduler sleep queue to be polled later at given time.
    pub(crate) fn schedule_interrupt_wakeup(interrupt: &RawInterruptHandler, wakeup_time: u64) {
        // TODO: setting the deadline should be legal only if not already set
        interrupt
            .suspendable
            .set_deadline(Some(crate::Instant { tick: wakeup_time }));

        if let Err(_) = PreemptLock::try_with(|pkey| {
            let mut scheduler = Scheduler::borrow_mut(pkey);
            scheduler.schedule_interrupt_wakeup_now(pkey, interrupt);
        }) {
            // Preemption lock is held by a thread or lower priority interrupt handler.
            // Delayed insertion to sleep queue will be executed when the lock is released.
            PENDING_SLEEP.push_back(&interrupt.suspendable);
        };
    }

    fn resume_interrupt_now(
        &mut self,
        _pkey: PreemptLockKey<'_>,
        interrupt: &'static RawInterruptHandler,
    ) {
        // Interrupts cannot block, so the interrupt handler function cannot be resumed.
        // Asynchronous tasks associated with the interrupt can block, and be resumed.

        if interrupt.suspendable.in_sleep_queue() {
            self.sleep_queue
                .remove(Pin::static_ref(&interrupt.suspendable));
        }

        interrupt.set_pending_executor_poll();
    }

    // Thread or ISR context
    #[allow(dead_code)]
    pub(crate) fn resume_interrupt(interrupt: &'static RawInterruptHandler) {
        if let Err(_) = PreemptLock::try_with(|pkey| {
            let mut scheduler = Scheduler::borrow_mut(pkey);
            scheduler.resume_interrupt_now(pkey, interrupt);
        }) {
            // Could not acquire pre-emption lock in ISR, because some thread or ongoing lower
            // priority ISR holds the lock.
            // Store unblocked thread in pending ready list instead, from which it will be
            // moved to ready list when the preempt lock is released.
            PENDING_RESUME.push_back(&interrupt.suspendable);
        };
    }
}

// Thread scheduling
impl Scheduler {
    // Resume thread due to notification. Will set pending reschedule flag if the resumed thread has
    // higher priority than the current thread.
    fn resume_thread_now(&mut self, pkey: PreemptLockKey<'_>, thread: &'static RawThread) {
        match thread.state.get(pkey) {
            ThreadExecutionState::Ready | ThreadExecutionState::Running => {
                // Thread is already in ready queue or running
            }
            ThreadExecutionState::Blocked => {
                thread.set_resume_event();
                // Remove from sleep queue if blocking operation has deadline
                if thread.suspendable.in_sleep_queue() {
                    self.sleep_queue
                        .remove(Pin::static_ref(&thread.suspendable));
                }
                self.blocked_list.remove(Pin::static_ref(thread));
                self.insert_to_ready_queue(pkey, thread);
            }
            ThreadExecutionState::Suspended => {
                thread.set_resume_event();
                self.suspended_list.remove(Pin::static_ref(thread));
                self.insert_to_ready_queue(pkey, thread);
            }
            ThreadExecutionState::Created => {
                // Created thread does not yet have a closure, so it cannot be resumed
                // until it becomes Started.
            }
            ThreadExecutionState::Started => {
                self.insert_to_ready_queue(pkey, thread);
            }
        }

        let current_priority = self.current_thread.active_priority();
        let locks_priority = self.locks_priority_ceiling();
        let min_priority = current_priority.max_valid(locks_priority);
        if min_priority < thread.active_priority() {
            Scheduler::set_pending_reschedule(RescheduleKind::YieldToHigher)
        }
    }

    // Thread or ISR context
    pub(crate) fn resume_thread(thread: &'static RawThread) {
        if let Err(_) = PreemptLock::try_with(|pkey| {
            let mut scheduler = Scheduler::borrow_mut(pkey);
            scheduler.resume_thread_now(pkey, thread);
        }) {
            // Could not acquire pre-emption lock in ISR, because some thread or ongoing lower
            // priority ISR holds the lock.
            // Store unblocked thread in pending ready list instead, from which it will be
            // moved to ready list when the preempt lock is released.
            PENDING_RESUME.push_back(&thread.suspendable);
        };
    }

    fn block_thread(
        &mut self,
        pkey: PreemptLockKey<'_>,
        thread: &'static RawThread,
        timeout_opt: Option<u64>,
    ) {
        match timeout_opt {
            Some(timeout) => {
                thread
                    .suspendable
                    .set_deadline(Some(crate::Instant { tick: timeout }));
                self.insert_to_wakeup_queue(pkey, thread);
                self.reprogram_alarm(pkey);
            }
            None => {
                thread.suspendable.set_deadline(None);
            }
        }

        self.insert_to_blocked_queue(pkey, thread);
    }

    fn suspend_thread(
        &mut self,
        pkey: PreemptLockKey<'_>,
        maybe_thread: Option<&'static RawThread>,
    ) {
        let thread = maybe_thread.unwrap_or(self.current_thread);

        match thread.state.get(pkey) {
            ThreadExecutionState::Ready => {
                self.ready_queue.remove(Pin::static_ref(thread));
                self.insert_to_suspended_list(pkey, thread);
            }
            ThreadExecutionState::Running => {
                if let Some(ready) = self.choose_thread_to_run(RescheduleKind::None) {
                    let previous_current = self.switch_thread(pkey, ready);
                    self.insert_to_suspended_list(pkey, previous_current);
                }
            }
            ThreadExecutionState::Blocked => {
                // A blocked thread cannot be suspended
                panic!("Cannot suspend blocked thread");
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

    pub(crate) fn suspend_thread_isr(maybe_thread: Option<&'static RawThread>) {
        if let Err(_) = PreemptLock::try_with(|pkey| {
            let mut scheduler = Scheduler::borrow_mut(pkey);
            scheduler.suspend_thread(pkey, maybe_thread);
        }) {
            panic!("Cannot suspend a task while holding the preemption lock");
        }
    }

    fn check_stack_overflow(&self) {
        if !unsafe { self.current_thread.stack.assume_init_ref() }.is_alive() {
            panic!(
                "Stack overflow detected from canary in thread {:?}",
                self.current_thread.name
            );
        }
    }

    /// Returns new thread to switch to, or `None` if staying in the current thread.
    fn choose_thread_to_run<'a>(&mut self, kind: RescheduleKind) -> Option<&'a RawThread> {
        let current_priority = self.current_thread.active_priority();

        // Highest priority of any locks held by the current or blocked threads.
        let locks_priority = self.locks_priority_ceiling();

        // Minimum priority of a thread that is allowed to run next
        let min_priority = match kind {
            RescheduleKind::YieldToEqualOrHigher => {
                current_priority.max_valid(locks_priority.succ())
            }
            RescheduleKind::YieldToHigher => {
                // Any thread that has higher priority than the current thread
                current_priority.max_valid(locks_priority).succ()
            }
            RescheduleKind::None => {
                // Any thread that is above the lock ceiling
                Priority::THREAD_MIN.max_valid(locks_priority.succ())
            }
        };

        // Return highest priority thread that is at or above the minimum priority allowed to run
        if let Some(ready_thread) = self.ready_queue.head() {
            if min_priority <= ready_thread.active_priority() {
                return self.ready_queue.pop_front();
            }
        }

        // If no suitable thread was found in ready queue, stay in the current thread
        // if allowed. Staying in current thread is allowed only when YieldEqualOrHigher.
        if kind == RescheduleKind::YieldToEqualOrHigher {
            return None;
        }

        // In None and YieldToHigher, the fallback is the Idle thread, which is special
        // in that it is not allowed to use CeilingLock, and therefore does not have
        // to strictly follow the ceiling locking protocol.
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            None
        } else {
            Some(self.idle_thread)
        }
    }

    fn switch_thread(
        &mut self,
        pkey: PreemptLockKey<'_>,
        new: &'static RawThread,
    ) -> &'static RawThread {
        // Whenever current thread is switched out, check its stack canary for
        // stack overflow that could have occurred during the thread execution.
        self.check_stack_overflow();
        //printkln!("[scheduler] switching to thread {}", new.name);
        new.state.set(pkey, ThreadExecutionState::Running);

        let new_thread_ref = new.as_ref();
        if new.thread_id == IDLE_THREAD_ID {
            tracing::system_idle();
        }

        CURRENT_THREAD_CONTEXT.store(new.context.as_ptr() as *mut _, Ordering::Relaxed);
        let old = core::mem::replace(&mut self.current_thread, new);

        tracing::thread_exec_end(old.as_ref());
        tracing::thread_exec_begin(new_thread_ref);
        old.state.set(pkey, ThreadExecutionState::Ready);

        old
    }

    fn reprogram_alarm(&self, _pkey: PreemptLockKey<'_>) {
        match self.sleep_queue.head() {
            Some(sleeping_thread) => {
                if let Some(wakeup_time) = sleeping_thread.deadline() {
                    set_alarm(wakeup_time.tick);
                } else {
                    panic!("No wakeup time set for thread in wakeup queue");
                }
            }
            None => {
                // Disable wakeup
                set_alarm(u64::MAX);
            }
        }
    }

    fn locks_priority_ceiling(&self) -> PriorityStatus {
        if let Some(blocked_thread) = self.blocked_list.head() {
            let blocked_prio = blocked_thread.lock_priority();
            let current_prio = self.current_thread.lock_priority();
            blocked_prio.max(current_prio)
        } else {
            self.current_thread.lock_priority()
        }
    }

    fn reschedule<'key>(&mut self, pkey: PreemptLockKey<'key>, kind: RescheduleKind) {
        // Wakeup sleeping threads that should have been woken up
        let now = crate::clock_ticks();
        loop {
            if let Some(sleeping_thread) = self.sleep_queue.head() {
                if let Some(wakeup_time) = sleeping_thread.deadline() {
                    if wakeup_time.tick > now {
                        // No more threads to wake up
                        set_alarm(wakeup_time.tick);
                        break;
                    }
                }
            } else {
                // Wakeup queue is empty, disable wakeup
                set_alarm(u64::MAX);
                break;
            }

            if let Some(suspended) = self.sleep_queue.pop_front() {
                self.wakeup_suspendable(pkey, suspended)
            }
        }

        if let Some(ready) = self.choose_thread_to_run(kind) {
            let previous_current = self.switch_thread(pkey, ready);
            self.insert_to_ready_queue(pkey, previous_current);
        }
    }

    pub(crate) fn cond_reschedule<'key>(pkey: PreemptLockKey<'key>) {
        let scheduler = Scheduler::borrow_mut(pkey);
        if let Some(ready_thread) = scheduler.ready_queue.head() {
            if scheduler.current_thread.active_priority() < ready_thread.active_priority() {
                // Rescheduling will be executed when preemption lock is released
                Scheduler::set_pending_reschedule(RescheduleKind::YieldToHigher);
            }
        }
    }

    pub(crate) fn set_pending_reschedule(kind: RescheduleKind) {
        PENDING_RESCHEDULE_KIND.update(kind)
    }

    pub(crate) fn is_reschedule_pending() -> bool {
        PENDING_RESCHEDULE_KIND.is_some()
    }

    pub(crate) fn complete_pending(pkey: PreemptLockKey<'_>) {
        let mut scheduler = Scheduler::borrow_mut(pkey);
        while let Some(resuming) = PENDING_RESUME.pop_front() {
            scheduler.resume_suspendable_now(pkey, resuming);
        }

        while let Some(suspending) = PENDING_SLEEP.pop_front() {
            scheduler.suspend_suspendable_now(pkey, suspending);
        }
    }

    // ISR context
    pub(crate) fn execute_pending_reschedule() {
        match PENDING_RESCHEDULE_KIND.take() {
            RescheduleKind::None => (),
            kind => {
                if let Err(_) = PreemptLock::try_with(|pkey| {
                    let mut scheduler = Scheduler::borrow_mut(pkey);
                    scheduler.reschedule(pkey, kind);
                }) {
                    // Thread or lower priority interrupt handler is holding the lock.
                    // Postpone thread switch execution to lock release.
                    Scheduler::set_pending_reschedule(kind);
                };
            }
        }
    }

    pub(crate) fn start_thread_isr(thread: &'static mut RawThread) {
        thread.suspendable.set_thread(thread as *const _);
        tracing::thread_new(thread.as_ref());
        Scheduler::resume_thread(thread);
    }

    // Pre-emption
    // ISR context
    pub(crate) fn wakeup_scheduler_isr() {
        Scheduler::set_pending_reschedule(RescheduleKind::YieldToHigher);

        // Maybe not needed because wakeup interrupt is disabled
        //set_alarm(u64::MAX);
    }

    // Yield current thread
    // ISR context
    pub(crate) fn yield_current_thread_isr() {
        // Yielding can switch to another thread with equal priority
        Scheduler::set_pending_reschedule(RescheduleKind::YieldToEqualOrHigher);
    }

    // Blocking
    // ISR context
    pub(crate) fn wait_current_thread_isr(
        wait_list: *mut LinkedList<Suspendable, WaitQueueTag>,
        ceiling: Priority,
    ) {
        if let Err(_) = PreemptLock::try_with(|pkey| {
            let mut scheduler = Scheduler::borrow_mut(pkey);

            if scheduler.current_thread.thread_id == scheduler.idle_thread.thread_id {
                panic!("Idle thread cannot block");
            }

            match scheduler.choose_thread_to_run(RescheduleKind::None) {
                Some(ready) => {
                    let icb = unsafe { current_interrupt().unwrap().as_ref() };
                    let blocked_thread = scheduler.switch_thread(pkey, ready);

                    // Protect access to waiter queue with priority lock.
                    // SAFETY: list is accessed within raised priority section.
                    // TODO: priority is not immutable
                    let waiter_queue = unsafe { &mut *wait_list };
                    let old_prio = icb.raise_section_lock_priority(ceiling);

                    let thread_prio = blocked_thread.suspendable.priority();

                    waiter_queue.insert_after(
                        Pin::static_ref(&blocked_thread.suspendable),
                        |queue_waiter| queue_waiter.priority() >= thread_prio,
                    );

                    icb.set_section_lock_priority(old_prio);

                    scheduler.block_thread(pkey, blocked_thread, None);
                }
                None => {
                    // There should always be the idle thread to run
                    unreachable!()
                }
            }
        }) {
            // Error: Thread is blocking in a wait list while it holds the preempt lock.
            unreachable!()
        }
    }

    pub(crate) fn wait_current_thread_event_isr(mut events: u32, deadline: Option<u64>) -> u32 {
        match PreemptLock::try_with(|pkey| {
            let mut scheduler = Scheduler::borrow_mut(pkey);

            if scheduler.current_thread.thread_id == scheduler.idle_thread.thread_id {
                panic!("Idle thread cannot block");
            }

            // Set waited events mask. If any events are sent to the thread, it will
            // be notified if the required events are received.
            scheduler
                .current_thread
                .waited_events_mask
                .store(events, Ordering::SeqCst);

            // Extract require_all flag from the events mask.
            let require_all = events & REQUIRE_ALL_EVENTS != 0;
            events &= !REQUIRE_ALL_EVENTS;

            // Read and clear matching events from sent events mask.
            let received_events = scheduler
                .current_thread
                .sent_events_mask
                .fetch_and(!events, Ordering::SeqCst)
                & events;

            // Note: If there is a send_events call from ISR between the above read and clear,
            // and blocking of the thread, then the ISR will put the thread into pending resume
            // queue, and the thread will be unblocked when the preemption lock is released.

            // Block thread if required events are not received
            if (!require_all && received_events == 0) || (received_events & events) != events {
                match scheduler.choose_thread_to_run(RescheduleKind::None) {
                    Some(ready) => {
                        let blocked_thread = scheduler.switch_thread(pkey, ready);
                        scheduler.block_thread(pkey, blocked_thread, deadline);
                    }
                    None => {
                        // There should always be the idle thread to run
                        unreachable!()
                    }
                }
            }

            received_events
        }) {
            Ok(received_events) => received_events,
            Err(_) => {
                // Error: Thread is blocking to wait for events while it holds the preempt lock.
                unimplemented!()
            }
        }
    }

    // Delay
    // ISR context
    /// Puts the current thread into scheduler sleep queue to be woken up later at given time.
    pub(crate) fn delay_task_until(wakeup_time: u64) {
        if let Err(_) = PreemptLock::try_with(|pkey| {
            let mut scheduler = Scheduler::borrow_mut(pkey);
            if scheduler.current_thread.thread_id == scheduler.idle_thread.thread_id {
                panic!("Idle thread cannot block");
            }

            match scheduler.choose_thread_to_run(RescheduleKind::None) {
                Some(ready) => {
                    let previous_current = scheduler.switch_thread(pkey, ready);
                    scheduler.block_thread(pkey, previous_current, Some(wakeup_time));
                }
                None => {
                    // There should always be the idle thread to run
                    unreachable!()
                }
            }
        }) {
            // Error: Thread is trying to sleep while it holds the preempt lock.
            unimplemented!()
        };
    }

    pub(crate) fn threads(&self) -> impl Iterator<Item = &RawThread> {
        Some(self.idle_thread)
            .into_iter()
            .chain(Some(self.current_thread).into_iter())
            .chain(self.ready_queue.cursor_front())
            .chain(self.blocked_list.cursor_front())
            .chain(self.suspended_list.cursor_front())
    }

    pub fn thread_info<'a, 'key: 'a>(
        &'a self,
        pkey: PreemptLockKey<'key>,
    ) -> impl Iterator<Item = ThreadInfo> + '_ {
        self.threads().map(move |thread| thread.get_info(pkey))
    }
}

#[no_mangle]
pub fn _private_current_thread_context() -> *mut () {
    Scheduler::current_thread_context() as *mut ()
}

pub fn print_threads() {
    printkln!("NAME       PRI  STATUS ENTRY");
    PreemptLock::with(|pkey| {
        let scheduler = Scheduler::borrow_mut(pkey);
        for info in scheduler.thread_info(pkey) {
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
