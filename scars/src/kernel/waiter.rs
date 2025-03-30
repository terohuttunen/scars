use crate::Priority;
use crate::cell::{LockedPinRefCell, PinRefCell};
use crate::in_interrupt;
use crate::kernel::atomic_queue::{AtomicNode, impl_atomic_linked};
use crate::kernel::interrupt::RawInterruptHandler;
use crate::kernel::list::{LinkedList, LinkedListNode, LinkedListTag, Node, impl_linked};
use crate::kernel::scheduler::{ExecStateTag, ExecutionContext, Scheduler};
use crate::sync::CeilingLock;
use crate::syscall;
use crate::task::task::RawTask;
use crate::thread::RawThread;
use crate::time::Instant;
use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::sync::atomic::AtomicPtr;
use core::task::{RawWaker, Waker};

pub struct WaitQueueTag {}

impl LinkedListTag for WaitQueueTag {}

pub struct SleepQueueTag {}

impl LinkedListTag for SleepQueueTag {}

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

    /// When PreemptLock cannot be acquired, the Suspendable cannot be inserted into Scheduler queues,
    /// and it is instead added to the pending schedule queue to wait for scheduling when the lock is
    /// released.
    pub(crate) pending_schedule_link: AtomicNode<Self, ExecStateTag>,
}

impl Suspendable {
    pub const fn new() -> Suspendable {
        Suspendable {
            kind: SuspendableKind::None,
            deadline: Cell::new(None),
            wait_queue_link: Node::new(),
            sleep_queue_link: Node::new(),
            pending_schedule_link: AtomicNode::new(),
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
        }
    }

    pub const fn new_interrupt(interrupt: *const RawInterruptHandler) -> Suspendable {
        Suspendable {
            kind: SuspendableKind::Interrupt(interrupt),
            deadline: Cell::new(None),
            wait_queue_link: Node::new(),
            sleep_queue_link: Node::new(),
            pending_schedule_link: AtomicNode::new(),
        }
    }

    pub const fn new_async(priority: Priority, waker: Waker) -> Suspendable {
        Suspendable {
            kind: SuspendableKind::Async(priority, waker),
            deadline: Cell::new(None),
            wait_queue_link: Node::new(),
            sleep_queue_link: Node::new(),
            pending_schedule_link: AtomicNode::new(),
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
}

impl_linked!(wait_queue_link, Suspendable, WaitQueueTag);
impl_linked!(sleep_queue_link, Suspendable, SleepQueueTag);
impl_atomic_linked!(pending_schedule_link, Suspendable, ExecStateTag);

pub struct WaitQueue<const CEILING: Priority> {
    queue: LockedPinRefCell<LinkedList<Suspendable, WaitQueueTag>, CeilingLock<CEILING>>,
}

impl<const CEILING: Priority> WaitQueue<CEILING> {
    pub const fn new() -> WaitQueue<CEILING> {
        WaitQueue {
            queue: LockedPinRefCell::new(LinkedList::new()),
        }
    }

    pub fn wait(&self) {
        syscall::thread_wait(self.queue.as_ptr(), CEILING.into_any());
    }

    pub async fn async_wait(&'static self) {
        let mut waiter_queued: bool = false;
        poll_fn(|cx| {
            CeilingLock::with(|ckey| {
                let mut queue = Pin::static_ref(&self.queue).borrow_mut(ckey);
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

    #[inline(never)]
    pub fn notify_one(&self) {
        CeilingLock::with(|ckey| {
            let mut queue = unsafe { Pin::new_unchecked(&self.queue) }.borrow_mut(ckey);

            if let Some(waiter) = queue.as_mut().pop_front() {
                waiter.notify()
            }
        })
    }

    pub fn notify_all(&self) {
        CeilingLock::with(|ckey| {
            let mut queue = unsafe { Pin::new_unchecked(&self.queue) }.borrow_mut(ckey);

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
