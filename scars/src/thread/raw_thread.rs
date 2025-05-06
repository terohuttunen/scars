use super::{INVALID_THREAD_ID, InheritanceLockListTag, LockListTag, ThreadInfo, ThreadRef};
use crate::cell::{LockedCell, LockedPinRefCell};
use crate::event_set::{EventSet, TryWaitEventsError};
use crate::events::WaitEventsUntilError;
use crate::kernel::{
    Priority,
    hal::Context,
    interrupt::set_ceiling_threshold,
    list::{LinkedList, Node, impl_linked},
    scheduler::ExecStateTag,
    scheduler::Scheduler,
    stack::StackRefMut,
    waiter::{Suspendable, WaitQueueHandle},
};
use crate::priority::PriorityStatus;
use crate::sync::{
    InheritanceLock, OnceLock, PreemptLock, RawCeilingLock, preempt_lock::PreemptLockKey,
};
use crate::task::ThreadExecutor;
use crate::time::Instant;
use crate::tls::{LocalCell, LocalStorage};
use core::mem::MaybeUninit;
use core::pin::Pin;
use core::sync::atomic::Ordering;

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
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

#[repr(align(16))]
#[repr(C)]
pub struct RawThread {
    pub thread_id: u32,

    // Thread name
    pub name: &'static str,

    pub(crate) main_fn: *const (),

    pub(crate) stack: MaybeUninit<StackRefMut>,

    // Thread base priority
    pub base_priority: Priority,

    // Nesting ceiling lock priority
    pub(crate) nesting_lock_priority: LockedCell<PriorityStatus, PreemptLock>,

    pub(crate) inherited_priority: LockedCell<PriorityStatus, PreemptLock>,

    // Effective priority of the thread. This is the maximum of the base priority and the
    // priority of any lock held by the thread.
    pub(crate) priority: LockedCell<Priority, PreemptLock>,

    // List of scoped ceiling locks which this thread is the current owner of. Ordered in descending
    // ceiling priority order so that list head is always one of the highest priority
    // locks.
    pub(crate) ceiling_locks:
        LockedPinRefCell<LinkedList<RawCeilingLock, LockListTag>, PreemptLock>,

    // List of inheritance locks which this thread is the current owner of. Ordered in no particular
    // order.
    pub(crate) inheritance_locks:
        LockedPinRefCell<LinkedList<InheritanceLock, InheritanceLockListTag>, PreemptLock>,

    // Thread state that tells in which queue the thread currently is
    //  Stopped: Not in any queue
    //  Ready: In ready queue
    //  Running: Currently running, not in any queue
    //  Blocked: In blocked queue
    pub(crate) state: LockedCell<ThreadExecutionState, PreemptLock>,

    // Intrusive linked list entry for inserting the thread into ready, suspended, or blocked queue
    pub(crate) exec_queue_link: Node<RawThread, ExecStateTag>,

    pub(crate) suspendable: Suspendable,

    // Holds reference to the wait queue that the thread is waiting on, if any.
    pub(crate) wait_queue: LockedCell<Option<WaitQueueHandle>, PreemptLock>,

    pub(crate) events: EventSet,

    pub(crate) local_storage: OnceLock<LocalStorage>,

    /// Thread context holds the KHAL defined thread information such as
    /// trap frame on embedded targets, or pthreads thread in simulator.
    /// The context is initialized when the thread is started.
    pub(crate) context: MaybeUninit<Context>,
}

impl RawThread {
    pub(crate) const fn new(
        name: &'static str,
        base_priority: Priority,
        main_fn: *const (),
    ) -> RawThread {
        RawThread {
            thread_id: INVALID_THREAD_ID,
            state: LockedCell::new(ThreadExecutionState::Created),
            name,
            base_priority,
            nesting_lock_priority: LockedCell::new(PriorityStatus::invalid()),
            inherited_priority: LockedCell::new(PriorityStatus::invalid()),
            priority: LockedCell::new(base_priority),
            main_fn,
            stack: MaybeUninit::uninit(),
            ceiling_locks: LockedPinRefCell::new(LinkedList::new()),
            inheritance_locks: LockedPinRefCell::new(LinkedList::new()),
            exec_queue_link: Node::new(),
            suspendable: Suspendable::new(),
            wait_queue: LockedCell::new(None),
            events: EventSet::new(),
            local_storage: OnceLock::new(),
            context: MaybeUninit::uninit(),
        }
    }

    pub unsafe fn init(self: Pin<&mut Self>) {
        let thread_ptr = &*self as *const RawThread;
        self.suspendable_mut().init_thread(thread_ptr);
    }

    pub unsafe fn start(&'static mut self) {
        if *self.state.get_mut() != ThreadExecutionState::Created {
            panic!("Cannot start thread twice");
        }

        *self.state.get_mut() = ThreadExecutionState::Started;

        crate::thread_start(self);
    }

