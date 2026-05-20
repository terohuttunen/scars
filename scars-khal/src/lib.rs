//! Hardware Abstraction Layer (HAL) for the SCARS kernel.
//!
//! This crate provides a set of traits that define the interface between the SCARS kernel
//! and the underlying hardware. It enables the kernel to be portable across different
//! hardware platforms while maintaining consistent behavior.
//!
//! # Architecture Support
//!
//! The HAL is designed to support multiple architectures:
//! - Cortex-M (ARM) through `scars-khal-stm32f4`
//! - RISC-V through `scars-khal-e310x`
//! - Simulator through `scars-khal-sim`
//!
//! # Key Components
//!
//! - [`InterruptController`]: Manages interrupt handling, priorities, and masking
//!   - Priority-based interrupt handling
//!   - Interrupt threshold control
//!   - Interrupt claiming and completion
//!
//! - [`AlarmClockController`]: Provides timing and scheduling capabilities
//!   - Monotonic clock with configurable frequency
//!   - Wakeup timer functionality
//!
//! - [`CoreController`]: Controls per-core execution and identifies the
//!   calling core
//!   - Thread context management
//!   - System call handling
//!   - Error and exception handling
//!   - Service call mechanism for deferred kernel operations
//!   - Per-core identity (`current_core_id`, `NUM_CORES`) and cross-core
//!     service-call dispatch (`pend_service_call_on`)
//!
//! - [`HardwareAbstractionLayer`]: Combines all controllers into a single interface
//!
//! # Callbacks and Kernel Integration
//!
//! The HAL calls into the kernel through several callback functions at specific points:
//!
//! - [`kernel_wakeup_handler`]: Called by the HAL when the alarm timer triggers
//!   - Implemented by the kernel to handle wakeup events
//!   - Called from interrupt context
//!   - Used for scheduling and timer management
//!
//! - [`kernel_interrupt_handler`]: Called by the HAL when an interrupt is pending
//!   - Implemented by the kernel to handle interrupts
//!   - Called from interrupt context
//!   - The kernel claims and completes interrupts
//!
//! - [`kernel_syscall_handler`]: Called by the HAL when a system call is made
//!   - Implemented by the kernel to handle system calls
//!   - Called from thread context
//!   - Processes system call requests from user threads
//!
//! - [`kernel_exception_handler`]: Called by the HAL when a hardware exception occurs
//!   - Implemented by the kernel to handle faults
//!   - Called from exception context
//!   - Handles platform-independent error processing
//!
//! - [`kernel_service_call_handler`]: Called by the HAL when a service call executes
//!   - Implemented by the kernel to handle deferred operations
//!   - Called when execution flow allows (typically at lowest interrupt priority)
//!   - Used for event processing and context switching
//!
//! # System Startup
//!
//! The system startup sequence follows these steps:
//!
//! 1. HAL initialization:
//!    ```rust
//!    unsafe fn init(hal: *mut Self) {
//!        // Initialize hardware peripherals
//!        // Set up interrupt vectors
//!        // Configure timers
//!    }
//!    ```
//!    - The HAL initializes the hardware interface
//!    - Sets up interrupt handling
//!    - Configures timers and other hardware resources
//!
//! 2. Kernel startup:
//!    - The kernel is started and performs its initialization
//!    - Sets up its internal structures
//!    - Initializes the idle thread
//!    - Calls `start_first_thread` to begin execution
//!
//! 3. First thread execution:
//!    ```rust
//!    fn start_first_thread(idle_context: *mut Self::Context) -> ! {
//!        // Initialize thread context
//!        // Set up stack and registers
//!        // Start execution
//!    }
//!    ```
//!    - Sets up the idle thread context
//!    - Transfers control to the first thread
//!    - The system is now running
//!
//! # Safety
//!
//! This crate is marked as `#![no_std]` and is designed for use in bare-metal environments.
//! Implementations must ensure:
//! - Thread safety and proper synchronization when accessing hardware resources
//! - Proper interrupt handling and masking
//! - Safe context switching
//! - Proper error handling for faults
//!
//! # Interrupt Priority Handling
//!
//! The kernel HAL must guarantee that no hardware interrupts below or at the interrupt threshold
//! are serviced concurrently. This is achieved through the interrupt priority system:
//!
//! - Interrupts with priority less than or equal to the threshold are blocked
//! - Only interrupts with priority higher than the threshold will be serviced
//!
//! The HAL must ensure that:
//! 1. The interrupt threshold is properly maintained across context switches
//! 2. Interrupt priorities are correctly mapped to hardware priority levels
//! 3. No interrupt below the threshold can preempt an interrupt above the threshold
//!
//! When implementing the HAL, you must:
//! - Store the interrupt threshold in the thread context
//! - Restore the threshold when switching contexts
//! - Check interrupt priorities against the threshold before servicing, if
//!   not implemented on hardware.
//!
//! # Usage
//!
//! To use this crate, implement the required traits for your target hardware. Here's a comprehensive example:
//!
//! ```rust
//! use scars_khal::{
//!     HardwareAbstractionLayer,
//!     InterruptController,
//!     AlarmClockController,
//!     CoreController,
//!     Fault,
//!     GetInterruptNumber,
//!     ContextInfo,
//! };
//! use scars_fault::Fault;
//!
//! // Define your hardware-specific error type
//! #[derive(Debug, Fault)]
//! enum HardwareError {
//!     #[fault("Invalid interrupt number: {number}")]
//!     InvalidInterrupt { number: u16 },
//!     #[fault("Invalid priority level: {level}")]
//!     InvalidPriority { level: u8 },
//!     #[fault("Timer configuration error: {reason}")]
//!     TimerError { reason: &'static str },
//!     #[fault("Context switch error: {reason}")]
//!     ContextError { reason: &'static str },
//! }
//!
//! // Define your interrupt claim type
//! struct InterruptClaim {
//!     interrupt_number: u16,
//! }
//!
//! impl GetInterruptNumber for InterruptClaim {
//!     fn get_interrupt_number(&self) -> u16 {
//!         self.interrupt_number
//!     }
//! }
//!
//! // Define your thread context type
//! struct ThreadContext {
//!     stack_top: *const u8,
//! }
//!
//! impl ContextInfo for ThreadContext {
//!     fn stack_top_ptr(&self) -> *const u8 {
//!         self.stack_top
//!     }
//!
//!     unsafe fn init(
//!         name: &'static str,
//!         main_fn: *const (),
//!         argument: Option<*const u8>,
//!         stack_ptr: *const u8,
//!         stack_size: usize,
//!         context: *mut Self,
//!     ) {
//!         (*context).stack_top = stack_ptr;
//!     }
//! }
//!
//! // Define your hardware abstraction layer
//! struct MyHardware {
//!     // Add hardware-specific fields
//! }
//!
//! // Implement InterruptController
//! impl InterruptController for MyHardware {
//!     const MAX_INTERRUPT_PRIORITY: usize = 7;
//!     const MAX_INTERRUPT_NUMBER: usize = 32;
//!     type InterruptClaim = InterruptClaim;
//!
//!     fn get_interrupt_priority(interrupt_number: u16) -> u8 {
//!         0
//!     }
//!
//!     fn set_interrupt_priority(interrupt_number: u16, prio: u8) -> u8 {
//!         0
//!     }
//!
//!     fn claim_interrupt() -> Self::InterruptClaim {
//!         InterruptClaim { interrupt_number: 0 }
//!     }
//!
//!     fn complete_interrupt(claim: Self::InterruptClaim) {
//!     }
//!
//!     fn enable_interrupt(interrupt_number: u16) {
//!     }
//!
//!     fn disable_interrupt(interrupt_number: u16) {
//!     }
//!
//!     fn get_interrupt_threshold() -> u8 {
//!         0
//!     }
//!
//!     fn set_interrupt_threshold(threshold: u8) {
//!     }
//!
//!     fn interrupt_status() -> bool {
//!         false
//!     }
//!
//!     fn acquire() -> bool {
//!         false
//!     }
//!
//!     fn restore(restore_state: bool) {
//!     }
//! }
//!
//! // Implement AlarmClockController
//! impl AlarmClockController for MyHardware {
//!     const TICK_FREQ_HZ: u64 = 1_000_000;
//!
//!     fn clock_ticks() -> u64 {
//!         0
//!     }
//!
//!     fn set_wakeup(at: Option<u64>) {
//!     }
//! }
//!
//! // Implement CoreController
//! impl CoreController for MyHardware {
//!     type StackAlignment = A8;
//!     type Context = ThreadContext;
//!     type HardwareError = HardwareError;
//!
//!     const NUM_CORES: usize = 1;
//!
//!     fn current_core_id() -> u8 { 0 }
//!
//!     fn pend_service_call_on(_core: u8) {}
//!
//!     fn start_first_thread(idle_context: *mut Self::Context) -> ! {
//!         loop {}
//!     }
//!
//!     fn on_abort() -> ! {
//!         loop {}
//!     }
//!
//!     fn on_exit(exit_code: i32) -> ! {
//!         loop {}
//!     }
//!
//!     fn on_fault(info: &FaultInfo) -> ! {
//!         loop {}
//!     }
//!
//!     fn on_breakpoint() {
//!     }
//!
//!     fn on_idle() {
//!     }
//!
//!     fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
//!         0
//!     }
//!
//!     fn current_thread_context() -> *const Self::Context {
//!         core::ptr::null()
//!     }
//!
//!     fn set_current_thread_context(context: *const Self::Context) {
//!     }
//! }
//!
//! // Implement HardwareAbstractionLayer
//! impl HardwareAbstractionLayer for MyHardware {
//!     const NAME: &'static str = "My Hardware";
//!
//!     unsafe fn init(hal: *mut Self) {
//!     }
//! }
//!
//! // Hardware-specific interrupt handler
//! #[no_mangle]
//! unsafe extern "C" fn interrupt_handler() {
//!     unsafe { MyHardware::kernel_interrupt_handler() };
//! }
//!
//! // Hardware-specific timer handler
//! #[no_mangle]
//! unsafe extern "C" fn timer_handler() {
//!     unsafe { MyHardware::kernel_wakeup_handler() };
//! }
//!
//! // Hardware-specific syscall handler
//! #[no_mangle]
//! unsafe extern "C" fn syscall_handler(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize {
//!     unsafe { MyHardware::kernel_syscall_handler(id, arg0, arg1, arg2) }
//! }
//! ```
//!
//! # Error Handling
//!
//! The crate uses the [`Fault`] trait for handling fatal errors.
//! Implementations should provide meaningful error information and handle errors
//! appropriately for their platform.

