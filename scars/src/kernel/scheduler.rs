use crate::cell::{LockedCell, LockedRefCell, RefMut};
use crate::kernel::list::{impl_linked, Linked, LinkedList, LinkedListTag};
use crate::kernel::tracing;
use crate::kernel::wait_queue::WaitQueueTag;
use crate::kernel::{
    atomic_list::AtomicQueue,
    hal::{
        disable_alarm_interrupt, disable_interrupts, enable_alarm_interrupt, enable_interrupts,
        get_interrupt_threshold, set_alarm, set_interrupt_threshold, start_first_task, Context,
    },
    handle_runtime_error,
    interrupt::{
        current_interrupt, in_interrupt, init_isr_stack_canary, set_ceiling_threshold,
        uncritical_section, InterruptControlBlock,
    },
    priority::{any_interrupt_priority, AnyPriority, Priority},
    stack::StackCanary,
    syscall,
    task::{Task, TaskControlBlock, TaskInfo, TaskState, IDLE_TASK_ID, INVALID_TASK_ID},
    RuntimeError, Stack, TaskPriority, WaitQueue,
};
use crate::printkln;
use crate::sync::{
    interrupt_lock::InterruptLockKey, preempt_lock::PreemptLockKey, InterruptLock, KeyToken, Lock,
    PreemptLock, RawCeilingLock,
};
use core::cell::SyncUnsafeCell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use scars_hal::{ContextInfo, FlowController};

pub struct ExecStateTag {}

impl LinkedListTag for ExecStateTag {}

extern "C" {
    static _isr_stack_end: u8;
}