    pub fn get_info(&self, pkey: PreemptLockKey<'_>) -> ThreadInfo {
        let stack_addr = unsafe { self.stack.assume_init_ref() }.bottom_ptr() as *const ();
        let stack_size = unsafe { self.stack.assume_init_ref() }.alloc_size();
        ThreadInfo {
            name: self.name,
            state: self.state.get(pkey),
            base_priority: self.base_priority,
            stack_addr,
            stack_size,
            entry: self.main_fn,
        }
    }

    pub fn suspendable_ref(self: Pin<&Self>) -> Pin<&Suspendable> {
        unsafe { Pin::new_unchecked(&self.get_ref().suspendable) }
    }

    pub fn suspendable_mut(self: Pin<&mut Self>) -> Pin<&mut Suspendable> {
        unsafe { Pin::new_unchecked(&mut self.get_unchecked_mut().suspendable) }
    }

    pub fn as_thread_ref(self: Pin<&'static Self>) -> ThreadRef {
        ThreadRef::new(self.get_ref())
    }

    // Pin projection of scoped_locks list
    pub(crate) fn ceiling_locks(
        self: Pin<&Self>,
    ) -> Pin<&LockedPinRefCell<LinkedList<RawCeilingLock, LockListTag>, PreemptLock>> {
        unsafe { Pin::map_unchecked(self, |s| &s.ceiling_locks) }
    }

    pub(crate) fn inheritance_locks(
        self: Pin<&Self>,
    ) -> Pin<&LockedPinRefCell<LinkedList<InheritanceLock, InheritanceLockListTag>, PreemptLock>>
    {
        unsafe { Pin::map_unchecked(self, |s| &s.inheritance_locks) }
    }

