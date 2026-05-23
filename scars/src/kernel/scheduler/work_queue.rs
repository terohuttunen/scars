use crate::kernel::atomic_queue::{AtomicNode, AtomicQueue, impl_atomic_linked};
use crate::kernel::scheduler::ExecStateTag;
use crate::kernel::scheduler::RawScheduler;
use crate::sync::PreemptLockKey;
use crate::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use core::marker::PhantomData;
use core::pin::Pin;

pub type WorkQueue = AtomicWorkQueue;
pub type WorkQueueNode = AtomicNode<RawPendingWorkEntry, ExecStateTag>;

pub type RawPendingWorkCallback =
    fn(*const (), PreemptLockKey<'_>, Pin<&mut RawScheduler>, u32) -> bool;

pub struct RawPendingWorkEntry {
    pub(crate) node: WorkQueueNode,

    /// Mask of operations that could not be completed because of locks.
    /// Handlers OR new bits in via [`Self::set_pending`] and consume the
    /// mask in `complete()` — the work queue itself never inspects it.
    pub(crate) pending_mask: AtomicU32,

    /// Callback invoked when the queue drains this entry. Set at
    /// construction; immutable thereafter. Returns `true` if the entry
    /// has been fully processed; `false` if any op bit remains to be
    /// retried (handler has re-OR'd those bits into `pending_mask`).
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
            complete_fn,
            receiver: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    /// Bind this entry to its owner. Idempotent; safe to call from any
    /// context.
    pub fn set_receiver(&self, receiver: *const ()) {
        self.receiver.store(receiver as *mut (), Ordering::Release);
    }

    /// Invoke the callback if a receiver has been bound. Returns
    /// `true` if the operation completed, `false` if the handler asked
    /// to be re-queued. Skipped (treated as completed) when no receiver
    /// is bound.
    pub fn complete(
        &self,
        pkey: PreemptLockKey<'_>,
        scheduler: Pin<&mut RawScheduler>,
        ops: u32,
    ) -> bool {
        let receiver = self.receiver.load(Ordering::Acquire);
        if receiver.is_null() {
            true
        } else {
            (self.complete_fn)(receiver as *const (), pkey, scheduler, ops)
        }
    }

    pub fn set_pending(&self, mask: u32) {
        self.pending_mask.fetch_or(mask, Ordering::Relaxed);
    }
}

impl_atomic_linked!(node, RawPendingWorkEntry, ExecStateTag);

/// Compile-time binding of a receiver type `Self` to a
/// [`PendingWorkEntry<Self>`]'s completion callback.
///
/// `complete` receives `Pin<&'static Self>`: queue entries are intrusive
/// nodes that the scheduler may dispatch arbitrarily later than the
/// publishing call, so the receiver must outlive any queue membership.
///
/// The return value tells the drain whether the entry is done. `true`
/// means every dispatched op bit was handled; `false` means at least
/// one op could not be completed and the handler has re-OR'd those
/// bits into `pending_mask`.
pub trait PendingWorkHandler: Sized + 'static {
    fn complete(
        this: Pin<&'static Self>,
        pkey: PreemptLockKey<'_>,
        sched: Pin<&mut RawScheduler>,
        ops: u32,
    ) -> bool;
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
) -> bool {
    // SAFETY: `receiver` was published by `PendingWorkEntry::<H>::set_receiver`,
    // which only accepts `Pin<&'static H>`.
    let this: Pin<&'static H> = unsafe { Pin::new_unchecked(&*(receiver as *const H)) };
    H::complete(this, pkey, sched, ops)
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
    pub fn complete(
        &self,
        pkey: PreemptLockKey<'_>,
        scheduler: Pin<&mut RawScheduler>,
        ops: u32,
    ) -> bool {
        self.raw.complete(pkey, scheduler, ops)
    }

    /// Project to the raw entry for queue operations.
    pub fn raw(self: Pin<&Self>) -> Pin<&RawPendingWorkEntry> {
        unsafe { self.map_unchecked(|t| &t.raw) }
    }
}

pub struct AtomicWorkQueue {
    /// Set by every external producer; cleared by the drain. The outer
    /// drain loop iterates as long as this is set, so any post that
    /// races against an in-flight drain still wakes the consumer.
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

    pub fn queue_work(&'static self, pending_work: Pin<&RawPendingWorkEntry>, work: u32) {
        pending_work.set_pending(work);
        let _ = self.queue.try_push_back(pending_work);
        // Unconditional: external posts during an in-flight drain still
        // need the outer loop to re-iterate. Handler-initiated re-queues
        // from inside `complete_work` go through the queue directly, not
        // through this entry point, so they don't set the flag.
        self.work_pending.store(true, Ordering::Release);
    }

    pub fn complete_work(
        &'static self,
        pkey: PreemptLockKey<'_>,
        mut raw_scheduler: Pin<&mut RawScheduler>,
    ) {
        while self.work_pending.swap(false, Ordering::AcqRel) {
            // First entry the handler asked to re-queue in this pass.
            // When we pop it again we've cycled through every entry
            // that was in the queue at the moment of deferral — break
            // and let the outer loop re-iterate only if an external
            // producer signaled `work_pending` (which our internal
            // re-queues do not).
            let mut first_deferred: Option<*const RawPendingWorkEntry> = None;
            while let Some(pending) = self.queue.pop_front() {
                if Some(pending.get_ref() as *const _) == first_deferred {
                    // Cycled. Put back at tail; the next external post
                    // bumps `work_pending` and starts a fresh pass.
                    let _ = self.queue.try_push_back(pending);
                    break;
                }
                let ops = pending.pending_mask.swap(0, Ordering::AcqRel);
                let completed = pending.as_ref().complete(pkey, raw_scheduler.as_mut(), ops);
                if !completed {
                    // The operation cannot be completed right now. Defer to later time.
                    let _ = self.queue.try_push_back(pending);
                    if first_deferred.is_none() {
                        first_deferred = Some(pending.get_ref() as *const _);
                    }
                }
            }
        }
    }

    pub fn work_pending(&'static self) -> bool {
        self.work_pending.load(Ordering::Acquire)
    }
}
