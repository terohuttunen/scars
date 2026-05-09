use super::raw_task::RawTask;
use crate::events::Events;
use core::future::Future;
use core::marker::PhantomData;
use core::pin::Pin;
use core::task::{Context, Poll};

/// Future for waiting on events from the executor's event handler.
///
/// This is used by async tasks to wait for events sent via
/// `EventHandlerExecutorHandle::send_events()` or from interrupt handlers.
///
/// # Example
///
/// ```ignore
/// const BUTTON_EVENT: Events = 1 << 0;
///
/// // In async task:
/// loop {
///     let events = WaitForEvents::new(BUTTON_EVENT).await;
///     // Handle button press
/// }
///
/// // In interrupt handler:
/// executor_handle.send_events(BUTTON_EVENT);
/// ```
pub struct WaitForEvents {
    /// Events to wait for (mask)
    events: Events,
    /// Whether to wait for any (true) or all (false) of the events
    wait_any: bool,
    // To make sure that WaitForEvents is not Send or Sync
    _phantom: PhantomData<*const ()>,
}

impl WaitForEvents {
    /// Create a new WaitForEvents that waits for ANY of the specified events
    pub fn new(events: Events) -> WaitForEvents {
        WaitForEvents {
            events,
            wait_any: true,
            _phantom: PhantomData,
        }
    }

    /// Create a new WaitForEvents that waits for ALL of the specified events
    pub fn all(events: Events) -> WaitForEvents {
        WaitForEvents {
            events,
            wait_any: false,
            _phantom: PhantomData,
        }
    }

    /// Wait for any of the specified events
    pub fn any(events: Events) -> WaitForEvents {
        Self::new(events)
    }
}

impl Future for WaitForEvents {
    type Output = Events;

    fn poll(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Events> {
        let task = unsafe { &*(cx.waker().data() as *const RawTask) };
        let executor = task.get_executor().expect("polled task has no executor");

        if !executor.supports_events() {
            panic!("WaitForEvents requires an event-handler executor");
        }

        // wait_any: clear matching bits in one atomic op.
        // wait_all: only clear if all required bits are present (peek
        // first; the peek/consume race is the same one the previous
        // implementation had).
        let consumed = if self.wait_any {
            executor.consume_events(self.events)
        } else {
            let pending = executor.peek_events();
            if (pending & self.events) == self.events {
                executor.consume_events(self.events)
            } else {
                0
            }
        };

        if consumed != 0 {
            Poll::Ready(consumed)
        } else {
            // Push back into the executor's atomic pending-ready queue so
            // the next event arrival (which queues `executor_poll_handler`)
            // re-polls us.
            let pinned = unsafe { Pin::new_unchecked(task) };
            executor.resume_task(pinned);
            Poll::Pending
        }
    }
}
