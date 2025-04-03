#![no_std]
pub use unrecoverable_error_macros::UnrecoverableError;

/// A trait for unrecoverable errors.
///
/// This trait is used to represent errors that cannot be recovered from.
/// It is similar to the `std::error::Error` trait, but does not require
/// the error to be `'static` in source return value, which is not
/// necessary if the stack is not unwound when the error is raised.

pub trait UnrecoverableError: core::fmt::Debug + core::fmt::Display {
    fn source(&self) -> Option<&(dyn UnrecoverableError)> {
        None
    }
}
