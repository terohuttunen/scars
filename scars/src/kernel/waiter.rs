use crate::Priority;
use crate::cell::{LockedPinRefCell, PinRefCell};
use crate::in_interrupt;
use crate::kernel::atomic_queue::{AtomicNode, impl_atomic_linked};
use crate::kernel::interrupt::RawInterruptHandler;
use crate::kernel::list::{LinkedList, LinkedListNode, LinkedListTag, Node, impl_linked};
use crate::kernel::scheduler::{ExecStateTag, ExecutionContext, Scheduler};
use crate::sync::{CeilingLock, NestingLock, PreemptLock};
use crate::syscall;
use crate::task::task::RawTask;
use crate::thread::RawThread;
use crate::time::Instant;
use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{RawWaker, Waker};

pub struct WaitQueueTag {}

impl LinkedListTag for WaitQueueTag {}

pub struct SleepQueueTag {}

impl LinkedListTag for SleepQueueTag {}

pub(crate) const SUSPENDABLE_PENDING_RESUME: u32 = 1;
pub(crate) const SUSPENDABLE_PENDING_WAKEUP: u32 = 2;
pub(crate) const SUSPENDABLE_PENDING_SUSPEND: u32 = 4;

pub enum SuspendableKind {
    None,
    Thread(*const RawThread),
    Interrupt(*const RawInterruptHandler),
    Async(Priority, Waker),
}

/// Suspendable represents a thread, interrupt handler, or async task that can wait in a WaitQueue,
/// or wait for a timeout in the kernel sleep queue.
pub struct Suspendable {
    pub(crate) kind: SuspendableKind,

    /// Time when the Suspendable should be woken up from sleep. This is used to
    /// implement timeouts or delays. TODO: this is safe to modify only when
    /// not part of a list.
    deadline: Cell<Option<Instant>>,

    /// Link for the WaitQueue.
    pub(crate) wait_queue_link: Node<Self, WaitQueueTag>,

    /// Link for the kernel sleep queue.
    pub(crate) sleep_queue_link: Node<Self, SleepQueueTag>,

    /// When PreemptLock cannot be acquired, operations on the Suspendable are postponed.
    /// This link is used to insert the Suspendable into the pending schedule queue.
    pub(crate) pending_schedule_link: AtomicNode<Self, ExecStateTag>,

    /// Mask of pending operations that could not be completed because of PreemptLock.
    pub(crate) pending_mask: AtomicU32,
}

impl Suspendable {
    pub const fn new() -> Suspendable {
        Suspendable {
            kind: SuspendableKind::None,
            deadline: Cell::new(None),
            wait_queue_link: Node::new(),
            sleep_queue_link: Node::new(),
            pending_schedule_link: AtomicNode::new(),
            pending_mask: AtomicU32::new(0),
        }
    }

    pub fn init_thread(self: Pin<&mut Self>, thread_ptr: *const RawThread) {
        let this = unsafe { self.get_unchecked_mut() };
        this.kind = SuspendableKind::Thread(thread_ptr);
    }

    pub const fn new_thread(thread: *const RawThread) -> Suspendable {
        Suspendable {
            kind: SuspendableKind::Thread(thread),
            deadline: Cell::new(None),
            wait_queue_link: Node::new(),
            sleep_queue_link: Node::new(),
            pending_schedule_link: AtomicNode::new(),
            pending_mask: AtomicU32::new(0),
        }
    }

    pub const fn new_interrupt(interrupt: *const RawInterruptHandler) -> Suspendable {
        Suspendable {
            kind: SuspendableKind::Interrupt(interrupt),
            deadline: Cell::new(None),
            wait_queue_link: Node::new(),
            sleep_queue_link: Node::new(),
            pending_schedule_link: AtomicNode::new(),
            pending_mask: AtomicU32::new(0),
        }
    }

    pub const fn new_async(priority: Priority, waker: Waker) -> Suspendable {
        Suspendable {
            kind: SuspendableKind::Async(priority, waker),
            deadline: Cell::new(None),
            wait_queue_link: Node::new(),
            sleep_queue_link: Node::new(),
            pending_schedule_link: AtomicNode::new(),
            pending_mask: AtomicU32::new(0),
        }
    }

