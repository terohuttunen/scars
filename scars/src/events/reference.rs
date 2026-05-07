use super::raw::RawEventHandler;
use crate::events::Events;
use crate::kernel::scheduler::Scheduler;
use crate::priority::Priority;
use core::ptr::NonNull;

/// Reference to an interrupt handler
#[derive(Copy, Clone)]
pub struct EventHandlerRef(NonNull<RawEventHandler>);

impl EventHandlerRef {
    /// Create a new InterruptRef from a static reference
    #[allow(dead_code)]
    pub(crate) fn new(handler: &'static RawEventHandler) -> EventHandlerRef {
        EventHandlerRef(NonNull::from(handler))
    }

    /// Create an InterruptRef from a raw pointer (unsafe)
    pub unsafe fn from_ptr(ptr: *const RawEventHandler) -> EventHandlerRef {
        EventHandlerRef(unsafe { NonNull::new_unchecked(ptr as *mut RawEventHandler) })
    }

    /// Get the base priority of this event
    pub fn base_priority(&self) -> Priority {
        unsafe { self.as_ref() }.priority()
    }

    /// Send events to this event handler
    ///
    /// This is the primary interface for triggering software interrupts (events).
    pub fn send_events(&self, events: Events) {
        Scheduler::send_event(
            unsafe { core::pin::Pin::new_unchecked(self.0.as_ref()) },
            events,
        );
    }

    /// Check if this handler has pending events
    pub fn has_pending_events(&self) -> bool {
        unsafe { self.as_ref() }.has_pending_events()
    }

    /// Get a reference to the underlying RawEventHandler
    ///
    /// # Safety
    ///
    /// It is not in general safe to cast a pointer into a reference, and then
    /// dereference the reference. If you know that you are not violating
    /// any of the aliasing rules, you can use this method to obtain a reference
    /// to the underlying data and call re-entrant methods and read immutable data.
    pub(crate) unsafe fn as_ref(&self) -> &'static RawEventHandler {
        unsafe { self.0.as_ref() }
    }

    /// Get a mutable reference to the underlying RawEventHandler
    ///
    /// # Safety
    ///
    /// Same safety requirements as as_ref, but for mutable access.
    /// Caller must ensure exclusive access to avoid aliasing violations.
    #[allow(dead_code)]
    pub(crate) unsafe fn as_mut(&self) -> &'static mut RawEventHandler {
        unsafe { self.0.as_ptr().as_mut().unwrap() }
    }
}

unsafe impl Send for EventHandlerRef {}
unsafe impl Sync for EventHandlerRef {}