#![no_std]
pub mod callbacks;
pub use aligned::*;
pub use callbacks::KernelCallbacks;
pub use scars_fault::{Fault, FaultContext, FaultContextNode, FaultInfo};

unsafe extern "Rust" {
    pub unsafe fn start_kernel() -> !;
}

pub trait GetInterruptNumber {
    fn get_interrupt_number(&self) -> u16;
}

/// Trait for interrupt controllers.
///
/// This trait provides a set of methods for managing interrupts, including
/// querying and setting interrupt priorities, claiming and completing interrupts,
/// and enabling and disabling interrupts.
///
/// Implementations of this trait must be thread-safe.
pub trait InterruptController: Sync {
    /// The maximum priority level for interrupts.
    ///
    /// This constant defines the highest possible priority level for interrupts.
    const MAX_INTERRUPT_PRIORITY: usize;

    /// The maximum interrupt number.
    ///
    /// This constant defines the highest possible interrupt number.
    /// This has effect on the sizes of internal kernel data structures.
    const MAX_INTERRUPT_NUMBER: usize;

    type InterruptClaim: GetInterruptNumber;

    /// Returns the current priority level of the specified interrupt.
    ///
    /// # Arguments
    ///
    /// * `interrupt_number` - The interrupt number to query the priority for.
    ///
    /// # Returns
    ///
    /// The priority level of the interrupt as a `u8`, where higher values indicate higher priority.
    fn get_interrupt_priority(interrupt_number: u16) -> u8;

