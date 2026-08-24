pub mod builder;
pub(crate) mod context;
pub mod handler;
pub mod options;
pub mod pending;
pub(crate) mod raw;
pub mod reference;
pub mod sender;

pub use options::EventOptions;
pub use sender::EventSender;

use crate::sync::atomic::AtomicU32;
// Imports used only by the thread-wait path (`WaitEvents`).
#[cfg(feature = "multithreading")]
use {
    crate::kernel::scheduler::{ExecutionContext, Scheduler},
    crate::sync::atomic::{AtomicBool, Ordering},
    crate::syscall,
    crate::thread::RawThread,
    crate::time::Instant,
    core::pin::Pin,
};

/// Type alias for event mask values, making it easy to change the underlying type
pub type Events = u32;

/// Type alias for atomic event mask values
pub type AtomicEvents = AtomicU32;

/// System-level event constant used by the async executor
pub const EXECUTOR_WAKEUP_EVENT: Events = 1u32 << 28;

/// Default event bit for interrupt OnInterrupt polling
pub const DEFAULT_INTERRUPT_EVENT_BIT: u8 = 29;

/// Error returned by try_wait when events are not immediately available
#[cfg(feature = "multithreading")]
#[derive(Copy, Clone, Debug)]
pub enum TryWaitError {
    /// Would block waiting for events. Contains the events that are currently pending.
    WouldBlock(Events),
}

/// Error returned by wait_until when timeout is reached
#[cfg(feature = "multithreading")]
#[derive(Copy, Clone, Debug)]
pub enum WaitTimeoutError {
    /// Timeout occurred. Contains the events that were received before timeout.
    Timeout(Events),
}

/// Event synchronization primitive for thread communication.
///
/// WaitEvents provides a mechanism for threads to wait for specific events
/// and receive events from other threads.
///
/// # Memory Ordering
///
/// WaitEvents uses optimized atomic operations:
/// - Regular accessors use Acquire/Release ordering for performance
/// - Critical synchronization points use SeqCst for correctness
/// - All operations are thread-safe and lock-free
///
/// # Examples
///
/// Basic event waiting:
/// ```rust,ignore
/// // Create WaitEvents waiting for events 1 and 4
/// let mut context = WaitEvents::with_events(0b10010);
/// let received = context.wait(); // Blocks until both events 1 and 4 are received
/// ```
///
/// Using builder pattern:
/// ```rust,ignore
/// let mut context = WaitEvents::builder()
///     .events(0b1111)
///     .wait_any()        // Wait for ANY event (not all)
///     .keep_unwanted()   // Don't clear unmatched events
///     .build();
/// ```
///
/// Non-blocking check:
/// ```rust,ignore
/// let mut context = WaitEvents::builder()
///     .events(0b0001)
///     .no_wait()
///     .build();
///
/// match context.try_wait() {
///     Ok(events) => println!("Event received: {}", events),
///     Err(TryWaitError::WouldBlock(pending)) => {
///         println!("Would block - pending: {:x}", pending);
///     }
/// }
/// ```
///
/// Wait with timeout:
/// ```rust,ignore
/// use scars::time::{Instant, Duration};
///
/// let mut context = WaitEvents::with_events(0b1010);
/// let deadline = Instant::now() + Duration::from_millis(100);
///
/// match context.wait_until(deadline) {
///     Ok(()) => println!("Events received in time!"),
///     Err(WaitTimeoutError::Timeout(received)) => {
///         println!("Timeout - received: {:x}", received);
///     }
/// }
/// ```
#[cfg(feature = "multithreading")]
#[derive(Debug)]
pub struct WaitEvents {
    // Configuration (set by user)
    waited_events: AtomicEvents,
    options: EventOptions,

    // Results (set by kernel)
    returned_events: AtomicEvents,
    timed_out: AtomicBool,
}

#[cfg(feature = "multithreading")]
impl WaitEvents {
    pub const fn new() -> Self {
        Self {
            waited_events: AtomicEvents::new(0),
            options: EventOptions::empty(), // Default: wait for ALL, blocking, clear unwanted
            returned_events: AtomicEvents::new(0),
            timed_out: AtomicBool::new(false),
        }
    }

    pub fn with_events(events: Events) -> Self {
        let context = Self::new();
        context.set_events(events);
        context
    }

    /// Create WaitEvents with specified events and options
    pub fn with_options(events: Events, options: EventOptions) -> Self {
        Self {
            waited_events: AtomicEvents::new(events),
            options,
            returned_events: AtomicEvents::new(0),
            timed_out: AtomicBool::new(false),
        }
    }

    /// Create a builder for constructing WaitEvents with fluent API
    pub fn builder() -> WaitEventsBuilder {
        WaitEventsBuilder::new()
    }

    pub fn set_events(&self, events: Events) {
        self.waited_events.store(events, Ordering::Release);
    }

    pub fn events(&self) -> Events {
        self.waited_events.load(Ordering::Acquire)
    }

    /// Check if this wait context has timed out
    pub(crate) fn is_timed_out(&self) -> bool {
        self.timed_out.load(Ordering::Acquire)
    }

    /// Set the timeout flag (called by kernel when timeout occurs)
    pub(crate) fn set_timed_out(&self) {
        self.timed_out.store(true, Ordering::Release);
    }

    /// Get current event options
    pub fn options(&self) -> EventOptions {
        self.options
    }

