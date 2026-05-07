use bitflags::bitflags;
use core::fmt;

bitflags! {
    /// Event options controlling wait and clearing behavior.
    ///
    /// Use the builder pattern for compile-time safety:
    ///
    /// # Examples
    ///
    /// Basic usage with builder pattern:
    /// ```rust,ignore
    /// // Wait for any of the specified events (default waits for all)
    /// let opts = EventOptions::builder().wait_any().build();
    ///
    /// // Non-blocking check for any events
    /// let opts = EventOptions::builder().wait_any().no_wait().build();
    ///
    /// // Keep unmatched events pending
    /// let opts = EventOptions::builder().wait_any().keep_unwanted().build();
    ///
    /// // Get all pending events and clear them
    /// let opts = EventOptions::builder().return_all().build();
    ///
    /// // Combined: return all events with no blocking
    /// let opts = EventOptions::builder().return_all().no_wait().build();
    /// ```
    ///
    /// The builder prevents invalid combinations at compile time:
    /// ```rust,ignore
    /// // These won't compile:
    /// // EventOptions::builder().wait_all().no_wait().build(); // no_wait requires wait_any
    /// // EventOptions::builder().keep_unwanted().return_all().build(); // mutually exclusive
    /// ```
    ///
    /// Simple constructors:
    /// ```rust,ignore
    /// // Direct constructor methods
    /// let opts = EventOptions::wait_any();
    /// let opts = EventOptions::no_wait();
    ///
    /// // Manual flag combination (not recommended)
    /// let opts = EventOptions::WAIT_ANY | EventOptions::KEEP_UNWANTED;
    /// ```
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct EventOptions: u8 {
        /// Wait for ANY event (vs ALL events - default)
        const WAIT_ANY = 0b0001;

        /// Don't block, return immediately (non-blocking check)
        const NO_WAIT = 0b0010;

        /// Keep unwanted/unreceived events pending instead of clearing them
        const KEEP_UNWANTED = 0b0100;

        /// Return all pending events and clear them all
        const RETURN_ALL = 0b1000;
    }
}

impl Default for EventOptions {
    fn default() -> Self {
        Self::empty() // All flags off = wait for ALL events, blocking, clear unwanted
    }
}

impl EventOptions {
    /// Wait for ANY of the specified events
    ///
    /// For more complex combinations, use `EventOptions::builder().wait_any()...`
    pub const fn wait_any() -> Self {
        Self::builder().wait_any().build()
    }

    /// Wait for ALL of the specified events (default behavior)
    ///
    /// For more complex combinations, use `EventOptions::builder().wait_all()...`
    pub const fn wait_all() -> Self {
        Self::builder().wait_all().build()
    }

    /// Non-blocking check for events (implies WAIT_ANY)
    ///
    /// For more complex combinations, use `EventOptions::builder().wait_any().no_wait()...`
    pub const fn no_wait() -> Self {
        Self::builder().wait_any().no_wait().build()
    }

    /// Keep unwanted events pending (can be combined with other options)
    ///
    /// **Alternative**: Can also use the builder pattern:
    /// ```rust,ignore
    /// EventOptions::builder().wait_any().keep_unwanted().build()
    /// ```
    pub const fn keep_unwanted(self) -> Self {
        self.union(Self::KEEP_UNWANTED)
    }

    /// Return and clear all pending events (implies WAIT_ANY)
    ///
    /// For more complex combinations, use `EventOptions::builder().return_all()...`
    pub const fn return_all() -> Self {
        Self::builder().return_all().build()
    }

    // Convenience accessors

    /// Check if waiting for ANY events (true) or ALL events (false)
    pub const fn wait_any_enabled(self) -> bool {
        self.contains(Self::WAIT_ANY)
    }

    /// Check if this is a non-blocking wait
    pub const fn no_wait_enabled(self) -> bool {
        self.contains(Self::NO_WAIT)
    }

