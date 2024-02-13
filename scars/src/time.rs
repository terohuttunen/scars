use crate::kernel::hal::{clock_ticks, TICK_FREQ_HZ};
use core::cmp::Ordering;
use core::ops::{Add, Mul, Sub};

#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug)]
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

    // checked_add, saturating_add, checked_sub, saturating_sub, checked_mul, saturating_mul, checked_div
    // as_secs_f64, as_secs_f32, from_secs_f64, from_secs_f32, mul_f64, mul_f32, div_f64, div_f32
    // div_duration_f64, div_duration_f32
}

impl Mul<u32> for Duration {
    type Output = Duration;
    fn mul(self, rhs: u32) -> Duration {
        Duration {
            ticks: self.ticks * rhs as u64,
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

impl PartialOrd<Self> for Duration {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.ticks.partial_cmp(&other.ticks)
    }
}

// AddAssign, Div, DivAssign, SubAssign, Sum
#[derive(PartialEq, Eq, Clone, Copy, Hash, Debug)]
pub struct Instant {
    pub(crate) tick: u64,
}

impl Instant {
    pub fn now() -> Instant {
        Instant {
            tick: clock_ticks(),
        }
    }

    /// Returns the amount of time elapsed since this instant
    ///
    /// Returns a Duration of zero if current time is earlier than self.
    pub fn elapsed(&self) -> Duration {
        let now_tick = clock_ticks();
        if self.tick < now_tick {
            Duration {
                ticks: now_tick - self.tick,
            }
        } else {
            Duration { ticks: 0 }
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

impl PartialOrd<Self> for Instant {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.tick.partial_cmp(&other.tick)
    }
}
