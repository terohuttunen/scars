//! Core event handler implementation

use super::pending::PendingEvents;
use super::sender::EventReceiver;
use crate::events::Events;
use crate::kernel::atomic_queue::AtomicNode;
use crate::kernel::hal::CoreId;
use crate::kernel::scheduler::Scheduler;
use crate::kernel::scheduler::event_queue::PendingNotifyTag;
use crate::local::LocalStorage;
use crate::priority::Priority;

use crate::sync::atomic::Ordering;
use core::pin::Pin;

/// Core event handler structure
#[repr(align(16))]
#[repr(C)]
pub struct RawEventHandler {
    priority: Priority,

    /// Core this handler is bound to. Set at construction from the
    /// wrapping `EventHandler<PRIO, F, CORE>`.
    pub core: CoreId,

    /// The handler function to call when an event is received
    handler_fn: fn(*mut ()),
    arg_ptr: *mut (),

    /// Node for pending event processing queue
    pub(crate) pending_event_processing_node: AtomicNode<RawEventHandler, PendingNotifyTag>,

    pub(crate) local_storage: LocalStorage,

    pending: PendingEvents,
}

impl RawEventHandler {
    pub const fn new(priority: Priority, core: CoreId) -> RawEventHandler {
        RawEventHandler {
            priority,
            core,
            handler_fn: |_| {},
            arg_ptr: core::ptr::null_mut(),
            pending_event_processing_node: AtomicNode::new(),
            local_storage: LocalStorage::new(),
            pending: PendingEvents::new(),
        }
    }

    pub fn priority(&self) -> Priority {
        self.priority
    }

    pub fn handler_fn(&self) -> fn(*mut ()) {
        self.handler_fn
    }

    pub(crate) fn execute(&self) {
        (self.handler_fn)(self.arg_ptr)
    }

    /// Add events to this interrupt's pending set
    pub fn add_pending_events(&self, events: crate::events::Events) {
        self.pending.add_pending_events(events);
    }

    /// Check if there are pending events
    pub fn has_pending_events(&self) -> bool {
        self.pending.has_pending_events()
    }

    pub fn send_events(&'static self, events: Events) {
        self.pending.add_pending_events(events);
        Scheduler::queue_pending_events(Pin::static_ref(self));
    }

    /// Get the embedded local storage slot for this event handler.
    pub fn local_storage(&self) -> &LocalStorage {
        &self.local_storage
    }

    /// Get a pointer to this interrupt handler
    pub fn as_ptr(&self) -> *const RawEventHandler {
        self as *const RawEventHandler
    }

    /// Set pending events
    ///
    /// Inserts the event handler to kernel pending event handler queue
    pub fn set_pending_event_processing(self: Pin<&'static Self>) {
        Scheduler::queue_pending_events(self);
    }

    /// Peek at pending events without consuming them
    pub fn peek_pending_events(&self) -> Events {
        self.pending.peek_pending_events()
    }

    /// Consume (clear) specific events atomically and return what was consumed
    ///
    /// This atomically clears the specified event bits and returns the events
    /// that were actually pending (intersection of pending and requested).
    pub fn consume_events(&self, events: Events) -> Events {
        // Atomically read current and clear the requested bits
        let prev = self
            .pending
            .pending_events
            .fetch_and(!events, Ordering::SeqCst);
        // Return only the events that were actually pending
        prev & events
    }

    /// Attach event handler
    pub unsafe fn attach<const CORE: CoreId>(
        &mut self,
        handler_fn: fn(*mut ()),
        arg_ptr: *mut (),
        _key: crate::sync::interrupt_lock::CoreInterruptLockKey<'_, CORE>,
    ) {
        self.handler_fn = handler_fn;
        self.arg_ptr = arg_ptr;
    }
}

unsafe impl Sync for RawEventHandler {}
unsafe impl Send for RawEventHandler {}

impl EventReceiver for RawEventHandler {
    fn send_events(&'static self, events: Events) {
        Self::send_events(self, events)
    }
    fn has_pending_events(&self) -> bool {
        Self::has_pending_events(self)
    }
    fn base_priority(&self) -> Priority {
        self.priority()
    }
}

// Support impl for pending event processing
// Implementation block for atomic queue functionality
crate::kernel::atomic_queue::impl_atomic_linked!(
    pending_event_processing_node,
    RawEventHandler,
    PendingNotifyTag
);
