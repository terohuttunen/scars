use super::{builder::EventHandlerBuilder, raw::RawEventHandler};

use crate::priority::Priority;
use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use static_cell::StaticCell;

pub trait EventHandlerFn: FnMut() + Send + 'static {}

impl<F: FnMut() + Send + 'static> EventHandlerFn for F {}

/// Event handler and closure container
///
/// Event handlers are software interrupts
pub struct EventHandler<const PRIO: Priority, F: EventHandlerFn> {
    handler: StaticCell<RawEventHandler>,
    closure: UnsafeCell<MaybeUninit<F>>,
}

impl<const PRIO: Priority, F: EventHandlerFn> EventHandler<PRIO, F> {
    pub const fn new() -> EventHandler<PRIO, F> {
        assert!(
            PRIO.is_interrupt(),
            "Event handler priority must be an interrupt priority"
        );
        EventHandler {
            handler: StaticCell::new(),
            closure: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    pub fn init(&'static self) -> EventHandlerBuilder<PRIO, F> {
        let handler = self.handler.init_with(|| RawEventHandler::new(PRIO));
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

unsafe impl<const PRIO: Priority, F: EventHandlerFn> Sync for EventHandler<PRIO, F> {}
