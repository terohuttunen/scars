use crate::events::context::event_handler_context;
use crate::events::{Events, raw::RawEventHandler};
use crate::kernel::atomic_queue::AtomicQueue;
use crate::kernel::list::LinkedListTag;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};

use super::{Scheduler, hal};

pub struct PendingNotifyTag {}

impl LinkedListTag for PendingNotifyTag {}

/// Pending event processing queue
///
/// This manages the queue of event handlers that have pending events and need
/// to be processed by the scheduler. It ensures thread-safe queuing and processing
/// of events with proper priority management.
pub struct PendingEventsQueue {
    events_pending: AtomicBool,
    queue: AtomicQueue<RawEventHandler, PendingNotifyTag>,
}

impl PendingEventsQueue {
    pub const fn new() -> Self {
        Self {
            events_pending: AtomicBool::new(false),
            queue: AtomicQueue::new(),
        }
    }

    /// Queue an event processing.
    pub fn queue_event_handler(&'static self, handler: Pin<&'static RawEventHandler>) {
        if handler.pending_event_processing_node.is_linked() {
            // Event processing already queued for this handler
            return;
        }

        let _ = self.queue.try_push_back(handler);
        self.events_pending.store(true, Ordering::Release);
    }

    /// Process all pending events
    ///
    /// Events are processed in their proper event handler context to ensure
    /// correct priority and local storage.
    pub fn process_pending_events(&'static self) {
        use crate::interrupt::interrupt_context;
        use crate::kernel::hal;

        let interrupt = match crate::interrupt::current_interrupt() {
            Some(mut interrupt) => unsafe { interrupt.as_mut() },
            _ => panic!("Not in interrupt context"),
        };

        while self.events_pending.swap(false, Ordering::AcqRel) {
            while let Some(pending_event_handler) = self.queue.pop_front() {
                // Save current interrupt threshold
                let saved_threshold = hal::get_interrupt_threshold();

                // Set threshold to interrupt's priority for proper ceiling protocol
                let interrupt_priority = pending_event_handler.get_ref().priority();
                if let crate::priority::Priority::Interrupt(prio) = interrupt_priority {
                    hal::set_interrupt_threshold(prio);
                }

                // Process events in the event handler's own context
                unsafe {
                    event_handler_context(
                        interrupt,
                        &*pending_event_handler as *const _
                            as *mut crate::events::raw::RawEventHandler,
                        || {
                            pending_event_handler.execute();
                        },
                    );
                }

                // Restore original interrupt threshold
                hal::set_interrupt_threshold(saved_threshold);
            }
        }
    }

    /// Check if there are any pending events to process
    #[allow(dead_code)]
    pub fn has_pending_events(&'static self) -> bool {
        self.events_pending.load(Ordering::Acquire)
    }

    /// Set pending event processing flag
    /// Used by other parts of the system to indicate that event processing is needed
    #[allow(dead_code)]
    pub fn set_pending_event_processing(&'static self) {
        self.events_pending.store(true, Ordering::Release);
    }
}

unsafe impl Sync for PendingEventsQueue {}

/// Scheduler event processing API
impl Scheduler {
    /// Send an event to an event handler, queueing it for processing if needed
    pub(crate) fn send_event(
        handler: Pin<&'static RawEventHandler>,
        events: crate::events::Events,
    ) {
        // Add events to the event handler's pending set
        handler.add_pending_events(events);

        // Thread-level event handlers are executed synchronously
        // by a thread that is waiting on the event handler. Interrupt
        // level handlers are scheduled by the kernel.
        if handler.priority().is_interrupt() {
            // Queue the event handler for processing
            Self::queue_pending_events(handler);
        }
    }

    /// Queue an event handler for deferred event processing
    pub(crate) fn queue_pending_events(event_handler: Pin<&'static RawEventHandler>) {
        Scheduler::instance()
            .pending_events
            .queue_event_handler(event_handler);
        hal::pend_service_call();
    }

    /// Process all pending events
    pub(crate) fn process_all_pending_events() {
        Scheduler::instance()
            .pending_events
            .process_pending_events();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Events;

    // Test event constants
    const EVENT_A: Events = 1 << 0;
    const EVENT_B: Events = 1 << 1;
    const EVENT_C: Events = 1 << 2;

    // Mock handler function for testing
    fn test_handler(_context: *const (), events: Events) -> Events {
        // Simple rule: if EVENT_A is present, generate EVENT_B
        if events & EVENT_A != 0 { EVENT_B } else { 0 }
    }
}
