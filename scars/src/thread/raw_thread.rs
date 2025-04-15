use super::{INVALID_THREAD_ID, LockListTag, ThreadInfo, ThreadRef};
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
    waiter::Suspendable,
};
use crate::priority::{AtomicPriorityStatusPair, PriorityStatus};
use crate::sync::{OnceLock, PreemptLock, RawCeilingLock, preempt_lock::PreemptLockKey};
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

    // Atomically updated pair of (section lock priority, highest owned lock priority).
    // Either lock priority can be INVALID_PRIORITY if no such lock is held.
    lock_priorities: AtomicPriorityStatusPair,

    // List of scioed locks which this thread is the current owner of. Ordered in descending
    // ceiling priority order so that list head is always one of the highest priority
    // locks. List of scoped locks must be maintained because in Rust programming model
    // nested lock guards may be dropped in any order.
    pub(crate) scoped_locks: LockedPinRefCell<LinkedList<RawCeilingLock, LockListTag>, PreemptLock>,

    // Thread state that tells in which queue the thread currently is
    //  Stopped: Not in any queue
    //  Ready: In ready queue
    //  Running: Currently running, not in any queue
    //  Blocked: In blocked queue
    pub(crate) state: LockedCell<ThreadExecutionState, PreemptLock>,

    // Intrusive linked list entry for inserting the thread into ready, suspended, or blocked queue
    pub(crate) exec_queue_link: Node<RawThread, ExecStateTag>,

    pub(crate) suspendable: Suspendable,

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
            lock_priorities: AtomicPriorityStatusPair::new((
                PriorityStatus::invalid(),
                PriorityStatus::invalid(),
            )),
            main_fn,
            stack: MaybeUninit::uninit(),
            scoped_locks: LockedPinRefCell::new(LinkedList::new()),
            exec_queue_link: Node::new(),
            suspendable: Suspendable::new(),
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
    pub(crate) fn scoped_locks(
        self: Pin<&Self>,
    ) -> Pin<&LockedPinRefCell<LinkedList<RawCeilingLock, LockListTag>, PreemptLock>> {
        unsafe { Pin::map_unchecked(self, |s| &s.scoped_locks) }
    }

    pub(crate) unsafe fn acquire_scoped_lock<'key>(
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
                self.scoped_locks()
                    .borrow_mut(pkey)
                    .as_mut()
                    .insert_after(lock, |a| a.ceiling_priority > ceiling_priority);

                self.update_owned_lock_priority(pkey);
            }
            state => panic!(
                "Thread {} cannot acquire ceiling lock in {:?} state",
                self.name, state
            ),
        }
    }

    pub(crate) unsafe fn release_scoped_lock<'key>(
        self: Pin<&'static Self>,
        pkey: PreemptLockKey<'key>,
        lock: Pin<&RawCeilingLock>,
    ) {
        let owner = lock.owner.load(Ordering::Relaxed);
        if !owner.is_null() {
            if owner == self.get_ref() as *const _ as *mut () {
                self.scoped_locks().borrow_mut(pkey).as_mut().remove(lock);
                lock.owner.store(core::ptr::null_mut(), Ordering::Release);

                self.update_owned_lock_priority(pkey);

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

    /// Highest lock priority. Returns `INVALID_PRIORITY` if no locks owned by the thread.
    pub(crate) fn lock_priority<'key>(&self) -> PriorityStatus {
        let priorities = self.lock_priorities.load(Ordering::SeqCst);
        priorities.0.max(priorities.1)
    }

    /// Thread priority
    ///
    /// A thread can temporary boost its priority by acquiring locks. If a thread
    /// owns any locks, the highest owned lock priority will be returned; otherwise,
    /// returns the thread base priority.
    pub(crate) fn priority<'key>(&self) -> Priority {
        let lock_priority = self.lock_priority();
        self.base_priority.max_valid(lock_priority)
    }

    // Returns previous priority
    pub(crate) fn raise_nesting_lock_priority(&self, new_priority: Priority) -> PriorityStatus {
        let new_priority = new_priority.max(self.base_priority);
        loop {
            let current_priorities = self.lock_priorities.load(Ordering::SeqCst);
            let new_priorities = (
                current_priorities.0.max(new_priority.into()),
                current_priorities.1,
            );

            if let Ok(_) = self.lock_priorities.compare_exchange(
                current_priorities,
                new_priorities,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                let ceiling = new_priorities.0.max(new_priorities.1);
                set_ceiling_threshold(ceiling);
                return current_priorities.0;
            }
        }
    }

    pub(crate) fn set_nesting_lock_priority(self: Pin<&Self>, new_priority: PriorityStatus) {
        loop {
            let current_priorities = self.lock_priorities.load(Ordering::SeqCst);
            let new_priorities = (new_priority, current_priorities.1);

            if let Ok(_) = self.lock_priorities.compare_exchange(
                current_priorities,
                new_priorities,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                let ceiling = new_priorities.0.max(new_priorities.1);
                set_ceiling_threshold(ceiling);
                break;
            }
        }

        PreemptLock::with(|pl| Scheduler::cond_reschedule(pl));
    }

    fn update_owned_lock_priority<'key>(self: Pin<&Self>, pkey: PreemptLockKey<'key>) {
        let new_priority = if let Some(head) = self.scoped_locks().borrow(pkey).as_ref().head() {
            PriorityStatus::from(head.ceiling_priority)
        } else {
            PriorityStatus::invalid()
        };

        loop {
            let current_priorities = self.lock_priorities.load(Ordering::SeqCst);
            let new_priorities = (current_priorities.0, new_priority);

            if let Ok(_) = self.lock_priorities.compare_exchange(
                current_priorities,
                new_priorities,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                let ceiling = new_priorities.0.max(new_priorities.1);
                set_ceiling_threshold(ceiling);
                break;
            }
        }
    }

    pub fn resume(&'static self) {
        Scheduler::resume_thread(Pin::static_ref(self));
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
