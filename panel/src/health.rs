//! Which numbers deserve the user's attention. Only these are colored, so that color keeps
//! meaning something: a number in its usual range is drawn plainly.
//!
//! The limits are constants that both the colors and their explanations use, so a tooltip can
//! never name a limit other than the one that colored the number.

use b3313_protocol::TICK_INTERVAL;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Good,
    /// Worth watching.
    Fair,
    /// Players notice it, or the server cannot keep up.
    Poor,
}

impl Health {
    /// What a tile says beside a colored number, so the state does not rest on color alone.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Good => "Healthy",
            Self::Fair => "Watch",
            Self::Poor => "Problem",
        }
    }
}

/// Packet loss, as a fraction, from which it is worth watching, and from which movement
/// visibly skips.
const LOSS_FAIR: f64 = 0.01;
const LOSS_POOR: f64 = 0.05;
/// The longest wait between updates, which come every tick (33 ms): a quarter of a second
/// without one shows as others' copy of the player freezing and jumping.
const GAP_FAIR: Duration = Duration::from_millis(100);
const GAP_POOR: Duration = Duration::from_millis(250);

/// A player's lost packets, as a fraction. A few are normal on the internet.
#[must_use]
pub fn packet_loss(rate: f64) -> Health {
    if rate < LOSS_FAIR {
        Health::Good
    } else if rate < LOSS_POOR {
        Health::Fair
    } else {
        Health::Poor
    }
}

#[must_use]
pub fn packet_loss_scale() -> String {
    let (fair, poor) = (LOSS_FAIR * 100.0, LOSS_POOR * 100.0);
    format!(
        "Under {fair:.0}% is normal · {fair:.0}–{poor:.0}% worth watching · {poor:.0}% or more \
         and their movement visibly skips for others"
    )
}

/// The longest wait between a player's updates.
#[must_use]
pub fn update_gap(longest: Duration) -> Health {
    if longest <= GAP_FAIR {
        Health::Good
    } else if longest <= GAP_POOR {
        Health::Fair
    } else {
        Health::Poor
    }
}

#[must_use]
pub fn update_gap_scale() -> String {
    let (fair, poor, tick) = (
        GAP_FAIR.as_millis(),
        GAP_POOR.as_millis(),
        TICK_INTERVAL.as_millis(),
    );
    format!(
        "Updates come every {tick} ms. Up to {fair} ms is normal · up to {poor} ms worth \
         watching · longer and others see them freeze and jump"
    )
}

/// How long a tick takes on average. Past half the tick interval there is little headroom; past
/// all of it the server falls behind.
#[must_use]
pub fn tick_time(mean: Duration) -> Health {
    if mean < TICK_INTERVAL / 2 {
        Health::Good
    } else if mean < TICK_INTERVAL {
        Health::Fair
    } else {
        Health::Poor
    }
}

#[must_use]
pub fn tick_time_scale() -> String {
    let (half, full) = ((TICK_INTERVAL / 2).as_millis(), TICK_INTERVAL.as_millis());
    format!(
        "A tick comes every {full} ms. Under {half} ms is normal · under {full} ms worth \
         watching · longer and the server falls behind"
    )
}

/// A full server turns new players away, which the user may not expect.
#[must_use]
pub fn capacity(players: usize, limit: usize) -> Health {
    if players >= limit {
        Health::Fair
    } else {
        Health::Good
    }
}

/// Packets dropped for sending too fast: any at all means a bridge misbehaves or is tampered with.
#[must_use]
pub fn rate_limited(dropped: u64) -> Health {
    if dropped == 0 {
        Health::Good
    } else {
        Health::Fair
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Health, capacity, packet_loss, packet_loss_scale, rate_limited, tick_time, tick_time_scale,
        update_gap, update_gap_scale,
    };
    use std::time::Duration;

    #[test]
    fn each_measure_has_its_boundaries() {
        assert_eq!(packet_loss(0.0), Health::Good);
        assert_eq!(packet_loss(0.02), Health::Fair);
        assert_eq!(packet_loss(0.05), Health::Poor);

        assert_eq!(update_gap(Duration::from_millis(66)), Health::Good);
        assert_eq!(update_gap(Duration::from_millis(200)), Health::Fair);
        assert_eq!(update_gap(Duration::from_millis(400)), Health::Poor);

        assert_eq!(tick_time(Duration::from_millis(2)), Health::Good);
        assert_eq!(tick_time(Duration::from_millis(20)), Health::Fair);
        assert_eq!(tick_time(Duration::from_millis(40)), Health::Poor);

        assert_eq!(capacity(15, 16), Health::Good);
        assert_eq!(capacity(16, 16), Health::Fair);

        assert_eq!(rate_limited(0), Health::Good);
        assert_eq!(rate_limited(3), Health::Fair);
    }

    #[test]
    fn explanations_name_the_limits_that_color_the_numbers() {
        let loss = packet_loss_scale();
        assert!(
            loss.contains("Under 1%") && loss.contains("5% or more"),
            "{loss}"
        );
        let gap = update_gap_scale();
        assert!(gap.contains("100 ms") && gap.contains("250 ms"), "{gap}");
        let tick = tick_time_scale();
        assert!(tick.contains("16 ms") && tick.contains("33 ms"), "{tick}");
    }
}
