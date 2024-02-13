use crate::cell::LockedRefCell;
use crate::kernel::{
    interrupt::in_interrupt,
    list::{LinkedList, LinkedListTag},
    priority::AnyPriority,
    scheduler, syscall, Scheduler, TaskControlBlock,
};
use crate::sync::{atomic_pair_lock, CeilingLock, InterruptLock};

pub struct WaitQueueTag {}

impl LinkedListTag for WaitQueueTag {}

pub(crate) struct WaitQueue<const CEILING: AnyPriority> {
    list: LockedRefCell<LinkedList<TaskControlBlock, WaitQueueTag>, CeilingLock<CEILING>>,
}

impl<const CEILING: AnyPriority> WaitQueue<CEILING> {
    pub const fn new() -> WaitQueue<CEILING> {
        WaitQueue {
            list: LockedRefCell::new(LinkedList::new()),
        }
    }

    #[inline(never)]
    pub fn notify_one(&self) {
        if let Some(task) = CeilingLock::with(|ckey| self.list.borrow_mut(ckey).pop_front()) {
            Scheduler::notify_task(task);
        }
    }

    #[inline(never)]
    pub fn notify_all(&self) {
        CeilingLock::with(|ckey| {
            let mut list = self.list.borrow_mut(ckey);
            while let Some(task) = list.pop_front() {
                Scheduler::notify_task(task);
            }
        });
    }

    // Only task can wait in queue. Runtime error if called from interrupt handler.
    pub fn wait(&self) {
        if in_interrupt() {
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }
        // Atomically insert current task to wait-list and execute task_wait syscall,
        // to prevent notifications between inserting to list and blocking in kernel.
        // Insertion to wait-list is done within a CeilingLock that does not prevent
        // higher priority tasks or interrupts from executing. Higher than ceiling
        // priority tasks and interrupts are not allowed to access the wait-list.
        atomic_pair_lock::<CeilingLock<CEILING>, InterruptLock, _, _, _>(
            |ckey| {
                let mut list = self.list.borrow_mut(ckey);
                let current_task = Scheduler::current_task();
                list.insert_after_condition(current_task, |queue_task, task| {
                    queue_task.active_priority() >= task.active_priority()
                });
            },
            |_, _, _| {},
            |ikey, _| syscall::task_wait(ikey),
        )
    }
}
