use super::{InterruptNumber, MAX_INTERRUPT_NUMBER, RawInterruptHandler};
use crate::cell::LockedCell;
use crate::sync::interrupt_lock::{InterruptLock, InterruptLockKey};

static INTERRUPT_VECTORS: LockedCell<[InterruptVector; MAX_INTERRUPT_NUMBER + 1], InterruptLock> =
    LockedCell::new(
        [InterruptVector {
            handler_ptr: core::ptr::null(),
            icb_ptr: core::ptr::null(),
        }; MAX_INTERRUPT_NUMBER + 1],
    );

/// Get the interrupt vector for a given interrupt number
pub(crate) fn get_interrupt_vector(
    number: InterruptNumber,
    key: InterruptLockKey<'_>,
) -> InterruptVector {
    INTERRUPT_VECTORS.as_array_of_cells()[number as usize].get(key)
}

/// Set the interrupt vector for a given interrupt number  
pub(crate) fn set_interrupt_vector(
    number: InterruptNumber,
    key: InterruptLockKey<'_>,
    vector: InterruptVector,
) {
    INTERRUPT_VECTORS.as_array_of_cells()[number as usize].set(key, vector)
}

/// Interrupt vector entry for hardware dispatch
#[repr(C)]
#[derive(Copy, Clone)]
pub struct InterruptVector {
    pub(crate) handler_ptr: *const (),
    pub(crate) icb_ptr: *const RawInterruptHandler,
}

impl InterruptVector {
    /// Create a new interrupt vector entry
    pub(crate) const fn new(handler_ptr: *const (), icb_ptr: *const RawInterruptHandler) -> Self {
        Self {
            handler_ptr,
            icb_ptr,
        }
    }

    /// Create an empty interrupt vector entry
    pub const fn empty() -> Self {
        Self {
            handler_ptr: core::ptr::null(),
            icb_ptr: core::ptr::null(),
        }
    }

    /// Check if this vector entry is empty
    pub fn is_empty(&self) -> bool {
        self.handler_ptr.is_null() || self.icb_ptr.is_null()
    }
}