pub enum ExecutionContext {
    Interrupt(&'static InterruptControlBlock),
    Task(&'static TaskControlBlock),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum RescheduleKind {
    /// No rescheduling
    None = 0,

    /// Yield to task with higher priority
    YieldToHigher = 1,

    /// Yield to task with equal or higher priority.
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

// Tasks waiting to be resumed when preemption lock is released
static PENDING_RESUME: AtomicQueue<TaskControlBlock, ExecStateTag> = AtomicQueue::new();

// The kind of pending reschedule. Rescheduling may be triggered by different
// events, but they are always executed either at the end of interrupt handling,
// or when the preemption lock is released.
static PENDING_RESCHEDULE: AtomicRescheduleKind = AtomicRescheduleKind::new(RescheduleKind::None);

// Scheduler is accessible only while holding the preemption lock
static SCHEDULER: SyncUnsafeCell<MaybeUninit<LockedRefCell<Scheduler, PreemptLock>>> =
    SyncUnsafeCell::new(MaybeUninit::uninit());

pub struct Scheduler {
    // Task execution state is either ready, blocked, or pending resume
    // Tasks that are ready to run are in the ready_queue sorted in descending
    // priority order.
    ready_queue: LinkedList<TaskControlBlock, ExecStateTag>,

    // Tasks that are blocked in wait list, or waiting for timed wakeup, are in blocked
    // list sorted in descending lock priority order.
    blocked_list: LinkedList<TaskControlBlock, ExecStateTag>,

    // A wakeup time sorted queue of tasks that are waiting for timed wakeup.
    wakeup_queue: LinkedList<TaskControlBlock, WaitQueueTag>,

    idle_task: &'static TaskControlBlock,

    // Currently running task
    current_task: &'static TaskControlBlock,
}

impl Scheduler {
    pub(crate) fn new(idle_task: &'static TaskControlBlock) -> Scheduler {
        Scheduler {
            ready_queue: LinkedList::new(),
            wakeup_queue: LinkedList::new(),
            blocked_list: LinkedList::new(),
            idle_task,
            current_task: idle_task,
        }
    }

    pub(super) fn start() -> ! {
        let idle_task = crate::kernel::idle::init_idle_task();
        unsafe {
            let _ = (&mut *SCHEDULER.get()).write(LockedRefCell::new(Scheduler::new(idle_task)));
        }
        let idle_context = Scheduler::current_task_context();
        start_first_task(idle_context)
    }

    fn borrow_mut(pkey: PreemptLockKey<'_>) -> RefMut<'_, Scheduler> {
        unsafe { (&*SCHEDULER.get()).assume_init_ref().borrow_mut(pkey) }
    }

    // Current task reference is an exception, and it can be accessed without locking.
    // Current task changes only when exiting the outermost interrupt handler.
    // TBD: there might be an interrupt.. perhaps make it atomic outside scheduler.
    pub(crate) fn current_task() -> &'static TaskControlBlock {
        unsafe { &*(&*SCHEDULER.get()).assume_init_ref().as_ptr() }.current_task
    }

    pub(crate) fn current_task_context() -> *mut Context {
        Scheduler::current_task().context.as_ptr() as *mut _
    }

    pub(crate) fn current_execution_context() -> ExecutionContext {
        match current_interrupt() {
            Some(interrupt_context) => {
                ExecutionContext::Interrupt(unsafe { interrupt_context.as_ref() })
            }
            None => ExecutionContext::Task(Scheduler::current_task()),
        }
    }

    /// Returns new task to switch to, or `None` if staying in the current task.
    fn choose_task_to_run<'a>(&mut self, kind: RescheduleKind) -> Option<&'a TaskControlBlock> {
        let current_priority = self.current_task.active_priority();

        // Highest priority of any locks held by the current or blocked tasks.
        let locks_priority = self.locks_priority_ceiling();

        // Minimum priority of a task that is allowed to run next
        let min_priority = match kind {
            RescheduleKind::YieldToEqualOrHigher => current_priority.max(locks_priority.succ()),
            RescheduleKind::YieldToHigher => {
                // Any task that has higher priority than the current task
                current_priority.max(locks_priority).succ()
            }
            RescheduleKind::None => {
                // Any task that is above the lock ceiling
                Priority::MIN_TASK_PRIORITY.max(locks_priority.succ())
            }
        };

        // Return highest priority task that is at or above the minimum priority allowed to run
        if let Some(ready_task) = self.ready_queue.head() {
            if min_priority <= ready_task.active_priority() {
                return self.ready_queue.pop_front();
            }
        }

        // If no suitable task was found in ready queue, stay in the current task
        // if allowed. Staying in current task is allowed only when YieldEqualOrHigher.
        if kind == RescheduleKind::YieldToEqualOrHigher {
            return None;
        }

        // In None and YieldToHigher, the fallback is the Idle task, which is special
        // in that it is not allowed to use CeilingLock, and therefore does not have
        // to strictly follow the ceiling locking protocol.
        if Scheduler::current_task().task_id == self.idle_task.task_id {
            None
        } else {
            Some(self.idle_task)
        }
    }

    fn insert_to_ready_queue(&mut self, task: &'static TaskControlBlock) {
        task.state.set(TaskState::Ready);
        if task.task_id == self.idle_task.task_id {
            // Idle task is always ready, but it is never inserted to ready queue.
            return;
        }
        tracing::task_ready_begin(task.as_ref());
        if !task.lock_priority().is_valid() {
            // If task is not holding any locks, then task goes to the back of its priority queue
            self.ready_queue
                .insert_after_condition(task, |queue_task, task| {
                    queue_task.active_priority() >= task.active_priority()
                });
        } else {
            // If task is holding any locks, then it goes to the front of its priority queue
            self.ready_queue
                .insert_after_condition(task, |queue_task, task| {
                    queue_task.active_priority() > task.active_priority()
                });
        }
    }

    fn insert_to_blocked_queue(&mut self, task: &'static TaskControlBlock) {
        task.state.set(TaskState::Blocked);
        if task.task_id == self.idle_task.task_id {
            panic!("Idle task may not block");
        }

        tracing::task_ready_end(task.as_ref());
        self.blocked_list
            .insert_after_condition(task, |queue_task, task| {
                let queue_task_priority = queue_task.lock_priority();
                let task_priority = task.lock_priority();

                if queue_task_priority.is_valid() && task_priority.is_valid() {
                    queue_task_priority >= task_priority
                } else if queue_task_priority.is_valid() {
                    true
                } else if task_priority.is_valid() {
                    false
                } else {
                    false
                }
            });
    }

    fn insert_to_wakeup_queue(&mut self, pl: PreemptLockKey<'_>, task: &'static TaskControlBlock) {
        task.state.set(TaskState::Blocked);
        if task.task_id == self.idle_task.task_id {
            panic!("Idle task may not block");
        }

        tracing::task_ready_end(task.as_ref());
        self.wakeup_queue
            .insert_after_condition(task, |queue_task, task| {
                queue_task.wakeup_time.get(pl) <= task.wakeup_time.get(pl)
            });
    }

    pub(crate) fn resume_task(pl: PreemptLockKey<'_>, task: &'static TaskControlBlock) {
        let mut scheduler = Scheduler::borrow_mut(pl);
        match task.state.get() {
            TaskState::Blocked => {
                // If task has wakeup time, then it is also in the wakeup queue
                if task.wakeup_time.get(pl).is_some() {
                    scheduler.wakeup_queue.remove(task);
                }
                scheduler.blocked_list.remove(task);
                scheduler.insert_to_ready_queue(task);
            }
            TaskState::Stopped | TaskState::PendingReady => {
                scheduler.insert_to_ready_queue(task);
            }
            state => {
                panic!("invalid {} state for resume: {:?}", task.name, state);
            }
        }

        let current_task = Scheduler::current_task();
        let current_priority = current_task.active_priority();
        let locks_priority = scheduler.locks_priority_ceiling();
        let min_priority = current_priority.max(locks_priority);
        if min_priority < task.active_priority() {
            Scheduler::set_pending_reschedule(RescheduleKind::YieldToHigher)
        }
    }

    fn block_task(
        &mut self,
        pl: PreemptLockKey<'_>,
        task: &'static TaskControlBlock,
        timeout_opt: Option<u64>,
    ) {
        match timeout_opt {
            Some(timeout) => {
                task.wakeup_time.set(pl, Some(timeout));
                self.insert_to_wakeup_queue(pl, task);
                self.reprogram_wakeup(pl);
            }
            None => {
                task.wakeup_time.set(pl, None);
            }
        }

        self.insert_to_blocked_queue(task);
    }

    fn check_stack_overflow(&self) {
        let stack = &Scheduler::current_task().stack;
        let canary = stack.canary_ref();
        if !canary.is_alive() {
            panic!(
                "Stack overflow detected from canary in task {:?}",
                Scheduler::current_task().name
            );
        }
    }

    fn switch_task(
        &mut self,
        _pkey: PreemptLockKey<'_>,
        new: &'static TaskControlBlock,
    ) -> &'static TaskControlBlock {
        // Whenever current task is switched out, check its stack canary for
        // stack overflow that could have occurred during the task execution.
        self.check_stack_overflow();
        //printkln!("[scheduler] switching to task {}", new.name);
        new.state.set(TaskState::Running);
        let new_prio = new.active_priority();
        let new_task_ref = new.as_ref();
        if new.task_id == IDLE_TASK_ID {
            tracing::system_idle();
        }
        let old = core::mem::replace(&mut self.current_task, new);
        tracing::task_exec_end(old.as_ref());
        tracing::task_exec_begin(new_task_ref);
        old.state.set(TaskState::Ready);
        // Running task has changed, current priority threshold is updated.
        // If a task is allowed to run, then it has higher priority than
        // any locks held currently.
        set_ceiling_threshold(new_prio);

        old
    }

    fn reprogram_wakeup(&self, pl: PreemptLockKey<'_>) {
        match self.wakeup_queue.head() {
            Some(sleeping_task) => {
                if let Some(wakeup_time) = sleeping_task.wakeup_time.get(pl) {
                    set_alarm(wakeup_time);
                } else {
                    panic!("No wakeup time set for task in wakeup queue");
                }
            }
            None => {
                // Disable wakeup
                set_alarm(u64::MAX);
            }
        }
    }

    fn locks_priority_ceiling(&self) -> Priority {
        if let Some(blocked_task) = self.blocked_list.head() {
            let blocked_prio = blocked_task.lock_priority();
            let current_prio = Scheduler::current_task().lock_priority();
            blocked_prio.max(current_prio)
        } else {
            Scheduler::current_task().lock_priority()
        }
    }

    // Start scheduler in ISR
    pub(crate) fn start_isr(_ikey: InterruptLockKey<'_>) {
        // Scheduler has already been initialized with the idle task
        // when it was created. When kernel returns from trap handler,
        // it will return to the idle task.
        set_alarm(u64::MAX);
        enable_alarm_interrupt();
    }

    // Notification
    // Task or ISR context
    pub(crate) fn notify_task(task: &'static TaskControlBlock) {
        if let Err(_) = PreemptLock::try_with(|pkey| {
            Scheduler::resume_task(pkey, task);
        }) {
            // Could not acquire pre-emption lock in ISR, because some task or ongoing lower
            // priority ISR holds the lock.
            // Store unblocked task in pending ready list instead, from which it will be
            // moved to ready list when the preempt lock is released.
            if task.task_id == IDLE_TASK_ID {
                panic!("");
            }
            task.state.set(TaskState::PendingReady);
            PENDING_RESUME.push_back(task);
        };
    }

    fn reschedule<'key>(&mut self, pkey: PreemptLockKey<'key>, kind: RescheduleKind) {
        // Wakeup sleeping tasks that should have been woken up
        let now = crate::clock_ticks();
        loop {
            if let Some(sleeping_task) = self.wakeup_queue.head() {
                if let Some(wakeup_time) = sleeping_task.wakeup_time.get(pkey) {
                    if wakeup_time > now {
                        // No more tasks to wake up
                        set_alarm(wakeup_time);
                        break;
                    }
                } else {
                    panic!("Sleeping task must have wakeup time");
                }
            } else {
                // Wakeup queue is empty, disable wakeup
                set_alarm(u64::MAX);
                break;
            }

            if let Some(sleeping_task) = self.wakeup_queue.pop_front() {
                self.blocked_list.remove(sleeping_task);
                self.insert_to_ready_queue(sleeping_task);
            }
        }

        if let Some(ready) = self.choose_task_to_run(kind) {
            let previous_current = self.switch_task(pkey, ready);
            self.insert_to_ready_queue(previous_current);
        }
    }

    pub(crate) fn cond_reschedule<'key>(pkey: PreemptLockKey<'key>) {
        let scheduler = Scheduler::borrow_mut(pkey);
        if let Some(ready_task) = scheduler.ready_queue.head() {
            if Scheduler::current_task().active_priority() < ready_task.active_priority() {
                // Rescheduling will be executed when preemption lock is released
                Scheduler::set_pending_reschedule(RescheduleKind::YieldToHigher);
            }
        }
    }

