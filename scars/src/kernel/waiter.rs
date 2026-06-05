use crate::Priority;
use crate::cell::{LockedPinRefCell, PinRefCell};
use crate::events::raw::RawEventHandler;
use crate::in_interrupt;
use crate::kernel::atomic_queue::{AtomicNode, impl_atomic_linked};
use crate::kernel::list::{LinkedList, LinkedListNode, LinkedListTag, Node, impl_linked};
use crate::kernel::scheduler::{ExecStateTag, ExecutionContext, Scheduler};
use crate::sync::atomic::{AtomicU32, Ordering};
use crate::sync::lock::preempt_lock::CorePreemptLockKey;
use crate::sync::{CeilingLock, CorePreemptLock, NestingLock, PreemptLock, PreemptLockKey};
use crate::task::raw_task::RawTask;
use crate::thread::RawThread;
use crate::time::Instant;
use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::pin::{Pin, pin};
use core::task::{RawWaker, Waker};

pub struct WaitQueueTag {}

impl LinkedListTag for WaitQueueTag {}

pub struct WaitQueueEntry {
    priority: Priority,

    /// Link for the WaitQueue.
    pub(crate) wait_queue_link: Node<Self, WaitQueueTag>,

    on_resume: fn(*const ()),
    arg: *const (),
}

impl WaitQueueEntry {
    pub const fn new() -> WaitQueueEntry {
        WaitQueueEntry {
            priority: Priority::MIN,
            wait_queue_link: Node::new(),
            on_resume: |_| {},
            arg: core::ptr::null(),
        }
    }

    /// Wire `owner` as the receiver of the on-resume callback. The
    /// generic trampoline is monomorphized per `H`.
    pub fn init_for<H: WaitQueueEntryHandler>(&mut self, owner: &'static H) {
        self.on_resume = waiter_trampoline::<H>;
        self.arg = owner as *const H as *const ();
    }

    pub fn notify(&self) {
        (self.on_resume)(self.arg);
    }

    pub fn priority(&self, _pkey: PreemptLockKey<'_>) -> Priority {
        self.priority
    }
}

impl_linked!(wait_queue_link, WaitQueueEntry, WaitQueueTag);

/// Owner of a [`WaitQueueEntry`]. Implemented by types whose entries
/// participate in a wait queue and need to be notified when removed
/// (resumed). Mirrors [`TimerHandler`](crate::kernel::scheduler::TimerHandler)
/// and [`PendingWorkHandler`](crate::kernel::scheduler::PendingWorkHandler).
pub trait WaitQueueEntryHandler: Sized + 'static {
    fn on_resume(this: &'static Self);
}

fn waiter_trampoline<H: WaitQueueEntryHandler>(arg: *const ()) {
    let this: &'static H = unsafe { &*(arg as *const H) };
    H::on_resume(this);
}

#[derive(Clone, Copy)]
pub(crate) struct WaitQueueHandle {
    queue: *const (),
    vtable: &'static WaitQueueVTable,
}

#[allow(dead_code)]
impl WaitQueueHandle {
    pub unsafe fn try_remove(
        &self,
        pkey: PreemptLockKey<'_>,
        suspendable: Pin<&WaitQueueEntry>,
    ) -> Result<(), ()> {
        unsafe { (self.vtable.try_remove)(self.queue, pkey, suspendable.get_ref()) }
    }

    pub unsafe fn try_reinsert(
        &self,
        pkey: PreemptLockKey<'_>,
        suspendable: Pin<&WaitQueueEntry>,
    ) -> Result<(), ()> {
        unsafe { (self.vtable.try_reinsert)(self.queue, pkey, suspendable.get_ref()) }
    }

    pub fn to_raw(&self) -> (*const (), *const WaitQueueVTable) {
        (self.queue, self.vtable)
    }

    pub unsafe fn from_raw(queue: *const (), vtable: *const WaitQueueVTable) -> Self {
        Self {
            queue,
            vtable: unsafe { &*(vtable as *const WaitQueueVTable) },
        }
    }
}

#[allow(dead_code)]
pub(crate) struct WaitQueueVTable {
    pub(crate) try_remove:
        unsafe fn(*const (), PreemptLockKey<'_>, *const WaitQueueEntry) -> Result<(), ()>,
    pub(crate) try_reinsert:
        unsafe fn(*const (), PreemptLockKey<'_>, *const WaitQueueEntry) -> Result<(), ()>,
}

/// Priority-ordered wait list of [`WaitQueueEntry`] without an owning
/// lock. Intended to live inside a [`Protected`](crate::sync::Protected)
/// `T` field; mutual exclusion is provided by the enclosing
/// `Protected::with`. All entries are pinned (they hold intrusive
/// pointers), so all mutating methods take `Pin<&mut Self>`.
pub struct WaitList {
    list: LinkedList<WaitQueueEntry, WaitQueueTag>,
}

impl WaitList {
    pub const fn new() -> WaitList {
        WaitList {
            list: LinkedList::new(),
        }
    }

    pub fn is_empty(self: Pin<&Self>) -> bool {
        let list = unsafe { self.map_unchecked(|s| &s.list) };
        list.is_empty()
    }

    /// Insert `entry` in priority order. Higher-priority entries come
    /// first; ties insert after existing same-priority entries (FIFO
    /// within a priority).
    pub fn push_priority(
        self: Pin<&mut Self>,
        pkey: PreemptLockKey<'_>,
        entry: Pin<&WaitQueueEntry>,
    ) {
        let priority = entry.priority(pkey);
        let list = unsafe { self.map_unchecked_mut(|s| &mut s.list) };
        list.insert_after(entry, |s| s.priority(pkey) >= priority);
    }

    pub fn pop_front<'item>(self: Pin<&mut Self>) -> Option<Pin<&'item WaitQueueEntry>> {
        let list = unsafe { self.map_unchecked_mut(|s| &mut s.list) };
        list.pop_front()
    }

    pub fn remove(self: Pin<&mut Self>, entry: Pin<&WaitQueueEntry>) {
        let list = unsafe { self.map_unchecked_mut(|s| &mut s.list) };
        list.remove(entry);
    }

    /// Remove `entry` (assumed currently in the list) and re-insert at
    /// its current priority. Used by the priority-inheritance reinsert
    /// path when a waiter's priority changes.
    pub fn reinsert(self: Pin<&mut Self>, pkey: PreemptLockKey<'_>, entry: Pin<&WaitQueueEntry>) {
        let priority = entry.priority(pkey);
        let mut list = unsafe { self.map_unchecked_mut(|s| &mut s.list) };
        list.as_mut().remove(entry);
        list.insert_after(entry, |s| s.priority(pkey) >= priority);
    }
}