    /// Sets the priority level for the specified interrupt.
    ///
    /// # Arguments
    ///
    /// * `interrupt_number` - The interrupt number to set the priority for.
    /// * `prio` - The new priority level to set, where higher values indicate higher priority.
    ///
    /// # Returns
    ///
    /// The previous priority level of the interrupt as a `u8`.
    fn set_interrupt_priority(interrupt_number: u16, prio: u8) -> u8;

    /// Claims the highest priority pending interrupt.
    ///
    /// Returns a claim object that must be used to complete the interrupt
    /// handling with [`complete_interrupt`]. This function is called by the
    /// kernel interrupt handler when an interrupt is pending.
    ///
    /// # Example
    ///
    /// ```rust
    /// let claim = hal.claim_interrupt();
    /// // handle the interrupt
    /// hal.complete_interrupt(claim);
    /// ```
    ///
    /// The claim object contains the interrupt number of the claimed interrupt.
    fn claim_interrupt() -> Self::InterruptClaim;

    /// Completes the interrupt handling for the specified claim.
    ///
    /// # Arguments
    ///
    /// * `claim` - The claim object returned by [`claim_interrupt`].
    fn complete_interrupt(claim: Self::InterruptClaim);

    /// Enables the specified interrupt.
    ///
    /// # Arguments
    ///
    /// * `interrupt_number` - The interrupt number to enable.
    fn enable_interrupt(interrupt_number: u16);

