use crate::events::Events;
use crate::kernel::atomic_queue::{AtomicNode, AtomicQueue, impl_atomic_linked};
use crate::kernel::scheduler::RawScheduler;
use crate::kernel::scheduler::{ExecStateTag, Scheduler};
use crate::kernel::waiter::{
    SUSPENDABLE_PENDING_RECONFIGURE, SUSPENDABLE_PENDING_RESUME, SUSPENDABLE_PENDING_SUSPEND,
    SUSPENDABLE_PENDING_WAKEUP, WaitQueueEntry,
};
use crate::priority::Priority;
use crate::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use crate::sync::preempt_lock::PreemptLockKey;
use core::cell::Cell;
use core::marker::PhantomData;
use core::pin::Pin;

pub type WorkQueue = AtomicWorkQueue;
pub type WorkQueueNode = AtomicNode<RawPendingWorkEntry, ExecStateTag>;

pub type RawPendingWorkCallback = fn(*const (), PreemptLockKey<'_>, Pin<&mut RawScheduler>, u32);

pub struct RawPendingWorkEntry {
    pub(crate) node: WorkQueueNode,

    /// Mask of operations that could not be completed because of locks
    pub(crate) pending_mask: AtomicU32,

    /// The ceiling priority that is required to complete the operations.
    pub(crate) required_ceiling: Cell<Option<Priority>>,

    /// Callback invoked when the queue drains this entry. Set at
    /// construction; immutable thereafter.
    complete_fn: RawPendingWorkCallback,
    /// Pointer to the entity receiving the dispatch (passed to
    /// `complete_fn`). `null` until [`Self::set_receiver`] has been called
    /// — entries with no receiver are skipped on dispatch.
    receiver: AtomicPtr<()>,
}

impl RawPendingWorkEntry {
    pub const fn new(complete_fn: RawPendingWorkCallback) -> Self {
        Self {
            node: WorkQueueNode::new(),
            pending_mask: AtomicU32::new(0),
            required_ceiling: Cell::new(None),
            complete_fn,
            receiver: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    /// Bind this entry to its owner. Idempotent; safe to call from any
    /// context.
    pub fn set_receiver(&self, receiver: *const ()) {
        self.receiver.store(receiver as *mut (), Ordering::Release);
    }

    /// Invoke the callback if a receiver has been bound; otherwise skip.
    pub fn complete(&self, pkey: PreemptLockKey<'_>, scheduler: Pin<&mut RawScheduler>, ops: u32) {
        let receiver = self.receiver.load(Ordering::Acquire);
        if !receiver.is_null() {
            (self.complete_fn)(receiver as *const (), pkey, scheduler, ops);
        }
    }

    pub fn set_pending(&self, mask: u32) {
        self.pending_mask.fetch_or(mask, Ordering::Relaxed);
    }

    pub fn set_required_ceiling(&self, ceiling: Option<Priority>) {
        // TODO: max?
        self.required_ceiling.set(ceiling);
    }
}

impl_atomic_linked!(node, RawPendingWorkEntry, ExecStateTag);

/// Compile-time binding of a receiver type `Self` to a
/// [`PendingWorkEntry<Self>`]'s completion callback.
///
/// `complete` receives `Pin<&'static Self>`: queue entries are intrusive
/// nodes that the scheduler may dispatch arbitrarily later than the
/// publishing call, so the receiver must outlive any queue membership.
pub trait PendingWorkHandler: Sized + 'static {
    fn complete(
        this: Pin<&'static Self>,
        pkey: PreemptLockKey<'_>,
        sched: Pin<&mut RawScheduler>,
        ops: u32,
    );
}

/// Typed wrapper over [`RawPendingWorkEntry`] parameterized by a
/// [`PendingWorkHandler`] impl.
#[repr(transparent)]
pub struct PendingWorkEntry<H: PendingWorkHandler> {
    raw: RawPendingWorkEntry,
    _phantom: PhantomData<fn() -> H>,
}

/// Monomorphized adapter from [`RawPendingWorkCallback`] to
/// [`PendingWorkHandler::complete`].
fn pending_work_trampoline<H: PendingWorkHandler>(
    receiver: *const (),
    pkey: PreemptLockKey<'_>,
    sched: Pin<&mut RawScheduler>,
    ops: u32,
) {
    // SAFETY: `receiver` was published by `PendingWorkEntry::<H>::set_receiver`,
    // which only accepts `Pin<&'static H>`.
    let this: Pin<&'static H> = unsafe { Pin::new_unchecked(&*(receiver as *const H)) };
    H::complete(this, pkey, sched, ops);
}

impl<H: PendingWorkHandler> PendingWorkEntry<H> {
    pub const fn new() -> Self {
        Self {
            raw: RawPendingWorkEntry::new(pending_work_trampoline::<H>),
            _phantom: PhantomData,
        }
    }

    pub fn set_receiver(&self, owner: Pin<&'static H>) {
        self.raw
            .set_receiver(owner.get_ref() as *const _ as *const ());
    }

    #[allow(dead_code)]
    pub fn set_pending(&self, mask: u32) {
        self.raw.set_pending(mask)
    }

    #[allow(dead_code)]
    pub fn set_required_ceiling(&self, ceiling: Option<Priority>) {
        self.raw.set_required_ceiling(ceiling)
    }

    #[allow(dead_code)]
    pub fn complete(&self, pkey: PreemptLockKey<'_>, scheduler: Pin<&mut RawScheduler>, ops: u32) {
        self.raw.complete(pkey, scheduler, ops)
    }

    /// Project to the raw entry for queue operations.
    pub fn raw(self: Pin<&Self>) -> Pin<&RawPendingWorkEntry> {
        unsafe { self.map_unchecked(|t| &t.raw) }
    }
}

pub struct AtomicWorkQueue {
    work_pending: AtomicBool,
    queue: AtomicQueue<RawPendingWorkEntry, ExecStateTag>,
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
        pending_work: Pin<&RawPendingWorkEntry>,
        work: u32,
        required_ceiling: Option<Priority>,
    ) {
        pending_work.set_pending(work);
        pending_work.set_required_ceiling(required_ceiling);
        let _ = self.queue.try_push_back(pending_work);
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
            let mut first_reinserted: Option<*const RawPendingWorkEntry> = None;

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
                pending
                    .as_ref()
                    .complete(pkey, raw_scheduler.as_mut(), pending_ops);
            }
        }
    }

    pub fn work_pending(&'static self) -> bool {
        self.work_pending.load(Ordering::Acquire)
    }
}
