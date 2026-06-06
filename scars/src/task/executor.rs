use super::JoinHandle;
use super::raw_task::{RawTask, RawTaskHandle, TaskHandle, TaskReadyListTag};
use crate::Priority;
use crate::cell::PinRefCell;
use crate::events::Events;
use crate::kernel::atomic_queue::AtomicQueue;
use crate::kernel::list::LinkedList;
use crate::kernel::waiter::WaitQueueTag;
use crate::time::Instant;
use core::pin::Pin;

pub struct PollResult {
    // If the executor wants to get polled again after
    // a specific amount of time, it can set this to a deadline
    pub deadline_opt: Option<Instant>,
}

pub struct RawExecutor {
    ready_queue: PinRefCell<LinkedList<RawTask, TaskReadyListTag>>,
    sleep_queue: PinRefCell<LinkedList<RawTask, WaitQueueTag>>,
    pending_ready_queue: AtomicQueue<RawTask, TaskReadyListTag>,
}

impl RawExecutor {
    pub const fn new() -> RawExecutor {
        RawExecutor {
            ready_queue: PinRefCell::new(LinkedList::new()),
            sleep_queue: PinRefCell::new(LinkedList::new()),
            pending_ready_queue: AtomicQueue::new(),
        }
    }

    pub(crate) fn spawn(&'static self, task_handle: &RawTaskHandle) {
        let task_to_spawn = task_handle.as_ref();
        Pin::static_ref(&self.ready_queue)
            .borrow_mut()
            .as_mut()
            .push_back(task_to_spawn);
    }

    pub(crate) fn poll(&'static self) -> PollResult {
        self.resume_sleeping_tasks();
        self.resume_pending_tasks();
        self.poll_ready_tasks();

        let deadline_opt = Pin::static_ref(&self.sleep_queue)
            .borrow()
            .as_ref()
            .head()
            .map(|task| task.wakeup_time);

        PollResult { deadline_opt }
    }

    fn poll_ready_tasks(&'static self) {
        while let Some(ready_task) = Pin::static_ref(&self.ready_queue)
            .borrow_mut()
            .as_mut()
            .pop_front()
        {
            ready_task.poll();
        }
    }

    // Safe to call from ISR or another thread
    // Note: This method must be called for the task's executor only
    pub(crate) fn resume_task(&'static self, task: Pin<&RawTask>) {
        // If it fails, it already is in the queue. Ignore failures
        // to make resume_task idempotent.
        let _ = self.pending_ready_queue.try_push_back(task);
    }

    pub(crate) fn task_sleep_until(&'static self, mut task: Pin<&mut RawTask>, deadline: Instant) {
        task.as_mut().set_wakeup_time(deadline);
        Pin::static_ref(&self.sleep_queue)
            .borrow_mut()
            .as_mut()
            .insert_after(task.into_ref(), |queue_task| {
                queue_task.wakeup_time <= deadline
            });
    }

    pub fn resume_pending_tasks(&'static self) -> bool {
        let mut task_became_ready: bool = false;
        while let Some(pending_ready_task) = self.pending_ready_queue.pop_front() {
            if !pending_ready_task.is_ready_queued() {
                Pin::static_ref(&self.ready_queue)
                    .borrow_mut()
                    .as_mut()
                    .push_back(pending_ready_task);
            }
            task_became_ready = true;
        }

        task_became_ready
    }

