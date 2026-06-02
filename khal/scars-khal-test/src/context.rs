use core::cell::Cell;
use core::ptr::NonNull;
use scars_khal::ContextInfo;

/// Recorded thread context.
///
/// The synchronous harness never executes a thread body, so the context
/// only stores the static description the kernel hands it at `init`.
/// `start_first_thread` reads `main_fn` to run the idle body directly on
/// the host stack (reaching the test runner); every other "switch" is a
/// recorded change of the current-context pointer, not a transfer of
/// control.
pub struct TestContext {
    pub name: &'static str,
    pub main_fn: *const (),
    pub argument: Option<NonNull<u8>>,
    pub stack_top_ptr: Cell<*const u8>,
}

impl ContextInfo for TestContext {
    fn stack_top_ptr(&self) -> *const u8 {
        self.stack_top_ptr.get()
    }

    unsafe fn init(
        name: &'static str,
        main_fn: *const (),
        argument: Option<*const u8>,
        stack_ptr: *const u8,
        _stack_size: usize,
        context: *mut Self,
    ) {
        // `context` points at uninitialized storage (`MaybeUninit` inside
        // the `RawThread`). None of these fields have drop glue, so the
        // assignments do not read the uninitialized values.
        unsafe {
            (*context).name = name;
            (*context).main_fn = main_fn;
            (*context).argument = argument.map(|a| NonNull::new_unchecked(a as *mut _));
            (*context).stack_top_ptr = Cell::new(stack_ptr);
        }
    }
}

impl core::fmt::Debug for TestContext {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TestContext {{ name: {:?} }}", self.name)
    }
}
