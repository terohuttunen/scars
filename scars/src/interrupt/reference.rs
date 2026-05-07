//! Interrupt handler reference types
//!
//! This module provides safe reference types for accessing interrupt handlers
//! and managing their lifecycle.

use super::{InterruptNumber, RawInterruptHandler};
use crate::priority::Priority;
use crate::sync::interrupt_lock::InterruptLock;

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

    /// Create an InterruptRef from a raw pointer (unsafe)
    #[allow(dead_code)]
    pub(crate) unsafe fn from_ptr(ptr: *const RawInterruptHandler) -> InterruptRef {
        InterruptRef(unsafe { NonNull::new_unchecked(ptr as *mut RawInterruptHandler) })
    }

    /// Get the base priority of this interrupt
    pub fn base_priority(&self) -> Priority {
        unsafe { self.as_ref() }.base_priority()
    }

    /// Get the interrupt number
    pub fn interrupt_number(&self) -> InterruptNumber {
        unsafe { self.as_ref() }.interrupt_number()
    }

    /// Enable this interrupt
    pub fn enable(&self) {
        InterruptLock::with(|key| {
            let handler = unsafe { self.as_ref() };
            if !handler.is_attached(key) {
                panic!("Attempt to enable interrupt that has not been attached");
            }
            handler.enable_interrupt(key);
        })
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
