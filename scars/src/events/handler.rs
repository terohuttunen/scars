use super::{builder::EventHandlerBuilder, raw::RawEventHandler};

use crate::kernel::hal::CoreId;
use crate::priority::Priority;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use static_cell::StaticCell;

pub trait EventHandlerFn: FnMut() + Send + 'static {}

impl<F: FnMut() + Send + 'static> EventHandlerFn for F {}

/// Event handler and closure container
///
/// Event handlers are software interrupts
pub struct EventHandler<
    const PRIO: Priority,
    F: EventHandlerFn,
    const CORE: CoreId = { CoreId::DEFAULT },
> {
    handler: StaticCell<RawEventHandler>,
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: Priority, F: EventHandlerFn, const CORE: CoreId> EventHandler<PRIO, F, CORE> {
    pub const fn new() -> EventHandler<PRIO, F, CORE> {
        assert!(
            PRIO.is_interrupt(),
            "Event handler priority must be an interrupt priority"
        );
        EventHandler {
            handler: StaticCell::new(),
            closure: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    pub fn init(&'static self) -> EventHandlerBuilder<PRIO, F, CORE> {
        let handler = self.handler.init_with(|| RawEventHandler::new(PRIO, CORE));
        let closure = unsafe { &mut *self.closure.get() };
        EventHandlerBuilder::new(handler, closure)
    }

    /// C-style wrapper function for the event handler closure
    ///
    /// Because EventHandler is generic on the closure type `F`, a unique wrapper
    /// function is generated for each closure. `arg_ptr` (passed here as
    /// `closure_ptr`) is the closure storage pointer set by `attach`.
    pub(crate) fn closure_wrapper(closure_ptr: *mut ()) {
        let closure = unsafe { &mut *(closure_ptr as *mut F) };
        closure();
    }
}

unsafe impl<const PRIO: Priority, F: EventHandlerFn, const CORE: CoreId> Sync
    for EventHandler<PRIO, F, CORE>
{
}
