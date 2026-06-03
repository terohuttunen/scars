#[cfg(feature = "priority-inheritance")]
use super::InheritanceLockListTag;
#[cfg(feature = "raii-locks")]
use super::LockListTag;
use super::{INVALID_THREAD_ID, ThreadInfo, ThreadRef};
use crate::cell::LockedCell;
use crate::events::{AtomicEvents, Events, sender::EventReceiver};
#[cfg(any(feature = "raii-locks", feature = "priority-inheritance"))]
use crate::kernel::list::LinkedList;
use crate::kernel::{
    Priority, hal,
    scheduler::RawScheduler,
    scheduler::{PendingWorkEntry, PendingWorkHandler},
    stack::StackRefMut,
};
use crate::local::LocalStorage;
use crate::priority::PriorityOpt;
#[cfg(feature = "priority-inheritance")]
use crate::sync::InheritanceLock;
#[cfg(any(feature = "raii-locks", feature = "priority-inheritance"))]
use crate::sync::Protected;
#[cfg(feature = "raii-locks")]
use crate::sync::RawCeilingLock;
use crate::sync::atomic::Ordering;
use crate::sync::{PreemptLock, PreemptLockKey};
use core::mem::MaybeUninit;
use core::pin::Pin;
#[cfg(feature = "multithreading")]
use {
    crate::events::WaitEvents,
    crate::kernel::waiter::{WaitQueueEntry, WaitQueueEntryHandler, WaitQueueHandle},
    crate::kernel::{
        hal::CoreId,
        list::{Node, impl_linked},
        scheduler::{
            ExecStateTag, ExecutionContext, RawPendingWorkEntry, Scheduler, Timer, TimerHandler,
        },
    },
    crate::sync::atomic::AtomicPtr,
    crate::time::Instant,
    core::ptr,
};

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
#[repr(C)]
pub enum ThreadExecutionState {
    /// The initial state after thread creation. Memory has been allocated,
    /// but does not yet have an initialized closure. A thread that has been
    /// created but not started, cannot become ready.
    Created,

    /// Thread has been started. The thread has an initialized closure.
    Started,

    /// Thread is ready to run
    Ready,

    /// Thread is running
    Running,

    /// Thread is blocked in a WaitQueue or sleeping and waiting for wakeup.
    Blocked,

    /// Thread is suspended
    Suspended,
}

/// Per-thread kernel data. CORE-erased: stored in the per-core
/// scheduler's intrusive queues without static `CORE` typing. The
/// runtime `core: u8` field is the truth; per-method wrong-core
/// checks raise [`RuntimeError::WrongCore`] before any cell access.
#[repr(align(16))]
#[repr(C)]
pub(crate) struct RawThread {
    pub thread_id: u32,

    // Thread name
    pub name: &'static str,

    pub main_fn: *const (),

    pub stack: MaybeUninit<StackRefMut>,

    // Thread base priority
    pub base_priority: Priority,

    // Core this thread is bound to. The scheduler enqueues this thread
    // into the per-core ready/blocked lists indexed by `core`; the
    // thread runs only on that core. All cell accessors check
    // `pkey.core == self.core` before touching the cells.
    pub core: crate::kernel::hal::CoreId,

    // Nesting ceiling lock priority
    pub nesting_lock_priority: LockedCell<PriorityOpt, PreemptLock>,

    #[cfg(feature = "priority-inheritance")]
    pub inherited_priority: LockedCell<PriorityOpt, PreemptLock>,

    // Effective priority of the thread. This is the maximum of the base priority and the
    // priority of any lock held by the thread.
    pub priority: LockedCell<Priority, PreemptLock>,

    // List of scoped ceiling locks which this thread is the current owner of. Ordered in descending
    // ceiling priority order so that list head is always one of the highest priority
    // locks.
    #[cfg(feature = "raii-locks")]
    pub ceiling_locks: Protected<LinkedList<RawCeilingLock, LockListTag>, PreemptLock>,

