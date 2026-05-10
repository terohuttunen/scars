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

pub use scars_fault_macros::{Fault, FaultContext, fault, fault_handler};

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
}

/// A trait for faults — errors that cannot be recovered from.
#[cfg(feature = "display")]
pub trait Fault: core::fmt::Debug + core::fmt::Display {}

/// A contextual frame attached to a propagating fault.
///
/// Where [`Fault`] names *what* went wrong (the leaf), `FaultContext`
/// describes *where and under what conditions* the fault was observed:
/// the running thread, the active interrupt, captured registers, a
/// backtrace, etc. Each layer in the propagation pipeline (kernel,
/// KHAL) can prepend a context node before forwarding.
///
/// Implemented through `#[derive(FaultContext)]`, which uses the same
/// `#[fault("...")]` format-string attribute as `#[derive(Fault)]`.
#[cfg(feature = "defmt")]
pub trait FaultContext: core::fmt::Debug {
    /// Format this context frame into the given `defmt::Formatter`.
    fn defmt_format(&self, fmt: defmt::Formatter<'_>);
}

/// A contextual frame attached to a propagating fault.
#[cfg(feature = "display")]
pub trait FaultContext: core::fmt::Debug + core::fmt::Display {}

#[cfg(feature = "defmt")]
impl defmt::Format for dyn Fault + '_ {
    fn format(&self, fmt: defmt::Formatter<'_>) {
        self.defmt_format(fmt)
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for dyn FaultContext + '_ {
    fn format(&self, fmt: defmt::Formatter<'_>) {
        self.defmt_format(fmt)
    }
}

/// A node in the singly-linked chain of `FaultContext` frames carried
/// by a [`FaultInfo`]. Built on the stack of the propagating call
/// chain — no allocation.
pub struct FaultContextNode<'a> {
    /// The contextual frame at this node.
    pub frame: &'a dyn FaultContext,
    /// The next (deeper) node in the chain, or `None` at the tail.
    pub next: Option<&'a FaultContextNode<'a>>,
}

/// Information about a fault: the value itself, the capture location,
/// and the context chain accumulated as the fault propagates.
pub struct FaultInfo<'a> {
    /// The fault that occurred.
    pub error: &'a dyn Fault,
    /// Source location where the fault was raised.
    pub location: Option<&'a core::panic::Location<'a>>,
    /// Innermost context node, or `None` if no context has been
    /// attached yet.
    pub context: Option<&'a FaultContextNode<'a>>,
}

impl<'a> FaultInfo<'a> {
    /// Borrow `self` and return a new `FaultInfo` with `node` prepended
    /// to the context chain.
    ///
    /// Allocation-free: the caller's stack frame owns `node`. The
    /// returned `FaultInfo` borrows from the same scope and from
    /// `node`, so it must be passed onward (e.g. handed to the next
    /// layer in the propagation pipeline) within that frame's
    /// lifetime.
    pub fn with_context<'b>(&'b self, node: &'b FaultContextNode<'b>) -> FaultInfo<'b>
    where
        'a: 'b,
    {
        FaultInfo {
            error: self.error,
            location: self.location,
            context: Some(node),
        }
    }

    /// Iterate the context chain from innermost to outermost frame.
    pub fn context_iter(&self) -> FaultContextIter<'_> {
        FaultContextIter { next: self.context }
    }
}

/// Iterator over the [`FaultContext`] chain in a [`FaultInfo`].
pub struct FaultContextIter<'a> {
    next: Option<&'a FaultContextNode<'a>>,
}

impl<'a> Iterator for FaultContextIter<'a> {
    type Item = &'a dyn FaultContext;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.next?;
        self.next = node.next;
        Some(node.frame)
    }
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
    for (i, frame) in info.context_iter().enumerate() {
        defmt::error!("  {}: {}", i + 1, frame);
    }
    defmt::panic!()
}

/// The default fault handler.
///
/// Walks `info.context` only under the `defmt` backend (where
/// `defmt::error!` is available without an allocator). The display
/// backend falls back to a single `panic!` on the leaf fault — the
/// kernel-installed handler is expected to walk the chain itself
/// before terminating.
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
        context: None,
    };
    unsafe { _fault_handler(&info) }
}
