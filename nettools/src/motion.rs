//! Measures how evenly other players are drawn, for judging interpolation without a game.
//!
//! Simulated players move along one axis at a constant `SPEED`. A perfect link would draw each of
//! them moving the same distance every frame; stalls and catch-up jumps show as variation in the
//! drawn speed.

use b3313_protocol::{PLAYER_STATE_SIZE, PlayerId, PlayerState};
use std::collections::HashMap;
use std::fmt;
use std::time::{Duration, Instant};

/// About a running Mario's speed, in units per second.
pub const SPEED: f32 = 500.0;
/// Frames before this much time has passed are not measured, while the link settles.
const SETTLE: Duration = Duration::from_secs(1);

/// `struct MpPlayerSlot` offsets.
const AREA: usize = 1;
const POSITION_X: usize = 4;

/// A player in area `area` of level 16, `seconds` into a run at `SPEED` along x.
#[must_use]
pub fn moving_state(area: u8, seconds: f32) -> PlayerState {
    let mut state = [0; PLAYER_STATE_SIZE];
    // active, areaIndex, levelNum, then pos[0].
    let place = [1, area, 0, 16];
    let x = (SPEED * seconds).to_be_bytes();
    for (slot, byte) in state.iter_mut().zip(place.iter().chain(&x)) {
        *slot = *byte;
    }
    state
}

fn position_x(state: &PlayerState) -> Option<f32> {
    let bytes = state.get(POSITION_X..POSITION_X + 4)?;
    Some(f32::from_be_bytes(bytes.try_into().ok()?))
}

/// Drawn speeds, frame to frame, for every player drawn in the observer's area. Players elsewhere
/// would not be drawn by the game, and get only occasional updates.
#[derive(Debug)]
pub struct MotionMeter {
    start: Instant,
    area: u8,
    last: HashMap<PlayerId, (Instant, f32)>,
    speeds: Vec<f64>,
}

impl MotionMeter {
    #[must_use]
    pub fn new(start: Instant, area: u8) -> Self {
        Self {
            start,
            area,
            last: HashMap::new(),
            speeds: Vec::new(),
        }
    }

    /// Records one frame: what was drawn at `now`.
    pub fn frame(&mut self, now: Instant, drawn: &[(PlayerId, PlayerState)]) {
        for (player, state) in drawn {
            if state.get(AREA) != Some(&self.area) {
                continue;
            }
            let Some(x) = position_x(state) else {
                continue;
            };
            let previous = self.last.insert(*player, (now, x));
            if now.saturating_duration_since(self.start) < SETTLE {
                continue;
            }
            if let Some((then, last_x)) = previous {
                let seconds = now.saturating_duration_since(then).as_secs_f64();
                if seconds > 0.0 {
                    self.speeds.push(f64::from(x - last_x) / seconds);
                }
            }
        }
    }

    #[must_use]
    pub fn report(&self) -> MotionReport {
        let count = u32::try_from(self.speeds.len()).unwrap_or(u32::MAX);
        let n = f64::from(count.max(1));
        let mean = self.speeds.iter().sum::<f64>() / n;
        let variance = self.speeds.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n;
        let stalls = self.speeds.iter().filter(|&&s| s == 0.0).count();
        MotionReport {
            frames: count,
            mean,
            deviation: variance.sqrt(),
            stalled: u32::try_from(stalls).unwrap_or(u32::MAX),
            fastest: self.speeds.iter().copied().fold(0.0, f64::max),
        }
    }
}

/// How evenly players were drawn over a run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionReport {
    /// Frame-to-frame measurements, summed over every player drawn.
    pub frames: u32,
    pub mean: f64,
    /// The standard deviation of the drawn speed; zero for perfectly even motion.
    pub deviation: f64,
    /// Frames in which a moving player did not move.
    pub stalled: u32,
    pub fastest: f64,
}

impl fmt::Display for MotionReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.frames == 0 {
            return f.write_str("nobody was drawn in the observer's area");
        }
        let share = |part: f64| part * 100.0 / f64::from(self.frames.max(1));
        write!(
            f,
            "drawn at {:.0} units/s on average (true {SPEED}), varying by {:.0} units/s \
             ({:.0}%) frame to frame, {:.0} at the fastest; stalled in {:.1}% of {} frames",
            self.mean,
            self.deviation,
            self.deviation * 100.0 / f64::from(SPEED),
            self.fastest,
            share(f64::from(self.stalled)),
            self.frames,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{MotionMeter, SETTLE, SPEED, moving_state, position_x};
    use b3313_protocol::PlayerId;
    use std::time::{Duration, Instant};

    fn player() -> PlayerId {
        PlayerId::new(1).unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn moving_states_carry_their_place_and_position() {
        let state = moving_state(3, 2.0);
        assert_eq!(state.get(..4), Some(&[1, 3, 0, 16][..]));
        assert_eq!(position_x(&state), Some(SPEED * 2.0));
    }

    #[test]
    fn even_motion_has_no_variation_and_stalls_are_counted() {
        let start = Instant::now();
        let mut even = MotionMeter::new(start, 1);
        let mut halting = MotionMeter::new(start, 1);
        for frame in 0..200_u16 {
            let at = start + Duration::from_millis(u64::from(frame) * 10);
            let seconds = f32::from(frame) / 100.0;
            // A player in another area moves too, but is not measured.
            let other = PlayerId::new(2).unwrap_or_else(|| unreachable!());
            let elsewhere = (other, moving_state(2, 0.0));
            even.frame(at, &[(player(), moving_state(1, seconds)), elsewhere]);
            // Every other frame repeats the last position, then catches up.
            let shown = f32::from(frame - frame % 2) / 100.0;
            halting.frame(at, &[(player(), moving_state(1, shown))]);
        }

        let report = even.report();
        assert!((report.mean - f64::from(SPEED)).abs() < 0.5, "{report}");
        assert!(report.deviation < 0.5, "{report}");
        assert_eq!(report.stalled, 0);
        // Only frames after the settling time count.
        let settled = 200 - u32::try_from(SETTLE.as_millis() / 10).unwrap_or(0);
        assert_eq!(report.frames, settled);

        let report = halting.report();
        assert!((report.mean - f64::from(SPEED)).abs() < 5.0, "{report}");
        assert!(report.deviation > f64::from(SPEED) * 0.9, "{report}");
        assert_eq!(report.stalled, report.frames / 2);
    }
}