    // List of inheritance locks which this thread is the current owner of. Ordered in no particular
    // order.
    #[cfg(feature = "priority-inheritance")]
    pub inheritance_locks:
        Protected<LinkedList<InheritanceLock, InheritanceLockListTag>, PreemptLock>,

    // Thread state that tells in which queue the thread currently is
    //  Stopped: Not in any queue
    //  Ready: In ready queue
    //  Running: Currently running, not in any queue
    //  Blocked: In blocked queue
    pub state: LockedCell<ThreadExecutionState, PreemptLock>,

    // Intrusive linked list entry for inserting the thread into ready, suspended, or blocked queue
    #[cfg(feature = "multithreading")]
    pub exec_queue_link: Node<Self, ExecStateTag>,

    #[cfg(feature = "multithreading")]
    pub wait_entry: WaitQueueEntry,
    #[cfg(feature = "multithreading")]
    pub timer: Timer<Self>,

    pub pending_work: PendingWorkEntry<Self>,

    // Holds reference to the wait queue that the thread is waiting on, if any.
    #[cfg(feature = "multithreading")]
    pub wait_queue: LockedCell<Option<WaitQueueHandle>, PreemptLock>,

    // Set by the timer-fire wake path when a deadlined wait expired
    // without a notifier reaching it. Read and cleared by
    // `Scheduler::take_last_wait_timed_out` after the syscall returns.
    #[cfg(feature = "multithreading")]
    pub wait_timed_out: LockedCell<bool, PreemptLock>,

    // Deadline the next drain-driven block will arm. Written by the
    // commit site inside `Protected::with_barrier_until` and consumed
    // (reset to `None`) by `Scheduler::block_current`. `None` means
    // "block without a timer."
    #[cfg(feature = "multithreading")]
    pub pending_block_deadline: LockedCell<Option<Instant>, PreemptLock>,

    // Event system fields
    pub pending_events: AtomicEvents,
    #[cfg(feature = "multithreading")]
    pub current_wait_events: AtomicPtr<WaitEvents>,

    pub local_storage: LocalStorage,

    /// Thread context holds the KHAL defined thread information such as
    /// trap frame on embedded targets, or pthreads thread in simulator.
    /// The context is initialized when the thread is started.
    pub context: MaybeUninit<hal::Context>,
}

impl RawThread {
    /// Bits passed via `PendingWorkHandler::complete(ops)` to dispatch a
    /// deferred thread operation. Bits are interpreted exclusively by
    /// [`PendingWorkHandler for RawThread`] — they share no namespace with
    /// other handlers' op masks.
    pub(crate) const OP_RESUME: u32 = 1 << 0;
    pub(crate) const OP_WAKEUP: u32 = 1 << 1;
    pub(crate) const OP_SUSPEND: u32 = 1 << 2;
    pub(crate) const OP_START: u32 = 1 << 3;
    pub(crate) const OP_CHECK_EVENTS: u32 = 1 << 4;
    pub(crate) const OP_REINSERT_WAIT_QUEUE: u32 = 1 << 5;

    pub(crate) const fn new(
        name: &'static str,
        base_priority: Priority,
        core: crate::kernel::hal::CoreId,
        main_fn: *const (),
    ) -> RawThread {
        RawThread {
            thread_id: INVALID_THREAD_ID,
            state: LockedCell::new(ThreadExecutionState::Created),
            name,
            base_priority,
            core,
            nesting_lock_priority: LockedCell::new(PriorityOpt::none()),
            #[cfg(feature = "priority-inheritance")]
            inherited_priority: LockedCell::new(PriorityOpt::none()),
            priority: LockedCell::new(base_priority),
            main_fn,
            stack: MaybeUninit::uninit(),
            #[cfg(feature = "raii-locks")]
            ceiling_locks: Protected::new(LinkedList::new()),
            #[cfg(feature = "priority-inheritance")]
            inheritance_locks: Protected::new(LinkedList::new()),
            #[cfg(feature = "multithreading")]
            exec_queue_link: Node::new(),
            #[cfg(feature = "multithreading")]
            wait_entry: WaitQueueEntry::new(),
            #[cfg(feature = "multithreading")]
            timer: Timer::new(),
            pending_work: PendingWorkEntry::new(),
            #[cfg(feature = "multithreading")]
            wait_queue: LockedCell::new(None),
            #[cfg(feature = "multithreading")]
            wait_timed_out: LockedCell::new(false),
            #[cfg(feature = "multithreading")]
            pending_block_deadline: LockedCell::new(None),
            pending_events: AtomicEvents::new(0),
            #[cfg(feature = "multithreading")]
            current_wait_events: AtomicPtr::new(ptr::null_mut()),
            local_storage: LocalStorage::new(),
            context: MaybeUninit::uninit(),
        }
    }