    /// Set event options (not thread-safe, should be called before waiting)
    pub fn set_options(&mut self, options: EventOptions) {
        self.options = options;
    }

    /// Check if waiting for ANY events (true) or ALL events (false)
    pub fn wait_any(&self) -> bool {
        self.options.wait_any_enabled()
    }

    /// Check if this is a non-blocking wait
    pub fn no_wait(&self) -> bool {
        self.options.no_wait_enabled()
    }

    /// Check if unwanted events should be kept pending
    pub fn keep_unwanted(&self) -> bool {
        self.options.keep_unwanted_enabled()
    }

    /// Check if all pending events should be returned and cleared
    pub fn return_all(&self) -> bool {
        self.options.return_all_enabled()
    }

    #[allow(dead_code)]
    pub(crate) fn clear_waited_events(&self) {
        self.waited_events.store(0, Ordering::Release);
    }

    /// Get and clear the events that were determined by kernel to be returned
    pub(crate) fn take_returned_events(&self) -> Events {
        self.returned_events.swap(0, Ordering::SeqCst)
    }

    /// Mask of a thread's pending events this wait takes when it completes.
    ///
    /// `return_all` takes everything pending; otherwise only the waited events.
    fn consume_mask(&self) -> Events {
        if self.options.return_all_enabled() {
            Events::MAX
        } else {
            self.events()
        }
    }

    /// Take this wait's events out of `thread`'s pending set and record them
    /// as the events the wait returns.
    pub(crate) fn consume_pending_events(&self, thread: Pin<&RawThread>) -> Events {
        let mask = self.consume_mask();
        let received = thread.pending_events.fetch_and(!mask, Ordering::SeqCst) & mask;
        self.returned_events.store(received, Ordering::SeqCst);
        received
    }

    /// Whether the thread should block, given the events pending for it.
    ///
    /// The complement of [`WaitEvents::should_resume`], which the wake paths
    /// use. A `no_wait` wait never blocks.
    pub(crate) fn should_block(&self, all_pending: Events) -> bool {
        !self.options.no_wait_enabled() && !self.should_resume(all_pending)
    }

    /// Determine if a thread should be resumed based on pending events and wait criteria
    ///
    /// This is used by `send_events()` to decide whether to resume a waiting thread.
    ///
    /// # Arguments
    ///
    /// * `all_pending` - All pending events for the thread (including the newly sent ones)
    ///
    /// # Returns
    ///
    /// Returns `true` if the thread should be resumed, `false` otherwise.
    pub(crate) fn should_resume(&self, all_pending: Events) -> bool {
        let waited_events = self.events();
        let matched = all_pending & waited_events;

        if self.wait_any() {
            matched != 0 // Resume if any requested event is available
        } else {
            matched == waited_events // Resume if all requested events are available
        }
    }

    // Wait operations that make syscalls

