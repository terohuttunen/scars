use super::JoinHandle;
use super::raw_task::RawTaskHandle;
pub use super::raw_task::{RawTask, Task, TaskHandle, TaskReadyListTag};
pub use super::task_pool::TaskPool;
use crate::Priority;
use crate::cell::PinRefCell;
use crate::events::{
    EXECUTOR_WAKEUP_EVENT, EventOptions, Events, WaitEvents, raw::RawEventHandler,
    sender::EventReceiver,
};
use crate::kernel::atomic_queue::AtomicQueue;
use crate::kernel::list::LinkedList;
use crate::kernel::scheduler::EventTimer;
use crate::kernel::waiter::WaitQueueTag;
use crate::sync::interrupt_lock::InterruptLock;
use crate::thread::ThreadRef;
use crate::time::Instant;
use crate::tls::{
    LocalCell, LocalStorage, Publish, PublishCtx, PublishError, SharedStorage,
    SharedStorageProvider,
};
// Note: `LocalStorage` (the type) and `LocalStorage` (the static-dispatch
// namespace) are the same Rust item — methods like `LocalStorage::get<T>()`
// dispatch to the current execution context.
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::ptr::NonNull;
use core::task::Poll;
use static_cell::StaticCell;

pub struct LocalExecutor;

impl LocalExecutor {
    pub fn is_available() -> bool {
        LocalStorage::contains::<ExecutorHandle>()
    }

