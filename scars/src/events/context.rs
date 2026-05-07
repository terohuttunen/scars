use super::raw::RawEventHandler;
use crate::interrupt::RawInterruptHandler;

/// Execute code in event handler context with proper context switching.
///
/// Sets `interrupt.current_event_handler` to the running event handler
/// for the duration of `f`, then restores the previous value. Local
/// storage dispatch on this interrupt picks the event handler's
/// `LocalStorage` (which may itself redirect via `share_with`) while
/// the handler runs. Same-priority handlers are mutually exclusive, so
/// the set/restore pair is safe.
#[inline]
pub unsafe fn event_handler_context<R>(
    interrupt: &mut RawInterruptHandler,
    raw_ptr: *mut RawEventHandler,
    f: impl FnOnce() -> R,
) -> R {
    let prev = interrupt.current_event_handler.replace(raw_ptr);

    let rval = f();

    interrupt.current_event_handler.set(prev);

    rval
}
