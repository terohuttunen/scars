use crate::cell::{LockedCell, LockedRefCell};
use crate::kernel::{
    atomic_list::{impl_atomic_linked, AtomicQueueLink},
    hal::Context,
    interrupt::set_ceiling_threshold,
    list::{impl_linked, Link, LinkedList, LinkedListTag},
    priority::{AtomicPriorityPair, INVALID_PRIORITY},
    scheduler::ExecStateTag,
    scheduler::Scheduler,
    stack::StackRef,
    wait_queue::WaitQueueTag,
    AnyPriority, AtomicPriority, Priority, TaskPriority,
};
use crate::sync::{preempt_lock::PreemptLockKey, KeyToken, Lock, PreemptLock, RawCeilingLock};
use core::cell::{Cell, OnceCell, UnsafeCell};
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ops::Deref;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use scars_hal::{ContextInfo, FlowController};

#[macro_export]
macro_rules! make_task {
    ($name:expr, $prio:expr, $size:expr) => {{
        static STACK: $crate::Stack<{ $size }> = $crate::Stack::new();
        type T = impl ::core::marker::Sized + ::core::marker::Send + ::core::ops::FnMut();
        static TASK: $crate::Task<{ $prio }, T> = $crate::Task::new($name, STACK.borrow());

        &TASK
    }};
}

pub const INVALID_TASK_ID: u32 = 0;
pub const IDLE_TASK_ID: u32 = 1;

static NEXT_FREE_TASK_ID: AtomicU32 = AtomicU32::new(IDLE_TASK_ID);

pub struct LockListTag {}

impl LinkedListTag for LockListTag {}

pub struct TaskInfo {
    pub name: &'static str,
    pub state: TaskState,
    pub base_priority: Priority,
    pub stack_addr: *const (),
    pub stack_size: usize,
    pub entry: *const (),
}

#[derive(PartialEq, Eq, Hash, Copy, Clone, Debug)]
#[repr(C)]
pub enum TaskState {
    /// The initial state after task creation
    Stopped,

    /// Task is ready to run
    Ready,

    /// Task is is ready to run and waiting to be transferred to ready-queue
    PendingReady,

    /// Task is running
    Running,

    /// Task is blocked in a WaitQueue or sleeping and waiting for wakeup.
    Blocked,

    Suspended,
}

#[repr(align(16))]
#[repr(C)]
pub struct TaskControlBlock {
    pub task_id: u32,

    // task name
    pub name: &'static str,

    pub(crate) main_fn: *const (),

    pub(crate) stack: StackRef,

    // Task base priority
    pub base_priority: Priority,

    // Atomically updated pair of (section lock priority, highest owned lock priority).
    // Either lock priority can be INVALID_PRIORITY if no such lock is held.
    lock_priorities: AtomicPriorityPair,

    // List of locks which this task is the current owner of. Ordered in descending
    // ceiling priority order so that list head is always one of the highest priority
    // locks. List of owned locks must be maintained because in Rust programming model
    // nested locks may be released in any order.
    pub(crate) owned_locks: LockedRefCell<LinkedList<RawCeilingLock, LockListTag>, PreemptLock>,

    pub(crate) wakeup_time: LockedCell<Option<u64>, PreemptLock>,

    // TBD: could this be within a wrapper that allows read-only from task,
    // and mutable only from ISR? This should always change in ISR only.
    // Probably best to put this into PreemptCell as well as it should not
    // change if task switch cannot occur.
    pub(crate) state: Cell<TaskState>,

    // Intrusive linked list entry for inserting the task into ready, suspended, or blocked queue
    pub(crate) exec_queue_link: Link<TaskControlBlock, ExecStateTag>,

    // Intrusive linked list entry for putting the task into sleep or wait queue
    pub(crate) wait_queue_link: Link<TaskControlBlock, WaitQueueTag>,

    pub(crate) pending_queue_link: AtomicQueueLink<TaskControlBlock, ExecStateTag>,

    /// Task context holds the BSP defined task information such as
    /// trap frame on embedded targets, or pthreads thread in simulator.
    /// The context is initialized when the task is started.
    pub(crate) context: MaybeUninit<Context>,
}

impl TaskControlBlock {
    const fn new(
        name: &'static str,
        stack: StackRef,
        base_priority: Priority,
        main_fn: *const (),
    ) -> TaskControlBlock {
        TaskControlBlock {
            task_id: INVALID_TASK_ID,
            state: Cell::new(TaskState::Stopped),
            name,
            base_priority,
            lock_priorities: AtomicPriorityPair::new((INVALID_PRIORITY, INVALID_PRIORITY)),
            wakeup_time: LockedCell::new(None),
            main_fn,
            stack,
            owned_locks: LockedRefCell::new(LinkedList::new()),
            exec_queue_link: Link::new(),
            wait_queue_link: Link::new(),
            pending_queue_link: AtomicQueueLink::new(),
            context: MaybeUninit::uninit(),
        }
    }

    pub unsafe fn start(&mut self) {
        if self.state.get() != TaskState::Stopped {
            panic!("Cannot start task twice");
        }

        crate::task_start(self);
    }