    pub(crate) fn set_pending_reschedule(kind: RescheduleKind) {
        PENDING_RESCHEDULE.update(kind)
    }

    pub(crate) fn is_reschedule_pending() -> bool {
        PENDING_RESCHEDULE.is_some()
    }

    pub(crate) fn get_task_pending_resume() -> Option<&'static TaskControlBlock> {
        PENDING_RESUME.pop_front()
    }

    // ISR context
    pub(crate) fn execute_pending_task_switch(ikey: InterruptLockKey<'_>) {
        match PENDING_RESCHEDULE.take() {
            RescheduleKind::None => (),
            kind => {
                if let Err(_) = PreemptLock::try_with(|pkey| {
                    uncritical_section(ikey, || {
                        let mut scheduler = Scheduler::borrow_mut(pkey);
                        scheduler.reschedule(pkey, kind);
                    })
                }) {
                    // Task or lower priority interrupt handler is holding the lock.
                    // Postpone task switch execution to lock release.
                    Scheduler::set_pending_reschedule(kind);
                };
            }
        }
    }

    pub(crate) fn start_task_isr(ikey: InterruptLockKey<'_>, task: &'static mut TaskControlBlock) {
        if task.state.get() != TaskState::Stopped {
            panic!("Task already running");
        }

        tracing::task_new(task.as_ref());

        if let Err(_) = PreemptLock::try_with(|pkey| {
            uncritical_section(ikey, || {
                Scheduler::resume_task(pkey, task);
            })
        }) {
            task.state.set(TaskState::PendingReady);
            PENDING_RESUME.push_back(task);
        };
    }

    // Pre-emption
    // ISR context
    pub(crate) fn wakeup_scheduler_isr(_ikey: InterruptLockKey<'_>) {
        Scheduler::set_pending_reschedule(RescheduleKind::YieldToHigher);

        // Maybe not needed because wakeup interrupt is disabled
        set_alarm(u64::MAX);
    }

    // Yield current task
    // ISR context
    pub(crate) fn yield_current_task_isr(_ikey: InterruptLockKey<'_>) {
        // Yielding can switch to another task with equal priority
        Scheduler::set_pending_reschedule(RescheduleKind::YieldToEqualOrHigher);
    }

    // Blocking
    // ISR context
    pub(crate) fn wait_current_task_isr(ikey: InterruptLockKey<'_>) {
        if let Err(_) = PreemptLock::try_with(|pkey| {
            // To allow nested interrupts, enable interrupts while rescheduling.
            uncritical_section(ikey, || {
                let mut scheduler = Scheduler::borrow_mut(pkey);
                if Scheduler::current_task().task_id == scheduler.idle_task.task_id {
                    panic!("Idle task cannot block");
                }

                match scheduler.choose_task_to_run(RescheduleKind::None) {
                    Some(ready) => {
                        let blocked_task = scheduler.switch_task(pkey, ready);
                        scheduler.block_task(pkey, blocked_task, None);
                    }
                    None => {
                        // There should always be the idle task to run
                        unreachable!()
                    }
                }
            })
        }) {
            // Error: Task is blocking in a wait list while it holds the preempt lock.
            unreachable!()
        }
    }

    // Delay
    // ISR context
    pub(crate) fn delay_until_isr(ikey: InterruptLockKey<'_>, wakeup_time: u64) {
        if let Err(_) = PreemptLock::try_with(|pkey| {
            // To allow nested interrupts, enable interrupts while rescheduling.
            uncritical_section(ikey, || {
                let mut scheduler = Scheduler::borrow_mut(pkey);
                if Scheduler::current_task().task_id == scheduler.idle_task.task_id {
                    panic!("Idle task cannot block");
                }

                match scheduler.choose_task_to_run(RescheduleKind::None) {
                    Some(ready) => {
                        let previous_current = scheduler.switch_task(pkey, ready);
                        scheduler.block_task(pkey, previous_current, Some(wakeup_time));
                    }
                    None => {
                        // There should always be the idle task to run
                        unreachable!()
                    }
                }
            })
        }) {
            // Error: Task is trying to sleep while it holds the preempt lock.
            unreachable!()
        };
    }

    pub(crate) fn tasks(&self) -> impl Iterator<Item = &TaskControlBlock> {
        Some(self.current_task)
            .into_iter()
            .chain(self.ready_queue.cursor_front())
            .chain(self.blocked_list.cursor_front())
    }

    pub fn task_info(&self) -> impl Iterator<Item = TaskInfo> + '_ {
        self.tasks().map(|task| task.get_info())
    }
}

#[no_mangle]
pub fn _private_current_task_context() -> *mut () {
    Scheduler::current_task_context() as *mut ()
}

pub fn print_tasks() {
    printkln!("NAME       PRI  STATUS ENTRY");
    PreemptLock::with(|pkey| {
        let scheduler = Scheduler::borrow_mut(pkey);
        for info in scheduler.task_info() {
            printkln!(
                "{:<10} {:<4}   {:<4} {:x?}",
                info.name,
                info.base_priority,
                match info.state {
                    TaskState::Running => "Exec ",
                    TaskState::Ready => "Ready",
                    TaskState::Blocked => "Block",
                    TaskState::Suspended => "Susp ",
                    _ => "?",
                },
                info.entry,
            );
        }
    });
}