    /// Wait for events indefinitely.
    ///
    /// Blocks the current thread until the specified events are received.
    /// The behavior depends on the EventOptions:
    /// - `wait_all()` (default): Waits for ALL specified events
    /// - `wait_any()`: Waits for ANY of the specified events
    ///
    /// The returned events depend on the clearing options:
    /// - **Default**: Returns only the requested events that were received
    /// - **`return_all()`**: Returns all pending events (may include unrequested events)
    /// - **`keep_unwanted()`**: Returns only the matched events (subset of requested events)
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let mut context = WaitEvents::with_events(0b1010); // Wait for events 1 and 3
    /// let received = context.wait(); // Blocks until both events 1 and 3 are received
    /// println!("Received: {:b}", received);
    /// ```
    ///
    /// ```rust,ignore
    /// let mut context = WaitEvents::builder()
    ///     .events(0b1010)
    ///     .wait_any()
    ///     .build();
    /// let received = context.wait(); // Blocks until either event 1 OR event 3 is received
    /// ```
    pub fn wait(&mut self) -> Events {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(current_thread) => {
                // Clear timeout flag before starting new wait operation
                self.timed_out
                    .store(false, crate::sync::atomic::Ordering::SeqCst);

                syscall::thread_wait_event(self as *mut _);

                // After syscall returns, check for any additional events that may have been sent
                // while the thread was blocked/unblocked and return the received events
                let (consumed, _available) = self.finalize_wait(current_thread);
                consumed
            }
            ExecutionContext::Interrupt(_) => {
                // Error: cannot wait in an interrupt handler
                crate::runtime_error!(crate::kernel::RuntimeError::InterruptHandlerViolation);
            }
        }
    }

    /// Wait for events with a timeout deadline.
    ///
    /// Blocks the current thread until either:
    /// - The specified events are received (returns `Ok(Events)`)
    /// - The deadline is reached (returns `Err(WaitTimeoutError::Timeout(received))`)
    ///
    /// The wait behavior depends on EventOptions like `wait()`, and the returned/error
    /// events follow the same clearing rules as documented in the error types.
    ///
    /// # Arguments
    ///
    /// * `deadline` - Absolute time when the wait should timeout
    ///
    /// # Returns
    ///
    /// * `Ok(Events)` - Events were received before the deadline. The returned events depend on EventOptions:
    ///   - **Default**: Only the requested events that were received
    ///   - **`return_all()`**: All pending events that were received (may include unrequested events)
    ///   - **`keep_unwanted()`**: Only the matched events that were received
    /// * `Err(WaitTimeoutError::Timeout(received))` - Deadline was reached. Contains events received before timeout.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// use scars::time::{Instant, Duration};
    ///
    /// let mut context = WaitEvents::with_events(0b0001);
    /// let deadline = Instant::now() + Duration::from_millis(100);
    ///
    /// match context.wait_until(deadline) {
    ///     Ok(events) => {
    ///         println!("Event received: {:b}", events);
    ///     }
    ///     Err(WaitTimeoutError::Timeout) => {
    ///         println!("Timed out waiting for events");
    ///     }
    /// }
    /// ```
    ///
    /// Timeout with partial event reception:
    /// ```rust,ignore
    /// let mut context = WaitEvents::with_events(0b1111); // Wait for events 0,1,2,3
    /// let deadline = Instant::now() + Duration::from_millis(50);
    ///
    /// // Assume only events 0 and 1 arrive before timeout
    /// match context.wait_until(deadline) {
    ///     Ok(events) => {
    ///         println!("All events received: {:b}", events);
    ///     }
    ///     Err(WaitTimeoutError::Timeout(received)) => {
    ///         println!("Timed out - received: {:b}", received);
    ///         // received = events that were received before timeout
    ///     }
    /// }
    /// ```
    ///
    /// Success with `return_all()` option:
    /// ```rust,ignore
    /// let mut context = WaitEvents::builder()
    ///     .events(0b0011) // Wait for events 0,1
    ///     .return_all()
    ///     .build();
    ///
    /// // Assume events 0,1,4 arrive before timeout
    /// match context.wait_until(deadline) {
    ///     Ok(events) => {
    ///         // events = 0b10011 (includes unrequested event 4 due to return_all)
    ///         println!("Got extra events: {:b}", events);
    ///     }
    ///     _ => {}
    /// }
    /// ```
    ///
    /// Timeout with `return_all()` option:
    /// ```rust,ignore
    /// let mut context = WaitEvents::builder()
    ///     .events(0b0011) // Wait for events 0,1
    ///     .return_all()
    ///     .build();
    ///
    /// // Assume only event 4 arrives before timeout (not what we're waiting for)
    /// match context.wait_until(deadline) {
    ///     Err(WaitTimeoutError::Timeout(received)) => {
    ///         // received shows what events were received before timeout
    ///         println!("Error - received: {:b}", received);
    ///     }
    ///     _ => {}
    /// }
    /// ```
    pub fn wait_until(&mut self, deadline: Instant) -> Result<Events, WaitTimeoutError> {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(current_thread) => {
                // Clear timeout flag before starting new wait operation
                self.timed_out
                    .store(false, crate::sync::atomic::Ordering::SeqCst);

                syscall::thread_wait_event_until(self as *mut _, deadline);

                // After syscall returns, check for any additional events that may have been sent
                // while the thread was blocked/unblocked
                let (consumed, _available) = self.finalize_wait(current_thread);

                // Check if we timed out
                if self.is_timed_out() {
                    Err(WaitTimeoutError::Timeout(consumed))
                } else {
                    Ok(consumed)
                }
            }
            ExecutionContext::Interrupt(_) => {
                // Error: cannot wait in an interrupt handler
                crate::runtime_error!(crate::kernel::RuntimeError::InterruptHandlerViolation);
            }
        }
    }

    fn finalize_wait(&self, thread: Pin<&RawThread>) -> (Events, Events) {
        // Clear the current_wait_events pointer now that thread is returning from syscall
        // Keep SeqCst here for thread coordination.
        //
        // A non-null pointer means the wait suspended, and the wake paths only
        // make the thread ready, so its events are still pending. Take them
        // here. A wait that did not suspend consumed in the syscall.
        let suspended = !thread
            .current_wait_events
            .swap(core::ptr::null_mut(), Ordering::SeqCst)
            .is_null();
        if suspended {
            self.consume_pending_events(thread);
        }

        // Get the events that the kernel determined should be returned to the user
        let returned = self.take_returned_events();

        // Peek at current pending events for the available field
        // (These might include events that arrived after kernel's decision)
        let available = thread.pending_events.load(Ordering::SeqCst);

        // Return what kernel said we consumed, plus current available state
        (returned, available)
    }

    /// Try to wait for events without blocking.
    ///
    /// Checks if the specified events are immediately available without blocking
    /// the thread. This is useful for polling-style event checking.
    ///
    /// The returned/error events depend on EventOptions as documented in the error types.
    ///
    /// # Returns
    ///
    /// * `Ok(Events)` - Events were immediately available. The returned events depend on EventOptions:
    ///   - **Default**: Only the requested events that were available
    ///   - **`return_all()`**: All pending events that were available (may include unrequested events)
    ///   - **`keep_unwanted()`**: Only the matched events that were available
    /// * `Err(TryWaitError::WouldBlock(pending))` - Events are not available. Contains currently pending events.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let mut context = WaitEvents::with_events(0b0001);
    ///
    /// match context.try_wait() {
    ///     Ok(events) => {
    ///         println!("Event immediately available: {:b}", events);
    ///     }
    ///     Err(TryWaitError::WouldBlock(pending)) => {
    ///         println!("Would block - pending: {:b}", pending);
    ///         // pending shows all currently pending events
    ///     }
    /// }
    /// ```
    ///
    /// Success with `return_all()` option:
    /// ```rust,ignore
    /// let mut context = WaitEvents::builder()
    ///     .events(0b0011) // Wait for events 0,1
    ///     .return_all()
    ///     .build();
    ///
    /// // Assume events 0,1,4 are immediately available
    /// match context.try_wait() {
    ///     Ok(events) => {
    ///         // events = 0b10011 (includes unrequested event 4 due to return_all)
    ///         println!("Got extra events: {:b}", events);
    ///     }
    ///     _ => {}
    /// }
    /// ```
    ///
    /// Would block with `return_all()` option:
    /// ```rust,ignore
    /// let mut context = WaitEvents::builder()
    ///     .events(0b0011) // Wait for events 0,1
    ///     .return_all()
    ///     .build();
    ///
    /// // Assume only event 4 is pending (not what we're waiting for)
    /// match context.try_wait() {
    ///     Err(TryWaitError::WouldBlock(pending)) => {
    ///         // pending shows all currently pending events
    ///         println!("Error - pending: {:b}", pending);
    ///     }
    ///     _ => {}
    /// }
    /// ```
    ///
    /// Polling loop example:
    /// ```rust,ignore
    /// let mut context = WaitEvents::with_events(0b1111);
    ///
    /// loop {
    ///     match context.try_wait() {
    ///         Ok(events) => {
    ///             println!("Got events: {:b}", events);
    ///             break;
    ///         }
    ///         Err(TryWaitError::WouldBlock(_)) => {
    ///             // Do other work while waiting
    ///             // Could also check if available events contains useful data
    ///             do_other_work();
    ///         }
    ///     }
    /// }
    /// ```
    pub fn try_wait(&mut self) -> Result<Events, TryWaitError> {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(current_thread) => {
                // Clear timeout flag before starting new wait operation
                self.timed_out
                    .store(false, crate::sync::atomic::Ordering::SeqCst);

                // Make syscall with immediate deadline (try without blocking)
                syscall::thread_wait_event_until(self as *mut _, crate::time::Instant::now());

                // Check if we got events or would block
                let (consumed, available) = self.finalize_wait(current_thread);

                if self.is_timed_out() {
                    // Would block - return what was available
                    Err(TryWaitError::WouldBlock(available))
                } else {
                    // Got events - return what was consumed
                    Ok(consumed)
                }
            }
            ExecutionContext::Interrupt(_current_interrupt) => {
                unreachable!("Cannot wait events in interrupt context");
            }
        }
    }

    /// Peek at pending events without consuming them.
    ///
    /// This method returns all currently pending events for the current thread
    /// that match the waited events, without clearing any events. This is
    /// useful for checking event availability before deciding how to proceed.
    ///
    /// # Examples
    ///
    /// ```rust,ignore
    /// let context = WaitEvents::with_events(0b0101);
    ///
    /// // Check what events are pending without consuming them
    /// let pending = context.peek();
    /// if pending & HIGH_PRIORITY_EVENT != 0 {
    ///     // Handle high priority events immediately
    ///     let events = context.wait();
    /// } else if pending != 0 {
    ///     // Some events are available, but maybe defer processing
    ///     do_other_work();
    /// } else {
    ///     // No events pending, safe to block
    ///     let events = context.wait();
    /// }
    /// ```
    pub fn peek(&self) -> Events {
        match Scheduler::current_execution_context() {
            ExecutionContext::Thread(current_thread) => {
                let events = self.events();
                let all_pending = current_thread.peek_pending_events();

                // Return only the events we're waiting for that are currently pending
                all_pending & events
            }
            ExecutionContext::Interrupt(_current_interrupt) => {
                unreachable!("Cannot peek events from an interrupt context")
            }
        }
    }
}

