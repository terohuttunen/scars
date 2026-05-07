//! Type-erased handle for delivering [`Events`] to a sender source
//! (an event handler or a thread).

use crate::events::Events;
use crate::priority::Priority;

/// Method table used by [`EventSender`] to dispatch into the underlying
/// sender source.
pub struct EventSenderVTable {
    pub(crate) send_events: fn(*const (), Events),
    pub(crate) has_pending_events: fn(*const ()) -> bool,
    pub(crate) priority: fn(*const ()) -> Priority,
}

/// `Copy` handle that delivers events to whatever produced it.
#[derive(Copy, Clone)]
pub struct EventSender {
    target: *const (),
    vtable: &'static EventSenderVTable,
}

impl EventSender {
    /// Construct a sender for an [`EventReceiver`] implementor. The
    /// type parameter pairs the target pointer and the vtable so they
    /// can never be mismatched at the call site.
    #[inline]
    pub const fn from_receiver<R: EventReceiver>(receiver: &'static R) -> Self {
        Self {
            target: receiver as *const R as *const (),
            vtable: &R::SENDER_VTABLE,
        }
    }

    /// Deliver `events` to the underlying source.
    #[inline]
    pub fn send_events(&self, events: Events) {
        (self.vtable.send_events)(self.target, events);
    }

    /// Returns `true` if the source has events pending consumption.
    #[inline]
    pub fn has_pending_events(&self) -> bool {
        (self.vtable.has_pending_events)(self.target)
    }

    /// Base priority of the source.
    #[inline]
    pub fn base_priority(&self) -> Priority {
        (self.vtable.priority)(self.target)
    }
}

unsafe impl Send for EventSender {}
unsafe impl Sync for EventSender {}

/// A `'static` source that can produce an [`EventSender`]. The trait
/// default supplies the per-`Self` vtable and the [`sender`](Self::sender)
/// factory; implementors provide only the three contract methods.
pub trait EventReceiver: Sized + 'static {
    fn send_events(&'static self, events: Events);
    fn has_pending_events(&self) -> bool;
    fn base_priority(&self) -> Priority;

    #[doc(hidden)]
    const SENDER_VTABLE: EventSenderVTable = EventSenderVTable {
        send_events: vt_send_events::<Self>,
        has_pending_events: vt_has_pending_events::<Self>,
        priority: vt_priority::<Self>,
    };

    /// Cheap, copyable handle that delivers events to this receiver.
    fn sender(&'static self) -> EventSender {
        EventSender::from_receiver(self)
    }
}

fn vt_send_events<R: EventReceiver>(target: *const (), events: Events) {
    let this: &'static R = unsafe { &*(target as *const R) };
    R::send_events(this, events);
}

fn vt_has_pending_events<R: EventReceiver>(target: *const ()) -> bool {
    R::has_pending_events(unsafe { &*(target as *const R) })
}

fn vt_priority<R: EventReceiver>(target: *const ()) -> Priority {
    R::base_priority(unsafe { &*(target as *const R) })
}
