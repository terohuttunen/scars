#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(not(any(feature = "std", feature = "semihosting", feature = "rtt")))]
compile_error!("scars-bench: enable one of features `std`, `semihosting`, or `rtt`");

#[cfg(feature = "semihosting")]
use semihosting::println;

#[cfg(feature = "rtt")]
use defmt::println;
#[cfg(feature = "rtt")]
use defmt_rtt as _;

unsafe extern "Rust" {
    unsafe fn exit_scars(exit_code: i32) -> !;
}

/// Aggregated timing data for a benchmark, in nanoseconds.
///
/// A `Measurement` accumulates per-iteration samples and tracks the
/// minimum, maximum, and running sum so an average can be computed at
/// report time. Use [`Measurement::add_ns`] to fold in a sample, or
/// [`Measurement::from_total_ns`] when the iteration cost was measured
/// in aggregate (a single bulk timing divided by an iteration count).
#[derive(Clone, Copy)]
pub struct Measurement {
    pub min_ns: u64,
    pub max_ns: u64,
    pub sum_ns: u64,
    pub iters: u32,
}

impl Measurement {
    pub const fn new() -> Self {
        Self {
            min_ns: u64::MAX,
            max_ns: 0,
            sum_ns: 0,
            iters: 0,
        }
    }

    /// Fold one per-iteration sample into the running min/max/avg.
    pub fn add_ns(&mut self, sample_ns: u64) {
        if sample_ns < self.min_ns {
            self.min_ns = sample_ns;
        }
        if sample_ns > self.max_ns {
            self.max_ns = sample_ns;
        }
        self.sum_ns = self.sum_ns.saturating_add(sample_ns);
        self.iters = self.iters.saturating_add(1);
    }

    /// Build a Measurement from a bulk timing — `total_ns` covers `iters`
    /// operations. Min/max collapse to the average since no per-op data
    /// was captured.
    pub const fn from_total_ns(total_ns: u64, iters: u32) -> Self {
        let avg = if iters == 0 {
            0
        } else {
            total_ns / iters as u64
        };
        Self {
            min_ns: avg,
            max_ns: avg,
            sum_ns: total_ns,
            iters,
        }
    }

    pub const fn avg_ns(&self) -> u64 {
        if self.iters == 0 {
            0
        } else {
            self.sum_ns / self.iters as u64
        }
    }
}

impl Default for Measurement {
    fn default() -> Self {
        Self::new()
    }
}

/// Print one line in the canonical bench-output format:
/// `BENCH <name> min=<ns> max=<ns> avg=<ns> unit=ns iters=<n>`
pub fn report(name: &str, m: &Measurement) {
    println!(
        "BENCH {} min={} max={} avg={} unit=ns iters={}",
        name,
        m.min_ns,
        m.max_ns,
        m.avg_ns(),
        m.iters
    );
}

/// Terminate the benchmark binary with success (exit code 0).
pub fn bench_done() -> ! {
    println!("BENCH_DONE");
    unsafe { exit_scars(0) }
}

/// Terminate the benchmark binary with failure (exit code 1).
pub fn bench_fail() -> ! {
    println!("BENCH_FAILED");
    unsafe { exit_scars(1) }
}