    pub unsafe fn init_at(this: *mut Self) {
        unsafe {
            #[cfg(feature = "multithreading")]
            (*this).wait_entry.init_for::<Self>(&*this);
            // Bind the pending-work entry's receiver. The scheduler may
            // dispatch via this entry from any core (cross-core start
            // posts onto the target's deferred-work queue); binding
            // here ensures `RawPendingWorkEntry::complete` finds a
            // receiver before any such dispatch is possible.
            (*this)
                .pending_work
                .set_receiver(core::pin::Pin::new_unchecked(&*this));
        }
    }
}

// Thread lifecycle, wait-queue, and wakeup-timer methods.
#[cfg(feature = "multithreading")]
impl RawThread {
    /// Set the wakeup deadline for this thread's timer. Pass `None` to
    /// disable. The expiration handler is the [`TimerHandler`] impl below.
    pub(crate) fn set_wakeup_deadline(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'_>,
        scheduler: Pin<&mut RawScheduler>,
        deadline: Option<Instant>,
    ) {
        self.get_timer().arm(pkey, scheduler, self, deadline);
    }

    pub(crate) fn get_timer(self: Pin<&Self>) -> Pin<&Timer<Self>> {
        unsafe { self.map_unchecked(|t| &t.timer) }
    }

    pub(crate) fn get_wait_entry(self: Pin<&Self>) -> Pin<&WaitQueueEntry> {
        unsafe { self.map_unchecked(|t| &t.wait_entry) }
    }

    pub(crate) fn get_pending_work(self: Pin<&Self>) -> Pin<&RawPendingWorkEntry> {
        let typed: Pin<&PendingWorkEntry<Self>> =
            unsafe { self.map_unchecked(|t| &t.pending_work) };
        typed.raw()
    }

    /// Post `op` to the deferred-work queue belonging to this thread's
    /// core. Same-core posts ride the next preempt-lock release; remote
    /// posts ping the target's service call automatically.
    pub(crate) fn schedule_deferred_op(self: Pin<&Self>, op: u32) {
        Scheduler::schedule_deferred_operation_on(self.core, self.get_pending_work(), op);
    }

    pub unsafe fn start(&'static mut self) {
        if *self.state.get_mut() != ThreadExecutionState::Created {
            panic!("Cannot start thread twice");
        }

        *self.state.get_mut() = ThreadExecutionState::Started;

        if self.core == CoreId::current() {
            // Same-core path: announce the thread and make it runnable
            // through this core's scheduler.
            let thread = Pin::static_ref(&*self);
            crate::printkln!("Starting thread {}", thread.name);
            crate::kernel::tracing::thread_new(thread.as_thread_ref());
            Scheduler::resume_thread(thread);
        } else {
            // Cross-core path: post a START op onto the target core's
            // deferred-work queue and ping its service call. The
            // target's `_kernel_service_call_handler` will drain the
            // queue and run `Scheduler::resume_thread(self)`.
            Pin::static_ref(&*self).schedule_deferred_op(Self::OP_START);
        }
    }
}

