use crate::kernel::atomic_queue::{AtomicNode, AtomicQueue};
use crate::kernel::scheduler::RawScheduler;
use crate::kernel::scheduler::{ExecStateTag, Scheduler};
use crate::kernel::waiter::{
    SUSPENDABLE_PENDING_RESUME, SUSPENDABLE_PENDING_SUSPEND, SUSPENDABLE_PENDING_WAKEUP,
    Suspendable,
};
use crate::priority::Priority;
use crate::sync::preempt_lock::PreemptLockKey;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};

pub type WorkQueue = AtomicWorkQueue;
pub type WorkQueueNode = AtomicNode<Suspendable, ExecStateTag>;

pub struct AtomicWorkQueue {
    work_pending: AtomicBool,
    queue: AtomicQueue<Suspendable, ExecStateTag>,
}

impl AtomicWorkQueue {
    pub fn new() -> Self {
        Self {
            work_pending: AtomicBool::new(false),
            queue: AtomicQueue::new(),
        }
    }

    pub fn queue_work(
        &'static self,
        suspendable: Pin<&Suspendable>,
        work: u32,
        required_ceiling: Option<Priority>,
    ) {
        suspendable.set_pending(work);
        suspendable.set_required_ceiling(required_ceiling);
        let _ = self.queue.try_push_back(suspendable);
        self.work_pending.store(true, Ordering::Release);
    }

    pub fn complete_work(
        &'static self,
        pkey: PreemptLockKey<'_>,
        mut raw_scheduler: Pin<&mut RawScheduler>,
    ) {
        let current_priority = Scheduler::current_priority(pkey);

        while self.work_pending.swap(false, Ordering::AcqRel) {
            // Track first reinserted operation to detect when no progress can be made.
            let mut first_reinserted: Option<*const Suspendable> = None;

            while let Some(pending) = self.queue.pop_front() {
                if let Some(first_reinserted) = first_reinserted {
                    if pending.get_ref() as *const _ == first_reinserted {
                        // This is the first operation that was reinserted.
                        // No more progress can be made.
                        self.queue.push_back(pending);
                        break;
                    }
                }

                // If operation has a required ceiling, check if it can be completed at the
                // priority of the current context. If not, postpone it and reinsert it at the
                // end of the queue.
                if let Some(required_ceiling) = pending.required_ceiling.get() {
                    if current_priority > required_ceiling {
                        // The operation cannot be completed at the priority of the current context..
                        self.queue.push_back(pending);
                        if first_reinserted.is_none() {
                            first_reinserted = Some(pending.get_ref() as *const _);
                        }
                        continue;
                    }
                }

                // Complete the work.
                let pending_ops = pending.pending_mask.swap(0, Ordering::AcqRel);

                if pending_ops & SUSPENDABLE_PENDING_SUSPEND != 0 {
                    raw_scheduler
                        .as_mut()
                        .suspend_suspendable_now(pkey, pending);
                } else if pending_ops & SUSPENDABLE_PENDING_WAKEUP != 0 {
                    raw_scheduler.as_mut().wakeup_suspendable(pkey, pending);
                } else if pending_ops & SUSPENDABLE_PENDING_RESUME != 0 {
                    raw_scheduler.as_mut().resume_suspendable_now(pkey, pending);
                }
            }
        }
    }

    pub fn work_pending(&'static self) -> bool {
        self.work_pending.load(Ordering::Acquire)
    }
}