    /// Check if unwanted events should be kept pending
    pub const fn keep_unwanted_enabled(self) -> bool {
        self.contains(Self::KEEP_UNWANTED)
    }

    /// Check if all pending events should be returned and cleared
    pub const fn return_all_enabled(self) -> bool {
        self.contains(Self::RETURN_ALL)
    }
}

// Typestate markers for EventOptions builder
mod event_options_state {
    pub struct Unset;
    pub struct WaitAny;
    pub struct WaitAll;
    pub struct NoWait;
    pub struct KeepUnwanted;
    pub struct ReturnAll;
}

/// Builder for EventOptions with compile-time validation
///
/// Prevents invalid combinations like `keep_unwanted() + return_all()` at compile time.
///
/// # Examples
///
/// ```rust,ignore
/// // Basic patterns
/// let opts = EventOptions::builder().wait_any().build();
/// let opts = EventOptions::builder().wait_all().no_wait().build(); // Compile error!
/// let opts = EventOptions::builder().return_all().keep_unwanted().build(); // Compile error!
///
/// // Valid combinations
/// let opts = EventOptions::builder().wait_any().keep_unwanted().build();
/// let opts = EventOptions::builder().return_all().build(); // Implies wait_any
/// ```
pub struct EventOptionsBuilder<
    WaitMode = event_options_state::Unset,
    ClearMode = event_options_state::Unset,
    BlockMode = event_options_state::Unset,
> {
    options: EventOptions,
    _phantom: core::marker::PhantomData<(WaitMode, ClearMode, BlockMode)>,
}

impl EventOptions {
    /// Create a new EventOptions builder
    pub const fn builder() -> EventOptionsBuilder {
        EventOptionsBuilder {
            options: EventOptions::empty(),
            _phantom: core::marker::PhantomData,
        }
    }
}

// Initial state - can choose any mode
impl EventOptionsBuilder {
    /// Wait for ANY of the specified events
    pub const fn wait_any(
        self,
    ) -> EventOptionsBuilder<
        event_options_state::WaitAny,
        event_options_state::Unset,
        event_options_state::Unset,
    > {
        EventOptionsBuilder {
            options: self.options.union(EventOptions::WAIT_ANY),
            _phantom: core::marker::PhantomData,
        }
    }

    /// Wait for ALL of the specified events (default behavior)
    pub const fn wait_all(
        self,
    ) -> EventOptionsBuilder<
        event_options_state::WaitAll,
        event_options_state::Unset,
        event_options_state::Unset,
    > {
        EventOptionsBuilder {
            options: self.options, // WAIT_ANY flag off means wait for all
            _phantom: core::marker::PhantomData,
        }
    }

    /// Return and clear all pending events (forces wait_any mode)
    pub const fn return_all(
        self,
    ) -> EventOptionsBuilder<
        event_options_state::WaitAny,
        event_options_state::ReturnAll,
        event_options_state::Unset,
    > {
        EventOptionsBuilder {
            options: self
                .options
                .union(EventOptions::RETURN_ALL)
                .union(EventOptions::WAIT_ANY),
            _phantom: core::marker::PhantomData,
        }
    }

    /// Build with default options (wait_all, blocking, clear unwanted)
    pub const fn build(self) -> EventOptions {
        self.options
    }
}

// From WaitAny state
impl EventOptionsBuilder<event_options_state::WaitAny> {
    /// Keep unwanted events pending
    pub const fn keep_unwanted(
        self,
    ) -> EventOptionsBuilder<
        event_options_state::WaitAny,
        event_options_state::KeepUnwanted,
        event_options_state::Unset,
    > {
        EventOptionsBuilder {
            options: self.options.union(EventOptions::KEEP_UNWANTED),
            _phantom: core::marker::PhantomData,
        }
    }

    /// Return and clear all pending events
    pub const fn return_all(
        self,
    ) -> EventOptionsBuilder<
        event_options_state::WaitAny,
        event_options_state::ReturnAll,
        event_options_state::Unset,
    > {
        EventOptionsBuilder {
            options: self.options.union(EventOptions::RETURN_ALL),
            _phantom: core::marker::PhantomData,
        }
    }

