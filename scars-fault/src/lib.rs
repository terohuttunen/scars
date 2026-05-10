#![no_std]
#![feature(linkage)]
#![warn(missing_docs)]

//! A crate for handling faults in `#![no_std]` environments.
//!
//! This crate provides a mechanism for handling faults in `#![no_std]` environments,
//! similar to how `panic!` works in standard Rust, but with more control over error handling.
//!
//! # Overview
//!
//! The crate provides:
//! - A trait for faults
//! - A default error handler that panics
//! - A way to override the error handler
//! - Location tracking for errors
//! - Custom error formatting through derive macros
//!
//! # Usage
//!
//! ## Basic Usage
//!
//! ```rust
//! use scars_fault::{Fault, fault};
//!
//! #[derive(Debug, Fault)]
//! #[fault("Invalid configuration: {field} = {value}")]
//! struct ConfigError<'a> {
//!     field: &'a str,
//!     value: &'a str,
//! }
//!
//! // This will call the default handler (which panics)
//! fault!(ConfigError {
//!     field: "timeout",
//!     value: "1000",
//! });
//! ```
//!
//! ## Custom Handler
//!
//! ```rust
//! use scars_fault::{Fault, fault, FaultInfo};
//!
//! #[derive(Debug, Fault)]
//! #[fault("Network error: {reason}")]
//! struct NetworkError<'a> {
//!     reason: &'a str,
//! }
//!
//! #[fault_handler]
//! fn my_handler(info: &FaultInfo) -> ! {
//!     if let Some(location) = info.location {
//!         // Log error with location
//!     }
//!     // Terminate the program
//!     core::process::exit(1);
//! }
//!
//! // This will call the custom handler
//! fault!(NetworkError {
//!     reason: "connection refused",
//! });
//! ```
//!
//! ## Enum Errors
//!
//! ```rust
//! use scars_fault::{Fault, fault};
//!
//! #[derive(Debug, Fault)]
//! enum MyError<'a> {
//!     #[fault("Invalid input: {value}")]
//!     InvalidInput { value: &'a str },
//!     #[fault("Timeout after {ms}ms")]
//!     Timeout { ms: u32 },
//!     #[fault("Connection failed: {reason}")]
//!     ConnectionFailed { reason: &'a str },
//! }
//!
//! // This will call the default handler with a formatted message
//! fault!(MyError::Timeout { ms: 1000 });
//! ```
//!
//! # Features
//!
//! - `location` (enabled by default): Enables location tracking for errors
//!

pub use scars_fault_macros::{Fault, fault, fault_handler};

/// A trait for faults.
///
/// This trait is used to represent errors that cannot be recovered from.
/// It is similar to the `std::error::Error` trait, but does not require
/// the error to be `'static` in source return value, which is not
/// necessary if the stack is not unwound when the error is raised.
///
/// # Examples
///
/// Basic usage:
///
/// ```rust
/// use scars_fault::Fault;
///
/// #[derive(Debug, Fault)]
/// #[fault("Invalid configuration: {field} = {value}")]
/// struct ConfigError<'a> {
///     field: &'a str,
///     value: &'a str,
/// }
/// ```
///
/// With error chaining:
///
/// ```rust
/// use scars_fault::Fault;
///
/// #[derive(Debug, Fault)]
/// #[fault("Wrapped error: {inner}")]
/// struct WrappedError<'a> {
///     inner: Box<dyn Fault + 'a>,
/// }
///
/// impl<'a> Fault for WrappedError<'a> {
///     fn source(&self) -> Option<&(dyn Fault)> {
///         Some(&*self.inner)
///     }
/// }
/// ```
pub trait Fault: core::fmt::Debug + core::fmt::Display {
    /// Returns the source of this error, if any.
    ///
    /// This is similar to `std::error::Error::source`, but returns a reference
    /// to an `Fault` instead of a `dyn Error`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use scars_fault::Fault;
    ///
    /// #[derive(Debug)]
    /// struct WrappedError {
    ///     source: Box<dyn Fault>,
    /// }
    ///
    /// impl core::fmt::Display for WrappedError {
    ///     fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    ///         write!(f, "Wrapped error")
    ///     }
    /// }
    ///
    /// impl Fault for WrappedError {
    ///     fn source(&self) -> Option<&dyn Fault> {
    ///         Some(&*self.source)
    ///     }
    /// }
    /// ```
    fn source(&self) -> Option<&dyn Fault> {
        None
    }
}

/// Information about a fault, including the error itself and optional location.
///
/// This struct is passed to error handlers and contains all the information
/// about the error that occurred.
///
/// # Examples
///
/// ```rust
/// use scars_fault::{Fault, FaultInfo};
///
/// #[fault_handler]
/// fn my_handler(info: &FaultInfo) -> ! {
///     if let Some(location) = info.location {
///         // Log error with location
///     }
///     // Terminate the program
///     core::process::exit(1);
/// }
/// ```
pub struct FaultInfo<'a> {
    /// The fault that occurred
    pub error: &'a dyn Fault,
    /// Optional location where the error occurred
    pub location: Option<&'a core::panic::Location<'a>>,
}

/// The default error handler that panics.
#[linkage = "weak"]
#[unsafe(no_mangle)]
pub unsafe fn _fault_handler(info: &FaultInfo) -> ! {
    if let Some(location) = info.location {
        panic!("Fault at {}: {}", location, info.error);
    } else {
        panic!("Fault: {}", info.error);
    }
}

/// Function to handle faults.
///
/// This function is called by the `fault!` macro to handle
/// faults. It will call the user-defined handler if it exists,
/// otherwise use the default handler.
///
/// # Safety
///
/// This function is marked as unsafe because it calls an external function
/// that may not exist. The caller must ensure that either:
/// - A custom handler is defined using `#[fault_handler]`
/// - The default handler is available
///
/// # Examples
///
/// ```rust
/// use scars_fault::{Fault, handle_fault};
///
/// #[derive(Debug)]
/// struct MyError;
///
/// impl core::fmt::Display for MyError {
///     fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
///         write!(f, "My error occurred")
///     }
/// }
///
/// impl Fault for MyError {}
///
/// // This is equivalent to using the macro:
/// // fault!(MyError);
/// unsafe {
///     handle_fault(&MyError);
/// }
/// ```
#[track_caller]
pub fn handle_fault(error: &dyn Fault) -> ! {
    let info = FaultInfo {
        error,
        location: Some(core::panic::Location::caller()),
    };
    unsafe { _fault_handler(&info) }
}
