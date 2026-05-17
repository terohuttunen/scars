use super::{InterruptBuilder, InterruptNumber, RawInterruptHandler};

use crate::kernel::hal::CoreId;
use crate::priority::Priority;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use static_cell::StaticCell;

pub trait InterruptHandlerFn: FnMut() + Send + 'static {}

impl<F: FnMut() + Send + 'static> InterruptHandlerFn for F {}

/// Static interrupt handler container
pub struct InterruptHandler<
    const PRIO: Priority,
    F: InterruptHandlerFn,
    const CORE: CoreId = { CoreId::DEFAULT },
> {
    handler: StaticCell<RawInterruptHandler>,
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: Priority, F: InterruptHandlerFn, const CORE: CoreId>
    InterruptHandler<PRIO, F, CORE>
{
    /// Create a new interrupt handler for the given interrupt number
    pub const fn new() -> InterruptHandler<PRIO, F, CORE> {
        assert!(
            PRIO.is_interrupt(),
            "Interrupt handler priority must be an interrupt priority"
        );
        InterruptHandler {
            handler: StaticCell::new(),
            closure: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Initialize the interrupt handler with the given interrupt number
    pub fn init(&'static self, intnum: InterruptNumber) -> InterruptBuilder<PRIO, F, CORE> {
        let handler = self
            .handler
            .init_with(|| RawInterruptHandler::new(PRIO, CORE));

        let handler_ptr = handler as *mut _;
        RawInterruptHandler::init_at(handler_ptr, intnum);

        let closure = unsafe { &mut *self.closure.get() };
        InterruptBuilder::new(handler, closure)
    }

    /// C-style wrapper function for the interrupt closure
    pub(crate) unsafe extern "C" fn closure_wrapper(context_ptr: *mut ::core::ffi::c_void) {
        let context = unsafe { &*(context_ptr as *const RawInterruptHandler) };
        let closure = unsafe { &mut *(context.closure_ptr() as *mut F) };

        // Call closure
        closure();
    }
}

unsafe impl<const PRIO: Priority, F: InterruptHandlerFn, const CORE: CoreId> Sync
    for InterruptHandler<PRIO, F, CORE>
{
}