#[cfg(feature = "multithreading")]
unsafe impl Sync for WaitEvents {}
#[cfg(feature = "multithreading")]
unsafe impl Send for WaitEvents {}

// Typestate markers for builder
#[cfg(feature = "multithreading")]
mod wait_events_state {
    pub struct CanKeepUnwanted;
    pub struct CannotKeepUnwanted;
    pub struct CanReturnAll;
    pub struct CannotReturnAll;
}

/// Builder for constructing WaitEvents
///
/// The builder uses typestates to prevent conflicting options at compile time.
/// For example, `keep_unwanted()` and `return_all()` are mutually exclusive
/// and cannot be chained together.
///
/// # Examples
///
/// ```rust,ignore
/// // Valid combinations
/// let context1 = WaitEvents::builder()
///     .events(0x05)
///     .wait_any()
///     .keep_unwanted()
///     .build();
///
/// let context2 = WaitEvents::builder()
///     .events(0x03)
///     .return_all()
///     .build();
///
/// // This would NOT compile:
/// // let invalid = WaitEvents::builder()
/// //     .keep_unwanted()
/// //     .return_all()  // Error! Cannot use both
/// //     .build();
/// ```
#[cfg(feature = "multithreading")]
pub struct WaitEventsBuilder<
    KeepState = wait_events_state::CanKeepUnwanted,
    ReturnState = wait_events_state::CanReturnAll,
> {
    events: Events,
    options: EventOptions,
    _keep_state: core::marker::PhantomData<KeepState>,
    _return_state: core::marker::PhantomData<ReturnState>,
}

#[cfg(feature = "multithreading")]
impl WaitEventsBuilder<wait_events_state::CanKeepUnwanted, wait_events_state::CanReturnAll> {
    const fn new() -> Self {
        Self {
            events: 0,
            options: EventOptions::empty(),
            _keep_state: core::marker::PhantomData,
            _return_state: core::marker::PhantomData,
        }
    }
}