    fn resume_sleeping_tasks(&'static self) {
        // Resume sleeping tasks that should be woken up
        let now = Instant::now();
        let mut sleep_queue = Pin::static_ref(&self.sleep_queue).borrow_mut();
        loop {
            let head_wakeup_time_opt = sleep_queue.as_ref().head().map(|task| task.wakeup_time);
            if let Some(head_wakeup_time) = head_wakeup_time_opt {
                if head_wakeup_time <= now {
                    let task = sleep_queue.as_mut().pop_front().unwrap();
                    if !task.is_ready_queued() {
                        Pin::static_ref(&self.ready_queue)
                            .borrow_mut()
                            .as_mut()
                            .push_back(task);
                    }
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum BlockOnError {
    NotSupported,
}

#[derive(Copy, Clone)]
pub struct ExecutorHandle {
    executor: *const (),
    vtable: &'static ExecutorVTable,
}

impl ExecutorHandle {
    /// Construct a handle for an [`Executor`] implementor. The type
    /// parameter pairs the target pointer and the vtable so they can
    /// never be mismatched at the call site.
    #[inline]
    pub const fn from_executor<E: Executor>(executor: &'static E) -> Self {
        Self {
            executor: executor as *const E as *const (),
            vtable: &E::EXECUTOR_VTABLE,
        }
    }

    pub fn raw(&self) -> *const RawExecutor {
        (self.vtable.raw)(self.executor)
    }

    pub fn notify(&self) {
        (self.vtable.notify)(self.executor);
    }

    pub fn priority(&self) -> Priority {
        (self.vtable.priority)(self.executor)
    }

    pub fn block_on(&self, task_handle: &RawTaskHandle) -> Result<(), BlockOnError> {
        (self.vtable.block_on)(self.executor, task_handle)
    }

    /// Whether this executor implements [`block_on`](Self::block_on)
    /// without going through the call.
    pub fn supports_block_on(&self) -> bool {
        self.vtable.supports_block_on
    }

    pub fn spawn<T>(&self, task_handle: TaskHandle<T>) -> JoinHandle<T> {
        unsafe { RawTask::set_executor(task_handle.as_raw().raw_task_ptr(), *self) };
        let raw = unsafe { &*self.raw() };
        raw.spawn(task_handle.as_raw());
        JoinHandle::new(task_handle)
    }

    /// Send events to tasks waiting on this executor.
    ///
    /// This can be called from interrupt handlers to notify tasks about events.
    /// Executors that don't support events silently ignore the call.
    pub fn send_events(&self, events: Events) {
        (self.vtable.send_events)(self.executor, events);
    }

    /// Whether [`send_events`](Self::send_events) actually delivers,
    /// rather than silently dropping the request.
    pub fn supports_send_events(&self) -> bool {
        self.vtable.supports_send_events
    }

    /// Read this executor's pending-events bitfield without clearing.
    pub fn peek_events(&self) -> Events {
        (self.vtable.peek_events)(self.executor)
    }

    /// Atomically clear and return the intersection of `mask` and the
    /// pending events.
    pub fn consume_events(&self, mask: Events) -> Events {
        (self.vtable.consume_events)(self.executor, mask)
    }

    /// Push `task` onto the executor's pending-ready queue so the next
    /// poll cycle will re-poll it.
    pub fn resume_task(&self, task: Pin<&RawTask>) {
        (self.vtable.resume_task)(self.executor, task);
    }

    /// Whether [`peek_events`](Self::peek_events) /
    /// [`consume_events`](Self::consume_events) report real values
    /// rather than the default zero.
    pub fn supports_events(&self) -> bool {
        self.vtable.supports_events
    }
}

pub struct ExecutorVTable {
    raw: fn(*const ()) -> *const RawExecutor,
    notify: fn(*const ()),
    priority: fn(*const ()) -> Priority,
    block_on: fn(*const (), &RawTaskHandle) -> Result<(), BlockOnError>,
    send_events: fn(*const (), Events),
    peek_events: fn(*const ()) -> Events,
    consume_events: fn(*const (), Events) -> Events,
    resume_task: fn(*const (), Pin<&RawTask>),
    supports_block_on: bool,
    supports_send_events: bool,
    supports_events: bool,
}

/// Executor implementations expose themselves as an [`ExecutorHandle`].
/// The trait default supplies a per-`Self` vtable and a [`handle`](Self::handle)
/// factory; implementors provide only the methods they support.
pub trait Executor: Sized + 'static {
    fn raw(&'static self) -> *const RawExecutor;
    fn notify(&'static self);
    fn priority(&self) -> Priority;

    /// Default rejects with [`BlockOnError::NotSupported`]. Override
    /// alongside [`SUPPORTS_BLOCK_ON`](Self::SUPPORTS_BLOCK_ON) on
    /// executors that drive a task to completion in the calling context.
    fn block_on(&'static self, _task: &RawTaskHandle) -> Result<(), BlockOnError> {
        Err(BlockOnError::NotSupported)
    }

    /// Set to `true` by implementors that override [`block_on`](Self::block_on).
    /// Surfaces through [`ExecutorHandle::supports_block_on`].
    const SUPPORTS_BLOCK_ON: bool = false;

    /// Default no-op. Override alongside
    /// [`SUPPORTS_SEND_EVENTS`](Self::SUPPORTS_SEND_EVENTS) on
    /// executors whose tasks can wait for events.
    fn send_events(&'static self, _events: Events) {}

    /// Set to `true` by implementors that override [`send_events`](Self::send_events).
    /// Surfaces through [`ExecutorHandle::supports_send_events`].
    const SUPPORTS_SEND_EVENTS: bool = false;

    /// Read this executor's pending-events bitfield without clearing.
    /// Default 0; override on executors that own an event source
    /// alongside [`SUPPORTS_EVENTS`](Self::SUPPORTS_EVENTS).
    fn peek_events(&'static self) -> Events {
        0
    }

    /// Atomically clear and return the intersection of `mask` and the
    /// pending events. Default 0.
    fn consume_events(&'static self, _mask: Events) -> Events {
        0
    }

    /// Set to `true` by implementors that override
    /// [`peek_events`](Self::peek_events) /
    /// [`consume_events`](Self::consume_events). Surfaces through
    /// [`ExecutorHandle::supports_events`].
    const SUPPORTS_EVENTS: bool = false;

    /// Push `task` onto this executor's pending-ready queue so the next
    /// poll cycle will re-poll it. Default panics — only executors that
    /// own a queue support this.
    fn resume_task(&'static self, _task: Pin<&RawTask>) {
        panic!("executor does not support resume_task");
    }

    #[doc(hidden)]
    const EXECUTOR_VTABLE: ExecutorVTable = ExecutorVTable {
        raw: vt_raw::<Self>,
        notify: vt_notify::<Self>,
        priority: vt_priority::<Self>,
        block_on: vt_block_on::<Self>,
        send_events: vt_send_events::<Self>,
        peek_events: vt_peek_events::<Self>,
        consume_events: vt_consume_events::<Self>,
        resume_task: vt_resume_task::<Self>,
        supports_block_on: Self::SUPPORTS_BLOCK_ON,
        supports_send_events: Self::SUPPORTS_SEND_EVENTS,
        supports_events: Self::SUPPORTS_EVENTS,
    };

    fn handle(&'static self) -> ExecutorHandle {
        ExecutorHandle::from_executor(self)
    }
}

fn vt_raw<E: Executor>(target: *const ()) -> *const RawExecutor {
    E::raw(unsafe { &*(target as *const E) })
}
fn vt_notify<E: Executor>(target: *const ()) {
    E::notify(unsafe { &*(target as *const E) })
}
fn vt_priority<E: Executor>(target: *const ()) -> Priority {
    E::priority(unsafe { &*(target as *const E) })
}
fn vt_block_on<E: Executor>(target: *const (), task: &RawTaskHandle) -> Result<(), BlockOnError> {
    E::block_on(unsafe { &*(target as *const E) }, task)
}
fn vt_send_events<E: Executor>(target: *const (), events: Events) {
    E::send_events(unsafe { &*(target as *const E) }, events)
}
fn vt_peek_events<E: Executor>(target: *const ()) -> Events {
    E::peek_events(unsafe { &*(target as *const E) })
}
fn vt_consume_events<E: Executor>(target: *const (), mask: Events) -> Events {
    E::consume_events(unsafe { &*(target as *const E) }, mask)
}
fn vt_resume_task<E: Executor>(target: *const (), task: Pin<&RawTask>) {
    E::resume_task(unsafe { &*(target as *const E) }, task)
}