    /// Disables the specified interrupt.
    ///
    /// # Arguments
    ///
    /// * `interrupt_number` - The interrupt number to disable.
    fn disable_interrupt(interrupt_number: u16);

    /// Returns the current interrupt priority threshold.
    ///
    /// See [`set_interrupt_threshold`](Self::set_interrupt_threshold)
    /// for the threshold convention. A round-trip
    /// `set_interrupt_threshold(t); get_interrupt_threshold()` must
    /// return `t` for every value the kernel actually uses (priority
    /// ceilings and `MAX_INTERRUPT_PRIORITY`).
    fn get_interrupt_threshold() -> u8;

    /// Sets the interrupt priority threshold.
    ///
    /// # Convention
    ///
    /// The threshold acts as a filter: interrupts with priority less
    /// than or equal to `threshold` are blocked; interrupts with
    /// priority strictly greater than `threshold` are serviced.
    ///
    /// `threshold == MAX_INTERRUPT_PRIORITY` is reserved as the "no
    /// masking" sentinel: every interrupt is deliverable regardless
    /// of priority. The kernel uses this value for the thread-context
    /// ceiling (see `set_ceiling_threshold` in `scars::interrupt`).
    ///
    /// Implementations whose underlying state does not naturally
    /// encode the sentinel must convert at this boundary; `get` /
    /// `set` must round-trip every value the kernel passes.
    ///
    /// # Arguments
    ///
    /// * `threshold` - The new interrupt priority threshold as a `u8`.
    fn set_interrupt_threshold(threshold: u8);

    /// Returns the current interrupt status.
    ///
    /// # Returns
    ///
    /// `true` if interrupts are currently enabled, `false` otherwise.
    fn interrupt_status() -> bool;

    /// Acquires the interrupt lock.
    ///
    /// This function acquires the interrupt lock, which prevents other interrupts
    /// from occurring until the lock is released.
    ///
    /// # Returns
    ///
    /// The previous interrupt status before acquiring the lock.
    fn acquire() -> bool;

    /// Restores the interrupt lock to the previous state.
    ///
    /// Can also be used to acquire or release the lock, by passing
    /// `false` or `true` as the `restore_state` argument.
    ///
    /// # Arguments
    ///
    /// * `restore_state` - The previous interrupt status to restore.
    fn restore(restore_state: bool);
}

pub type Ticks = u64;

pub trait AlarmClockController: Sync {
    /// Timer frequency as Ticks per second
    const TICK_FREQ_HZ: Ticks;

    /// Monotonously growing tick counter since some earlier epoch
    fn clock_ticks() -> Ticks;

    /// Set the wakeup time for the alarm clock.
    ///
    /// If `at` is `None`, the wakeup is disabled.
    fn set_wakeup(at: Option<Ticks>);
}

pub trait ContextInfo {
    /// Get the stack top pointer.
    ///
    /// Updated by the HAL when the thread is switched out.
    ///
    /// # Returns
    ///
    /// The stack top pointer as a `*const u8`.
    fn stack_top_ptr(&self) -> *const u8;

    /// Initialize the context.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the thread.
    /// * `main_fn` - The main function of the thread.
    /// * `argument` - The argument of the thread.
    /// * `stack_ptr` - The stack pointer of the thread.
    /// * `stack_size` - The size of the stack of the thread.
    /// * `context` - The context of the thread.
    unsafe fn init(
        name: &'static str,
        main_fn: *const (),
        argument: Option<*const u8>,
        stack_ptr: *const u8,
        stack_size: usize,
        context: *mut Self,
    );
}

/// Trait for flow controllers.
///
/// This trait provides a set of methods for controlling the execution flow
/// of the kernel.
/// Identifier of the core that runs `init` and owns the single existing
/// scheduler on single-core platforms. Used as the default value for the
/// `CORE` const generic on every thread and lock type, so user code that
/// doesn't care about core affinity behaves as if pinned to this core.
pub const DEFAULT_CORE: u8 = 0;