#[cfg(feature = "multithreading")]
impl<K, R> WaitEventsBuilder<K, R> {
    /// Set the events to wait for
    pub const fn events(mut self, events: Events) -> Self {
        self.events = events;
        self
    }

    /// Wait for ANY of the specified events
    pub const fn wait_any(mut self) -> Self {
        self.options = self.options.union(EventOptions::WAIT_ANY);
        self
    }

    /// Wait for ALL of the specified events (default)
    pub const fn wait_all(mut self) -> Self {
        // Remove WAIT_ANY flag if present
        self.options =
            EventOptions::from_bits_truncate(self.options.bits() & !EventOptions::WAIT_ANY.bits());
        self
    }

    /// Non-blocking check for events
    pub const fn no_wait(mut self) -> Self {
        self.options = self
            .options
            .union(EventOptions::NO_WAIT)
            .union(EventOptions::WAIT_ANY);
        self
    }

    /// Build the WaitEvents
    pub const fn build(self) -> WaitEvents {
        WaitEvents {
            waited_events: AtomicEvents::new(self.events),
            options: self.options,
            returned_events: AtomicEvents::new(0),
            timed_out: AtomicBool::new(false),
        }
    }
}

// Methods that transition typestates
#[cfg(feature = "multithreading")]
impl<R> WaitEventsBuilder<wait_events_state::CanKeepUnwanted, R> {
    /// Keep unwanted events pending instead of clearing them
    /// This prevents using return_all() afterwards
    pub const fn keep_unwanted(
        self,
    ) -> WaitEventsBuilder<wait_events_state::CannotKeepUnwanted, wait_events_state::CannotReturnAll>
    {
        WaitEventsBuilder {
            events: self.events,
            options: self.options.union(EventOptions::KEEP_UNWANTED),
            _keep_state: core::marker::PhantomData,
            _return_state: core::marker::PhantomData,
        }
    }
}

#[cfg(feature = "multithreading")]
impl<K> WaitEventsBuilder<K, wait_events_state::CanReturnAll> {
    /// Return and clear all pending events
    /// This prevents using keep_unwanted() afterwards
    pub const fn return_all(
        self,
    ) -> WaitEventsBuilder<wait_events_state::CannotKeepUnwanted, wait_events_state::CannotReturnAll>
    {
        WaitEventsBuilder {
            events: self.events,
            options: self
                .options
                .union(EventOptions::RETURN_ALL)
                .union(EventOptions::WAIT_ANY),
            _keep_state: core::marker::PhantomData,
            _return_state: core::marker::PhantomData,
        }
    }
}

// These tests construct `WaitEvents`, which only exists with `multithreading`.
#[cfg(all(test, feature = "multithreading"))]
mod tests {
    use super::*;

    #[test_case]
    fn test_event_options_constructors() {
        // Test default
        let default_opts = EventOptions::default();
        assert!(!default_opts.wait_any_enabled());
        assert!(!default_opts.no_wait_enabled());
        assert!(!default_opts.keep_unwanted_enabled());
        assert!(!default_opts.return_all_enabled());

        // Test wait_any
        let wait_any_opts = EventOptions::wait_any();
        assert!(wait_any_opts.wait_any_enabled());
        assert!(!wait_any_opts.no_wait_enabled());
        assert!(!wait_any_opts.keep_unwanted_enabled());
        assert!(!wait_any_opts.return_all_enabled());

        // Test wait_all
        let wait_all_opts = EventOptions::wait_all();
        assert!(!wait_all_opts.wait_any_enabled());
        assert!(!wait_all_opts.no_wait_enabled());
        assert!(!wait_all_opts.keep_unwanted_enabled());
        assert!(!wait_all_opts.return_all_enabled());

        // Test no_wait
        let no_wait_opts = EventOptions::no_wait();
        assert!(no_wait_opts.wait_any_enabled());
        assert!(no_wait_opts.no_wait_enabled());
        assert!(!no_wait_opts.keep_unwanted_enabled());
        assert!(!no_wait_opts.return_all_enabled());

        // Test keep_unwanted chaining
        let keep_unwanted_opts = EventOptions::wait_any().keep_unwanted();
        assert!(keep_unwanted_opts.wait_any_enabled());
        assert!(!keep_unwanted_opts.no_wait_enabled());
        assert!(keep_unwanted_opts.keep_unwanted_enabled());
        assert!(!keep_unwanted_opts.return_all_enabled());

        // Test return_all
        let return_all_opts = EventOptions::return_all();
        assert!(return_all_opts.wait_any_enabled());
        assert!(!return_all_opts.no_wait_enabled());
        assert!(!return_all_opts.keep_unwanted_enabled());
        assert!(return_all_opts.return_all_enabled());
    }

    #[test_case]
    fn test_context_construction() {
        // Test new()
        let context = WaitEvents::new();
        assert_eq!(context.events(), 0);
        assert!(!context.wait_any());
        assert!(!context.no_wait());
        assert!(!context.keep_unwanted());
        assert!(!context.return_all());

        // Test with_events()
        let context = WaitEvents::with_events(0x0F);
        assert_eq!(context.events(), 0x0F);
        assert!(!context.wait_any());

        // Test with_options()
        let opts = EventOptions::wait_any().keep_unwanted();
        let context = WaitEvents::with_options(0x05, opts);
        assert_eq!(context.events(), 0x05);
        assert!(context.wait_any());
        assert!(context.keep_unwanted());
        assert!(!context.no_wait());
        assert!(!context.return_all());
    }

