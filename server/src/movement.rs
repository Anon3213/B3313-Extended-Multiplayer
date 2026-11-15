//! Checks how players move, against generous limits, and flags the unusual for the server's owner.
//!
//! Each state is timed by the sender's sequence number, one per `TICK_INTERVAL`, like the bridge's
//! interpolation, so delays on the way do not inflate speeds. Only moves within one level and area
//! count; a change of either is a warp, not movement.
//!
//! The limits are guesses kept generous rather than measurements: speeds above `flag_speed` are
//! only logged, and the one thing refused is jumping more often than `jumps_per_minute`. Refusing a
//! jump holds the player where they were for everyone else until their jumps come back under the
//! limit; it delays a teleport cheat rather than preventing it, so the flags are what let those
//! who moderate act.

use crate::Config;
use crate::limits::LogLimit;
use b3313_protocol::{TICK_INTERVAL, Whereabouts};
use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

/// Jumps are counted over this window.
const JUMP_WINDOW: Duration = Duration::from_secs(60);

/// What to make of one state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Normal,
    /// Faster than `flag_speed`, in units per second, but passed on.
    Unusual {
        speed: f32,
    },
    /// A jump of `distance` units beyond the player's allowance; not passed on.
    Refused {
        distance: f32,
    },
}

/// One player's movement over one reporting period.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MovementReport {
    /// The highest speed seen, in units per second, not counting jumps.
    pub fastest: f32,
    /// Jumps allowed, and the longest of them.
    pub jumps: u32,
    pub longest_jump: f32,
    /// States flagged as unusually fast, and jumps refused.
    pub unusual: u32,
    pub refused: u32,
}