    pub fn get() -> &'static ExecutorHandle {
        let p = LocalStorage::as_ptr::<ExecutorHandle>().unwrap();
        // SAFETY: `ExecutorHandle` is a small `Copy` value that the
        // codebase only ever reads — there are no `with_mut::<ExecutorHandle>`
        // / `set::<ExecutorHandle>` paths. Returning `&'static` here is the
        // long-standing contract of `LocalExecutor::get()`.
        unsafe { &*p }
    }

    pub fn task_sleep_until(task: Pin<&mut RawTask>, deadline: Instant) {
        LocalStorage::with::<ExecutorHandle, _>(|executor| {
            let raw = unsafe { &*executor.raw() };
            raw.task_sleep_until(task, deadline);
        })
        .unwrap();
    }

    pub fn spawn<T>(task_handle: TaskHandle<T>) -> JoinHandle<T> {
        LocalStorage::with::<ExecutorHandle, _>(|executor| {
            let raw = unsafe { &*executor.raw() };
            raw.spawn(task_handle.as_raw());
        })
        .unwrap();
        JoinHandle::new(task_handle)
    }

    pub fn priority() -> Priority {
        LocalStorage::with::<ExecutorHandle, _>(|e| e.priority()).unwrap()
    }

    pub fn resume_task(&'static self, task: Pin<&RawTask>) {
        LocalStorage::with::<ExecutorHandle, _>(|executor| {
            let raw = unsafe { &*executor.raw() };
            raw.resume_task(task);
        })
        .unwrap();
    }

    pub fn resume_pending_tasks(&self) {
        LocalStorage::with::<ExecutorHandle, _>(|executor| {
            let raw = unsafe { &*executor.raw() };
            raw.resume_pending_tasks();
        })
        .unwrap();
    }

    pub fn block_on<F: Future>(future: F) -> F::Output {
        // The handle is needed across `init`, `block_on`, and the output
        // read below — too interleaved for a single closure. Use the
        // raw-pointer escape; same justification as `LocalExecutor::get()`.
        let executor: &'static ExecutorHandle =
            unsafe { &*LocalStorage::as_ptr::<ExecutorHandle>().unwrap() };
        let pinned_task = core::pin::pin!(Task::new());
        let task_handle = pinned_task.init(future, executor);

        if let Err(BlockOnError::NotSupported) = executor.block_on(task_handle.as_raw()) {
            panic!("block_on() is not supported in this context")
        }

        let mut output = Poll::Pending;
        task_handle.try_read_output(&mut output, core::task::Waker::noop());
        match output {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("Task was not ready after block_on"),
        }
    }
}

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

    fn spawn(&'static self, task_handle: &RawTaskHandle) {
        let task_to_spawn = task_handle.as_ref();
        Pin::static_ref(&self.ready_queue)
            .borrow_mut()
            .as_mut()
            .push_back(task_to_spawn);
    }

    fn poll(&'static self) -> PollResult {
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
        // Add task to the pending ready queue of the interrupt executor
        self.pending_ready_queue.push_back(task);
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
            Pin::static_ref(&self.ready_queue)
                .borrow_mut()
                .as_mut()
                .push_back(pending_ready_task);
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
                    Pin::static_ref(&self.ready_queue)
                        .borrow_mut()
                        .as_mut()
                        .push_back(task);
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

pub struct RawEventHandlerExecutor {
    raw: RawExecutor,
    event_handler: RawEventHandler,
    event_timer: EventTimer,
    wakeup_event: Events,
    /// Cell used by [`Publish`] to install this executor's handle into
    /// the active local-storage namespace.
    handle_cell: LocalCell<ExecutorHandle>,
}

impl RawEventHandlerExecutor {
    pub const fn new(priority: Priority) -> Self {
        Self {
            raw: RawExecutor::new(),
            event_handler: RawEventHandler::new(priority),
            event_timer: EventTimer::new(),
            wakeup_event: (1 << 31),
            handle_cell: LocalCell::new(),
        }
    }

    pub fn spawn<T>(&'static self, task_handle: TaskHandle<T>) -> JoinHandle<T> {
        unsafe { RawTask::set_executor(task_handle.as_raw().raw_task_ptr(), self.handle()) };
        self.raw.spawn(task_handle.as_raw());
        JoinHandle::new(task_handle)
    }

    pub fn priority(&self) -> Priority {
        self.event_handler.priority()
    }

    /// Local storage slot for this executor (delegates to event handler).
    pub fn local_storage(&self) -> &LocalStorage {
        self.event_handler.local_storage()
    }

    /// Attach executor handler
    pub unsafe fn attach(
        &mut self,
        handler_fn: fn(*mut ()),
        arg_ptr: *mut (),
        key: crate::sync::interrupt_lock::InterruptLockKey<'_>,
    ) {
        unsafe { self.event_handler.attach(handler_fn, arg_ptr, key) };
    }

    /// Send events to tasks waiting on this executor.
    ///
    /// This can be called from interrupt handlers to notify tasks about events.
    pub fn send_events(&'static self, events: Events) {
        self.event_handler.send_events(events);
    }

    /// Get a reference to the event handler for advanced event operations.
    pub fn event_handler(&'static self) -> &'static RawEventHandler {
        &self.event_handler
    }

    /// Internal handler that polls the executor when events are received
    fn executor_poll_handler(raw_ptr: *mut ()) {
        let raw = unsafe { &*(raw_ptr as *const RawEventHandlerExecutor) };
        let poll_result = raw.raw.poll();

        if let Some(deadline) = poll_result.deadline_opt {
            // Arm the kernel timer to deliver EXECUTOR_WAKEUP_EVENT to our
            // event handler when the soonest task deadline is reached.
            let timer: Pin<&'static EventTimer> = unsafe { Pin::new_unchecked(&raw.event_timer) };
            let sender = raw.event_handler.sender();
            timer.arm(deadline, sender, EXECUTOR_WAKEUP_EVENT);
        }
    }
}

impl Executor for RawEventHandlerExecutor {
    const SUPPORTS_SEND_EVENTS: bool = true;
    const SUPPORTS_EVENTS: bool = true;

    fn raw(&'static self) -> *const RawExecutor {
        &self.raw
    }
    fn notify(&'static self) {
        self.event_handler.send_events(self.wakeup_event);
    }
    fn priority(&self) -> Priority {
        self.event_handler.priority()
    }
    fn send_events(&'static self, events: Events) {
        self.event_handler.send_events(events);
    }
    fn peek_events(&'static self) -> Events {
        self.event_handler.peek_pending_events()
    }
    fn consume_events(&'static self, mask: Events) -> Events {
        self.event_handler.consume_events(mask)
    }
    fn resume_task(&'static self, task: Pin<&RawTask>) {
        self.raw.resume_task(task);
    }
    // block_on inherits the default `Err(NotSupported)`.
}

impl Publish for RawEventHandlerExecutor {
    fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>) -> Result<(), PublishError> {
        ctx.put_init(&self.handle_cell, |_| self.handle())?;
        Ok(())
    }
}

/// Event handler executor container
///
/// An executor that runs at interrupt priority and can be triggered by events.
/// The executor implements its own handler internally to poll async tasks.
pub struct EventHandlerExecutor<const PRIO: Priority> {
    raw: StaticCell<RawEventHandlerExecutor>,
}

impl<const PRIO: Priority> EventHandlerExecutor<PRIO> {
    /// Create a new event handler executor
    pub const fn new() -> EventHandlerExecutor<PRIO> {
        assert!(
            PRIO.is_interrupt(),
            "Event handler executor priority must be an interrupt priority"
        );
        EventHandlerExecutor {
            raw: StaticCell::new(),
        }
    }

    /// Initialize the event handler executor and return a builder
    pub fn init(&'static self) -> EventHandlerExecutorBuilder<PRIO> {
        let raw = self.raw.init_with(|| RawEventHandlerExecutor::new(PRIO));
        EventHandlerExecutorBuilder::new(raw)
    }
}

unsafe impl<const PRIO: Priority> Sync for EventHandlerExecutor<PRIO> {}

/// Builder for EventHandlerExecutor configuration
pub struct EventHandlerExecutorBuilder<const PRIO: Priority> {
    raw: &'static mut RawEventHandlerExecutor,
}

impl<const PRIO: Priority> EventHandlerExecutorBuilder<PRIO> {
    pub(crate) fn new(raw: &'static mut RawEventHandlerExecutor) -> Self {
        Self { raw }
    }

    /// Publish the executor's handle into the active local-storage
    /// namespace, making it discoverable by same-priority handlers via
    /// [`LocalExecutor::get`].
    ///
    /// Should be called after [`with_shared_storage`](Self::with_shared_storage)
    /// when the executor joins an existing group; calling it before
    /// publishes into the executor's own storage, which is then orphaned
    /// once the storage is redirected.
    pub fn publish(self) -> Self {
        // SAFETY: self.raw is a valid &'static mut from StaticCell::init_with;
        // the &'static borrow does not alias the &'static mut because we
        // only use it to compute and install a copy of the handle.
        let raw_static: &'static RawEventHandlerExecutor =
            unsafe { &*(self.raw as *const RawEventHandlerExecutor) };
        raw_static.local_storage().head().publish(raw_static);
        self
    }

    pub fn set_shared_storage<S: SharedStorageProvider<PRIO>>(self, provider: &S) {
        let head = provider.shared_storage().head();
        self.raw.event_handler.local_storage.share_with(head);
    }

    pub fn with_shared_storage<S: SharedStorageProvider<PRIO>>(self, provider: &S) -> Self {
        let head = provider.shared_storage().head();
        self.raw.event_handler.local_storage.share_with(head);
        self
    }

    /// Finalize the builder and return a handle to the executor
    pub fn build(self) -> EventHandlerExecutorHandle<PRIO> {
        // Attach the internal executor poll handler
        let raw_ptr = self.raw as *const RawEventHandlerExecutor as *mut ();
        InterruptLock::with(|key| unsafe {
            self.raw
                .attach(RawEventHandlerExecutor::executor_poll_handler, raw_ptr, key)
        });

        // SAFETY: self.raw is a valid &'static mut from StaticCell::init_with
        EventHandlerExecutorHandle {
            raw: NonNull::from(self.raw),
        }
    }

    /// Get access to the raw executor for advanced configuration
    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawEventHandlerExecutor) -> R) -> R {
        f(self.raw)
    }

    /// Get the priority
    pub fn priority(&self) -> Priority {
        self.raw.priority()
    }

    /// Local storage slot for this executor.
    pub fn local_storage(&self) -> &LocalStorage {
        self.raw.local_storage()
    }
}

