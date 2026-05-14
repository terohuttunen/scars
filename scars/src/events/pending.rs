use crate::events::{AtomicEvents, Events};
use crate::sync::atomic::Ordering;

/// Atomic set of pending event flags.
#[repr(align(16))]
#[repr(C)]
pub struct PendingEvents {
    pub(crate) pending_events: AtomicEvents,
}

impl PendingEvents {
    pub const fn new() -> PendingEvents {
        PendingEvents {
            pending_events: AtomicEvents::new(0),
        }
    }

    /// Add events to the pending set
    pub fn add_pending_events(&self, events: Events) {
        self.pending_events.fetch_or(events, Ordering::SeqCst);
    }

    /// Check if there are pending events
    pub fn has_pending_events(&self) -> bool {
        self.pending_events.load(Ordering::SeqCst) != 0
    }

    /// Get a pointer to this pending event set
    pub fn as_ptr(&self) -> *const PendingEvents {
        self as *const PendingEvents
    }

    /// Peek at pending events without consuming them
    pub fn peek_pending_events(&self) -> Events {
        self.pending_events.load(Ordering::SeqCst)
    }
}
