//! Event handler builder and initialization
//!
//! This module provides the builder pattern for configuring and initializing
//! event handlers.

use super::{
    handler::{EventHandler, EventHandlerFn},
    raw::RawEventHandler,
    sender::{EventReceiver, EventSender},
};
use crate::events::Events;
use crate::priority::Priority;
use crate::sync::interrupt_lock::InterruptLock;
use crate::task::{ExecutorHandle, JoinHandle, TaskHandle};
use crate::tls::{LocalStorage, SharedStorage, SharedStorageProvider};

use core::mem::MaybeUninit;
use core::ptr::NonNull;

/// Event handler builder for configuration before attachment
pub struct EventHandlerBuilder<const PRIO: Priority, F: EventHandlerFn> {
    handler: &'static mut RawEventHandler,
    closure: &'static mut MaybeUninit<F>,
}

impl<const PRIO: Priority, F: EventHandlerFn> EventHandlerBuilder<PRIO, F> {
    pub(crate) fn new(
        handler: &'static mut RawEventHandler,
        closure: &'static mut MaybeUninit<F>,
    ) -> Self {
        Self { handler, closure }
    }

    pub fn set_shared_storage<S: SharedStorageProvider<PRIO>>(self, provider: &S) {
        let head = provider.shared_storage().head();
        self.handler.local_storage.share_with(head);
    }

    pub fn with_shared_storage<S: SharedStorageProvider<PRIO>>(self, provider: &S) -> Self {
        let head = provider.shared_storage().head();
        self.handler.local_storage.share_with(head);
        self
    }

    /// Attach a closure to this event handler
    pub fn attach(self, closure: F) -> EventHandlerHandle<PRIO> {
        let closure_ref = self.closure.write(closure);
        let closure_ptr = closure_ref as *const F as *mut _;
        InterruptLock::with(|key| unsafe {
            self.handler
                .attach(EventHandler::<PRIO, F>::closure_wrapper, closure_ptr, key)
        });

        // SAFETY: self.handler is a valid &'static mut from StaticCell
        EventHandlerHandle {
            handler: NonNull::from(self.handler),
        }
    }

    /// Get access to the raw event handler for advanced configuration
    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawEventHandler) -> R) -> R {
        f(self.handler)
    }

    /// Get the base priority
    pub fn base_priority(&self) -> Priority {
        self.handler.priority()
    }

    /// `LocalStorage` slot for this event handler.
    pub fn local_storage(&self) -> &'static LocalStorage {
        // SAFETY: handler is reachable for 'static.
        let h: &'static RawEventHandler = unsafe { &*(self.handler as *const RawEventHandler) };
        h.local_storage()
    }
}

/// Initialized event handler after it has been built
///
/// Uniquely owned reference to an event handler
pub struct EventHandlerHandle<const PRIO: Priority> {
    handler: NonNull<RawEventHandler>,
}

impl<const PRIO: Priority> EventHandlerHandle<PRIO> {
    /// Get a static reference to the raw event handler
    ///
    /// # Safety
    /// The NonNull pointer is guaranteed to be valid for 'static lifetime
    /// as it was created from a StaticCell.
    fn raw(&self) -> &'static RawEventHandler {
        // SAFETY: self.handler points to data in a StaticCell with 'static lifetime
        unsafe { self.handler.as_ref() }
    }

    /// Get a mutable reference to the raw event handler
    ///
    /// # Safety
    /// The NonNull pointer is guaranteed to be valid for 'static lifetime
    /// as it was created from a StaticCell.
    fn raw_mut(&mut self) -> &'static mut RawEventHandler {
        // SAFETY: self.handler points to data in a StaticCell with 'static lifetime
        // and we have &mut self ensuring exclusive access
        unsafe { self.handler.as_mut() }
    }

    /// Spawn an async task on this event handler's executor
    pub fn spawn<T>(&self, task: TaskHandle<T>) -> Result<JoinHandle<T>, ()> {
        self.raw()
            .local_storage()
            .head()
            .with::<ExecutorHandle, _>(|e| e.spawn(task))
            .ok_or(())
    }

    pub fn set_shared_storage<S: SharedStorageProvider<PRIO>>(&mut self, share: &S) {
        let head = share.shared_storage().head();
        self.raw_mut().local_storage.share_with(head);
    }

    /// Modify the raw event handler
    pub fn modify<R>(&mut self, f: impl FnOnce(&mut RawEventHandler) -> R) -> R {
        f(self.raw_mut())
    }

    /// `LocalStorage` slot for this event handler.
    pub fn local_storage(&self) -> &'static LocalStorage {
        self.raw().local_storage()
    }

    /// Get the base priority
    pub fn priority(&self) -> Priority {
        self.raw().priority()
    }

    /// Send events to this handler.
    pub fn send_events(&self, events: Events) {
        self.raw().send_events(events);
    }

    /// Get a cheap, copyable sender for this handler.
    pub fn sender(&self) -> EventSender {
        self.raw().sender()
    }
}

unsafe impl<const PRIO: Priority> Send for EventHandlerHandle<PRIO> {}
unsafe impl<const PRIO: Priority> Sync for EventHandlerHandle<PRIO> {}

impl<const PRIO: Priority> SharedStorageProvider<PRIO> for EventHandlerHandle<PRIO> {
    fn shared_storage(&self) -> SharedStorage<PRIO> {
        // SAFETY: handler runs at PRIO; sharers run at the same priority.
        unsafe { SharedStorage::from_head(self.raw().local_storage.head()) }
    }
}