/// Initialized event handler executor handle
///
/// Uniquely owned reference to an event handler executor
pub struct EventHandlerExecutorHandle<const PRIO: Priority> {
    raw: NonNull<RawEventHandlerExecutor>,
}

impl<const PRIO: Priority> EventHandlerExecutorHandle<PRIO> {
    /// Get a static reference to the raw executor
    ///
    /// # Safety
    /// The NonNull pointer is guaranteed to be valid for 'static lifetime
    /// as it was created from a StaticCell.
    fn raw(&self) -> &'static RawEventHandlerExecutor {
        // SAFETY: self.raw points to data in a StaticCell with 'static lifetime
        unsafe { self.raw.as_ref() }
    }

    /// Get a mutable reference to the raw executor
    ///
    /// # Safety
    /// The NonNull pointer is guaranteed to be valid for 'static lifetime
    /// as it was created from a StaticCell.
    fn raw_mut(&mut self) -> &'static mut RawEventHandlerExecutor {
        // SAFETY: self.raw points to data in a StaticCell with 'static lifetime
        // and we have &mut self ensuring exclusive access
        unsafe { self.raw.as_mut() }
    }

    /// Spawn an async task on this executor
    pub fn spawn<T>(&self, task_handle: TaskHandle<T>) -> JoinHandle<T> {
        self.raw().spawn(task_handle)
    }

    /// Get the executor handle for use with LocalStorage
    pub fn handle(&self) -> ExecutorHandle {
        self.raw().handle()
    }

    /// Modify the raw executor
    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawEventHandlerExecutor) -> R) -> R {
        f(self.raw_mut())
    }

    /// Local storage slot for this executor.
    pub fn local_storage(&self) -> &'static LocalStorage {
        self.raw().local_storage()
    }

    /// Get the priority
    pub fn priority(&self) -> Priority {
        self.raw().priority()
    }

    /// Send events to tasks waiting on this executor
    ///
    /// This can be called from interrupt handlers to notify tasks about events.
    /// Tasks can wait for these events using `WaitForExecutorEvents`.
    pub fn send_events(&self, events: Events) {
        self.raw().send_events(events);
    }

    /// Get a reference to the event handler for advanced event operations
    pub fn event_handler(&self) -> &'static RawEventHandler {
        self.raw().event_handler()
    }
}

