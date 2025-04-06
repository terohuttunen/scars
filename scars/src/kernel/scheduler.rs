use crate::cell::{LockedCell, LockedPinRefCell, LockedRefCell, PinRefMut, RefMut};
use crate::events::REQUIRE_ALL_EVENTS;
use crate::kernel::list::{LinkedList, LinkedListNode, LinkedListTag, impl_linked};
use crate::kernel::tracing;
use crate::kernel::{
    RuntimeError, Stack, ThreadPriority,
    atomic_queue::AtomicQueue,
    exception::{KernelError, handle_kernel_error},
    hal::{Context, set_alarm, set_current_thread_context, start_first_thread},
    handle_runtime_error,
    interrupt::{RawInterruptHandler, current_interrupt, in_interrupt, set_ceiling_threshold},
    priority::{AnyPriority, Priority, PriorityStatus},
    syscall,
    waiter::{SleepQueueTag, Suspendable, SuspendableKind, WaitQueueTag},
};
use crate::Instant;
use crate::printkln;
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
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicUsize, Ordering};
use scars_khal::{ContextInfo, FlowController};

use super::hal::current_thread_context;

pub struct ExecStateTag {}

impl LinkedListTag for ExecStateTag {}

unsafe extern "C" {
    static _isr_stack_end: u8;
}

pub enum ExecutionContext {
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

    // A wakeup-time sorted queue of suspendables that are waiting to be woken up at a specific time.
    sleep_queue: LinkedList<Suspendable, SleepQueueTag>,

    idle_thread: Pin<&'static RawThread>,

    // Currently running thread on state Running
    current_thread: Pin<&'static RawThread>,
}