    #[test_case]
    fn test_context_basic_operations() {
        let context = WaitEvents::new();

        // Test set_events
        context.set_events(0x12);
        assert_eq!(context.events(), 0x12);
    }

    #[test_case]
    fn test_context_options_accessors() {
        let mut context = WaitEvents::new();

        // Initial state
        assert!(!context.wait_any());
        assert!(!context.no_wait());
        assert!(!context.keep_unwanted());
        assert!(!context.return_all());

        // Test set_options
        let opts = EventOptions::wait_any().keep_unwanted();
        context.set_options(opts);
        assert!(context.wait_any());
        assert!(!context.no_wait());
        assert!(context.keep_unwanted());
        assert!(!context.return_all());

        // Test options() getter
        let retrieved_opts = context.options();
        assert!(retrieved_opts.wait_any_enabled());
        assert!(!retrieved_opts.no_wait_enabled());
        assert!(retrieved_opts.keep_unwanted_enabled());
        assert!(!retrieved_opts.return_all_enabled());
    }

    #[test_case]
    fn test_event_options_combinations() {
        // Test valid combinations
        let opts1 = EventOptions::wait_all().keep_unwanted();
        assert!(!opts1.wait_any_enabled());
        assert!(opts1.keep_unwanted_enabled());

        let opts2 = EventOptions::no_wait(); // Implies wait_any
        assert!(opts2.wait_any_enabled());
        assert!(opts2.no_wait_enabled());

        let opts3 = EventOptions::return_all(); // Implies wait_any
        assert!(opts3.wait_any_enabled());
        assert!(opts3.return_all_enabled());

        // Test that EventOptions is Copy and Clone
        let opts4 = opts1;
        assert_eq!(opts4.keep_unwanted_enabled(), opts1.keep_unwanted_enabled());

        let opts5 = opts1.clone();
        assert_eq!(opts5.wait_any_enabled(), opts1.wait_any_enabled());

        // Test bitwise operations
        let opts6 = EventOptions::WAIT_ANY | EventOptions::KEEP_UNWANTED;
        assert!(opts6.wait_any_enabled());
        assert!(opts6.keep_unwanted_enabled());
        assert!(!opts6.no_wait_enabled());
    }

    #[test_case]
    fn test_context_edge_cases() {
        // Test with maximum event values
        let context = WaitEvents::with_events(0xFFFFFFFF);
        assert_eq!(context.events(), 0xFFFFFFFF);

        // Test with zero events
        let context = WaitEvents::with_events(0);
        assert_eq!(context.events(), 0);
    }

    #[test_case]
    fn test_clear_waited_events() {
        let context = WaitEvents::with_events(0x07);

        // Clear waited events (internal method)
        context.clear_waited_events();
        assert_eq!(context.events(), 0);
    }

    #[test_case]
    fn test_event_options_debug_and_clone() {
        let opts = EventOptions::wait_any().keep_unwanted();

        // Test Clone trait
        let cloned_opts = opts.clone();
        assert_eq!(cloned_opts.wait_any_enabled(), opts.wait_any_enabled());
        assert_eq!(
            cloned_opts.keep_unwanted_enabled(),
            opts.keep_unwanted_enabled()
        );
        assert_eq!(cloned_opts.no_wait_enabled(), opts.no_wait_enabled());
        assert_eq!(cloned_opts.return_all_enabled(), opts.return_all_enabled());

        // Test Copy trait (implicit via assignment)
        let copied_opts = opts;
        assert_eq!(copied_opts.wait_any_enabled(), opts.wait_any_enabled());

        // Test bitflags operations
        assert!(opts.contains(EventOptions::WAIT_ANY));
        assert!(opts.contains(EventOptions::KEEP_UNWANTED));
        assert!(!opts.contains(EventOptions::NO_WAIT));
        assert!(opts.intersects(EventOptions::WAIT_ANY | EventOptions::NO_WAIT));
    }

    #[test_case]
    fn test_builder_pattern() {
        // Test basic builder usage
        let context = WaitEvents::builder().events(0x0F).wait_any().build();

        assert_eq!(context.events(), 0x0F);
        assert!(context.wait_any());
        assert!(!context.no_wait());
        assert!(!context.keep_unwanted());
        assert!(!context.return_all());

        // Test keep_unwanted path
        let context = WaitEvents::builder()
            .events(0x05)
            .wait_all()
            .keep_unwanted()
            .build();

        assert_eq!(context.events(), 0x05);
        assert!(!context.wait_any());
        assert!(context.keep_unwanted());
        assert!(!context.return_all());

        // Test return_all path
        let context = WaitEvents::builder().events(0x03).return_all().build();

        assert_eq!(context.events(), 0x03);
        assert!(context.wait_any()); // return_all implies wait_any
        assert!(!context.keep_unwanted());
        assert!(context.return_all());

        // Test no_wait
        let context = WaitEvents::builder().events(0x01).no_wait().build();

        assert_eq!(context.events(), 0x01);
        assert!(context.wait_any()); // no_wait implies wait_any
        assert!(context.no_wait());
    }

