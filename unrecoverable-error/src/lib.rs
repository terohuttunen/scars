#![no_std]
#![feature(linkage)]
#![warn(missing_docs)]

//! A crate for handling unrecoverable errors in `#![no_std]` environments.
//!
//! This crate provides a mechanism for handling unrecoverable errors in `#![no_std]` environments,
//! similar to how `panic!` works in standard Rust, but with more control over error handling.
//!
//! # Overview
//!
//! The crate provides:
//! - A trait for unrecoverable errors
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
//! use unrecoverable_error::{UnrecoverableError, unrecoverable_error};
//!
//! #[derive(Debug, UnrecoverableError)]
//! #[unrecoverable_error("Invalid configuration: {field} = {value}")]
//! struct ConfigError<'a> {
//!     field: &'a str,
//!     value: &'a str,
//! }
//!
//! // This will call the default handler (which panics)
//! unrecoverable_error!(ConfigError {
//!     field: "timeout",
//!     value: "1000",
//! });
//! ```
//!
//! ## Custom Handler
//!
//! ```rust
//! use unrecoverable_error::{UnrecoverableError, unrecoverable_error, UnrecoverableErrorInfo};
//!
//! #[derive(Debug, UnrecoverableError)]
//! #[unrecoverable_error("Network error: {reason}")]
//! struct NetworkError<'a> {
//!     reason: &'a str,
//! }
//!
//! #[unrecoverable_error_handler]
//! fn my_handler(info: &UnrecoverableErrorInfo) -> ! {
//!     if let Some(location) = info.location {
//!         // Log error with location
//!     }
//!     // Terminate the program
//!     core::process::exit(1);
//! }
//!
//! // This will call the custom handler
//! unrecoverable_error!(NetworkError {
//!     reason: "connection refused",
//! });
//! ```
//!
//! ## Enum Errors
//!
//! ```rust
//! use unrecoverable_error::{UnrecoverableError, unrecoverable_error};
//!
//! #[derive(Debug, UnrecoverableError)]
//! enum MyError<'a> {
//!     #[unrecoverable_error("Invalid input: {value}")]
//!     InvalidInput { value: &'a str },
//!     #[unrecoverable_error("Timeout after {ms}ms")]
//!     Timeout { ms: u32 },
//!     #[unrecoverable_error("Connection failed: {reason}")]
//!     ConnectionFailed { reason: &'a str },
//! }
//!
//! // This will call the default handler with a formatted message
//! unrecoverable_error!(MyError::Timeout { ms: 1000 });
//! ```
//!
//! # Features
//!
//! - `location` (enabled by default): Enables location tracking for errors
//!

pub use unrecoverable_error_macros::{UnrecoverableError, unrecoverable_error, unrecoverable_error_handler};

/// A trait for unrecoverable errors.
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
/// use unrecoverable_error::UnrecoverableError;
///
/// #[derive(Debug, UnrecoverableError)]
/// #[unrecoverable_error("Invalid configuration: {field} = {value}")]
/// struct ConfigError<'a> {
///     field: &'a str,
///     value: &'a str,
/// }
/// ```
///
/// With error chaining:
///
/// ```rust
/// use unrecoverable_error::UnrecoverableError;
///
/// #[derive(Debug, UnrecoverableError)]
/// #[unrecoverable_error("Wrapped error: {inner}")]
/// struct WrappedError<'a> {
///     inner: Box<dyn UnrecoverableError + 'a>,
/// }
///
/// impl<'a> UnrecoverableError for WrappedError<'a> {
///     fn source(&self) -> Option<&(dyn UnrecoverableError)> {
///         Some(&*self.inner)
///     }
/// }
/// ```
pub trait UnrecoverableError: core::fmt::Debug + core::fmt::Display {
    /// Returns the source of this error, if any.
    ///
    /// This is similar to `std::error::Error::source`, but returns a reference
    /// to an `UnrecoverableError` instead of a `dyn Error`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use unrecoverable_error::UnrecoverableError;
    ///
    /// #[derive(Debug)]
    /// struct WrappedError {
    ///     source: Box<dyn UnrecoverableError>,
    /// }
    ///
    /// impl core::fmt::Display for WrappedError {
    ///     fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
    ///         write!(f, "Wrapped error")
    ///     }
    /// }
    ///
    /// impl UnrecoverableError for WrappedError {
    ///     fn source(&self) -> Option<&(dyn UnrecoverableError)> {
    ///         Some(&*self.source)
    ///     }
    /// }
    /// ```
    fn source(&self) -> Option<&(dyn UnrecoverableError)> {
        None
    }
}

/// Information about an unrecoverable error, including the error itself and optional location.
///
/// This struct is passed to error handlers and contains all the information
/// about the error that occurred.
///
/// # Examples
///
/// ```rust
/// use unrecoverable_error::{UnrecoverableError, UnrecoverableErrorInfo};
///
/// #[unrecoverable_error_handler]
/// fn my_handler(info: &UnrecoverableErrorInfo) -> ! {
///     if let Some(location) = info.location {
///         // Log error with location
///     }
///     // Terminate the program
///     core::process::exit(1);
/// }
/// ```
pub struct UnrecoverableErrorInfo<'a> {
    /// The unrecoverable error that occurred
    pub error: &'a dyn UnrecoverableError,
    /// Optional location where the error occurred
    pub location: Option<&'a core::panic::Location<'a>>,
}


/// The default error handler that panics.
#[linkage = "weak"]
#[unsafe(no_mangle)]
pub unsafe fn _unrecoverable_error_handler(info: &UnrecoverableErrorInfo) -> ! {
    if let Some(location) = info.location {
        panic!("Unrecoverable error at {}: {}", location, info.error);
    } else {
        panic!("Unrecoverable error: {}", info.error);
    }
}

/// Function to handle unrecoverable errors.
///
/// This function is called by the `unrecoverable_error!` macro to handle
/// unrecoverable errors. It will call the user-defined handler if it exists,
/// otherwise use the default handler.
///
/// # Safety
///
/// This function is marked as unsafe because it calls an external function
/// that may not exist. The caller must ensure that either:
/// - A custom handler is defined using `#[unrecoverable_error_handler]`
/// - The default handler is available
///
/// # Examples
///
/// ```rust
/// use unrecoverable_error::{UnrecoverableError, handle_unrecoverable_error};
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
/// impl UnrecoverableError for MyError {}
///
/// // This is equivalent to using the macro:
/// // unrecoverable_error!(MyError);
/// unsafe {
///     handle_unrecoverable_error(&MyError);
/// }
/// ```
#[track_caller]
pub fn handle_unrecoverable_error(error: &dyn UnrecoverableError) -> ! {
    let info = UnrecoverableErrorInfo {
        error,
        location: Some(core::panic::Location::caller()),
    };
    unsafe {
        _unrecoverable_error_handler(&info)
    }
}

