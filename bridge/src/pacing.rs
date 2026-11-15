//! When the bridge sends the local player and when it draws everyone else.

use b3313_protocol::TICK_INTERVAL;
use std::time::{Duration, Instant};

/// How often the local player is sent, matching the server's tick. Real time, not game ticks: the
/// game's update rate depends on the emulator.
pub const SEND_INTERVAL: Duration = TICK_INTERVAL;
/// How often other players' blended states are written into the game: about 60 times a second,
/// so motion between snapshots shows at the display's rate.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(16);

/// A repeating deadline that keeps to its own timeline.
///
/// Each deadline follows the last one planned, not the moment it was noticed, so lateness does not
/// add up. Checking "has an interval passed since the last time" instead skipped a send whenever a
/// check came just short of the interval, which made every eighth state go missing.
#[derive(Clone, Copy, Debug)]
pub struct Schedule {
    next: Instant,
    interval: Duration,
}

impl Schedule {
    /// Starts with a deadline that is already due.
    #[must_use]
    pub const fn new(start: Instant, interval: Duration) -> Self {
        Self {
            next: start,
            interval,
        }
    }

    /// The next deadline, for waiting until.
    #[must_use]
    pub const fn next(&self) -> Instant {
        self.next
    }

    /// Reports whether the deadline has come, moving it on if so.
    ///
    /// After a long stall, it starts afresh instead of reporting every missed deadline in a burst.
    pub fn is_due(&mut self, now: Instant) -> bool {
        if now < self.next {
            return false;
        }
        self.next += self.interval;
        if self.next <= now {
            self.next = now + self.interval;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Schedule;
    use std::time::{Duration, Instant};

    #[test]
    fn late_checks_do_not_delay_later_deadlines() {
        let start = Instant::now();
        let interval = Duration::from_millis(33);
        let mut schedule = Schedule::new(start, interval);
        // Checked every 16 ms, a little late each time, as a loop drawing frames would.
        let frame_at = |frame: u32| start + Duration::from_micros(u64::from(frame) * 16_500);
        let due = (0..60)
            .filter(|&frame| schedule.is_due(frame_at(frame)))
            .count();

        // 60 frames of 16.5 ms span 990 ms: one send every 33 ms is 30.
        assert_eq!(due, 30);
        assert!(schedule.next() <= start + interval * 31);
    }

    #[test]
    fn a_long_stall_does_not_cause_a_burst() {
        let start = Instant::now();
        let interval = Duration::from_millis(33);
        let mut schedule = Schedule::new(start, interval);
        assert!(schedule.is_due(start));

        let after_stall = start + Duration::from_secs(1);
        assert!(schedule.is_due(after_stall));
        assert!(!schedule.is_due(after_stall));
        assert_eq!(schedule.next(), after_stall + interval);
    }
}
