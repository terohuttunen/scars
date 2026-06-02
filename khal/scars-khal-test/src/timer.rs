use crate::{TestHal, hal};
use core::sync::atomic::Ordering;
use scars_khal::*;

/// Mock tick rate: 1 tick = 1 µs. Round so test deadlines read cleanly.
pub const TICK_FREQ_HZ: u64 = 1_000_000;

/// Sentinel stored in `wakeup` when no alarm is armed.
pub(crate) const NO_WAKEUP: u64 = u64::MAX;

impl AlarmClockController for TestHal {
    const TICK_FREQ_HZ: u64 = self::TICK_FREQ_HZ;

    fn clock_ticks() -> u64 {
        hal().now.load(Ordering::SeqCst)
    }

    fn set_wakeup(at: Option<u64>) {
        hal()
            .wakeup
            .store(at.unwrap_or(NO_WAKEUP), Ordering::SeqCst);
    }
}