    #[test_case]
    fn test_builder_typestate_validation() {
        // These should compile - valid combinations
        let _valid1 = WaitEvents::builder().events(0x01).keep_unwanted().build();
        let _valid2 = WaitEvents::builder().events(0x01).return_all().build();
        let _valid3 = WaitEvents::builder().events(0x01).wait_any().build();

        // The following would NOT compile due to typestate constraints:
        // let _invalid = WaitEvents::builder().keep_unwanted().return_all().build(); // Error!
        // let _invalid = WaitEvents::builder().return_all().keep_unwanted().build(); // Error!

        // Test that we can chain other methods after typestate transitions
        let _valid4 = WaitEvents::builder()
            .events(0x02)
            .wait_any()
            .keep_unwanted()
            .build();

        let _valid5 = WaitEvents::builder()
            .events(0x04)
            .wait_all()
            .return_all()
            .build();
    }

    #[test_case]
    fn test_peek_functionality() {
        let context = WaitEvents::with_events(0x0F);

        // Initially no events pending, peek should return 0
        assert_eq!(context.peek(), 0);

        // Test that peek returns only the events we're waiting for
        // Note: In actual usage, events would be sent by other threads
        // For this test, we'll simulate the scenario by verifying the filtering logic
        assert_eq!(context.events(), 0x0F);

        // Test that peek is non-consuming - we can call it multiple times
        assert_eq!(context.peek(), 0);
        assert_eq!(context.peek(), 0);
    }

    #[test_case]
    fn test_consume_mask() {
        // Default: only the waited events are taken, the rest stay pending
        let context = WaitEvents::with_events(0b1010); // Wait for events 1 and 3
        assert_eq!(context.consume_mask(), 0b1010);

        // return_all: everything pending is taken
        let context = WaitEvents::with_options(0b0011, EventOptions::return_all());
        assert_eq!(context.consume_mask(), Events::MAX);

        // keep_unwanted takes the same set as the default
        let context = WaitEvents::with_options(0b0101, EventOptions::wait_any().keep_unwanted());
        assert_eq!(context.consume_mask(), 0b0101);
    }

    #[test_case]
    fn test_take_returned_events_leaves_zero() {
        // Every wait ends by taking this, which leaves it zero for the next one
        let context = WaitEvents::with_events(0b1010);
        context.returned_events.store(0b1010, Ordering::SeqCst);

        assert_eq!(context.take_returned_events(), 0b1010);
        assert_eq!(context.take_returned_events(), 0);
    }

    #[test_case]
    fn test_should_block() {
        // wait_all blocks until every waited event is pending
        let context = WaitEvents::with_events(0b1010); // Wait for events 1 and 3
        assert!(context.should_block(0b0000)); // Nothing pending
        assert!(context.should_block(0b0010)); // Only event 1 pending
        assert!(!context.should_block(0b1010)); // Both pending
        assert!(!context.should_block(0b1111)); // Both pending, plus others

        // wait_any blocks only while none of the waited events is pending
        let context = WaitEvents::with_options(0b1010, EventOptions::wait_any());
        assert!(context.should_block(0b0101)); // Only unwaited events pending
        assert!(!context.should_block(0b0010)); // Event 1 pending

        // return_all changes what a wait takes, not when it is satisfied
        let context = WaitEvents::with_options(0b0011, EventOptions::return_all());
        assert!(context.should_block(0b0100)); // Only an unwaited event pending
        assert!(!context.should_block(0b0110)); // Event 1 pending

        // no_wait never blocks
        let context = WaitEvents::builder().events(0b0011).no_wait().build();
        assert!(!context.should_block(0b0000));

        // Blocking is the complement of resuming, for every pending set
        let context = WaitEvents::with_events(0b1010);
        for pending in 0..0b1_0000 {
            assert_eq!(
                context.should_block(pending),
                !context.should_resume(pending)
            );
        }
    }

    #[test_case]
    fn test_should_wake() {
        // Test wait_all behavior (default)
        let context = WaitEvents::with_events(0b1010); // Wait for events 1 and 3
        assert!(!context.should_resume(0b0000)); // No events pending
        assert!(!context.should_resume(0b0010)); // Only event 1 pending
        assert!(!context.should_resume(0b1000)); // Only event 3 pending
        assert!(!context.should_resume(0b0111)); // Events 0,1,2 pending (missing event 3)
        assert!(context.should_resume(0b1010)); // Exact events 1,3 pending
        assert!(context.should_resume(0b1111)); // All events pending (includes 1,3)

        // Test wait_any behavior
        let context = WaitEvents::with_options(0b1010, EventOptions::wait_any()); // Wait for events 1 or 3
        assert!(!context.should_resume(0b0000)); // No events pending
        assert!(!context.should_resume(0b0101)); // Events 0,2 pending (neither 1 nor 3)
        assert!(context.should_resume(0b0010)); // Event 1 pending (matches)
        assert!(context.should_resume(0b1000)); // Event 3 pending (matches)
        assert!(context.should_resume(0b1010)); // Both events 1,3 pending
        assert!(context.should_resume(0b1111)); // All events pending

        // Test edge case: waiting for no events
        let context = WaitEvents::with_events(0b0000);
        assert!(context.should_resume(0b0000)); // Should wake immediately (wait for 0 events)
        assert!(context.should_resume(0b1111)); // Should still wake (0 events are available)

        // Test single event scenarios
        let context = WaitEvents::with_events(0b0001); // Wait for event 0
        assert!(!context.should_resume(0b0000)); // Event 0 not pending
        assert!(!context.should_resume(0b1110)); // Events 1,2,3 pending (missing event 0)
        assert!(context.should_resume(0b0001)); // Event 0 pending
        assert!(context.should_resume(0b1111)); // All events pending (includes event 0)
    }