impl RawScheduler {
    pub(crate) fn new(idle_thread: &'static RawThread) -> RawScheduler {
        RawScheduler {
            ready_queue: LinkedList::new(),
            sleep_queue: LinkedList::new(),
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

    fn sleep_queue(self: Pin<&Self>) -> Pin<&LinkedList<Suspendable, SleepQueueTag>> {
        unsafe { self.map_unchecked(|s| &s.sleep_queue) }
    }

    fn sleep_queue_mut(self: Pin<&mut Self>) -> Pin<&mut LinkedList<Suspendable, SleepQueueTag>> {
        unsafe { self.map_unchecked_mut(|s| &mut s.sleep_queue) }
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
        let thread_priority = thread.active_priority();
        if !thread.lock_priority().is_valid() {
            // If thread is not holding any locks, then thread goes to the back of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.active_priority() >= thread_priority
            });
        } else {
            // If thread is holding any locks, then it goes to the front of its priority queue
            self.ready_queue_mut().insert_after(thread, |queue_thread| {
                queue_thread.active_priority() > thread_priority
            });
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
        let thread_priority = thread.lock_priority();
        self.blocked_list_mut()
            .insert_after(thread, |queue_thread| {
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

    fn insert_to_wakeup_queue(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        thread.state.set(pkey, ThreadExecutionState::Blocked);
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not block");
        }

        tracing::thread_ready_end(thread.as_thread_ref());
        let deadline = thread.suspendable.deadline();
        self.sleep_queue_mut()
            .insert_after(thread.suspendable_ref(), |queue_thread| {
                queue_thread.deadline() <= deadline
            });
    }
}

// Suspendable operations. Suspendable is an abstraction for operations that can be suspended
// with optional deadline, and resumed later.
impl RawScheduler {
    // Resume thread due to wakeup while rescheduling
    fn wakeup_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        if thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread may not be woken up");
        }

        match thread.state.get(pkey) {
            ThreadExecutionState::Blocked => {
                thread.set_wakeup_event();

                self.as_mut().blocked_list_mut().remove(thread);
                self.insert_to_ready_queue(pkey, thread);
            }
            _ => (),
        }

        // Note: does not check for need to reschedule, as this is called from reschedule.
    }

    fn wakeup_interrupt(
        self: Pin<&mut Self>,
        _pkey: PreemptLockKey<'_>,
        interrupt: &'static RawInterruptHandler,
    ) {
        // Interrupts cannot go to sleep, so they cannot be woken up.
        // Asynchronous tasks associated with the interrupt can go to sleep, and be woken up.
        // Do not poll executor here, as we are holding the preemption lock.
        interrupt.set_pending_executor_poll();
    }

    fn wakeup_suspendable(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        suspendable: Pin<&Suspendable>,
    ) {
        match suspendable.kind {
            SuspendableKind::Thread(thread_ptr) => {
                let thread = unsafe { Pin::new_unchecked(&*thread_ptr) };
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

    fn resume_suspendable_now(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        suspendable: Pin<&Suspendable>,
    ) {
        match suspendable.kind {
            SuspendableKind::Thread(thread_ptr) => {
                let thread = unsafe { Pin::new_unchecked(&*thread_ptr) };
                self.resume_thread(pkey, thread);
            }
            SuspendableKind::Interrupt(interrupt) => {
                let interrupt = unsafe { &*interrupt };
                self.resume_interrupt(pkey, interrupt);
            }
            SuspendableKind::Async(_, ref waker) => {
                waker.wake_by_ref();
            }
            SuspendableKind::None => (),
        }
    }

    fn suspend_suspendable_now(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        suspendable: Pin<&Suspendable>,
    ) {
        match suspendable.kind {
            SuspendableKind::Thread(_thread_ptr) => {
                // No threads are put to the pending sleep queue yet.
                unimplemented!();
            }
            SuspendableKind::Interrupt(interrupt) => {
                let interrupt = unsafe { &*interrupt };
                self.as_mut().schedule_interrupt_wakeup(pkey, interrupt);
            }
            SuspendableKind::Async(_, _) => {
                // Asynchronous tasks cannot be suspended yet
                unimplemented!()
            }
            SuspendableKind::None => (),
        }

        self.as_ref().reprogram_alarm(pkey);
    }
}

// Interrupt scheduling
impl RawScheduler {
    fn schedule_interrupt_wakeup(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        interrupt: &'static RawInterruptHandler,
    ) {
        if interrupt.suspendable.in_sleep_queue() {
            self.as_mut()
                .sleep_queue_mut()
                .remove(Pin::static_ref(&interrupt.suspendable));
        }
        let deadline = interrupt.suspendable.deadline();
        self.as_mut()
            .sleep_queue_mut()
            .insert_after(Pin::static_ref(&interrupt.suspendable), |queue_interrupt| {
                queue_interrupt.deadline() <= deadline
            });
        self.as_ref().reprogram_alarm(pkey);
    }

    fn resume_interrupt(
        self: Pin<&mut Self>,
        _pkey: PreemptLockKey<'_>,
        interrupt: &'static RawInterruptHandler,
    ) {
        // Interrupts cannot block, so the interrupt handler function cannot be resumed.
        // Asynchronous tasks associated with the interrupt can block, and be resumed.

        if interrupt.suspendable.in_sleep_queue() {
            self.sleep_queue_mut()
                .remove(Pin::static_ref(&interrupt.suspendable));
        }

        interrupt.set_pending_executor_poll();
    }
}

// Thread scheduling
impl RawScheduler {
    // Resume thread due to notification. Will set pending reschedule flag if the resumed thread has
    // higher priority than the current thread.
    fn resume_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
    ) {
        match thread.state.get(pkey) {
            ThreadExecutionState::Ready | ThreadExecutionState::Running => {
                // Thread is already in ready queue or running
            }
            ThreadExecutionState::Blocked => {
                thread.set_resume_event();
                // Remove from sleep queue if blocking operation has deadline
                let suspendable = thread.suspendable_ref();
                if suspendable.in_sleep_queue() {
                    self.as_mut().sleep_queue_mut().remove(suspendable);
                }
                self.as_mut().blocked_list_mut().remove(thread);
                self.as_mut().insert_to_ready_queue(pkey, thread);
            }
            ThreadExecutionState::Suspended => {
                thread.set_resume_event();
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

        let current_priority = self.current_thread.active_priority();
        let locks_priority = self.as_ref().locks_priority_ceiling();
        let min_priority = current_priority.max_valid(locks_priority);
        if min_priority < thread.active_priority() {
            Scheduler::set_pending_reschedule(RESCHEDULE_KIND_YIELD_TO_HIGHER)
        }
    }

    fn block_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        thread: Pin<&'static RawThread>,
        timeout_opt: Option<u64>,
    ) {
        match timeout_opt {
            Some(timeout) => {
                thread
                    .suspendable
                    .set_deadline(Some(crate::Instant { tick: timeout }));
                self.as_mut().insert_to_wakeup_queue(pkey, thread);
                self.as_ref().reprogram_alarm(pkey);
            }
            None => {
                thread.suspendable.set_deadline(None);
            }
        }

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
                    .locks_priority_ceiling()
                    .unwrap_or_default(Priority::MIN);

                // Highest priority ready thread that is above the lock ceiling
                // will be the next to run. Or if there is no ready thread above
                // the lock ceiling, then the idle thread will be the next to run.
                let next = self
                    .as_mut()
                    .ready_queue_mut()
                    .pop_front_if(|ready| ready.active_priority() > locks_ceiling)
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

    fn reprogram_alarm(self: Pin<&Self>, _pkey: PreemptLockKey<'_>) {
        match self.sleep_queue().head() {
            Some(sleeping_thread) => {
                set_alarm(sleeping_thread.deadline().map(|d| d.tick));
            }
            None => {
                // Disable wakeup
                set_alarm(None);
            }
        }
    }

    fn locks_priority_ceiling(self: Pin<&Self>) -> PriorityStatus {
        if let Some(blocked_thread) = self.blocked_list().head() {
            let blocked_prio = blocked_thread.lock_priority();
            let current_prio = self.current_thread.lock_priority();
            blocked_prio.max(current_prio)
        } else {
            self.current_thread.lock_priority()
        }
    }

    fn reschedule<'key>(mut self: Pin<&mut Self>, pkey: PreemptLockKey<'key>, kind: usize) {
        // Wakeup sleeping threads that should have been woken up
        let now = Instant::now();
        loop {
            if let Some(sleeping_thread) = self.as_ref().sleep_queue().head() {
                if let Some(wakeup_time) = sleeping_thread.deadline() {
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

            if let Some(suspended) = self.as_mut().sleep_queue_mut().pop_front() {
                self.as_mut().wakeup_suspendable(pkey, suspended)
            }
        }

        let current_priority = self.current_thread.active_priority();

        // Minimum priority of a thread that is allowed to run next

        let next = if (kind & RESCHEDULE_KIND_YIELD_TO_EQUAL) != 0 {
            // Any thread that has equal or higher priority than the current thread
            self.as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| ready.active_priority() >= current_priority)
                .unwrap_or(self.current_thread)
        } else if (kind & RESCHEDULE_KIND_YIELD_TO_HIGHER) != 0 {
            // Any thread that has higher priority than the current thread
            self.as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| ready.active_priority() > current_priority)
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
            .locks_priority_ceiling()
            .unwrap_or_default(Priority::MIN);

        // Highest priority ready thread that is above the lock ceiling
        // will be the next to run. Or if there is no ready thread above
        // the lock ceiling, then the idle thread will be the next to run.
        let next = self
            .as_mut()
            .ready_queue_mut()
            .pop_front_if(|ready| ready.active_priority() > locks_ceiling)
            .unwrap_or(self.idle_thread);

        let previous = self.as_mut().switch_thread(pkey, next);
        self.as_mut()
            .block_thread(pkey, previous, Some(wakeup_time));
    }

    pub(crate) fn wait_current_thread(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        wait_list: *mut LinkedList<Suspendable, WaitQueueTag>,
        ceiling: Priority,
    ) {
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread cannot block");
        }

        // Highest priority of any locks held by the current or blocked threads.
        let locks_ceiling = self
            .as_ref()
            .locks_priority_ceiling()
            .unwrap_or_default(Priority::MIN);

        let next = self
            .as_mut()
            .ready_queue_mut()
            .pop_front_if(|ready| ready.active_priority() > locks_ceiling)
            .unwrap_or(self.idle_thread);

        let blocked_thread = self.as_mut().switch_thread(pkey, next);

        let icb = unsafe { current_interrupt().unwrap().as_ref() };
        let suspendable = blocked_thread.suspendable_ref();

        // Protect access to waiter queue with priority lock.
        // SAFETY: list is accessed within raised priority section.
        // TODO: priority is not immutable
        let waiter_queue = unsafe { Pin::new_unchecked(&mut *wait_list) };
        let old_prio = icb.raise_nesting_lock_priority(ceiling);

        let thread_prio = suspendable.priority();

        waiter_queue.insert_after(suspendable, |queue_waiter| {
            queue_waiter.priority() >= thread_prio
        });

        icb.set_nesting_lock_priority(old_prio);

        self.block_thread(pkey, blocked_thread, None);
    }

    pub(crate) fn wait_current_thread_event(
        mut self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        mut events: u32,
        deadline: Option<u64>,
    ) -> u32 {
        if self.current_thread.thread_id == self.idle_thread.thread_id {
            panic!("Idle thread cannot block");
        }

        // Set waited events mask. If any events are sent to the thread, it will
        // be notified if the required events are received.
        self.current_thread
            .waited_events_mask
            .store(events, Ordering::SeqCst);

        // Extract require_all flag from the events mask.
        let require_all = events & REQUIRE_ALL_EVENTS != 0;
        events &= !REQUIRE_ALL_EVENTS;

        // Read and clear matching events from sent events mask.
        let received_events = self
            .current_thread
            .sent_events_mask
            .fetch_and(!events, Ordering::SeqCst)
            & events;

        // Note: If there is a send_events call from ISR between the above read and clear,
        // and blocking of the thread, then the ISR will put the thread into pending resume
        // queue, and the thread will be unblocked when the preemption lock is released.

        // Block thread if required events are not received
        if (!require_all && received_events == 0) || (received_events & events) != events {
            // Highest priority of any locks held by the current or blocked threads.
            let locks_ceiling = self
                .as_ref()
                .locks_priority_ceiling()
                .unwrap_or_default(Priority::MIN);

            let next = self
                .as_mut()
                .ready_queue_mut()
                .pop_front_if(|ready| ready.active_priority() > locks_ceiling)
                .unwrap_or(self.idle_thread);

            let blocked_thread = self.as_mut().switch_thread(pkey, next);
            self.as_mut().block_thread(pkey, blocked_thread, deadline);
        }

        received_events
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

static SCHEDULER: SyncUnsafeCell<MaybeUninit<Scheduler>> =
    SyncUnsafeCell::new(MaybeUninit::uninit());

pub struct Scheduler {
    /// Schedulables waiting to be resumed when preemption lock is released
    pending_resume: AtomicQueue<Suspendable, ExecStateTag>,

    /// Schedulables waiting to go to sleep when preemption lock is released.
    /// Since tasks cannot sleep while holding the pre-emption lock, these are
    /// always interrupt handlers waiting to be polled at given deadline.
    pending_sleep: AtomicQueue<Suspendable, ExecStateTag>,

    // The kind of pending reschedule. Rescheduling may be triggered by different
    // events, but they are always executed either at the end of interrupt handling,
    // or when the preemption lock is released.
    pending_reschedule_kind: AtomicUsize,

    raw: LockedPinRefCell<RawScheduler, PreemptLock>,
}

impl Scheduler {
    fn new(idle_thread: &'static RawThread) -> Scheduler {
        Scheduler {
            pending_resume: AtomicQueue::new(),
            pending_sleep: AtomicQueue::new(),
            pending_reschedule_kind: AtomicUsize::new(RESCHEDULE_KIND_NONE),
            raw: LockedPinRefCell::new(RawScheduler::new(idle_thread)),
        }
    }

    pub(super) fn start() -> ! {
        let idle_thread = crate::kernel::idle::init_idle_thread();
        unsafe {
            let _ = (&mut *SCHEDULER.get()).write(Scheduler::new(idle_thread));
        }
        let idle_context = idle_thread.context.as_ptr() as *mut _;
        start_first_thread(idle_context)
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

    /// Puts the interrupt handler into scheduler sleep queue to be polled later at given time.
    pub(crate) fn schedule_interrupt_wakeup(
        interrupt: &'static RawInterruptHandler,
        wakeup_time: u64,
    ) {
        // TODO: setting the deadline should be legal only if not already set
        interrupt
            .suspendable
            .set_deadline(Some(crate::Instant { tick: wakeup_time }));

        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .schedule_interrupt_wakeup(pkey, interrupt);
        }) {
            Ok(()) => (),
            Err(_) => {
                // Preemption lock is held by a thread or lower priority interrupt handler.
                // Delayed insertion to sleep queue will be executed when the lock is released.
                Scheduler::instance()
                    .pending_sleep
                    .push_back(Pin::static_ref(&interrupt.suspendable));
            }
        };
    }

    pub(crate) fn cond_reschedule<'key>(pkey: PreemptLockKey<'key>) {
        let scheduler = Scheduler::pin_instance().borrow_mut(pkey);
        let pin_scheduler = scheduler.as_ref();

        if let Some(ready_thread) = pin_scheduler.ready_queue().head() {
            if pin_scheduler.current_thread.active_priority() < ready_thread.active_priority() {
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
    }

    pub(crate) fn is_reschedule_pending() -> bool {
        let scheduler = Scheduler::instance();
        scheduler.pending_reschedule_kind.load(Ordering::Relaxed) != RESCHEDULE_KIND_NONE
    }

    pub(crate) fn complete_pending(pkey: PreemptLockKey<'_>) {
        let scheduler = Scheduler::pin_instance();
        let mut raw_scheduler = scheduler.borrow_mut(pkey);

        while let Some(resuming) = scheduler.pending_resume.pop_front() {
            raw_scheduler
                .as_mut()
                .resume_suspendable_now(pkey, resuming);
        }

        while let Some(suspending) = scheduler.pending_sleep.pop_front() {
            raw_scheduler
                .as_mut()
                .suspend_suspendable_now(pkey, suspending);
        }
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
                Scheduler::instance()
                    .pending_resume
                    .push_back(thread.suspendable_ref());
            }
        };
    }

    // Thread or ISR context
    #[allow(dead_code)]
    pub(crate) fn resume_interrupt(interrupt: &'static RawInterruptHandler) {
        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .resume_interrupt(pkey, interrupt);
        }) {
            Ok(()) => (),
            Err(_) => {
                // Could not acquire pre-emption lock in ISR, because some thread or ongoing lower
                // priority ISR holds the lock.
                // Store unblocked thread in pending ready list instead, from which it will be
                // moved to ready list when the preempt lock is released.
                Scheduler::instance()
                    .pending_resume
                    .push_back(Pin::static_ref(&interrupt.suspendable));
            }
        }
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
            Err(_) => {
                // TODO: pending suspend?
                panic!("Cannot suspend a thread while holding the preemption lock");
            }
        }
    }

    pub(crate) fn start_thread(mut thread: Pin<&'static mut RawThread>) {
        unsafe {
            thread.as_mut().init();
        }

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
    pub(crate) fn wait_current_thread_isr(
        wait_list: *mut LinkedList<Suspendable, WaitQueueTag>,
        ceiling: Priority,
    ) {
        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .wait_current_thread(pkey, wait_list, ceiling);
        }) {
            Ok(()) => (),
            Err(_) => {
                // Error: Thread is blocking in a wait list while it holds the preempt lock.
                unreachable!()
            }
        }
    }

    pub(crate) fn wait_current_thread_event_isr(events: u32, deadline: Option<u64>) -> u32 {
        match PreemptLock::try_with(|pkey| {
            Scheduler::pin_instance()
                .borrow_mut(pkey)
                .as_mut()
                .wait_current_thread_event(pkey, events, deadline)
        }) {
            Ok(received_events) => received_events,
            Err(_) => {
                // Error: Thread is blocking to wait for events while it holds the preempt lock.
                unimplemented!()
            }
        }
    }
}

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
