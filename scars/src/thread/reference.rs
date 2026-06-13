use core::ptr::NonNull;

use super::RawThread;
use crate::events::sender::{EventReceiver, EventSender};
use crate::kernel::scheduler::{ExecutionContext, Scheduler};
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

    /// Get a cheap, copyable sender that delivers events to this thread.
    pub fn sender(&self) -> EventSender {
        unsafe { self.0.as_ref().sender() }
    }

    pub fn base_priority(&self) -> Priority {
        unsafe { self.0.as_ref().base_priority }
    }

    /// Accumulated CPU time consumed by this thread, excluding time spent
    /// in interrupt handlers that preempted it.
    ///
    /// For the thread currently running on the calling core the in-progress
    /// run-slice is included; for any other thread the value is current as
    /// of the last time it stopped running.
    #[cfg(feature = "execution-time")]
    pub fn execution_time(&self) -> crate::time::Duration {
        unsafe { self.as_ref() }.effective_execution_time()
    }

    /// Begin a new measurement window for this thread's execution-time monitor
    /// (configured at the builder with
    /// [`ThreadBuilder::monitor`](crate::thread::ThreadBuilder::monitor)). The
    /// previous window is closed into the observed worst-case execution time
    /// (see [`wcet`](Self::wcet)), so calling this once per cycle measures
    /// per-window CPU. The budget event is delivered at most once per window.
    ///
    /// Must be called from the thread's own core.
    #[cfg(feature = "execution-time-monitor")]
    pub fn restart_execution_time_monitor(&self) {
        let thread = unsafe { Pin::new_unchecked(self.as_ref()) };
        thread.restart_monitor();
    }

    /// Disarm this thread's execution-time monitor, closing the open window
    /// into the observed worst-case execution time. The WCET is retained. Must
    /// be called from the thread's own core.
    #[cfg(feature = "execution-time-monitor")]
    pub fn cancel_execution_time_monitor(&self) {
        let thread = unsafe { Pin::new_unchecked(self.as_ref()) };
        thread.cancel_monitor();
    }

    /// Observed worst-case execution time: the largest CPU this thread
    /// consumed in any completed monitoring window (the interval between two
    /// successive restarts). Zero if no window has completed. Must be read from
    /// the thread's own core.
    #[cfg(feature = "execution-time-monitor")]
    pub fn wcet(&self) -> crate::time::Duration {
        let raw = unsafe { self.as_ref() };
        crate::sync::PreemptLock::with(|pkey| raw.monitor_wcet(pkey))
    }

    /// Clear this thread's observed worst-case execution time. Must be called
    /// from the thread's own core.
    #[cfg(feature = "execution-time-monitor")]
    pub fn reset_wcet(&self) {
        let thread = unsafe { Pin::new_unchecked(self.as_ref()) };
        thread.reset_monitor_wcet();
    }

    pub(crate) unsafe fn as_ref(&self) -> &'static RawThread {
        unsafe { self.0.as_ref() }
    }

    pub unsafe fn current() -> ThreadRef {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(ctx) => ThreadRef::new_from_pin(ctx),
            ExecutionContext::Interrupt(_) => panic!("No current thread"),
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