impl RawThread {
    #[allow(dead_code)]
    pub fn get_info(&self, pkey: PreemptLockKey<'_>) -> ThreadInfo {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        let stack_addr = unsafe { self.stack.assume_init_ref() }.bottom_ptr() as *const ();
        let stack_size = unsafe { self.stack.assume_init_ref() }.alloc_size();
        ThreadInfo {
            name: self.name,
            state: self.state.get(pkey),
            base_priority: self.base_priority,
            core: self.core,
            stack_addr,
            stack_size,
            entry: self.main_fn,
        }
    }

    pub fn as_thread_ref(self: Pin<&'static Self>) -> ThreadRef {
        ThreadRef::new(self.get_ref())
    }

    // Pin projection of the owned ceiling-locks list.
    #[cfg(feature = "raii-locks")]
    fn ceiling_locks(
        self: Pin<&Self>,
    ) -> Pin<&Protected<LinkedList<RawCeilingLock, LockListTag>, PreemptLock>> {
        unsafe { Pin::map_unchecked(self, |s| &s.ceiling_locks) }
    }

    // Pin projection of the owned inheritance-locks list.
    #[cfg(feature = "priority-inheritance")]
    fn inheritance_locks(
        self: Pin<&Self>,
    ) -> Pin<&Protected<LinkedList<InheritanceLock, InheritanceLockListTag>, PreemptLock>> {
        unsafe { Pin::map_unchecked(self, |s| &s.inheritance_locks) }
    }