impl<const PRIO: Priority> SharedStorageProvider<PRIO> for EventHandlerExecutorHandle<PRIO> {
    fn shared_storage(&self) -> SharedStorage<PRIO> {
        // SAFETY: executor runs at PRIO; sharers run at the same priority.
        unsafe { SharedStorage::from_head(self.raw().local_storage().head()) }
    }
}

pub struct RawThreadExecutor {
    raw: RawExecutor,
    thread: ThreadRef,
    /// Cell used by [`Publish`] to install this executor's handle into
    /// a thread's local-storage namespace.
    handle_cell: LocalCell<ExecutorHandle>,
}

#[allow(dead_code)]
impl RawThreadExecutor {
    pub const fn new(thread: ThreadRef) -> Self {
        Self {
            raw: RawExecutor::new(),
            thread,
            handle_cell: LocalCell::new(),
        }
    }

    fn spawn(&'static self, task_handle: &RawTaskHandle) {
        unsafe { RawTask::set_executor(task_handle.raw_task_ptr(), self.handle()) };
        self.raw.spawn(task_handle);
    }

    fn block_on(&'static self, task_handle: &RawTaskHandle) {
        unsafe { RawTask::set_executor(task_handle.raw_task_ptr(), self.handle()) };
        self.raw.spawn(task_handle);

        loop {
            let poll_result = self.raw.poll();

            if task_handle.poll() {
                break;
            }

            let mut context =
                WaitEvents::with_options(EXECUTOR_WAKEUP_EVENT, EventOptions::wait_any());

            if let Some(deadline) = poll_result.deadline_opt {
                let _ = context.wait_until(deadline);
            } else {
                let _ = context.wait();
            }
        }
    }

