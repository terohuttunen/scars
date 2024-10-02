use crate::cell::{LockedCell, LockedRefCell};
use crate::events::{
    WaitEventsUntilError, REQUIRE_ALL_EVENTS, SCHEDULER_NOTIFY_EVENT, SCHEDULER_WAKEUP_EVENT,
};
use crate::kernel::{
    hal::Context,
    interrupt::set_ceiling_threshold,
    list::{impl_linked, Link, LinkedList, LinkedListTag},
    priority::{AtomicPriorityPair, INVALID_PRIORITY},
    scheduler::ExecStateTag,
    scheduler::Scheduler,
    stack::StackRef,
    waiter::Suspendable,
    Priority, ThreadPriority,
};
use crate::sync::{preempt_lock::PreemptLockKey, OnceLock, PreemptLock, RawCeilingLock};
use crate::syscall;
use crate::task::ThreadExecutor;
use crate::time::Instant;
use crate::tls::{LocalCell, LocalStorage};
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicU32, Ordering};
use scars_khal::ContextInfo;
use static_cell::ConstStaticCell;

#[macro_export]
macro_rules! make_thread {
    ($name: expr, $prio : expr, $stack_size : expr, $local_storage_size : expr, executor = true) => {{
        let mut thread = $crate::make_thread!($name, $prio, $stack_size, $local_storage_size);
        let executor = $crate::make_thread_executor!();
        thread.start_executor(executor);
        thread
    }};
    ($name: expr, $prio : expr, $stack_size : expr, $local_storage_size : expr) => {{
        let mut thread = $crate::make_thread!($name, $prio, $stack_size);
        let local_storage = $crate::make_local_storage!($local_storage_size);
        thread.set_local_storage(local_storage);
        thread
    }};
    ($name: expr, $prio : expr, $stack_size: expr) => {{
        static STACK: $crate::Stack<{ $stack_size }> = $crate::Stack::new();
        type T = impl ::core::marker::Sized + ::core::marker::Send + FnMut();
        static THREAD: $crate::Thread<{ $prio }, T> = $crate::Thread::new($name, STACK.borrow());
        THREAD.init()
    }};
}

pub const INVALID_THREAD_ID: u32 = 0;
pub const IDLE_THREAD_ID: u32 = 1;

static NEXT_FREE_THREAD_ID: AtomicU32 = AtomicU32::new(IDLE_THREAD_ID);

pub struct LockListTag {}

impl LinkedListTag for LockListTag {}

#[derive(Copy, Clone, Debug)]
pub enum TryWaitEventsError {
    WouldBlock,
}

pub struct ThreadInfo {
    pub name: &'static str,
    pub state: ThreadExecutionState,
    pub base_priority: Priority,
    pub stack_addr: *const (),
    pub stack_size: usize,
    pub entry: *const (),
}

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

    pub(crate) stack: StackRef,

    // Thread base priority
    pub base_priority: Priority,

    // Atomically updated pair of (section lock priority, highest owned lock priority).
    // Either lock priority can be INVALID_PRIORITY if no such lock is held.
    lock_priorities: AtomicPriorityPair,

    // List of locks which this thread is the current owner of. Ordered in descending
    // ceiling priority order so that list head is always one of the highest priority
    // locks. List of owned locks must be maintained because in Rust programming model
    // nested locks may be released in any order.
    pub(crate) owned_locks: LockedRefCell<LinkedList<RawCeilingLock, LockListTag>, PreemptLock>,

    // Thread state that tells in which queue the thread currently is
    //  Stopped: Not in any queue
    //  Ready: In ready queue
    //  Running: Currently running, not in any queue
    //  Blocked: In blocked queue
    pub(crate) state: LockedCell<ThreadExecutionState, PreemptLock>,

    // Intrusive linked list entry for inserting the thread into ready, suspended, or blocked queue
    pub(crate) exec_queue_link: Link<RawThread, ExecStateTag>,

    pub(crate) suspendable: Suspendable,

    pub(crate) waited_events_mask: AtomicU32,
    pub(crate) sent_events_mask: AtomicU32,

    pub(crate) local_storage: OnceLock<LocalStorage>,

    /// Thread context holds the KHAL defined thread information such as
    /// trap frame on embedded targets, or pthreads thread in simulator.
    /// The context is initialized when the thread is started.
    pub(crate) context: MaybeUninit<Context>,
}