    pub fn get_info(&self) -> TaskInfo {
        TaskInfo {
            name: self.name,
            state: self.state.get(),
            base_priority: self.base_priority,
            stack_addr: self.stack.bottom_ptr() as *const (),
            stack_size: self.stack.size(),
            entry: self.main_fn,
        }
    }

    pub fn as_ref(&'static self) -> TaskRef {
        TaskRef(self)
    }

    pub(crate) fn acquire_lock<'key>(
        &'static self,
        pkey: PreemptLockKey<'key>,
        lock: &RawCeilingLock,
    ) {
        if lock.owner.get(pkey) != INVALID_TASK_ID {
            panic!("This should not be possible");
        }

        match self.state.get() {
            TaskState::Running => {
                lock.owner.set(pkey, self.task_id);
                self.owned_locks
                    .borrow_mut(pkey)
                    .insert_after_condition(lock, |a, b| a.ceiling_priority > b.ceiling_priority);

                self.update_owned_lock_priority(pkey);
            }
            state => panic!(
                "Task {} cannot acquire ceiling lock in {:?} state",
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
            INVALID_TASK_ID => {
                // Task has not been acquired, nothing to release
                ()
            }
            task_id if task_id == self.task_id => {
                self.owned_locks.borrow_mut(pkey).remove(lock);
                lock.owner.set(pkey, INVALID_TASK_ID);

                self.update_owned_lock_priority(pkey);

                // A task is releasing a lock, therefore it must be running, and
                // have the highest priority at that time. If priority drops
                // below the priority of another ready task, rescheduling must
                // be executed.
                Scheduler::cond_reschedule(pkey);
            }
            _ => {
                // The `task` is not the owner of the lock. A lock can be released only
                // by the owner.
                crate::runtime_error!(RuntimeError::LockOwnerViolation);
            }
        }
    }

    /// Highest lock priority. Returns `INVALID_PRIORITY` if no locks owned by the task.
    pub(crate) fn lock_priority<'key>(&self) -> Priority {
        let priorities = self.lock_priorities.load(Ordering::SeqCst);
        priorities.0.max(priorities.1)
    }

    /// Task priority
    ///
    /// A task can temporary boost its priority by acquiring locks. If a task
    /// owns any locks, the highest owned lock priority will be returned; otherwise,
    /// returns the task base priority.
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
}

impl_linked!(exec_queue_link, TaskControlBlock, ExecStateTag);
impl_linked!(wait_queue_link, TaskControlBlock, WaitQueueTag);
impl_atomic_linked!(pending_queue_link, TaskControlBlock, ExecStateTag);

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct TaskRef(&'static TaskControlBlock);

impl TaskRef {
    pub(crate) fn new(tcb: &'static TaskControlBlock) -> TaskRef {
        TaskRef(tcb)
    }

    pub fn name(&self) -> &'static str {
        &self.0.name
    }
}

impl PartialEq for TaskRef {
    fn eq(&self, other: &TaskRef) -> bool {
        core::ptr::eq(self.0, other.0)
    }
}

impl Eq for TaskRef {}

pub struct Task<const PRIO: TaskPriority, F: FnMut() + Send> {
    task: UnsafeCell<TaskControlBlock>,
    // Task local storage is initialized when task is started.
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: TaskPriority, F: FnMut() + Send> Task<PRIO, F> {
    pub const fn new(name: &'static str, stack: StackRef) -> Task<PRIO, F> {
        Task {
            task: UnsafeCell::new(TaskControlBlock::new(
                name,
                stack,
                Priority::task_priority(PRIO),
                Self::closure_wrapper as *const (),
            )),
            closure: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    unsafe extern "C" fn closure_wrapper(closure_ptr: *mut ::core::ffi::c_void) {
        let closure = &mut *(closure_ptr as *mut F);
        closure();
    }

    pub fn init(&self, closure: F) {
        unsafe {
            let arg_ref = (*self.closure.get()).write(closure);
            let tcb = self.get_tcb_mut();
            tcb.task_id = NEXT_FREE_TASK_ID.fetch_add(1, Ordering::SeqCst);
            Context::init(
                tcb.name,
                tcb.main_fn,
                Some(arg_ref as *const F as *const u8),
                self.stack_ref().bottom_ptr(),
                self.stack_ref().size(),
                tcb.context.as_mut_ptr(),
            );

            // Initialize task stack canary
            tcb.stack.canary_mut().init();
        }
    }

    pub fn start(&self, closure: F) {
        unsafe {
            self.init(closure);
            (*self.task.get()).start();
        }
    }

    pub(crate) const fn get_tcb(&self) -> &TaskControlBlock {
        unsafe { &*self.task.get() }
    }

    /// SAFETY: Caller must guarantee that the task has not been started yet.
    pub(crate) unsafe fn get_tcb_mut(&self) -> &mut TaskControlBlock {
        unsafe { &mut *self.task.get() }
    }

    pub fn borrow(&'static self) -> TaskRef {
        TaskRef(self.get_tcb())
    }

    pub const fn name(&self) -> &'static str {
        self.get_tcb().name
    }

    pub const fn base_priority(&self) -> Priority {
        self.get_tcb().base_priority
    }

    pub const fn stack_ref(&self) -> &StackRef {
        &self.get_tcb().stack
    }
}

unsafe impl<const PRIO: TaskPriority, F: FnMut() + Send> Sync for Task<PRIO, F> {}