    // Safe to call from ISR or another thread
    fn resume_task(&'static self, task: Pin<&RawTask>) {
        self.raw.resume_task(task);
        self.thread.send_events(EXECUTOR_WAKEUP_EVENT);
    }

    fn task_sleep_until(&'static self, task: Pin<&mut RawTask>, deadline: Instant) {
        self.raw.task_sleep_until(task, deadline);
    }

    fn resume_pending_tasks(&'static self, notify_executor: bool) {
        let task_became_ready = self.raw.resume_pending_tasks();
        if task_became_ready & notify_executor {
            self.thread.send_events(EXECUTOR_WAKEUP_EVENT);
        }
    }

    pub fn priority(&self) -> Priority {
        self.thread.base_priority()
    }

    pub fn as_raw(&'static self) -> &'static RawExecutor {
        &self.raw
    }
}

impl Executor for RawThreadExecutor {
    const SUPPORTS_BLOCK_ON: bool = true;

    fn raw(&'static self) -> *const RawExecutor {
        &self.raw
    }
    fn notify(&'static self) {
        self.thread.send_events(EXECUTOR_WAKEUP_EVENT);
    }
    fn priority(&self) -> Priority {
        self.thread.base_priority()
    }
    fn block_on(&'static self, task: &RawTaskHandle) -> Result<(), BlockOnError> {
        Self::block_on(self, task);
        Ok(())
    }
    fn resume_task(&'static self, task: Pin<&RawTask>) {
        Self::resume_task(self, task);
    }
    // send_events / peek_events / consume_events inherit defaults.
}

impl Publish for ThreadExecutor {
    fn try_publish_to(&'static self, ctx: &mut PublishCtx<'_>) -> Result<(), PublishError> {
        ctx.put_init(&self.raw.handle_cell, |_| self.raw.handle())?;
        Ok(())
    }
}

pub struct ThreadExecutor {
    raw: RawThreadExecutor,
    // To make sure that ThreadExecutor is not Send or Sync
    _phantom: PhantomData<*const ()>,
}

#[allow(dead_code)]
impl ThreadExecutor {
    pub fn new() -> ThreadExecutor {
        let thread = unsafe { ThreadRef::current() };
        ThreadExecutor {
            raw: RawThreadExecutor::new(thread),
            _phantom: PhantomData,
        }
    }

    pub fn spawn<T>(&'static self, task_handle: TaskHandle<T>) -> JoinHandle<T> {
        self.raw.spawn(task_handle.as_raw());
        JoinHandle::new(task_handle)
    }

    pub fn block_on<T>(&'static self, task_handle: TaskHandle<T>) -> T {
        let mut output = Poll::Pending;
        self.raw.block_on(task_handle.as_raw());
        task_handle.try_read_output(&mut output, core::task::Waker::noop());
        match output {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("Task was not ready after block_on"),
        }
    }

    fn priority(&self) -> Priority {
        self.raw.thread.base_priority()
    }

    pub(crate) fn resume_task(&'static self, task: Pin<&RawTask>) {
        self.raw.resume_task(task);
    }

    pub(crate) fn task_sleep_until(&'static self, task: Pin<&mut RawTask>, deadline: Instant) {
        self.raw.task_sleep_until(task, deadline);
    }

    fn resume_pending_tasks(&'static self) {
        self.raw.resume_pending_tasks(true)
    }

    #[allow(dead_code)]
    fn as_raw(&'static self) -> &'static RawThreadExecutor {
        &self.raw
    }

    pub fn handle(&'static self) -> ExecutorHandle {
        self.raw.handle()
    }
}