    pub(crate) unsafe fn scoped_lock_acquired<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&RawCeilingLock>,
    ) {
        match self.state.get(pkey) {
            ThreadExecutionState::Running => {
                lock.owner
                    .compare_exchange(
                        core::ptr::null_mut(),
                        self.get_ref() as *const _ as *mut (),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    )
                    .unwrap_or_else(|_| {
                        panic!("Lock already owned. The scheduler should have prevented this.")
                    });
                let ceiling_priority = lock.ceiling_priority;
                self.ceiling_locks()
                    .borrow_mut(pkey)
                    .as_mut()
                    .insert_after(lock, |a| a.ceiling_priority > ceiling_priority);

                self.update_priority(pkey);
            }
            state => panic!(
                "Thread {} cannot acquire ceiling lock in {:?} state",
                self.name, state
            ),
        }
    }

    pub(crate) unsafe fn scoped_lock_released<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&RawCeilingLock>,
    ) {
        let owner = lock.owner.load(Ordering::Relaxed);
        if !owner.is_null() {
            if owner == self.get_ref() as *const _ as *mut () {
                self.ceiling_locks().borrow_mut(pkey).as_mut().remove(lock);
                lock.owner.store(core::ptr::null_mut(), Ordering::Release);

                self.update_priority(pkey);

                // A thread is releasing a lock, therefore it must be running, and
                // have the highest priority at that time. If priority drops
                // below the priority of another ready thread, rescheduling must
                // be executed.
                Scheduler::cond_reschedule(pkey);
            } else {
                // The `thread` is not the owner of the lock. A lock can be released only
                // by the owner.
                crate::runtime_error!(RuntimeError::LockOwnerViolation);
            }
        }
    }

    pub(crate) unsafe fn inheritance_lock_acquired<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&InheritanceLock>,
    ) {
        if self.ceiling_lock_priority(pkey).is_valid() {
            // Inheritance locks may not be acquired while holding any ceiling locks.
            crate::runtime_error!(RuntimeError::InheritanceLockNotAllowed);
        }

        let mut inheritance_locks = self.inheritance_locks().borrow_mut(pkey);
        inheritance_locks.as_mut().insert_after(lock, |_| false);
    }

    pub(crate) unsafe fn inheritance_lock_released<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&InheritanceLock>,
    ) {
        let mut inheritance_locks = self.inheritance_locks().borrow_mut(pkey);
        inheritance_locks.as_mut().remove(lock);

        // When last inheritance lock is released, reset inherited priority
        // and reschedule if necessary.
        if inheritance_locks.as_ref().is_empty() {
            self.inherited_priority.set(pkey, PriorityStatus::invalid());
            if self.update_priority(pkey) {
                Scheduler::thread_priority_changed(pkey, self);
            }
        }
    }

    pub(crate) fn inherit_priority<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        priority: Priority,
    ) {
        // Inherited priority can only be increased, until the thread releases
        // all inheritance locks.
        self.inherited_priority.set(
            pkey,
            self.inherited_priority
                .get(pkey)
                .max(PriorityStatus::from(priority)),
        );

        if self.update_priority(pkey) {
            Scheduler::thread_priority_changed(pkey, self);
        }
    }

    /// Highest lock priority. Returns `PriorityStatus::Invalid if no locks owned by the thread.
    pub(crate) fn ceiling_lock_priority<'key>(
        self: Pin<&Self>,
        pkey: PreemptLockKey<'key>,
    ) -> PriorityStatus {
        let nesting_lock_priority = self.nesting_lock_priority.get(pkey);

        let scoped_lock_priority =
            if let Some(head) = self.ceiling_locks().borrow(pkey).as_ref().head() {
                PriorityStatus::from(head.ceiling_priority)
            } else {
                PriorityStatus::invalid()
            };

        nesting_lock_priority.max(scoped_lock_priority)
    }

    /// Thread priority
    ///
    /// A thread can temporary boost its priority by acquiring locks. If a thread
    /// owns any locks, the highest owned lock priority will be returned; otherwise,
    /// returns the thread base priority.
    pub(crate) fn priority<'key>(self: Pin<&Self>, pkey: PreemptLockKey<'key>) -> Priority {
        self.priority.get(pkey)
    }

    fn update_priority<'key>(self: Pin<&Self>, pkey: PreemptLockKey<'key>) -> bool {
        let lock_priority = self.ceiling_lock_priority(pkey);
        let inherited_priority = self.inherited_priority.get(pkey);

        let new_priority = self
            .base_priority
            .max_valid(lock_priority)
            .max_valid(inherited_priority);

        let old_priority = self.priority.replace(pkey, new_priority);
        old_priority != new_priority
    }

    pub(crate) fn raise_nesting_lock_priority(
        self: Pin<&Self>,
        new_priority: Priority,
    ) -> PriorityStatus {
        PreemptLock::with(|pkey| {
            let old_priority = self.nesting_lock_priority.get(pkey);

            // Priorities can only be increased.
            if old_priority > PriorityStatus::from(new_priority) {
                crate::runtime_error!(RuntimeError::CeilingPriorityViolation);
            }

            let raised_priority = PriorityStatus::from(new_priority).max(old_priority);
            self.nesting_lock_priority.set(pkey, raised_priority);

            if self.update_priority(pkey) {
                set_ceiling_threshold(self.priority(pkey).into());
            }
            old_priority
        })
    }

    pub(crate) fn set_nesting_lock_priority(self: Pin<&Self>, new_priority: PriorityStatus) {
        PreemptLock::with(|pkey| {
            self.nesting_lock_priority.set(pkey, new_priority);

            if self.update_priority(pkey) {
                set_ceiling_threshold(self.priority(pkey).into());
            }
            Scheduler::cond_reschedule(pkey);
        });
    }

    pub fn resume(&'static self) {
        Scheduler::resume_thread(Pin::static_ref(self));
    }

    pub(crate) fn set_wait_queue(
        &self,
        wait_queue: Option<WaitQueueHandle>,
        pkey: PreemptLockKey<'_>,
    ) {
        self.wait_queue.set(pkey, wait_queue);
    }

    pub(crate) fn set_wakeup_event(&self) {
        self.events.set_wakeup_event();
    }

    pub(crate) fn set_resume_event(&self) {
        self.events.set_resume_event();
    }

    pub fn send_events(&'static self, events: u32) {
        if self.events.send_events(events) {
            self.resume();
        }
    }

    pub fn wait_events(&self, events: u32) -> u32 {
        self.events.wait_events(events)
    }

    pub fn wait_events_until(
        &self,
        events: u32,
        deadline: Instant,
    ) -> Result<u32, WaitEventsUntilError> {
        self.events.wait_events_until(events, deadline)
    }

    pub fn try_wait_events(&self, events: u32) -> Result<u32, TryWaitEventsError> {
        self.events.try_wait_events(events)
    }

    pub fn local_storage(&self) -> Option<&LocalStorage> {
        self.local_storage.get()
    }

    pub fn local_storage_mut(&mut self) -> Option<&mut LocalStorage> {
        self.local_storage.get_mut()
    }

    pub(crate) fn set_local_storage(
        &self,
        local_storage: LocalStorage,
    ) -> Result<(), LocalStorage> {
        self.local_storage.set(local_storage)
    }

    pub fn start_executor(&mut self, executor: &'static LocalCell<ThreadExecutor>) {
        let thread_ref = unsafe { ThreadRef::from_ptr(self as *const _) };
        self.local_storage_mut()
            .unwrap()
            .raw_put_init_with(executor, || ThreadExecutor::new(thread_ref));
    }
}

impl_linked!(exec_queue_link, RawThread, ExecStateTag);