    pub fn notify(&self) {
        match &self.kind {
            SuspendableKind::None => (),
            SuspendableKind::Thread(thread) => unsafe { (&**thread).resume() },
            SuspendableKind::Interrupt(_interrupt) => (), //interrupt.notify(),
            SuspendableKind::Async(_, waker) => waker.wake_by_ref(),
        }
    }

    pub fn priority(&self) -> Priority {
        match &self.kind {
            SuspendableKind::None => Priority::Thread(0),
            SuspendableKind::Thread(thread) => unsafe { (&**thread).base_priority },
            SuspendableKind::Interrupt(interrupt) => unsafe { (&**interrupt).base_priority() },
            SuspendableKind::Async(priority, _) => *priority,
        }
    }

    pub fn in_sleep_queue(&self) -> bool {
        self.sleep_queue_link.in_list()
    }

    pub fn has_deadline(&self) -> bool {
        self.deadline.get().is_some()
    }

    pub fn set_deadline(&self, deadline: Option<Instant>) {
        self.deadline.set(deadline);
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.deadline.get()
    }

    pub fn set_pending(&self, mask: u32) {
        self.pending_mask.fetch_or(mask, Ordering::Relaxed);
    }
}

impl_linked!(wait_queue_link, Suspendable, WaitQueueTag);
impl_linked!(sleep_queue_link, Suspendable, SleepQueueTag);
impl_atomic_linked!(pending_schedule_link, Suspendable, ExecStateTag);

#[derive(Clone, Copy)]
pub(crate) struct WaitQueueHandle {
    queue: *const (),
    vtable: &'static WaitQueueVTable,
}

impl WaitQueueHandle {
    pub unsafe fn insert(&self, suspendable: Pin<&Suspendable>) {
        unsafe { (self.vtable.insert)(self.queue, suspendable.get_ref()) }
    }

    pub unsafe fn remove(&self, suspendable: Pin<&Suspendable>) {
        unsafe { (self.vtable.remove)(self.queue, suspendable.get_ref()) }
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

pub(crate) struct WaitQueueVTable {
    insert: unsafe fn(*const (), *const Suspendable),
    remove: unsafe fn(*const (), *const Suspendable),
}

pub struct WaitQueue<L: NestingLock> {
    queue: LockedPinRefCell<LinkedList<Suspendable, WaitQueueTag>, L>,
}

impl<L: NestingLock> WaitQueue<L> {
    const WAIT_QUEUE_VTABLE: &'static WaitQueueVTable = &WaitQueueVTable {
        insert: Self::insert_unsafe,
        remove: Self::remove_unsafe,
    };

    pub const fn new() -> WaitQueue<L> {
        WaitQueue {
            queue: LockedPinRefCell::new(LinkedList::new()),
        }
    }

    fn insert(&self, suspendable: Pin<&Suspendable>) {
        L::with(|key| {
            let priority = suspendable.priority();

            let this = unsafe { Pin::new_unchecked(self) };
            let queue = unsafe { this.map_unchecked(|s| &s.queue) };
            queue
                .borrow_mut(key)
                .as_mut()
                .insert_after(suspendable, |s| s.priority() >= priority);
        })
    }

    fn remove(&self, suspendable: Pin<&Suspendable>) {
        L::with(|key| {
            let this = unsafe { Pin::new_unchecked(self) };
            let queue = unsafe { this.map_unchecked(|s| &s.queue) };
            queue.borrow_mut(key).as_mut().remove(suspendable);
        })
    }

    unsafe fn insert_unsafe(queue: *const (), suspendable: *const Suspendable) {
        let queue = unsafe { &*(queue as *const WaitQueue<L>) };
        let suspendable = unsafe { Pin::new_unchecked(&*(suspendable as *const Suspendable)) };
        queue.insert(suspendable);
    }

    unsafe fn remove_unsafe(queue: *const (), suspendable: *const Suspendable) {
        let queue = unsafe { &*(queue as *const WaitQueue<L>) };
        let suspendable = unsafe { Pin::new_unchecked(&*(suspendable as *const Suspendable)) };
        queue.remove(suspendable);
    }

    pub fn wait(&self) {
        syscall::thread_wait(self);
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
}

pub struct AsyncWaiterQueue {
    queue: PinRefCell<LinkedList<Suspendable, WaitQueueTag>>,
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
