// Reference solution of e-rs-clock (rendered by gen/run with the seeded constants).
use std::fmt;

const HOURS_PER_DAY: i64 = @HOURS@;
const MINUTES_PER_HOUR: i64 = @MINUTES@;
const SEPARATOR: char = @SEPARATOR@;
const ZERO_HOUR_AS_DAY_LENGTH: bool = @ZERO_AS_H@;

/// A time of day on this planet's clock; equal times compare equal.
#[derive(Debug, PartialEq, Eq)]
pub struct Clock {
    minutes: i64,
}

impl Clock {
    fn build(total: i64) -> Self {
        Clock { minutes: total.rem_euclid(HOURS_PER_DAY * MINUTES_PER_HOUR) }
    }

    pub fn new(hours: i32, minutes: i32) -> Self {
        Clock::build(hours as i64 * MINUTES_PER_HOUR + minutes as i64)
    }

    pub fn add_minutes(&self, minutes: i32) -> Self {
        Clock::build(self.minutes + minutes as i64)
    }
}

impl fmt::Display for Clock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut h = self.minutes / MINUTES_PER_HOUR;
        let m = self.minutes % MINUTES_PER_HOUR;
        if h == 0 && ZERO_HOUR_AS_DAY_LENGTH {
            h = HOURS_PER_DAY;
        }
        write!(f, "{h:02}{SEPARATOR}{m:02}")
    }
}