impl RawThread {
    const fn new(
        name: &'static str,
        base_priority: Priority,
        main_fn: *const (),
        stack: StackRef,
    ) -> RawThread {
        RawThread {
            thread_id: INVALID_THREAD_ID,
            state: LockedCell::new(ThreadExecutionState::Created),
            name,
            base_priority,
            lock_priorities: AtomicPriorityPair::new((INVALID_PRIORITY, INVALID_PRIORITY)),
            main_fn,
            stack,
            owned_locks: LockedRefCell::new(LinkedList::new()),
            exec_queue_link: Link::new(),
            suspendable: Suspendable::new(),
            waited_events_mask: AtomicU32::new(0),
            sent_events_mask: AtomicU32::new(0),
            local_storage: OnceLock::new(),
            context: MaybeUninit::uninit(),
        }
    }

    pub unsafe fn start(&'static mut self) {
        if *self.state.get_mut() != ThreadExecutionState::Created {
            panic!("Cannot start thread twice");
        }

        *self.state.get_mut() = ThreadExecutionState::Started;

        crate::thread_start(self);
    }

    pub fn get_info(&self, pkey: PreemptLockKey<'_>) -> ThreadInfo {
        ThreadInfo {
            name: self.name,
            state: self.state.get(pkey),
            base_priority: self.base_priority,
            stack_addr: self.stack.bottom_ptr() as *const (),
            stack_size: self.stack.size(),
            entry: self.main_fn,
        }
    }

    pub fn as_ref(&'static self) -> ThreadRef {
        ThreadRef::new(self)
    }