impl fmt::Display for MovementReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "moved at most {:.0} units/s", self.fastest)?;
        if self.jumps > 0 {
            let (jumps, longest) = (self.jumps, self.longest_jump);
            write!(f, "; {jumps} jumps, the longest {longest:.0} units")?;
        }
        if self.unusual > 0 || self.refused > 0 {
            let (unusual, refused) = (self.unusual, self.refused);
            let flags = format!("{unusual} states too fast, {refused} jumps refused");
            write!(f, "; UNUSUAL: {flags}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Movement {
    /// The last state passed on, which the next is measured from.
    last: Option<(u32, Whereabouts)>,
    /// When recent jumps were allowed.
    jumps: VecDeque<Instant>,
    flags: LogLimit,
    report: MovementReport,
}

impl Movement {
    /// Judges a state sent under `sequence`, which must be newer than any judged before.
    pub fn check(
        &mut self,
        sequence: u32,
        here: Whereabouts,
        now: Instant,
        limits: &Config,
    ) -> Verdict {
        let measured = self.last.and_then(|(last_sequence, before)| {
            let distance = before.distance_to(&here).filter(|d| d.is_finite())?;
            let intervals = sequence.wrapping_sub(last_sequence).max(1);
            let intervals = f32::from(u16::try_from(intervals).unwrap_or(u16::MAX));
            let seconds = TICK_INTERVAL.as_secs_f32() * intervals;
            Some((distance, distance / seconds))
        });
        // The first state, or one in another level or area, sets where the next is measured from.
        let Some((distance, speed)) = measured else {
            self.last = Some((sequence, here));
            return Verdict::Normal;
        };
        if distance > limits.jump_distance {
            while self
                .jumps
                .front()
                .is_some_and(|&jump| now.saturating_duration_since(jump) >= JUMP_WINDOW)
            {
                self.jumps.pop_front();
            }
            let allowance = usize::try_from(limits.jumps_per_minute).unwrap_or(usize::MAX);
            if self.jumps.len() >= allowance {
                self.report.refused += 1;
                return Verdict::Refused { distance };
            }
            self.jumps.push_back(now);
            self.report.jumps += 1;
            self.report.longest_jump = self.report.longest_jump.max(distance);
            self.last = Some((sequence, here));
            return Verdict::Normal;
        }
        self.last = Some((sequence, here));
        self.report.fastest = self.report.fastest.max(speed);
        if speed > limits.flag_speed {
            self.report.unusual += 1;
            return Verdict::Unusual { speed };
        }
        Verdict::Normal
    }

    /// Whether a flag may be logged now; at most one per `LOG_INTERVAL`.
    pub fn may_flag(&mut self, now: Instant) -> bool {
        self.flags.allow(now)
    }

    /// Returns the report so far and starts a new one.
    pub fn take_report(&mut self) -> MovementReport {
        std::mem::take(&mut self.report)
    }
}

#[cfg(test)]
mod tests {
    use super::{JUMP_WINDOW, Movement, Verdict};
    use crate::Config;
    use crate::limits::LOG_INTERVAL as FLAG_INTERVAL;
    use b3313_protocol::{TICK_INTERVAL, Whereabouts};
    use std::time::Instant;

    fn at(x: f32, area: u8) -> Whereabouts {
        Whereabouts {
            level: 16,
            area,
            position: [x, 0.0, 0.0],
        }
    }

    /// Judges a state at `x` in area 1, sent under `sequence`.
    fn step(
        movement: &mut Movement,
        limits: &Config,
        sequence: u32,
        x: f32,
        now: Instant,
    ) -> Verdict {
        movement.check(sequence, at(x, 1), now, limits)
    }

    #[test]
    fn speed_is_timed_by_sequence_and_ordinary_running_is_normal() {
        let (limits, now) = (Config::default(), Instant::now());
        let mut movement = Movement::default();
        step(&mut movement, &limits, 1, 0.0, now);
        // 33 units in one interval, then 66 over two, one state lost on the way: the same speed.
        assert_eq!(step(&mut movement, &limits, 2, 33.0, now), Verdict::Normal);
        assert_eq!(step(&mut movement, &limits, 4, 99.0, now), Verdict::Normal);
        let report = movement.take_report();
        let speed = 33.0 / TICK_INTERVAL.as_secs_f32();
        assert!((report.fastest - speed).abs() < 1.0, "{report}");
    }

    #[test]
    fn unusual_speed_is_flagged_but_passed_on() {
        let (limits, now) = (Config::default(), Instant::now());
        let mut movement = Movement::default();
        step(&mut movement, &limits, 1, 0.0, now);
        // Under the jump distance in one interval, but well over the flag speed.
        let fast = limits.flag_speed * TICK_INTERVAL.as_secs_f32() * 1.5;
        let verdict = step(&mut movement, &limits, 2, fast, now);
        assert!(matches!(verdict, Verdict::Unusual { .. }), "{verdict:?}");
        assert_eq!(movement.take_report().unusual, 1);
    }

    #[test]
    fn jumps_beyond_the_allowance_are_refused_until_it_recovers() {
        let limits = Config {
            jumps_per_minute: 2,
            ..Config::default()
        };
        let start = Instant::now();
        let mut movement = Movement::default();
        step(&mut movement, &limits, 1, 0.0, start);
        let far = limits.jump_distance * 3.0;
        assert_eq!(step(&mut movement, &limits, 2, far, start), Verdict::Normal);
        assert_eq!(step(&mut movement, &limits, 3, 0.0, start), Verdict::Normal);
        let refused = step(&mut movement, &limits, 4, far, start);
        assert!(matches!(refused, Verdict::Refused { .. }), "{refused:?}");
        // Still measured from the last state passed on, so running on from there is refused too.
        let onward = step(&mut movement, &limits, 5, far + 33.0, start);
        assert!(matches!(onward, Verdict::Refused { .. }), "{onward:?}");

        let later = start + JUMP_WINDOW;
        assert_eq!(step(&mut movement, &limits, 6, far, later), Verdict::Normal);
        let report = movement.take_report();
        assert_eq!((report.jumps, report.refused), (3, 2));
    }

    #[test]
    fn a_change_of_area_is_neither_movement_nor_a_jump() {
        let (limits, now) = (Config::default(), Instant::now());
        let mut movement = Movement::default();
        movement.check(1, at(0.0, 1), now, &limits);
        let warped = movement.check(2, at(50_000.0, 2), now, &limits);
        assert_eq!(warped, Verdict::Normal);
        let walked = movement.check(3, at(50_033.0, 2), now, &limits);
        assert_eq!(walked, Verdict::Normal);
        assert_eq!(movement.take_report().jumps, 0);
    }

    #[test]
    fn flags_are_logged_at_most_every_two_seconds() {
        let start = Instant::now();
        let mut movement = Movement::default();
        assert!(movement.may_flag(start));
        assert!(!movement.may_flag(start + FLAG_INTERVAL / 2));
        assert!(movement.may_flag(start + FLAG_INTERVAL));
    }
}