    #[test_case]
    fn test_event_options_builder_basic() {
        // Test basic builder patterns
        let opts1 = EventOptions::builder().wait_any().build();
        assert!(opts1.wait_any_enabled());
        assert!(!opts1.no_wait_enabled());
        assert!(!opts1.keep_unwanted_enabled());
        assert!(!opts1.return_all_enabled());

        let opts2 = EventOptions::builder().wait_all().build();
        assert!(!opts2.wait_any_enabled()); // wait_all means WAIT_ANY flag is off
        assert!(!opts2.no_wait_enabled());
        assert!(!opts2.keep_unwanted_enabled());
        assert!(!opts2.return_all_enabled());

        let opts3 = EventOptions::builder().return_all().build();
        assert!(opts3.wait_any_enabled()); // return_all forces wait_any
        assert!(opts3.return_all_enabled());
        assert!(!opts3.no_wait_enabled());
        assert!(!opts3.keep_unwanted_enabled());
    }

    #[test_case]
    fn test_event_options_builder_combinations() {
        // Test valid combinations
        let opts1 = EventOptions::builder().wait_any().keep_unwanted().build();
        assert!(opts1.wait_any_enabled());
        assert!(opts1.keep_unwanted_enabled());
        assert!(!opts1.return_all_enabled());
        assert!(!opts1.no_wait_enabled());

        let opts2 = EventOptions::builder().wait_any().no_wait().build();
        assert!(opts2.wait_any_enabled());
        assert!(opts2.no_wait_enabled());
        assert!(!opts2.keep_unwanted_enabled());
        assert!(!opts2.return_all_enabled());

        let opts3 = EventOptions::builder().return_all().no_wait().build();
        assert!(opts3.wait_any_enabled());
        assert!(opts3.return_all_enabled());
        assert!(opts3.no_wait_enabled());
        assert!(!opts3.keep_unwanted_enabled());

        let opts4 = EventOptions::builder().wait_all().keep_unwanted().build();
        assert!(!opts4.wait_any_enabled());
        assert!(opts4.keep_unwanted_enabled());
        assert!(!opts4.return_all_enabled());
        assert!(!opts4.no_wait_enabled());
    }

    #[test_case]
    fn test_event_options_builder_equivalence() {
        // Test that builder produces same results as legacy constructors
        let legacy_wait_any = EventOptions::wait_any();
        let builder_wait_any = EventOptions::builder().wait_any().build();
        assert_eq!(legacy_wait_any, builder_wait_any);

        let legacy_wait_all = EventOptions::wait_all();
        let builder_wait_all = EventOptions::builder().wait_all().build();
        assert_eq!(legacy_wait_all, builder_wait_all);

        let legacy_no_wait = EventOptions::no_wait();
        let builder_no_wait = EventOptions::builder().wait_any().no_wait().build();
        assert_eq!(legacy_no_wait, builder_no_wait);

        let legacy_return_all = EventOptions::return_all();
        let builder_return_all = EventOptions::builder().return_all().build();
        assert_eq!(legacy_return_all, builder_return_all);
    }

    #[test_case]
    fn test_event_options_builder_type_safety() {
        // These should compile fine
        let _valid1 = EventOptions::builder().wait_any().keep_unwanted().build();
        let _valid2 = EventOptions::builder().wait_any().return_all().build();
        let _valid3 = EventOptions::builder().return_all().no_wait().build();
        let _valid4 = EventOptions::builder().wait_all().keep_unwanted().build();

        // Build with defaults
        let _valid5 = EventOptions::builder().build();

        // The following would NOT compile due to typestate constraints:
        // let _invalid1 = EventOptions::builder().wait_all().no_wait().build(); // no_wait requires wait_any
        // let _invalid2 = EventOptions::builder().wait_all().return_all().build(); // return_all requires wait_any
        // let _invalid3 = EventOptions::builder().keep_unwanted().return_all().build(); // mutually exclusive
        // let _invalid4 = EventOptions::builder().return_all().keep_unwanted().build(); // mutually exclusive

        // Test that we can't call return_all after keep_unwanted
        let _intermediate = EventOptions::builder().wait_any().keep_unwanted();
        // _intermediate.return_all(); // This would not compile

        // Test that we can't call keep_unwanted after return_all
        let _intermediate = EventOptions::builder().return_all();
        // _intermediate.keep_unwanted(); // This would not compile
    }

    #[test_case]
    fn test_event_options_builder_chaining() {
        // Test various chaining orders
        let opts1 = EventOptions::builder()
            .wait_any()
            .keep_unwanted()
            .no_wait()
            .build();
        assert!(opts1.wait_any_enabled());
        assert!(opts1.keep_unwanted_enabled());
        assert!(opts1.no_wait_enabled());
        assert!(!opts1.return_all_enabled());

        let opts2 = EventOptions::builder().return_all().no_wait().build();
        assert!(opts2.wait_any_enabled()); // forced by return_all
        assert!(opts2.return_all_enabled());
        assert!(opts2.no_wait_enabled());
        assert!(!opts2.keep_unwanted_enabled());
    }
}