    #[cfg(feature = "raii-locks")]
    pub(crate) unsafe fn ceiling_lock_acquired<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&RawCeilingLock>,
    ) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        debug_assert_eq!(
            lock.core, self.core,
            "ceiling lock and thread on different cores"
        );
        // Add lock to thread's owned ceiling locks list (ordered by priority)
        let ceiling_priority = lock.ceiling_priority;
        self.ceiling_locks().with_pin_key(pkey, |_, list| {
            list.insert_after(lock, |a| a.ceiling_priority > ceiling_priority);
        });

        self.update_priority(pkey);
    }

    #[cfg(feature = "raii-locks")]
    pub(crate) unsafe fn ceiling_lock_released<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&RawCeilingLock>,
    ) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        debug_assert_eq!(
            lock.core, self.core,
            "ceiling lock and thread on different cores"
        );
        // Remove lock from thread's owned ceiling locks list
        self.ceiling_locks()
            .with_pin_key(pkey, |_, list| list.remove(lock));

        self.update_priority(pkey);
    }

    #[cfg(feature = "priority-inheritance")]
    pub(crate) unsafe fn inheritance_lock_acquired<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&InheritanceLock>,
    ) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        if self.ceiling_lock_priority(pkey).is_some() {
            // Inheritance locks may not be acquired while holding any ceiling locks.
            crate::runtime_error!(RuntimeError::InheritanceLockNotAllowed);
        }

        self.inheritance_locks().with_pin_key(pkey, |_, list| {
            list.insert_after(lock, |_| false);
        });
    }

    #[cfg(feature = "priority-inheritance")]
    pub(crate) unsafe fn inheritance_lock_released<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&InheritanceLock>,
    ) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        let became_empty = self.inheritance_locks().with_pin_key(pkey, |_, mut list| {
            list.as_mut().remove(lock);
            list.as_ref().is_empty()
        });

        // When last inheritance lock is released, reset inherited priority
        // and reschedule if necessary. The priority recompute touches the
        // ceiling list and scalar fields, not the inheritance list, so it
        // must run after the `with_pin_key` closure has released it.
        if became_empty {
            self.inherited_priority.set(pkey, PriorityOpt::none());
            if self.update_priority(pkey) {
                Scheduler::thread_priority_changed(pkey, self);
            }
        }
    }

    #[cfg(feature = "priority-inheritance")]
    pub(crate) fn inherit_priority<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        priority: Priority,
    ) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        // Inherited priority can only be increased, until the thread releases
        // all inheritance locks.
        self.inherited_priority.set(
            pkey,
            self.inherited_priority
                .get(pkey)
                .max(PriorityOpt::from(priority)),
        );

        if self.update_priority(pkey) {
            Scheduler::thread_priority_changed(pkey, self);
        }
    }

    /// Highest lock priority. Returns `PriorityOpt::None if no locks owned by the thread.
    pub(crate) fn ceiling_lock_priority<'key>(
        self: Pin<&Self>,
        pkey: PreemptLockKey<'key>,
    ) -> PriorityOpt {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        let nesting_lock_priority = self.nesting_lock_priority.get(pkey);

        #[cfg(feature = "raii-locks")]
        let nesting_lock_priority = {
            let scoped_lock_priority =
                self.ceiling_locks()
                    .with_pin_key(pkey, |_, list| match list.as_ref().head() {
                        Some(head) => PriorityOpt::from(head.ceiling_priority),
                        None => PriorityOpt::none(),
                    });
            nesting_lock_priority.max(scoped_lock_priority)
        };

        nesting_lock_priority
    }

    /// Whether the thread currently owns any inheritance lock. Used to
    /// reject acquiring a ceiling lock while priority inheritance is in
    /// play (the two protocols may not be combined — see
    /// [`RuntimeError::CeilingLockNotAllowed`]).
    #[cfg(feature = "priority-inheritance")]
    pub(crate) fn holds_inheritance_lock<'key>(
        self: Pin<&Self>,
        pkey: PreemptLockKey<'key>,
    ) -> bool {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        self.inheritance_locks()
            .with_pin_key(pkey, |_, list| list.as_ref().head().is_some())
    }

    /// Thread priority
    ///
    /// A thread can temporary boost its priority by acquiring locks. If a thread
    /// owns any locks, the highest owned lock priority will be returned; otherwise,
    /// returns the thread base priority.
    pub(crate) fn priority<'key>(self: Pin<&Self>, pkey: PreemptLockKey<'key>) -> Priority {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        self.priority.get(pkey)
    }

    fn update_priority<'key>(self: Pin<&Self>, pkey: PreemptLockKey<'key>) -> bool {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        let lock_priority = self.ceiling_lock_priority(pkey);

        let new_priority = self.base_priority.max_valid(lock_priority);
        #[cfg(feature = "priority-inheritance")]
        let new_priority = new_priority.max_valid(self.inherited_priority.get(pkey));

        let old_priority = self.priority.replace(pkey, new_priority);
        old_priority != new_priority
    }

    pub(crate) fn raise_nesting_lock_priority(
        self: Pin<&Self>,
        new_priority: Priority,
    ) -> PriorityOpt {
        PreemptLock::with(|pkey| {
            if self.core != pkey.core {
                crate::runtime_error!(RuntimeError::WrongCore);
            }
            let old_priority = self.nesting_lock_priority.get(pkey);

            // Priorities can only be increased.
            if old_priority > PriorityOpt::from(new_priority) {
                crate::runtime_error!(RuntimeError::CeilingPriorityViolation);
            }

            let raised_priority = PriorityOpt::from(new_priority).max(old_priority);
            self.nesting_lock_priority.set(pkey, raised_priority);

            self.update_priority(pkey);
            old_priority
        })
    }

    pub(crate) fn set_nesting_lock_priority(self: Pin<&Self>, new_priority: PriorityOpt) {
        PreemptLock::with(|pkey| {
            if self.core != pkey.core {
                crate::runtime_error!(RuntimeError::WrongCore);
            }
            self.nesting_lock_priority.set(pkey, new_priority);
            self.update_priority(pkey);
        });
    }
}

#[cfg(feature = "multithreading")]
impl RawThread {
    pub fn resume(&'static self) {
        Scheduler::resume_thread(Pin::static_ref(self));
    }

