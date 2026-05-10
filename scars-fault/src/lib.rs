#![no_std]
#![feature(linkage)]
#![warn(missing_docs)]

//! Fault handling for `#![no_std]` environments.
//!
//! Two formatting backends, mutually exclusive, picked by feature:
//!
//! - `defmt` (default for embedded targets): the [`Fault`] trait carries a
//!   dyn-safe `defmt_format` shim because `defmt::Format` itself isn't
//!   dyn-compatible. The default `_fault_handler` uses `defmt::error!` /
//!   `defmt::panic!`.
//! - `display` (used by the simulator): the [`Fault`] trait requires
//!   `core::fmt::Display`. The default handler `panic!`s.
//!
//! Faults are raised with the [`fault!`] macro, which forwards the value to
//! the registered handler. The handler is replaceable via [`fault_handler`].

#[cfg(all(feature = "defmt", feature = "display"))]
compile_error!("scars-fault: features `defmt` and `display` are mutually exclusive");

#[cfg(not(any(feature = "defmt", feature = "display")))]
compile_error!("scars-fault: enable one of features `defmt` or `display`");

pub use scars_fault_macros::{Fault, fault, fault_handler};

#[cfg(feature = "defmt")]
#[doc(hidden)]
pub use defmt as __defmt;

/// A trait for faults — errors that cannot be recovered from.
///
/// Implemented through `#[derive(Fault)]`. Under `defmt`, the derive also
/// emits a `defmt::Format` impl plus a `defmt_format` method used by
/// `dyn Fault`. Under `display`, it emits `core::fmt::Display`.
#[cfg(feature = "defmt")]
pub trait Fault: core::fmt::Debug {
    /// Format this fault into the given `defmt::Formatter`.
    ///
    /// Implemented automatically by `#[derive(Fault)]`.
    fn defmt_format(&self, fmt: defmt::Formatter<'_>);

    /// Returns the source of this fault, if any.
    fn source(&self) -> Option<&dyn Fault> {
        None
    }
}

/// A trait for faults — errors that cannot be recovered from.
#[cfg(feature = "display")]
pub trait Fault: core::fmt::Debug + core::fmt::Display {
    /// Returns the source of this fault, if any.
    fn source(&self) -> Option<&dyn Fault> {
        None
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for dyn Fault + '_ {
    fn format(&self, fmt: defmt::Formatter<'_>) {
        self.defmt_format(fmt)
    }
}

/// Information about a fault: the value itself and an optional capture
/// location.
pub struct FaultInfo<'a> {
    /// The fault that occurred.
    pub error: &'a dyn Fault,
    /// Source location where the fault was raised.
    pub location: Option<&'a core::panic::Location<'a>>,
}

/// The default fault handler.
///
/// Replaced when a user defines `#[fault_handler]`.
#[cfg(feature = "defmt")]
#[linkage = "weak"]
#[unsafe(no_mangle)]
pub unsafe fn _fault_handler(info: &FaultInfo) -> ! {
    if let Some(location) = info.location {
        defmt::error!(
            "Fault at {}:{}: {}",
            location.file(),
            location.line(),
            info.error
        );
    } else {
        defmt::error!("Fault: {}", info.error);
    }
    defmt::panic!()
}

/// The default fault handler.
#[cfg(feature = "display")]
#[linkage = "weak"]
#[unsafe(no_mangle)]
pub unsafe fn _fault_handler(info: &FaultInfo) -> ! {
    if let Some(location) = info.location {
        panic!("Fault at {}: {}", location, info.error);
    } else {
        panic!("Fault: {}", info.error);
    }
}

/// Dispatches a fault to the registered handler.
///
/// # Safety
///
/// Calls into an external `_fault_handler` symbol provided either by the
/// `#[fault_handler]`-marked function or the weak default above.
#[track_caller]
pub fn handle_fault(error: &dyn Fault) -> ! {
    let info = FaultInfo {
        error,
        location: Some(core::panic::Location::caller()),
    };
    unsafe { _fault_handler(&info) }
}
