//! Interrupt handler reference types
//!
//! This module provides safe reference types for accessing interrupt handlers
//! and managing their lifecycle.

use super::{InterruptNumber, RawInterruptHandler};
use crate::kernel::hal::CoreId;
use crate::priority::Priority;
use crate::sync::lock::interrupt_lock::InterruptLock;

use core::ptr::NonNull;

/// Reference to an interrupt handler
#[derive(Copy, Clone)]
pub struct InterruptRef(NonNull<RawInterruptHandler>);

impl InterruptRef {
    /// Create a new InterruptRef from a static reference
    #[allow(dead_code)]
    pub(crate) fn new(handler: &'static RawInterruptHandler) -> InterruptRef {
        InterruptRef(NonNull::from(handler))
    }

    /// Create an InterruptRef from a raw pointer.
    ///
    /// # Safety
    ///
    /// `ptr` must point at a fully-constructed `RawInterruptHandler`
    /// living for `'static` — in particular its `core` field must be
    /// set, since [`Self::enable`] / [`Self::disable`] read it for the
    /// wrong-core check.
    #[allow(dead_code)]
    pub(crate) unsafe fn from_ptr(ptr: *const RawInterruptHandler) -> InterruptRef {
        InterruptRef(unsafe { NonNull::new_unchecked(ptr as *mut RawInterruptHandler) })
    }

    /// Core this interrupt handler is bound to.
    pub fn core(&self) -> CoreId {
        unsafe { self.as_ref() }.core
    }

    /// Get the base priority of this interrupt
    pub fn base_priority(&self) -> Priority {
        unsafe { self.as_ref() }.base_priority()
    }

    /// Get the interrupt number
    pub fn interrupt_number(&self) -> InterruptNumber {
        unsafe { self.as_ref() }.interrupt_number()
    }

    /// Whether the underlying handler has been attached (its vector
    /// slot is non-null). Trips `WrongCore` if called from a different
    /// core than the handler is bound to.
    pub fn is_attached(&self) -> bool {
        let handler = unsafe { self.as_ref() };
        InterruptLock::with_core(handler.core, |key| handler.is_attached(key))
    }

    /// Enable this interrupt. Trips `WrongCore` if called from a
    /// different core than the handler is bound to — NVIC writes are
    /// local-core only.
    pub fn enable(&self) {
        let handler = unsafe { self.as_ref() };
        InterruptLock::with_core(handler.core, |key| {
            if !handler.is_attached(key) {
                panic!("Attempt to enable interrupt that has not been attached");
            }
            handler.enable_interrupt(key);
        });
    }

    /// Disable this interrupt. Trips `WrongCore` if called from a
    /// different core than the handler is bound to.
    pub fn disable(&self) {
        let handler = unsafe { self.as_ref() };
        InterruptLock::with_core(handler.core, |key| {
            handler.disable_interrupt(key);
        });
    }

    /// Get a reference to the underlying RawInterruptHandler
    ///
    /// # Safety
    ///
    /// It is not in general safe to cast a pointer into a reference, and then
    /// dereference the reference. If you know that you are not violating
    /// any of the aliasing rules, you can use this method to obtain a reference
    /// to the underlying data and call re-entrant methods and read immutable data.
    pub(crate) unsafe fn as_ref(&self) -> &'static RawInterruptHandler {
        unsafe { self.0.as_ref() }
    }

    /// Get a mutable reference to the underlying RawInterruptHandler
    ///
    /// # Safety
    ///
    /// Same safety requirements as as_ref, but for mutable access.
    /// Caller must ensure exclusive access to avoid aliasing violations.
    #[allow(dead_code)]
    pub(crate) unsafe fn as_mut(&self) -> &'static mut RawInterruptHandler {
        unsafe { self.0.as_ptr().as_mut().unwrap() }
    }
}

unsafe impl Send for InterruptRef {}
unsafe impl Sync for InterruptRef {}