    /// Suspend the referenced thread (`self`), removing it from scheduling
    /// until resumed.
    ///
    /// Suspending the current thread must switch off its own stack, which
    /// happens on the suspend syscall's service-call return; suspending any
    /// other thread is applied directly through the scheduler, dispatching to
    /// the thread's owning core if it differs.
    pub fn suspend(&'static self) {
        let is_current = matches!(
            Scheduler::current_execution_context(),
            ExecutionContext::Thread(ctx) if ptr::eq(ctx.get_ref(), self)
        );
        if is_current {
            crate::kernel::syscall::thread_suspend();
        } else {
            Scheduler::suspend_thread(Some(Pin::static_ref(self)));
        }
    }

    /// Set `wait_queue` to `handle`. The caller must have already
    /// enqueued this thread on the wait list referenced by `handle`;
    /// the commit then fires via
    /// `Scheduler::set_pending_reschedule(RESCHEDULE_KIND_BLOCK_CURRENT)`.
    pub(crate) fn arm_wait(&self, pkey: PreemptLockKey<'_>, handle: WaitQueueHandle) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        self.wait_queue.set(pkey, Some(handle));
    }

    /// Clear `wait_queue`. Used by the scheduler wake paths after a
    /// successful `try_remove` on the handle returned by `arm_wait`.
    pub(crate) fn disarm_wait(&self, pkey: PreemptLockKey<'_>) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        self.wait_queue.set(pkey, None);
    }

    /// Set the per-thread "wait timed out" flag. Called from the
    /// timer-fire wake path and from `wait_current_thread_until` to
    /// reset before the suspend.
    pub(crate) fn set_wait_timed_out(&self, pkey: PreemptLockKey<'_>, value: bool) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        self.wait_timed_out.set(pkey, value);
    }

    /// Read and clear the per-thread "wait timed out" flag.
    pub(crate) fn take_wait_timed_out(&self, pkey: PreemptLockKey<'_>) -> bool {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        self.wait_timed_out.replace(pkey, false)
    }

    /// Set the deadline for the next drain-driven block. Pass `None`
    /// to mean "no timer."
    pub(crate) fn set_pending_block_deadline(
        &self,
        pkey: PreemptLockKey<'_>,
        deadline: Option<Instant>,
    ) {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        self.pending_block_deadline.set(pkey, deadline);
    }

    /// Read the pending-block deadline and reset to `None` so the
    /// no-deadline path doesn't have to write the cell between
    /// commits.
    pub(crate) fn take_pending_block_deadline(&self, pkey: PreemptLockKey<'_>) -> Option<Instant> {
        if self.core != pkey.core {
            crate::runtime_error!(RuntimeError::WrongCore);
        }
        self.pending_block_deadline.replace(pkey, None)
    }
}

impl RawThread {
    pub fn send_events(&'static self, events: Events) {
        // Update pending events mask. The atomic OR is cross-core safe;
        // the target reads it under its own preempt lock during dispatch.
        let all_pending = self.pending_events.fetch_or(events, Ordering::SeqCst) | events;

        // Without multithreading no thread ever blocks on its own events (event
        // handlers are the delivery target), so there is nothing to wake.
        #[cfg(feature = "multithreading")]
        {
            if self.core != CoreId::current() {
                // Foreign-core target: hand the resume decision to the
                // owning core. The handler will re-read `pending_events`
                // and `current_wait_events`, which may have changed since
                // this post, so we don't act on the snapshot we just took.
                Pin::static_ref(self).schedule_deferred_op(Self::OP_CHECK_EVENTS);
                return;
            }

            // Same-core fast path: check if thread is waiting and should be woken.
            let wait_events_ptr = self.current_wait_events.load(Ordering::SeqCst);
            if !wait_events_ptr.is_null() {
                let wait_events = unsafe { &*wait_events_ptr };

                if wait_events.should_resume(all_pending) {
                    self.resume();
                }
            }
        }
        #[cfg(not(feature = "multithreading"))]
        let _ = all_pending;
    }

    pub fn peek_pending_events(&self) -> Events {
        self.pending_events.load(Ordering::SeqCst)
    }

    #[allow(dead_code)]
    pub fn local_storage(&self) -> &LocalStorage {
        &self.local_storage
    }
}

