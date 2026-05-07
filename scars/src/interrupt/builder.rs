//! Interrupt builder and initialization
//!
//! This module provides the builder pattern for configuring and initializing
//! interrupt handlers

use super::{InterruptHandler, InterruptHandlerFn, InterruptNumber, RawInterruptHandler};
use crate::priority::Priority;
use crate::sync::interrupt_lock::InterruptLock;
use crate::tls::{ConstLocalCell, LocalCell, LocalStorage, SharedStorage, SharedStorageProvider};

use core::mem::MaybeUninit;
use core::ptr::NonNull;

/// Interrupt builder for configuration before attachment
pub struct InterruptBuilder<const PRIO: Priority, F: InterruptHandlerFn> {
    handler: &'static mut RawInterruptHandler,
    closure: &'static mut MaybeUninit<F>,
}

impl<const PRIO: Priority, F: InterruptHandlerFn> InterruptBuilder<PRIO, F> {
    /// Create a new InterruptBuilder
    pub(crate) fn new(
        handler: &'static mut RawInterruptHandler,
        closure: &'static mut MaybeUninit<F>,
    ) -> Self {
        Self { handler, closure }
    }

    /// Store and initialize with LocalCell (runtime value creation)
    pub fn with_local<T: 'static>(
        self,
        storage: &'static LocalCell<T>,
        init_fn: impl FnOnce() -> T,
    ) -> Self {
        let head = self.handler.local_storage.head();

        if head.contains::<T>() {
            panic!("Type {} already initialized", core::any::type_name::<T>());
        }

        let _ = head.put_init_with(storage, init_fn);

        self
    }

    /// Store and initialize with ConstLocalCell (compile-time value)
    pub fn with_const_local<T: 'static>(self, storage: &'static ConstLocalCell<T>) -> Self {
        let head = self.handler.local_storage.head();

        if head.contains::<T>() {
            panic!("Type {} already initialized", core::any::type_name::<T>());
        }

        let _ = head.put_take(storage);

        self
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

    /// Attach a closure to this interrupt handler
    pub fn attach(self, closure: F) -> InterruptHandlerHandle<PRIO> {
        let closure_ref = self.closure.write(closure);
        let closure_ptr = closure_ref as *const F as *const ();
        InterruptLock::with(|key| unsafe {
            self.handler.attach(
                InterruptHandler::<PRIO, F>::closure_wrapper as *const (),
                closure_ptr,
                key,
            )
        });
        // SAFETY: self.handler is a valid &'static mut from StaticCell
        InterruptHandlerHandle {
            handler: NonNull::from(self.handler),
        }
    }

    /// Get the interrupt number
    pub fn interrupt_number(&self) -> InterruptNumber {
        self.handler.interrupt_number()
    }

    /// Get the base priority
    pub fn base_priority(&self) -> Priority {
        self.handler.base_priority()
    }
}

/// Initialized interrupt handler after attachment
///
/// Uniquely owned reference to an interrupt handler
pub struct InterruptHandlerHandle<const PRIO: Priority> {
    handler: NonNull<RawInterruptHandler>,
}

impl<const PRIO: Priority> InterruptHandlerHandle<PRIO> {
    /// Get a static reference to the raw interrupt handler
    ///
    /// # Safety
    /// The NonNull pointer is guaranteed to be valid for 'static lifetime
    /// as it was created from a StaticCell.
    fn raw(&self) -> &'static RawInterruptHandler {
        // SAFETY: self.handler points to data in a StaticCell with 'static lifetime
        unsafe { self.handler.as_ref() }
    }

    /// Get a mutable reference to the raw interrupt handler
    ///
    /// # Safety
    /// The NonNull pointer is guaranteed to be valid for 'static lifetime
    /// as it was created from a StaticCell. Caller must ensure no aliasing.
    #[allow(dead_code)]
    fn raw_mut(&mut self) -> &'static mut RawInterruptHandler {
        // SAFETY: self.handler points to data in a StaticCell with 'static lifetime
        // and we have &mut self ensuring exclusive access
        unsafe { self.handler.as_mut() }
    }

    /// `LocalStorage` slot for this handler.
    pub fn local_storage(&self) -> &'static LocalStorage {
        &self.raw().local_storage
    }

    /// Enable the interrupt and return a reference
    pub fn enable(&self) {
        let handler = self.raw();
        InterruptLock::with(|key| {
            if !handler.is_attached(key) {
                panic!("Attempt to enable interrupt that has not been attached");
            }
            handler.enable_interrupt(key);
        })
    }

    /// Get the interrupt number
    pub fn interrupt_number(&self) -> InterruptNumber {
        self.raw().interrupt_number()
    }

    /// Get the base priority
    pub fn priority(&self) -> Priority {
        self.raw().base_priority()
    }
}

impl<const PRIO: Priority> SharedStorageProvider<PRIO> for InterruptHandlerHandle<PRIO> {
    fn shared_storage(&self) -> SharedStorage<PRIO> {
        // SAFETY: handler runs at PRIO; sharers run at the same priority.
        unsafe { SharedStorage::from_head(self.raw().local_storage.head()) }
    }
}
