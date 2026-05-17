use crate::Priority;
use crate::cell::{LockedPinRefCell, PinRefCell};
use crate::events::raw::RawEventHandler;
use crate::in_interrupt;
use crate::kernel::atomic_queue::{AtomicNode, impl_atomic_linked};
use crate::kernel::list::{LinkedList, LinkedListNode, LinkedListTag, Node, impl_linked};
use crate::kernel::scheduler::{ExecStateTag, ExecutionContext, Scheduler};
use crate::sync::atomic::{AtomicU32, Ordering};
use crate::sync::{
    CeilingLock, CorePreemptLock, NestingLock, PreemptLock, PreemptLockKey,
    preempt_lock::CorePreemptLockKey,
};
use crate::syscall;
use crate::task::raw_task::RawTask;
use crate::thread::RawThread;
use crate::time::Instant;
use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::pin::{Pin, pin};
use core::task::{RawWaker, Waker};

pub struct WaitQueueTag {}

impl LinkedListTag for WaitQueueTag {}

pub(crate) const SUSPENDABLE_PENDING_RESUME: u32 = 1;
pub(crate) const SUSPENDABLE_PENDING_WAKEUP: u32 = 2;
pub(crate) const SUSPENDABLE_PENDING_SUSPEND: u32 = 4;
pub(crate) const SUSPENDABLE_PENDING_RECONFIGURE: u32 = 8;

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
    pub unsafe fn insert(&self, pkey: PreemptLockKey<'_>, suspendable: Pin<&WaitQueueEntry>) {
        unsafe { (self.vtable.insert)(self.queue, pkey, suspendable.get_ref()) }
    }

    pub unsafe fn remove(&self, pkey: PreemptLockKey<'_>, suspendable: Pin<&WaitQueueEntry>) {
        unsafe { (self.vtable.remove)(self.queue, pkey, suspendable.get_ref()) }
    }

    pub unsafe fn reinsert(&self, pkey: PreemptLockKey<'_>, suspendable: Pin<&WaitQueueEntry>) {
        unsafe { (self.vtable.reinsert)(self.queue, pkey, suspendable.get_ref()) }
    }

    pub fn required_ceiling(&self) -> Option<Priority> {
        (self.vtable.required_ceiling)(self.queue)
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
    insert: unsafe fn(*const (), PreemptLockKey<'_>, *const WaitQueueEntry),
    remove: unsafe fn(*const (), PreemptLockKey<'_>, *const WaitQueueEntry),
    reinsert: unsafe fn(*const (), PreemptLockKey<'_>, *const WaitQueueEntry), // reinsert after priority change
    required_ceiling: fn(*const ()) -> Option<Priority>,
}

pub struct WaitQueue<L: NestingLock> {
    queue: LockedPinRefCell<LinkedList<WaitQueueEntry, WaitQueueTag>, L>,
}

impl<L: NestingLock> WaitQueue<L> {
    const WAIT_QUEUE_VTABLE: &'static WaitQueueVTable = &WaitQueueVTable {
        insert: Self::insert_unsafe,
        remove: Self::remove_unsafe,
        reinsert: Self::reinsert_unsafe,
        required_ceiling: Self::required_ceiling,
    };

    pub const fn new() -> WaitQueue<L> {
        WaitQueue {
            queue: LockedPinRefCell::new(LinkedList::new()),
        }
    }

    fn insert(self: Pin<&Self>, pkey: PreemptLockKey<'_>, suspendable: Pin<&WaitQueueEntry>) {
        L::try_with(|key| {
            let key = L::upcast_key(key);
            let priority = suspendable.priority(pkey);

            let queue = unsafe { self.map_unchecked(|s| &s.queue) };
            queue
                .borrow_mut(key)
                .as_mut()
                .insert_after(suspendable, |s| s.priority(pkey) >= priority);
        })
        .unwrap_or_else(|_| unreachable!());
    }

    fn remove(self: Pin<&Self>, _pkey: PreemptLockKey<'_>, suspendable: Pin<&WaitQueueEntry>) {
        L::try_with(|key| {
            let key = L::upcast_key(key);
            let queue = unsafe { self.map_unchecked(|s| &s.queue) };
            queue.borrow_mut(key).as_mut().remove(suspendable);
        })
        .unwrap_or_else(|_| unreachable!());
    }

    fn reinsert(self: Pin<&Self>, pkey: PreemptLockKey<'_>, suspendable: Pin<&WaitQueueEntry>) {
        L::try_with(|key| {
            let key = L::upcast_key(key);
            let priority = suspendable.priority(pkey);
            let queue = unsafe { self.map_unchecked(|s| &s.queue) };
            queue.borrow_mut(key).as_mut().remove(suspendable);
            queue
                .borrow_mut(key)
                .as_mut()
                .insert_after(suspendable, |s| s.priority(pkey) >= priority);
        })
        .unwrap_or_else(|_| unreachable!());
    }

    unsafe fn insert_unsafe(
        queue: *const (),
        pkey: PreemptLockKey<'_>,
        suspendable: *const WaitQueueEntry,
    ) {
        let queue = unsafe { Pin::new_unchecked(&*(queue as *const WaitQueue<L>)) };
        let suspendable = unsafe { Pin::new_unchecked(&*(suspendable as *const WaitQueueEntry)) };
        queue.insert(pkey, suspendable);
    }

    unsafe fn remove_unsafe(
        queue: *const (),
        pkey: PreemptLockKey<'_>,
        suspendable: *const WaitQueueEntry,
    ) {
        let queue = unsafe { Pin::new_unchecked(&*(queue as *const WaitQueue<L>)) };
        let suspendable = unsafe { Pin::new_unchecked(&*(suspendable as *const WaitQueueEntry)) };
        if suspendable.wait_queue_link.in_list() {
            queue.remove(pkey, suspendable);
        }
    }

    unsafe fn reinsert_unsafe(
        queue: *const (),
        pkey: PreemptLockKey<'_>,
        suspendable: *const WaitQueueEntry,
    ) {
        let queue = unsafe { Pin::new_unchecked(&*(queue as *const WaitQueue<L>)) };
        let suspendable = unsafe { Pin::new_unchecked(&*(suspendable as *const WaitQueueEntry)) };
        queue.reinsert(pkey, suspendable);
    }

    fn required_ceiling(_queue: *const ()) -> Option<Priority> {
        L::required_ceiling().map(|c| Priority::from_any(c))
    }

    pub fn wait(&self) {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(thread) => {
                L::with(|key| {
                    let key = L::upcast_key(key);
                    let mut queue = unsafe { Pin::new_unchecked(&self.queue) }.borrow_mut(key);

                    queue.as_mut().push_back(thread.get_wait_entry());
                    let (queue_ptr, vtable) = self.to_raw();
                    let handle = unsafe { WaitQueueHandle::from_raw(queue_ptr, vtable) };

                    PreemptLock::with(|pkey| {
                        thread.wait_queue.set(pkey, Some(handle));
                    });
                });

                // If the thread is resumed before the wait-syscall is called, it
                // will return immediately and not block.
                syscall::thread_wait(self);
            }
            ExecutionContext::Interrupt(_) => {
                crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
            }
        }
    }

    pub(crate) fn to_raw(&self) -> (*const (), *const WaitQueueVTable) {
        (
            self as *const _ as *const (),
            Self::WAIT_QUEUE_VTABLE as *const WaitQueueVTable,
        )
    }

    pub async fn async_wait(&'static self) {
        let mut waiter_queued: bool = false;
        poll_fn(|cx| {
            L::with(|key| {
                let mut queue = Pin::static_ref(&self.queue).borrow_mut(key);
                let task = unsafe { &*(cx.waker().data() as *const RawTask) };

                if !waiter_queued {
                    queue
                        .as_mut()
                        .push_back(unsafe { Pin::new_unchecked(&task.waiter) });
                    waiter_queued = true;
                    core::task::Poll::Pending
                } else {
                    if !task.waiter.wait_queue_link.in_list() {
                        // Waiter has been queued, but it is no longer in the wait queue.
                        // This means that the task has been woken up.
                        core::task::Poll::Ready(())
                    } else {
                        core::task::Poll::Pending
                    }
                }
            })
        })
        .await
    }

    pub fn notify_one(&self) {
        L::with(|key| {
            let key = L::upcast_key(key);
            let mut queue = unsafe { Pin::new_unchecked(&self.queue) }.borrow_mut(key);

            if let Some(waiter) = queue.as_mut().pop_front() {
                waiter.notify()
            }
        })
    }

    pub fn notify_all(&self) {
        L::with(|key| {
            let key = L::upcast_key(key);
            let mut queue = unsafe { Pin::new_unchecked(&self.queue) }.borrow_mut(key);

            while let Some(waiter) = queue.as_mut().pop_front() {
                waiter.notify();
            }
        });
    }

    pub fn priority<'key>(&'key self, key: L::Key<'key>) -> Option<Priority> {
        let queue = unsafe { Pin::new_unchecked(&self.queue) };

        PreemptLock::with(|pkey| queue.borrow(key).as_ref().head().map(|s| s.priority(pkey)))
    }
}