    pub(crate) fn acquire_lock<'key>(
        &'static self,
        pkey: PreemptLockKey<'key>,
        lock: &RawCeilingLock,
    ) {
        if lock.owner.get(pkey) != INVALID_THREAD_ID {
            panic!("This should not be possible");
        }

        match self.state.get(pkey) {
            ThreadExecutionState::Running => {
                lock.owner.set(pkey, self.thread_id);
                self.owned_locks
                    .borrow_mut(pkey)
                    .insert_after_condition(lock, |a, b| a.ceiling_priority > b.ceiling_priority);

                self.update_owned_lock_priority(pkey);
            }
            state => panic!(
                "Thread {} cannot acquire ceiling lock in {:?} state",
                self.name, state
            ),
        }
    }

    pub(crate) fn release_lock<'key>(
        &'static self,
        pkey: PreemptLockKey<'key>,
        lock: &RawCeilingLock,
    ) {
        match lock.owner.get(pkey) {
            INVALID_THREAD_ID => {
                // Thread has not been acquired, nothing to release
                ()
            }
            thread_id if thread_id == self.thread_id => {
                self.owned_locks.borrow_mut(pkey).remove(lock);
                lock.owner.set(pkey, INVALID_THREAD_ID);

                self.update_owned_lock_priority(pkey);

                // A thread is releasing a lock, therefore it must be running, and
                // have the highest priority at that time. If priority drops
                // below the priority of another ready thread, rescheduling must
                // be executed.
                Scheduler::cond_reschedule(pkey);
            }
            _ => {
                // The `thread` is not the owner of the lock. A lock can be released only
                // by the owner.
                crate::runtime_error!(RuntimeError::LockOwnerViolation);
            }
        }
    }

    /// Highest lock priority. Returns `INVALID_PRIORITY` if no locks owned by the thread.
    pub(crate) fn lock_priority<'key>(&self) -> Priority {
        let priorities = self.lock_priorities.load(Ordering::SeqCst);
        priorities.0.max(priorities.1)
    }

    /// Thread priority
    ///
    /// A thread can temporary boost its priority by acquiring locks. If a thread
    /// owns any locks, the highest owned lock priority will be returned; otherwise,
    /// returns the thread base priority.
    pub(crate) fn active_priority<'key>(&self) -> Priority {
        let lock_priority = self.lock_priority();
        self.base_priority.max(lock_priority)
    }

    // Returns previous priority
    pub(crate) fn raise_section_lock_priority(&self, new_priority: Priority) -> Priority {
        let new_priority = new_priority.max(self.base_priority);
        loop {
            let current_priorities = self.lock_priorities.load(Ordering::SeqCst);
            let new_priorities = (current_priorities.0.max(new_priority), current_priorities.1);

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

    pub(crate) fn set_section_lock_priority(&self, new_priority: Priority) {
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

    fn update_owned_lock_priority<'key>(&self, pkey: PreemptLockKey<'key>) {
        let new_priority = if let Some(head) = self.owned_locks.borrow(pkey).head() {
            head.ceiling_priority
        } else {
            INVALID_PRIORITY
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
        Scheduler::resume_thread(self);
    }

    pub(crate) fn set_wakeup_event(&self) {
        self.sent_events_mask
            .fetch_or(SCHEDULER_WAKEUP_EVENT, Ordering::Release);
    }

    pub(crate) fn set_resume_event(&self) {
        self.sent_events_mask
            .fetch_or(SCHEDULER_NOTIFY_EVENT, Ordering::Release);
    }

    pub fn send_events(&'static self, mut events: u32) {
        // Remove require_all flag from incoming events. This flag is only used for receiving events.
        events &= !REQUIRE_ALL_EVENTS;

        // Update sent events mask
        let sent_events = self.sent_events_mask.fetch_or(events, Ordering::SeqCst) | events;

        // Read receiving events mask. This should be empty if the receiving thread is not
        // waiting for any events.
        let mut receiving_events = self.waited_events_mask.load(Ordering::SeqCst);

        // Extract and remove the require_all flag from the receiving events mask
        let require_all = (receiving_events & REQUIRE_ALL_EVENTS) != 0;
        receiving_events &= !REQUIRE_ALL_EVENTS;

        // Filter out events that have not been received.
        let received_events = receiving_events & sent_events;

        // Notify the thread if it is waiting for any/all of the sent events
        if (!require_all && received_events != 0)
            || (received_events != 0 && received_events == receiving_events)
        {
            Scheduler::resume_thread(self);
        }
    }

    pub fn wait_events(&self, events: u32) -> u32 {
        // Wait for the events. The syscall returns events read before blocking.
        let mut received_events = syscall::thread_wait_event(events);

        // Clear waited events mask to suppress thread notifications from sender.
        self.waited_events_mask.store(0, Ordering::SeqCst);

        // In case there were more events sent between thread notification and syscall return,
        // Read and clear matching events that could have lead to the thread wakeup, now that
        // the waited events mask has been cleared.
        received_events |= self.sent_events_mask.fetch_and(!events, Ordering::SeqCst) & events;

        received_events
    }

    pub fn wait_events_until(
        &self,
        mut events: u32,
        deadline: Instant,
    ) -> Result<u32, WaitEventsUntilError> {
        // Wait for the events. The syscall returns events read before blocking.
        let mut received_events = syscall::thread_wait_event_until(events, deadline);

        // Clear waited events mask to suppress thread notifications from sender.
        self.waited_events_mask.store(0, Ordering::SeqCst);

        // In case there were more events sent between thread notification and syscall return,
        // Read and clear matching events that could have lead to the thread wakeup, now that
        // the waited events mask has been cleared.
        events |= SCHEDULER_WAKEUP_EVENT;
        received_events |= self.sent_events_mask.fetch_and(!events, Ordering::SeqCst) & events;

        if received_events & SCHEDULER_WAKEUP_EVENT != 0 {
            Err(WaitEventsUntilError::Timeout(received_events))
        } else {
            Ok(received_events)
        }
    }

    pub fn try_wait_events(&self, mut events: u32) -> Result<u32, TryWaitEventsError> {
        let require_all = events & REQUIRE_ALL_EVENTS != 0;
        events &= !REQUIRE_ALL_EVENTS;

        let sent_events = self.sent_events_mask.load(Ordering::SeqCst);

        if (!require_all && sent_events & events != 0) || (sent_events & events) == events {
            Ok(self.wait_events(events))
        } else {
            Err(TryWaitEventsError::WouldBlock)
        }
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

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ThreadRef(NonNull<RawThread>);

impl ThreadRef {
    fn new(thread: &'static RawThread) -> ThreadRef {
        ThreadRef(NonNull::from(thread))
    }

    unsafe fn from_ptr(ptr: *const RawThread) -> ThreadRef {
        ThreadRef(NonNull::new_unchecked(ptr as *mut RawThread))
    }

    pub fn name(&self) -> &'static str {
        unsafe { self.0.as_ref().name }
    }

    pub fn send_events(&self, event: u32) {
        unsafe { self.0.as_ref().send_events(event) }
    }

    pub fn priority(&self) -> Priority {
        unsafe { self.0.as_ref().active_priority() }
    }

    pub(crate) unsafe fn as_ref(&self) -> &'static RawThread {
        self.0.as_ref()
    }
}

impl PartialEq for ThreadRef {
    fn eq(&self, other: &ThreadRef) -> bool {
        core::ptr::eq(self.0.as_ptr(), other.0.as_ptr())
    }
}

impl Eq for ThreadRef {}

unsafe impl Sync for ThreadRef {}
unsafe impl Send for ThreadRef {}

pub struct ThreadBuilder<const PRIO: ThreadPriority, F: FnMut() + Send + 'static> {
    thread: &'static mut RawThread,
    closure: &'static mut MaybeUninit<F>,
}

impl<const PRIO: ThreadPriority, F: FnMut() + Send + 'static> ThreadBuilder<PRIO, F> {
    pub fn start(self, closure: F) -> ThreadRef {
        let closure_ref = self.closure.write(closure);
        let closure_ptr = closure_ref as *const F as *const ();
        self.thread.thread_id = NEXT_FREE_THREAD_ID.fetch_add(1, Ordering::SeqCst);
        unsafe {
            Context::init(
                self.thread.name,
                self.thread.main_fn,
                Some(closure_ptr as *const u8),
                self.thread.stack.bottom_ptr(),
                self.thread.stack.size(),
                self.thread.context.as_mut_ptr(),
            );
            // Initialize thread stack canary
            self.thread.stack.canary_mut().init();
        }

        let thread_ref = unsafe { ThreadRef::from_ptr(self.thread as *const _) };
        unsafe { self.thread.start() };

        thread_ref
    }

    pub fn init(self, closure: F) -> ThreadRef {
        let closure_ref = self.closure.write(closure);
        let closure_ptr = closure_ref as *const F as *const ();
        self.thread.thread_id = NEXT_FREE_THREAD_ID.fetch_add(1, Ordering::SeqCst);
        unsafe {
            Context::init(
                self.thread.name,
                self.thread.main_fn,
                Some(closure_ptr as *const u8),
                self.thread.stack.bottom_ptr(),
                self.thread.stack.size(),
                self.thread.context.as_mut_ptr(),
            );
            // Initialize thread stack canary
            self.thread.stack.canary_mut().init();
        }

        ThreadRef::new(self.thread)
    }

    pub fn set_local_storage(&mut self, local_storage: LocalStorage) {
        self.thread
            .set_local_storage(local_storage)
            .unwrap_or_else(|_| {
                panic!("TLS already set");
            });
    }

    pub fn start_executor(&mut self, executor: &'static LocalCell<ThreadExecutor>) {
        self.thread.start_executor(executor);
    }

    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawThread) -> R) -> R {
        f(self.thread)
    }

    pub fn name(&self) -> &'static str {
        self.thread.name
    }

    pub fn base_priority(&self) -> Priority {
        self.thread.base_priority
    }

    pub fn as_ref(&self) -> ThreadRef {
        unsafe { ThreadRef::from_ptr(self.thread as *const _) }
    }

    pub fn stack_ref(&self) -> &StackRef {
        &self.thread.stack
    }
}

pub struct Thread<const PRIO: ThreadPriority, F: FnMut() + Send> {
    thread: ConstStaticCell<RawThread>,
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: ThreadPriority, F: FnMut() + Send> Thread<PRIO, F> {
    pub const fn new(name: &'static str, stack: StackRef) -> Thread<PRIO, F> {
        Thread {
            thread: ConstStaticCell::new(RawThread::new(
                name,
                Priority::thread_priority(PRIO),
                Self::closure_wrapper as *const (),
                stack,
            )),
            closure: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    unsafe extern "C" fn closure_wrapper(closure_ptr: *mut ::core::ffi::c_void) {
        let closure = &mut *(closure_ptr as *mut F);
        closure();
    }

    pub fn init(&'static self) -> ThreadBuilder<PRIO, F> {
        let thread = self.thread.take();
        thread.suspendable = Suspendable::new_thread(thread);
        let closure = unsafe { &mut *self.closure.get() };
        ThreadBuilder { thread, closure }
    }
}

unsafe impl<const PRIO: ThreadPriority, F: FnMut() + Send> Sync for Thread<PRIO, F> {}