    /// Non-blocking check for events
    pub const fn no_wait(
        self,
    ) -> EventOptionsBuilder<
        event_options_state::WaitAny,
        event_options_state::Unset,
        event_options_state::NoWait,
    > {
        EventOptionsBuilder {
            options: self.options.union(EventOptions::NO_WAIT),
            _phantom: core::marker::PhantomData,
        }
    }

    pub const fn build(self) -> EventOptions {
        self.options
    }
}

// From WaitAll state - more restricted
impl EventOptionsBuilder<event_options_state::WaitAll> {
    /// Keep unwanted events pending
    pub const fn keep_unwanted(
        self,
    ) -> EventOptionsBuilder<
        event_options_state::WaitAll,
        event_options_state::KeepUnwanted,
        event_options_state::Unset,
    > {
        EventOptionsBuilder {
            options: self.options.union(EventOptions::KEEP_UNWANTED),
            _phantom: core::marker::PhantomData,
        }
    }

    // NOTE: no_wait() and return_all() are not available from WaitAll state
    // as they require WAIT_ANY semantics

    pub const fn build(self) -> EventOptions {
        self.options
    }
}

// With KeepUnwanted set - cannot use return_all
impl<W> EventOptionsBuilder<W, event_options_state::KeepUnwanted> {
    pub const fn build(self) -> EventOptions {
        self.options
    }
}

impl<W> EventOptionsBuilder<W, event_options_state::KeepUnwanted, event_options_state::Unset> {
    /// Non-blocking check (only available with WaitAny mode)
    pub const fn no_wait(
        self,
    ) -> EventOptionsBuilder<W, event_options_state::KeepUnwanted, event_options_state::NoWait>
    where
        W: WaitAnyMode,
    {
        EventOptionsBuilder {
            options: self.options.union(EventOptions::NO_WAIT),
            _phantom: core::marker::PhantomData,
        }
    }
}

// With ReturnAll set - cannot use keep_unwanted
impl<W> EventOptionsBuilder<W, event_options_state::ReturnAll> {
    pub const fn build(self) -> EventOptions {
        self.options
    }
}

impl<W> EventOptionsBuilder<W, event_options_state::ReturnAll, event_options_state::Unset> {
    /// Non-blocking check (only available with WaitAny mode)
    pub const fn no_wait(
        self,
    ) -> EventOptionsBuilder<W, event_options_state::ReturnAll, event_options_state::NoWait>
    where
        W: WaitAnyMode,
    {
        EventOptionsBuilder {
            options: self.options.union(EventOptions::NO_WAIT),
            _phantom: core::marker::PhantomData,
        }
    }
}

// With NoWait set
impl<W, C> EventOptionsBuilder<W, C, event_options_state::NoWait> {
    pub const fn build(self) -> EventOptions {
        self.options
    }
}

// Trait to constrain no_wait to WaitAny mode only
pub trait WaitAnyMode {}
impl WaitAnyMode for event_options_state::WaitAny {}

impl fmt::Display for EventOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first;

        // Handle the wait mode (mutually exclusive)
        if self.contains(Self::WAIT_ANY) {
            write!(f, "WAIT_ANY")?;
            first = false;
        } else {
            write!(f, "WAIT_ALL")?;
            first = false;
        }

        // Add other flags
        if self.contains(Self::NO_WAIT) {
            if !first {
                write!(f, " | ")?;
            }
            write!(f, "NO_WAIT")?;
            first = false;
        }

        if self.contains(Self::KEEP_UNWANTED) {
            if !first {
                write!(f, " | ")?;
            }
            write!(f, "KEEP_UNWANTED")?;
            first = false;
        }

        if self.contains(Self::RETURN_ALL) {
            if !first {
                write!(f, " | ")?;
            }
            write!(f, "RETURN_ALL")?;
        }

        Ok(())
    }
}
