use super::JoinHandle;
use super::executor::{Executor, ExecutorHandle, RawExecutor};
use super::raw_task::{RawTask, TaskHandle};
use crate::Priority;
use crate::events::{EXECUTOR_WAKEUP_EVENT, Events, raw::RawEventHandler, sender::EventReceiver};
use crate::kernel::hal::CoreId;
use crate::kernel::scheduler::EventTimer;
use crate::local::{
    LocalCell, LocalStorage, Publish, PublishCtx, PublishError, SharedStorage,
    SharedStorageProvider,
};
use crate::sync::interrupt_lock::CoreInterruptLock;
use core::pin::Pin;
use core::ptr::NonNull;
use static_cell::StaticCell;

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
    pub const fn new(priority: Priority, core: CoreId) -> Self {
        Self {
            raw: RawExecutor::new(),
            event_handler: RawEventHandler::new(priority, core),
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
    pub unsafe fn attach<const CORE: CoreId>(
        &mut self,
        handler_fn: fn(*mut ()),
        arg_ptr: *mut (),
        key: crate::sync::interrupt_lock::CoreInterruptLockKey<'_, CORE>,
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
pub struct EventHandlerExecutor<const PRIO: Priority, const CORE: CoreId = { CoreId::DEFAULT }> {
    raw: StaticCell<RawEventHandlerExecutor>,
}

impl<const PRIO: Priority, const CORE: CoreId> EventHandlerExecutor<PRIO, CORE> {
    /// Create a new event handler executor
    pub const fn new() -> EventHandlerExecutor<PRIO, CORE> {
        assert!(
            PRIO.is_interrupt(),
            "Event handler executor priority must be an interrupt priority"
        );
        EventHandlerExecutor {
            raw: StaticCell::new(),
        }
    }

    /// Initialize the event handler executor and return a builder
    pub fn init(&'static self) -> EventHandlerExecutorBuilder<PRIO, CORE> {
        let raw = self
            .raw
            .init_with(|| RawEventHandlerExecutor::new(PRIO, CORE));
        EventHandlerExecutorBuilder::new(raw)
    }
}

unsafe impl<const PRIO: Priority, const CORE: CoreId> Sync for EventHandlerExecutor<PRIO, CORE> {}

/// Builder for EventHandlerExecutor configuration
pub struct EventHandlerExecutorBuilder<const PRIO: Priority, const CORE: CoreId = { CoreId::DEFAULT }> {
    raw: &'static mut RawEventHandlerExecutor,
}

impl<const PRIO: Priority, const CORE: CoreId> EventHandlerExecutorBuilder<PRIO, CORE> {
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
    pub fn build(self) -> EventHandlerExecutorHandle<PRIO, CORE> {
        // Attach the internal executor poll handler
        let raw_ptr = self.raw as *const RawEventHandlerExecutor as *mut ();
        CoreInterruptLock::<CORE>::with(|key| unsafe {
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
pub struct EventHandlerExecutorHandle<const PRIO: Priority, const CORE: CoreId = { CoreId::DEFAULT }> {
    raw: NonNull<RawEventHandlerExecutor>,
}

impl<const PRIO: Priority, const CORE: CoreId> EventHandlerExecutorHandle<PRIO, CORE> {
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

impl<const PRIO: Priority, const CORE: CoreId> SharedStorageProvider<PRIO>
    for EventHandlerExecutorHandle<PRIO, CORE>
{
    fn shared_storage(&self) -> SharedStorage<PRIO> {
        // SAFETY: executor runs at PRIO on CORE; sharers run at the
        // same priority on the same core.
        unsafe { SharedStorage::from_head(self.raw().local_storage().head()) }
    }
}