#[cfg(feature = "multithreading")]
impl WaitQueueEntryHandler for RawThread {
    fn on_resume(this: &'static Self) {
        this.resume();
    }
}

#[cfg(feature = "multithreading")]
impl TimerHandler for RawThread {
    fn on_expire(
        this: Pin<&'static Self>,
        mut sched: Pin<&mut RawScheduler>,
        pkey: PreemptLockKey<'_>,
    ) {
        if sched.as_mut().try_wakeup_thread(pkey, this).is_err() {
            // Ceiling too high to act inline; route through the work
            // queue. The drain re-attempts when priority allows.
            this.schedule_deferred_op(RawThread::OP_WAKEUP);
        }
    }
}

impl PendingWorkHandler for RawThread {
    #[cfg_attr(not(feature = "multithreading"), allow(unused_mut, unused_variables))]
    fn complete(
        this: Pin<&'static Self>,
        pkey: PreemptLockKey<'_>,
        mut sched: Pin<&mut RawScheduler>,
        ops: u32,
    ) -> bool {
        // Without multithreading there are no thread ops to dispatch (the idle
        // thread is never started, woken, suspended, or wait-queued).
        #[cfg(not(feature = "multithreading"))]
        {
            let _ = (this, pkey, sched, ops);
            true
        }
        // The work-queue drain already holds this core's preempt lock,
        // so we drive the internal `RawScheduler` methods directly.
        // Each `try_*` call reports back if the ceiling blocked the
        // op; we collect those bits and tell the drain to re-queue.
        #[cfg(feature = "multithreading")]
        {
            let mut retry: u32 = 0;

            if ops & Self::OP_START != 0 {
                crate::kernel::tracing::thread_new(this.as_thread_ref());
                if sched.as_mut().try_resume_thread(pkey, this).is_err() {
                    retry |= Self::OP_START;
                }
            }
            if ops & Self::OP_RESUME != 0 {
                if sched.as_mut().try_resume_thread(pkey, this).is_err() {
                    retry |= Self::OP_RESUME;
                }
            }
            if ops & Self::OP_WAKEUP != 0 {
                if sched.as_mut().try_wakeup_thread(pkey, this).is_err() {
                    retry |= Self::OP_WAKEUP;
                }
            }
            if ops & Self::OP_SUSPEND != 0 {
                // `suspend_thread` has no ceiling gating; it always
                // completes.
                sched.as_mut().suspend_thread(pkey, Some(this));
            }
            if ops & Self::OP_CHECK_EVENTS != 0 {
                // Re-read state under the preempt lock so the resume
                // decision sees the same `pending_events` /
                // `current_wait_events` the waiter would observe.
                let pending = this.pending_events.load(Ordering::SeqCst);
                let wait_events_ptr = this.current_wait_events.load(Ordering::SeqCst);
                if !wait_events_ptr.is_null() {
                    let wait_events = unsafe { &*wait_events_ptr };
                    if wait_events.should_resume(pending) {
                        if sched.as_mut().try_resume_thread(pkey, this).is_err() {
                            retry |= Self::OP_CHECK_EVENTS;
                        }
                    }
                }
            }
            if ops & Self::OP_REINSERT_WAIT_QUEUE != 0 {
                if let Some(handle) = this.wait_queue.get(pkey) {
                    let entry = this.get_wait_entry();
                    if unsafe { handle.try_reinsert(pkey, entry) }.is_err() {
                        retry |= Self::OP_REINSERT_WAIT_QUEUE;
                    }
                }
            }

            if retry != 0 {
                this.get_pending_work().set_pending(retry);
                false
            } else {
                true
            }
        }
    }
}

impl EventReceiver for RawThread {
    fn send_events(&'static self, events: Events) {
        Self::send_events(self, events)
    }
    fn has_pending_events(&self) -> bool {
        self.peek_pending_events() != 0
    }
    fn base_priority(&self) -> Priority {
        self.base_priority
    }
}

#[cfg(feature = "multithreading")]
impl_linked!(exec_queue_link, RawThread, ExecStateTag);