pub struct AsyncWaiterQueue {
    queue: PinRefCell<LinkedList<WaitQueueEntry, WaitQueueTag>>,
}

impl AsyncWaiterQueue {
    pub const fn new() -> AsyncWaiterQueue {
        AsyncWaiterQueue {
            queue: PinRefCell::new(LinkedList::new()),
        }
    }

    pub async fn wait(&'static self) {
        let mut waiter_queued: bool = false;
        poll_fn(|cx| {
            let task = unsafe { &*(cx.waker().data() as *const RawTask) };

            if !waiter_queued {
                let mut queue = unsafe { Pin::new_unchecked(&self.queue) }.borrow_mut();

                queue
                    .as_mut()
                    .push_back(unsafe { Pin::new_unchecked(&task.waiter) });

                waiter_queued = true;
                core::task::Poll::Pending
            } else {
                if !task.waiter.wait_queue_link.in_list() {
                    // Waiter has been queued, but it is no longer in the wait queue.
                    // This means that the task has been woken up.
                    core::task::Poll::Ready(())
                } else {
                    core::task::Poll::Pending
                }
            }
        })
        .await
    }

    pub fn notify_one(&self) {
        if in_interrupt() {
            // Error: cannot notify async waiter queue from interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        let mut queue = unsafe { Pin::new_unchecked(&self.queue) }.borrow_mut();

        if let Some(waiter) = queue.as_mut().pop_front() {
            waiter.notify()
        }
    }

    pub fn notify_all(&self) {
        if in_interrupt() {
            // Error: cannot notify async waiter queue from interrupt handler
            crate::runtime_error!(RuntimeError::InterruptHandlerViolation);
        }

        let mut queue = unsafe { Pin::new_unchecked(&self.queue) }.borrow_mut();

        while let Some(waiter) = queue.as_mut().pop_front() {
            waiter.notify();
        }
    }
}
