//! Numbers as the panel shows them.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// `0:42`, `12:05` or `3:02:09`.
#[must_use]
pub fn clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// A time of day, given as time since midnight: `21:05:09`, or `9:05:09 PM` on the 12-hour
/// clock.
#[must_use]
pub fn time_of_day(since_midnight: Duration, hour24: bool) -> String {
    let seconds = since_midnight.as_secs() % (24 * 3600);
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hour24 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        let half = if hours < 12 { "AM" } else { "PM" };
        let hour = match hours % 12 {
            0 => 12,
            hour => hour,
        };
        format!("{hour}:{minutes:02}:{seconds:02} {half}")
    }
}

/// How traffic is written: in bytes, or in bits as internet speeds usually are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RateUnit {
    #[default]
    Bytes,
    Bits,
}

impl RateUnit {
    pub const ALL: [Self; 2] = [Self::Bytes, Self::Bits];

    /// How the panel's preferences save it.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Bytes => "bytes",
            Self::Bits => "bits",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.key() == key)
    }

    /// How many of this unit a byte is, as a chart's values are multiplied for its scale.
    #[must_use]
    pub fn per_byte(self) -> f64 {
        match self {
            Self::Bytes => 1.0,
            Self::Bits => 8.0,
        }
    }

    /// The names of a rate in this unit, in thousands as network rates are.
    fn names(self) -> [&'static str; 3] {
        match self {
            Self::Bytes => ["B/s", "KB/s", "MB/s"],
            Self::Bits => ["bit/s", "kbit/s", "Mbit/s"],
        }
    }
}

/// The unit the user chose, one for the whole panel: every rate on screen is written in it,
/// from places that have no other way to ask.
static RATES_IN_BITS: AtomicBool = AtomicBool::new(false);

/// Writes rates in `unit` from now on.
pub fn set_rate_unit(unit: RateUnit) {
    RATES_IN_BITS.store(unit == RateUnit::Bits, Ordering::Relaxed);
}

#[must_use]
pub fn rate_unit() -> RateUnit {
    if RATES_IN_BITS.load(Ordering::Relaxed) {
        RateUnit::Bits
    } else {
        RateUnit::Bytes
    }
}

/// `850 B/s`, `4.2 KB/s` or `1.3 MB/s`, or in bits if the user chose them.
#[must_use]
pub fn byte_rate(bytes_per_second: f64) -> String {
    rate_in(bytes_per_second, rate_unit())
}

/// A rate of `bytes_per_second` written in `unit`, to one place past thousands.
fn rate_in(bytes_per_second: f64, unit: RateUnit) -> String {
    let rate = bytes_per_second * unit.per_byte();
    let [ones, thousands, millions] = unit.names();
    if rate >= 1_000_000.0 {
        format!("{:.1} {millions}", rate / 1_000_000.0)
    } else if rate >= 1000.0 {
        format!("{:.1} {thousands}", rate / 1000.0)
    } else {
        format!("{rate:.0} {ones}")
    }
}

/// `38 ms`, or `0.21 ms` for times under one.
#[must_use]
pub fn millis(duration: Duration) -> String {
    let millis = duration.as_secs_f64() * 1000.0;
    if millis < 1.0 {
        format!("{millis:.2} ms")
    } else {
        format!("{millis:.0} ms")
    }
}

/// A chart scale's value in milliseconds, as `0.05 ms` or `20 ms`: it is a round number, so
/// it is written as it is, not to a fixed number of places.
#[must_use]
pub fn scale_millis(millis: f64) -> String {
    format!("{millis} ms")
}

/// A chart scale's rate, already in the user's unit, as `500 B/s`, `20 KB/s` or `2 Mbit/s`.
#[must_use]
pub fn scale_rate(rate: f64) -> String {
    scale_rate_in(rate, rate_unit())
}

fn scale_rate_in(rate: f64, unit: RateUnit) -> String {
    let [ones, thousands, millions] = unit.names();
    if rate >= 1_000_000.0 {
        format!("{} {millions}", rate / 1_000_000.0)
    } else if rate >= 1000.0 {
        format!("{} {thousands}", rate / 1000.0)
    } else {
        format!("{rate} {ones}")
    }
}

/// A chart scale's value as a percentage, as `0.5%`.
#[must_use]
pub fn scale_percent(percent: f64) -> String {
    format!("{percent}%")
}

/// A chart scale's value as a count, as `5`.
#[must_use]
pub fn scale_count(count: f64) -> String {
    count.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        RateUnit, clock, millis, rate_in, scale_count, scale_millis, scale_percent, scale_rate_in,
        time_of_day,
    };
    use std::time::Duration;

    #[test]
    fn scale_values_are_written_without_needless_places() {
        assert_eq!(scale_rate_in(500.0, RateUnit::Bytes), "500 B/s");
        assert_eq!(scale_rate_in(20_000.0, RateUnit::Bytes), "20 KB/s");
        assert_eq!(scale_rate_in(2_000_000.0, RateUnit::Bits), "2 Mbit/s");
        assert_eq!(scale_millis(0.05), "0.05 ms");
        assert_eq!(scale_percent(0.5), "0.5%");
        assert_eq!(scale_count(5.0), "5");
    }

    #[test]
    fn times_of_day_follow_the_chosen_clock() {
        let evening = Duration::from_secs(21 * 3600 + 5 * 60 + 9);
        assert_eq!(time_of_day(evening, true), "21:05:09");
        assert_eq!(time_of_day(evening, false), "9:05:09 PM");
        let midnight = Duration::from_secs(30);
        assert_eq!(time_of_day(midnight, true), "00:00:30");
        assert_eq!(time_of_day(midnight, false), "12:00:30 AM");
        let noon = Duration::from_hours(12);
        assert_eq!(time_of_day(noon, false), "12:00:00 PM");
    }

    #[test]
    fn clocks_show_hours_only_when_there_are_some() {
        assert_eq!(clock(Duration::from_secs(42)), "0:42");
        assert_eq!(clock(Duration::from_secs(725)), "12:05");
        assert_eq!(clock(Duration::from_secs(3 * 3600 + 129)), "3:02:09");
    }

    #[test]
    fn rates_pick_a_readable_unit() {
        assert_eq!(rate_in(850.0, RateUnit::Bytes), "850 B/s");
        assert_eq!(rate_in(4210.0, RateUnit::Bytes), "4.2 KB/s");
        assert_eq!(rate_in(1_260_000.0, RateUnit::Bytes), "1.3 MB/s");
        // Eight bits a byte, in thousands too.
        assert_eq!(rate_in(100.0, RateUnit::Bits), "800 bit/s");
        assert_eq!(rate_in(4210.0, RateUnit::Bits), "33.7 kbit/s");
        assert_eq!(rate_in(1_260_000.0, RateUnit::Bits), "10.1 Mbit/s");
        assert_eq!(RateUnit::from_key("bits"), Some(RateUnit::Bits));
        assert_eq!(RateUnit::from_key("nibbles"), None);
        assert_eq!(millis(Duration::from_micros(210)), "0.21 ms");
        assert_eq!(millis(Duration::from_millis(38)), "38 ms");
    }
}
