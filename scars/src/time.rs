use crate::kernel::hal::{TICK_FREQ_HZ, clock_ticks};
use core::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, Debug)]
pub struct Duration {
    ticks: u64,
}

impl Duration {
    pub const SECOND: Duration = Duration {
        ticks: TICK_FREQ_HZ,
    };
    pub const MILLISECOND: Duration = Duration {
        ticks: TICK_FREQ_HZ / 1000,
    };
    pub const MICROSECOND: Duration = Duration {
        ticks: TICK_FREQ_HZ / 1_000_000,
    };
    pub const ZERO: Duration = Duration { ticks: 0 };
    pub const MAX: Duration = Duration { ticks: u64::MAX };

    pub const fn new(secs: u64, nanos: u32) -> Duration {
        Duration {
            ticks: Duration::SECOND.ticks * secs
                + (Duration::MICROSECOND.ticks * nanos as u64) / 1000,
        }
    }

    pub const fn from_secs(secs: u64) -> Duration {
        Duration {
            ticks: Duration::SECOND.ticks * secs,
        }
    }

    pub const fn from_millis(millis: u64) -> Duration {
        Duration {
            ticks: Duration::MILLISECOND.ticks * millis,
        }
    }

    pub const fn from_micros(micros: u64) -> Duration {
        Duration {
            ticks: Duration::MICROSECOND.ticks * micros,
        }
    }

    pub const fn from_ticks(ticks: u64) -> Duration {
        Duration { ticks }
    }

    pub const fn as_ticks(&self) -> u64 {
        self.ticks
    }

    pub const fn is_zero(&self) -> bool {
        self.ticks == Duration::ZERO.ticks
    }

    pub const fn as_secs(&self) -> u64 {
        self.ticks / Duration::SECOND.ticks
    }

    pub const fn as_millis(&self) -> u64 {
        self.ticks / Duration::MILLISECOND.ticks
    }

    pub const fn as_micros(&self) -> u64 {
        self.ticks / Duration::MICROSECOND.ticks
    }

    pub const fn as_nanos(&self) -> u64 {
        (self.ticks as u128 * 1_000_000_000 / TICK_FREQ_HZ as u128) as u64
    }

    pub const fn saturating_add(self, rhs: Duration) -> Duration {
        Duration {
            ticks: self.ticks.saturating_add(rhs.ticks),
        }
    }

    pub const fn saturating_sub(self, rhs: Duration) -> Duration {
        Duration {
            ticks: self.ticks.saturating_sub(rhs.ticks),
        }
    }

    pub const fn checked_add(self, rhs: Duration) -> Option<Duration> {
        match self.ticks.checked_add(rhs.ticks) {
            Some(ticks) => Some(Duration { ticks }),
            None => None,
        }
    }

    pub const fn checked_sub(self, rhs: Duration) -> Option<Duration> {
        match self.ticks.checked_sub(rhs.ticks) {
            Some(ticks) => Some(Duration { ticks }),
            None => None,
        }
    }

    /// Whole number of times `rhs` fits in `self`.
    pub const fn div_duration(self, rhs: Duration) -> u64 {
        self.ticks / rhs.ticks
    }
}

impl Mul<u32> for Duration {
    type Output = Duration;
    fn mul(self, rhs: u32) -> Duration {
        Duration {
            ticks: self.ticks * rhs as u64,
        }
    }
}

impl Mul<u64> for Duration {
    type Output = Duration;
    fn mul(self, rhs: u64) -> Duration {
        Duration {
            ticks: self.ticks * rhs,
        }
    }
}

impl Div<u32> for Duration {
    type Output = Duration;
    fn div(self, rhs: u32) -> Duration {
        Duration {
            ticks: self.ticks / rhs as u64,
        }
    }
}

impl Div<u64> for Duration {
    type Output = Duration;
    fn div(self, rhs: u64) -> Duration {
        Duration {
            ticks: self.ticks / rhs,
        }
    }
}

impl Add<Duration> for Duration {
    type Output = Duration;
    fn add(self, rhs: Duration) -> Duration {
        Duration {
            ticks: self.ticks + rhs.ticks,
        }
    }
}

impl Sub<Duration> for Duration {
    type Output = Duration;
    fn sub(self, rhs: Duration) -> Duration {
        Duration {
            ticks: self.ticks - rhs.ticks,
        }
    }
}

impl AddAssign<Duration> for Duration {
    fn add_assign(&mut self, rhs: Duration) {
        self.ticks += rhs.ticks;
    }
}

impl SubAssign<Duration> for Duration {
    fn sub_assign(&mut self, rhs: Duration) {
        self.ticks -= rhs.ticks;
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash, Debug)]
pub struct Instant {
    pub(crate) tick: u64,
}

impl Instant {
    pub const ZERO: Instant = Instant { tick: 0 };
    pub fn now() -> Instant {
        Instant {
            tick: clock_ticks(),
        }
    }

    pub const fn from_ticks(tick: u64) -> Instant {
        Instant { tick }
    }

    pub const fn as_ticks(&self) -> u64 {
        self.tick
    }

    /// Returns the amount of time elapsed since this instant
    ///
    /// Returns a Duration of zero if current time is earlier than self.
    pub fn elapsed(&self) -> Duration {
        self.now_duration_since()
    }

    fn now_duration_since(&self) -> Duration {
        Duration {
            ticks: clock_ticks().saturating_sub(self.tick),
        }
    }

    /// Time elapsed from `earlier` to `self`, saturating at zero if `earlier`
    /// is later.
    pub const fn duration_since(&self, earlier: Instant) -> Duration {
        Duration {
            ticks: self.tick.saturating_sub(earlier.tick),
        }
    }

    pub const fn checked_add(&self, duration: Duration) -> Option<Instant> {
        match self.tick.checked_add(duration.ticks) {
            Some(tick) => Some(Instant { tick }),
            None => None,
        }
    }
}

impl Add<Duration> for Instant {
    type Output = Instant;
    fn add(self, rhs: Duration) -> Instant {
        Instant {
            tick: self.tick + rhs.ticks,
        }
    }
}

impl Sub<Instant> for Instant {
    type Output = Duration;
    fn sub(self, rhs: Instant) -> Duration {
        Duration {
            ticks: self.tick.saturating_sub(rhs.tick),
        }
    }
}

impl AddAssign<Duration> for Instant {
    fn add_assign(&mut self, rhs: Duration) {
        self.tick += rhs.ticks;
    }
}
