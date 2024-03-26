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
        syscall::task_wait(self.list.as_ptr(), CEILING);
    }
}
