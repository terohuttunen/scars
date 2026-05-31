//! The [`CoreId`] core index.
//!
//! `CoreId` is the type of the `CORE` const generic used across the kernel,
//! the way `Priority` is the type of `PRIO`. Construction asserts
//! `core < NUM_CORES`, so any `CoreId` value is in range for the active khal.
//!
//! With the `multi-core` feature enabled `CoreId` wraps a `u8`. With it
//! disabled the system has a single core and `CoreId` is zero-sized, so the
//! per-object `core` fields and the wrong-core checks throughout the kernel
//! cost nothing.

use super::NUM_CORES;
#[cfg(feature = "multi-core")]
use super::kernel_hal;
#[cfg(feature = "multi-core")]
use scars_khal::CoreController;

/// Core index in `0..NUM_CORES`.
#[cfg(feature = "multi-core")]
#[derive(Copy, Clone, Debug, PartialEq, Eq, core::marker::ConstParamTy)]
#[repr(transparent)]
pub struct CoreId(u8);

/// Zero-sized core index for single-core builds.
#[cfg(not(feature = "multi-core"))]
#[derive(Copy, Clone, Debug, PartialEq, Eq, core::marker::ConstParamTy)]
pub struct CoreId;

// A zero-sized `CoreId` can only address core 0, so a `multi-core`-off build
// requires `NUM_CORES == 1`.
#[cfg(not(feature = "multi-core"))]
const _: () = assert!(
    NUM_CORES == 1,
    "enable the `multi-core` feature for targets with NUM_CORES > 1",
);

impl CoreId {
    /// The default core for kernel objects that do not pick one explicitly.
    /// Wraps `scars_khal::DEFAULT_CORE` (core 0).
    pub const DEFAULT: Self = Self::new(scars_khal::DEFAULT_CORE);

    /// Construct a `CoreId`, asserting `core < NUM_CORES`.
    #[cfg(feature = "multi-core")]
    pub const fn new(core: u8) -> Self {
        assert!(
            (core as usize) < NUM_CORES,
            "CoreId out of range for this target's NUM_CORES",
        );
        Self(core)
    }

    /// Construct a `CoreId`, asserting `core < NUM_CORES`. The index is
    /// zero-sized, so the value is validated but not stored.
    #[cfg(not(feature = "multi-core"))]
    pub const fn new(core: u8) -> Self {
        assert!(
            (core as usize) < NUM_CORES,
            "CoreId out of range for this target's NUM_CORES",
        );
        Self
    }

    /// Construct a `CoreId` without the range check.
    ///
    /// # Safety
    ///
    /// Caller must guarantee `core < NUM_CORES`. Used at the HAL boundary
    /// where the khal contract already establishes the invariant, avoiding
    /// the range branch on every [`CoreId::current`] call.
    #[cfg(feature = "multi-core")]
    #[inline(always)]
    pub const unsafe fn from_u8_unchecked(core: u8) -> Self {
        Self(core)
    }

    /// Construct a `CoreId` without the range check.
    ///
    /// # Safety
    ///
    /// Caller must guarantee `core < NUM_CORES`. The index is zero-sized, so
    /// the argument is discarded.
    #[cfg(not(feature = "multi-core"))]
    #[inline(always)]
    pub const unsafe fn from_u8_unchecked(_core: u8) -> Self {
        Self
    }

    /// The core index as a `u8`.
    #[cfg(feature = "multi-core")]
    #[inline(always)]
    pub const fn as_u8(self) -> u8 {
        self.0
    }

    /// The core index as a `u8`, always 0 in single-core builds.
    #[cfg(not(feature = "multi-core"))]
    #[inline(always)]
    pub const fn as_u8(self) -> u8 {
        0
    }

    /// The core index as a `usize`, for indexing `[T; NUM_CORES]` arrays.
    #[cfg(feature = "multi-core")]
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        // SAFETY: `CoreId` is constructed only via `new` (which asserts
        // `< NUM_CORES`) or `from_u8_unchecked` (whose caller upholds the
        // same invariant). The hint lets LLVM elide bounds checks on every
        // `arr[core.as_usize()]` where `arr: [T; NUM_CORES]`.
        unsafe { core::hint::assert_unchecked((self.0 as usize) < NUM_CORES) };
        self.0 as usize
    }

    /// The core index as a `usize`, always 0 in single-core builds.
    #[cfg(not(feature = "multi-core"))]
    #[inline(always)]
    pub const fn as_usize(self) -> usize {
        0
    }

    /// The id of the core executing this call.
    #[cfg(feature = "multi-core")]
    #[inline(always)]
    pub fn current() -> Self {
        // SAFETY: the HAL guarantees the returned id is `< NUM_CORES`.
        unsafe { Self::from_u8_unchecked(<kernel_hal::HAL as CoreController>::current_core_id()) }
    }

    /// The id of the core executing this call. Always core 0 in single-core
    /// builds, with no HAL read.
    #[cfg(not(feature = "multi-core"))]
    #[inline(always)]
    pub fn current() -> Self {
        Self
    }
}
