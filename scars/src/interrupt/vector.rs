use super::{InterruptNumber, MAX_INTERRUPT_NUMBER, RawInterruptHandler};
use crate::cell::LockedCell;
use crate::kernel::hal::NUM_CORES;
use crate::sync::lock::interrupt_lock::{InterruptLock, InterruptLockKey};

/// One vector table per core.
static INTERRUPT_VECTORS: [LockedCell<[InterruptVector; MAX_INTERRUPT_NUMBER + 1], InterruptLock>;
    NUM_CORES] = [const {
    LockedCell::new(
        [InterruptVector {
            handler_ptr: core::ptr::null(),
            context_ptr: core::ptr::null(),
        }; MAX_INTERRUPT_NUMBER + 1],
    )
}; NUM_CORES];

/// Get the interrupt vector for `number` on the core named by `key`.
pub(crate) fn get_interrupt_vector(
    number: InterruptNumber,
    key: InterruptLockKey<'_>,
) -> InterruptVector {
    INTERRUPT_VECTORS[key.core.as_usize()].as_array_of_cells()[number as usize].get(key)
}

/// Set the interrupt vector for `number` on the core named by `key`.
pub(crate) fn set_interrupt_vector(
    number: InterruptNumber,
    key: InterruptLockKey<'_>,
    vector: InterruptVector,
) {
    INTERRUPT_VECTORS[key.core.as_usize()].as_array_of_cells()[number as usize].set(key, vector)
}

/// Interrupt vector entry for hardware dispatch
#[repr(C)]
#[derive(Copy, Clone)]
pub struct InterruptVector {
    pub(crate) handler_ptr: *const (),
    pub(crate) context_ptr: *const RawInterruptHandler,
}

impl InterruptVector {
    /// Create a new interrupt vector entry
    pub(crate) const fn new(
        handler_ptr: *const (),
        context_ptr: *const RawInterruptHandler,
    ) -> Self {
        Self {
            handler_ptr,
            context_ptr,
        }
    }

    /// Create an empty interrupt vector entry
    pub const fn empty() -> Self {
        Self {
            handler_ptr: core::ptr::null(),
            context_ptr: core::ptr::null(),
        }
    }

    /// Check if this vector entry is empty
    pub fn is_empty(&self) -> bool {
        self.handler_ptr.is_null() || self.context_ptr.is_null()
    }
}