pub trait CoreController: Sync {
    type StackAlignment: Alignment;
    type Context: ContextInfo;
    type HardwareError: Fault;

    /// Number of independently scheduled cores on the platform.
    ///
    /// Must be at least 1. The kernel allocates per-core state of this
    /// size at compile time, so this is fixed for a given target.
    const NUM_CORES: usize;

    /// Identifier of the core executing the calling context.
    ///
    /// The returned value is in `0..NUM_CORES`. It is stable within a
    /// single execution context (thread body or interrupt handler) and
    /// changes only across context switches that migrate execution to
    /// a different core. Single-core platforms always return `0`.
    fn current_core_id() -> u8;

    /// Asynchronously request a service-call dispatch on `core`.
    ///
    /// The target core observes the request at its next service-call
    /// dispatch point. `core` may equal [`current_core_id`], in which
    /// case the effect is identical to [`pend_service_call`].
    ///
    /// [`current_core_id`]: Self::current_core_id
    /// [`pend_service_call`]: Self::pend_service_call
    fn pend_service_call_on(core: u8);

    /// Start the first thread.
    ///
    /// This function is called when the kernel is started. It should start the
    /// first thread of the kernel. This is called after kernel initialization
    /// is complete. Execution is started in the context of the idle thread.
    ///
    /// # Arguments
    ///
    /// * `idle_context` - The context of the idle thread.
    ///
    /// # Returns
    ///
    /// This function never returns.
    fn start_first_thread(idle_context: *mut Self::Context) -> !;

    /// Called when the kernel is aborted.
    ///
    /// This function is called when the kernel is aborted. It should not return.
    fn on_abort() -> !;

    /// Called when the kernel is exiting.
    ///
    /// This function is called when the kernel is exiting. It should not return.
    ///
    /// # Arguments
    ///
    /// * `exit_code` - The exit code of the kernel.
    fn on_exit(exit_code: i32) -> !;

    /// Called when an unrecoverable fault occurs.
    ///
    /// Receives the full [`FaultInfo`] — the leaf fault, the original
    /// capture location, and the context chain accumulated by upstream
    /// layers (typically the kernel, which has already prepended a
    /// `ThreadContext` / `InterruptContext` / `BootstrapContext`). The
    /// implementation may prepend its own platform-specific
    /// [`FaultContext`] (captured registers, MCAUSE/MEPC, etc.) before
    /// formatting and terminating. Must not return.
    fn on_fault(info: &FaultInfo) -> !;

    /// Called when a breakpoint is hit.
    fn on_breakpoint();

    /// Called by the kernel when the idle thread is running.
    ///
    /// It can be used to implement a low-power mode or other idle tasks.
    /// On ARM this calls `wfi` to enter a low-power mode, and on simulator
    /// it calls pthread_yield.
    fn on_idle();

    /// Called from the thread context when a system call is made.
    ///
    /// # Arguments
    ///
    /// * `id` - The ID of the system call.
    /// * `arg0` - The first argument of the system call.
    /// * `arg1` - The second argument of the system call.
    /// * `arg2` - The third argument of the system call.
    ///
    /// # Returns
    ///
    /// The return value of the system call.
    fn syscall(id: usize, arg0: usize, arg1: usize, arg2: usize) -> usize;

    fn current_thread_context() -> *const Self::Context;

    /// Set the current thread context.
    ///
    /// # Arguments
    ///
    /// * `context` - The context of the current thread.
    fn set_current_thread_context(context: *const Self::Context);

    /// Pend a service call for deferred kernel operations.
    ///
    /// This is an asynchronous mechanism for the kernel to defer operations
    /// like event processing and context switching. The service call will
    /// execute when the execution flow allows it (typically when interrupt
    /// processing completes and execution returns to the lowest priority level).
    fn pend_service_call();

    /// Clear the pending service call.
    ///
    /// Called by the kernel service call handler after processing is complete.
    /// This may be called automatically by hardware or manually by software
    /// depending on the platform implementation.
    fn clear_service_call();
}

pub trait HardwareAbstractionLayer:
    AlarmClockController + InterruptController + CoreController + Sync
{
    const NAME: &'static str;

    /// Get a reference to the global instance of this HAL
    fn instance() -> &'static Self;

    unsafe fn init(hal: *mut Self)
    where
        Self: Sized;
}
