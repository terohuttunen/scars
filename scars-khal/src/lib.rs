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
//! - [`FlowController`]: Controls thread execution and system flow
//!   - Thread context management
//!   - System call handling
//!   - Error and exception handling
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
//!   - Implemented by the kernel to handle unrecoverable errors
//!   - Called from exception context
//!   - Handles platform-independent error processing
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
//! - Proper error handling for unrecoverable errors
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
//!     FlowController,
//!     UnrecoverableError,
//!     GetInterruptNumber,
//!     ContextInfo,
//! };
//! use unrecoverable_error::UnrecoverableError;
//! 
//! // Define your hardware-specific error type
//! #[derive(Debug, UnrecoverableError)]
//! enum HardwareError {
//!     #[unrecoverable_error("Invalid interrupt number: {number}")]
//!     InvalidInterrupt { number: u16 },
//!     #[unrecoverable_error("Invalid priority level: {level}")]
//!     InvalidPriority { level: u8 },
//!     #[unrecoverable_error("Timer configuration error: {reason}")]
//!     TimerError { reason: &'static str },
//!     #[unrecoverable_error("Context switch error: {reason}")]
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
//!     fn get_interrupt_priority(&self, interrupt_number: u16) -> u8 {
//!         0
//!     }
//! 
//!     fn set_interrupt_priority(&self, interrupt_number: u16, prio: u8) -> u8 {
//!         0
//!     }
//! 
//!     fn claim_interrupt(&self) -> Self::InterruptClaim {
//!         InterruptClaim { interrupt_number: 0 }
//!     }
//! 
//!     fn complete_interrupt(&self, claim: Self::InterruptClaim) {
//!     }
//! 
//!     fn enable_interrupt(&self, interrupt_number: u16) {
//!     }
//! 
//!     fn disable_interrupt(&self, interrupt_number: u16) {
//!     }
//! 
//!     fn get_interrupt_threshold(&self) -> u8 {
//!         0
//!     }
//! 
//!     fn set_interrupt_threshold(&self, threshold: u8) {
//!     }
//! 
//!     fn interrupt_status(&self) -> bool {
//!         false
//!     }
//! 
//!     fn acquire(&self) -> bool {
//!         false
//!     }
//! 
//!     fn restore(&self, restore_state: bool) {
//!     }
//! }
//! 
//! // Implement AlarmClockController
//! impl AlarmClockController for MyHardware {
//!     const TICK_FREQ_HZ: u64 = 1_000_000;
//! 
//!     fn clock_ticks(&self) -> u64 {
//!         0
//!     }
//! 
//!     fn set_wakeup(&self, at: Option<u64>) {
//!     }
//! }
//! 
//! // Implement FlowController
//! impl FlowController for MyHardware {
//!     type StackAlignment = A8;
//!     type Context = ThreadContext;
//!     type HardwareError = HardwareError;
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
//!     fn on_error(error: &dyn UnrecoverableError) -> ! {
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
//! The crate uses the [`UnrecoverableError`] trait for handling fatal errors.
//! Implementations should provide meaningful error information and handle errors
//! appropriately for their platform.

#![no_std]
pub mod callbacks;
pub use aligned::*;
pub use callbacks::KernelCallbacks;
pub use unrecoverable_error::UnrecoverableError;

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
    fn get_interrupt_priority(&self, interrupt_number: u16) -> u8;

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
    fn set_interrupt_priority(&self, interrupt_number: u16, prio: u8) -> u8;

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
    fn claim_interrupt(&self) -> Self::InterruptClaim;

    /// Completes the interrupt handling for the specified claim.
    /// 
    /// # Arguments
    /// 
    /// * `claim` - The claim object returned by [`claim_interrupt`].
    fn complete_interrupt(&self, claim: Self::InterruptClaim);

    /// Enables the specified interrupt.
    /// 
    /// # Arguments
    /// 
    /// * `interrupt_number` - The interrupt number to enable.
    fn enable_interrupt(&self, interrupt_number: u16);

    /// Disables the specified interrupt.
    /// 
    /// # Arguments
    /// 
    /// * `interrupt_number` - The interrupt number to disable.
    fn disable_interrupt(&self, interrupt_number: u16);

    /// Returns the current interrupt priority threshold.
    /// 
    /// The threshold acts as a filter - interrupts with priority less than or equal
    /// to the threshold will be blocked. Only interrupts with priority higher than
    /// the threshold will be serviced.
    /// 
    /// # Returns
    /// 
    /// The current interrupt priority threshold as a `u8`.
    fn get_interrupt_threshold(&self) -> u8;

    /// Sets the interrupt priority threshold.
    /// 
    /// The threshold acts as a filter - interrupts with priority less than or equal
    /// to the threshold will be blocked. Only interrupts with priority higher than
    /// the threshold will be serviced.
    /// 
    /// # Arguments
    /// 
    /// * `threshold` - The new interrupt priority threshold as a `u8`.
    fn set_interrupt_threshold(&self, threshold: u8);

    /// Returns the current interrupt status.
    /// 
    /// # Returns
    /// 
    /// `true` if interrupts are currently enabled, `false` otherwise.
    fn interrupt_status(&self) -> bool;

    /// Acquires the interrupt lock.
    /// 
    /// This function acquires the interrupt lock, which prevents other interrupts
    /// from occurring until the lock is released.
    /// 
    /// # Returns   
    /// 
    /// The previous interrupt status before acquiring the lock.
    fn acquire(&self) -> bool;

    /// Restores the interrupt lock to the previous state.
    /// 
    /// Can also be used to acquire or release the lock, by passing
    /// `false` or `true` as the `restore_state` argument.
    /// 
    /// # Arguments
    /// 
    /// * `restore_state` - The previous interrupt status to restore.
    fn restore(&self, restore_state: bool);
}

pub type Ticks = u64;

pub trait AlarmClockController: Sync {
    /// Timer frequency as Ticks per second
    const TICK_FREQ_HZ: Ticks;

    /// Monotonously growing tick counter since some earlier epoch
    fn clock_ticks(&self) -> Ticks;

    /// Set the wakeup time for the alarm clock.
    ///
    /// If `at` is `None`, the wakeup is disabled.
    fn set_wakeup(&self, at: Option<Ticks>);
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
pub trait FlowController: Sync {
    type StackAlignment: Alignment;
    type Context: ContextInfo;
    type HardwareError: UnrecoverableError;

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

    /// Called when an error occurs.
    /// 
    /// This function is called when an error occurs. It should not return.
    /// 
    /// # Arguments
    /// 
    /// * `error` - The error that occurred.
    fn on_error(error: &dyn UnrecoverableError) -> !;

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
}

pub trait HardwareAbstractionLayer:
    AlarmClockController + InterruptController + FlowController + Sync
{
    const NAME: &'static str;

    unsafe fn init(hal: *mut Self)
    where
        Self: Sized;
}
