use core::ptr::NonNull;

use super::RawThread;
use crate::kernel::scheduler::Scheduler;
use crate::priority::Priority;
use core::pin::Pin;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct ThreadRef(NonNull<RawThread>);

impl ThreadRef {
    fn new_from_pin(thread: Pin<&'static RawThread>) -> ThreadRef {
        ThreadRef(NonNull::from(thread.get_ref()))
    }

    pub(crate) fn new(thread: &'static RawThread) -> ThreadRef {
        ThreadRef(NonNull::from(thread))
    }

    pub(crate) unsafe fn from_ptr(ptr: *const RawThread) -> ThreadRef {
        ThreadRef(unsafe { NonNull::new_unchecked(ptr as *mut RawThread) })
    }

    pub fn name(&self) -> &'static str {
        unsafe { self.0.as_ref().name }
    }

    pub fn send_events(&self, event: u32) {
        unsafe { self.0.as_ref().send_events(event) }
    }

    pub fn priority(&self) -> Priority {
        unsafe { self.0.as_ref().priority() }
    }

    pub(crate) unsafe fn as_ref(&self) -> &'static RawThread {
        unsafe { self.0.as_ref() }
    }

    pub unsafe fn current() -> ThreadRef {
        match Scheduler::current_execution_context() {
            crate::ExecutionContext::Thread(ctx) => ThreadRef::new_from_pin(ctx),
            crate::ExecutionContext::Interrupt(_) => panic!("No current thread"),
        }
    }
}

impl PartialEq for ThreadRef {
    fn eq(&self, other: &ThreadRef) -> bool {
        core::ptr::eq(self.0.as_ptr(), other.0.as_ptr())
    }
}

impl Eq for ThreadRef {}

unsafe impl Sync for ThreadRef {}
unsafe impl Send for ThreadRef {}
