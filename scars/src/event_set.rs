use crate::events::{
    REQUIRE_ALL_EVENTS, SCHEDULER_NOTIFY_EVENT, SCHEDULER_WAKEUP_EVENT, WaitEventsUntilError,
};
use crate::syscall;
use crate::time::Instant;
use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Copy, Clone, Debug)]
pub enum TryWaitEventsError {
    WouldBlock,
}

pub struct EventSet {
    waited_events_mask: AtomicU32,
    sent_events_mask: AtomicU32,
}

impl EventSet {
    pub const fn new() -> Self {
        Self {
            waited_events_mask: AtomicU32::new(0),
            sent_events_mask: AtomicU32::new(0),
        }
    }

    pub(crate) fn set_wakeup_event(&self) {
        self.sent_events_mask
            .fetch_or(SCHEDULER_WAKEUP_EVENT, Ordering::Release);
    }

    pub(crate) fn set_resume_event(&self) {
        self.sent_events_mask
            .fetch_or(SCHEDULER_NOTIFY_EVENT, Ordering::Release);
    }

    pub(crate) fn set_waited_events(&self, events: u32) {
        self.waited_events_mask.store(events, Ordering::SeqCst);
    }

    pub(crate) fn read_and_clear_matching_events(&self, events: u32) -> u32 {
        self.sent_events_mask.fetch_and(!events, Ordering::SeqCst) & events
    }

    pub fn clear_waited_events(&self) {
        self.waited_events_mask.store(0, Ordering::SeqCst);
    }

    fn will_not_block(require_all: bool, sent_events: u32, events: u32) -> bool {
        (!require_all && sent_events & events != 0) || (sent_events & events) == events
    }

    fn should_notify(require_all: bool, received_events: u32, receiving_events: u32) -> bool {
        (!require_all && received_events != 0)
            || (received_events != 0 && received_events == receiving_events)
    }

    pub fn send_events(&self, mut events: u32) -> bool {
        // Remove require_all flag from incoming events. This flag is only used for receiving events.
        events &= !REQUIRE_ALL_EVENTS;

        // Update sent events mask
        let sent_events = self.sent_events_mask.fetch_or(events, Ordering::SeqCst) | events;

        // Read receiving events mask. This should be empty if the receiving thread is not
        // waiting for any events.
        let mut receiving_events = self.waited_events_mask.load(Ordering::SeqCst);

        // Extract and remove the require_all flag from the receiving events mask
        let require_all = (receiving_events & REQUIRE_ALL_EVENTS) != 0;
        receiving_events &= !REQUIRE_ALL_EVENTS;

        // Filter out events that have not been received.
        let received_events = receiving_events & sent_events;

        // Notify the thread if it is waiting for any/all of the sent events
        if Self::should_notify(require_all, received_events, receiving_events) {
            // Thread is waiting for the events, resume it
            true
        } else {
            // Thread is not waiting for the events, do not resume it
            false
        }
    }

    pub fn wait_events(&self, events: u32) -> u32 {
        // Wait for the events. The syscall returns events read before blocking.
        let mut received_events = syscall::thread_wait_event(events);

        // Clear waited events mask to suppress thread notifications from sender.
        self.waited_events_mask.store(0, Ordering::SeqCst);

        // In case there were more events sent between thread notification and syscall return,
        // Read and clear matching events that could have lead to the thread wakeup, now that
        // the waited events mask has been cleared.
        received_events |= self.sent_events_mask.fetch_and(!events, Ordering::SeqCst) & events;

        received_events
    }

    pub fn wait_events_until(
        &self,
        mut events: u32,
        deadline: Instant,
    ) -> Result<u32, WaitEventsUntilError> {
        // Wait for the events. The syscall returns events read before blocking.
        let mut received_events = syscall::thread_wait_event_until(events, deadline);

        // Clear waited events mask to suppress thread notifications from sender.
        self.waited_events_mask.store(0, Ordering::SeqCst);

        // In case there were more events sent between thread notification and syscall return,
        // Read and clear matching events that could have lead to the thread wakeup, now that
        // the waited events mask has been cleared.
        events |= SCHEDULER_WAKEUP_EVENT;
        received_events |= self.sent_events_mask.fetch_and(!events, Ordering::SeqCst) & events;

        if received_events & SCHEDULER_WAKEUP_EVENT != 0 {
            Err(WaitEventsUntilError::Timeout(received_events))
        } else {
            Ok(received_events)
        }
    }

    pub fn try_wait_events(&self, mut events: u32) -> Result<u32, TryWaitEventsError> {
        let require_all = events & REQUIRE_ALL_EVENTS != 0;
        events &= !REQUIRE_ALL_EVENTS;

        let sent_events = self.sent_events_mask.load(Ordering::SeqCst);

        if Self::will_not_block(require_all, sent_events, events) {
            Ok(self.wait_events(events))
        } else {
            Err(TryWaitEventsError::WouldBlock)
        }
    }
}

unsafe impl Sync for EventSet {}
unsafe impl Send for EventSet {}
