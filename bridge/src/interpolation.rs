//! Draws other players slightly in the past, blending between the states either side.
//!
//! States arrive about every `TICK_INTERVAL`, but unevenly, and some never arrive. Drawing each
//! player `DELAY` behind their newest state means there is almost always a state on both sides of
//! the moment drawn, so motion stays smooth through jitter and the odd lost packet. The cost is
//! that delay, added to every other player's latency.
//!
//! Each player's states are placed in time by the sequence numbers their bridge sent them under,
//! one per `TICK_INTERVAL`. Neither arrival times nor server ticks would do: a state can just make
//! or just miss a tick, which bunched two states into one tick and left the next one empty.
//! Times here are microseconds on one sender's timeline, counted from the first state seen.

use b3313_protocol::{
    PLAYER_STATE_SIZE, PlayerId, PlayerState, PlayerUpdate, SESSION_SIZE, TICK_INTERVAL,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How far behind their newest state players are drawn: three send intervals, so two states in a
/// row can go missing before motion stops.
pub const DELAY: Duration = Duration::from_millis(100);
/// States kept per player, well beyond `DELAY`.
const HISTORY: usize = 8;
/// Arrivals the clock estimate is taken over, about two seconds.
const CLOCK_WINDOW: usize = 64;
/// Blend weights are fixed point out of this, so integer fields need no float conversions.
const WEIGHT_ONE: u16 = 4096;

fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

/// Places one sender's sequence numbers on the local clock.
///
/// A state arrives some unknown time after it was sent. The quickest arrival seen recently is the
/// best estimate of the fixed delay on the way; slower ones were held up, at the server or on the
/// network.
#[derive(Debug, Default)]
pub struct SenderClock {
    /// The first sequence seen and when it arrived; all times count from here.
    anchor: Option<(u32, Instant)>,
    /// How much later than the anchor's pace each recent state arrived.
    lateness: VecDeque<i64>,
}

impl SenderClock {
    /// Records that `sequence` arrived at `arrived`. Sequences must only increase.
    pub fn observe(&mut self, sequence: u32, arrived: Instant) {
        let (anchor_sequence, anchor_time) = *self.anchor.get_or_insert((sequence, arrived));
        let local = micros(arrived.saturating_duration_since(anchor_time));
        let sent = Self::time_since(anchor_sequence, sequence);
        if self.lateness.len() == CLOCK_WINDOW {
            self.lateness.pop_front();
        }
        self.lateness.push_back(local - sent);
    }

    /// How long after the anchor's slot `sequence`'s was, or, negative, how long before: an
    /// object's state may have been read in a slot before the first of its owner's this
    /// bridge heard.
    fn time_since(anchor_sequence: u32, sequence: u32) -> i64 {
        let ahead = sequence.wrapping_sub(anchor_sequence);
        let behind = anchor_sequence.wrapping_sub(sequence);
        let slots = if ahead <= behind {
            i64::from(ahead)
        } else {
            -i64::from(behind)
        };
        slots * micros(TICK_INTERVAL)
    }

    /// When `sequence` was sent, on this sender's timeline, if the clock has started.
    #[must_use]
    pub fn sent_time(&self, sequence: u32) -> Option<i64> {
        self.anchor
            .map(|(anchor_sequence, _)| Self::time_since(anchor_sequence, sequence))
    }

    /// The time to draw at `now`: the newest the sender can have sent, less `DELAY`.
    #[must_use]
    pub fn draw_time(&self, now: Instant) -> Option<i64> {
        let (_, anchor_time) = self.anchor?;
        let quickest = self.lateness.iter().min()?;
        Some(micros(now.saturating_duration_since(anchor_time)) - quickest - micros(DELAY))
    }
}

// `Fields` reads a state in this order: the place's four bytes, then each of these.
const _: () = {
    use crate::layout::len;
    assert!(len::PLAYER_SLOT_POS_OFFSET == 4);
    assert!(len::PLAYER_SLOT_ANGLE_OFFSET == 16);
    assert!(len::PLAYER_SLOT_ANIM_ID_OFFSET == 22);
    assert!(len::PLAYER_SLOT_ANIM_FRAME_OFFSET == 24);
    assert!(len::PLAYER_SLOT_PLATFORM_OFFSET == 26);
    assert!(len::PLAYER_SLOT_PLATFORM_OFFSET_OFFSET == 28);
};

/// A `PlayerState`'s fields, in the layout of the game's `struct MpPlayerSlot`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fields {
    place: [u8; 4],
    position: [f32; 3],
    angle: [i16; 3],
    animation: i16,
    frame: i16,
    /// The numbered object the player stands on, or 0, and where on it they are.
    platform: u16,
    offset: [i16; 3],
    rest: [u8; 2],
}

impl Fields {
    fn read(state: &PlayerState) -> Self {
        let mut bytes = state.as_slice();
        let mut take = |count: usize| {
            let (head, tail) = bytes.split_at(count.min(bytes.len()));
            bytes = tail;
            head
        };
        let place = to_array(take(4));
        let position = [0; 3].map(|_| f32::from_be_bytes(to_array(take(4))));
        let angle = [0; 3].map(|_| i16::from_be_bytes(to_array(take(2))));
        let animation = i16::from_be_bytes(to_array(take(2)));
        let frame = i16::from_be_bytes(to_array(take(2)));
        let platform = u16::from_be_bytes(to_array(take(2)));
        let offset = [0; 3].map(|_| i16::from_be_bytes(to_array(take(2))));
        let rest = to_array(take(2));
        Self {
            place,
            position,
            angle,
            animation,
            frame,
            platform,
            offset,
            rest,
        }
    }

    fn write(&self) -> PlayerState {
        let mut state = [0; PLAYER_STATE_SIZE];
        let mut at = 0;
        // Each field after the last, in the order `read` takes them.
        let mut put = |bytes: &[u8]| {
            if let Some(field) = state.get_mut(at..at + bytes.len()) {
                field.copy_from_slice(bytes);
            }
            at += bytes.len();
        };
        put(&self.place);
        self.position
            .iter()
            .for_each(|value| put(&value.to_be_bytes()));
        self.angle
            .iter()
            .for_each(|value| put(&value.to_be_bytes()));
        put(&self.animation.to_be_bytes());
        put(&self.frame.to_be_bytes());
        put(&self.platform.to_be_bytes());
        self.offset
            .iter()
            .for_each(|value| put(&value.to_be_bytes()));
        put(&self.rest);
        state
    }
}

/// Copies `bytes` into an array, padding with zeros; every caller passes exactly `N`.
fn to_array<const N: usize>(bytes: &[u8]) -> [u8; N] {
    let mut array = [0; N];
    array
        .iter_mut()
        .zip(bytes)
        .for_each(|(slot, &byte)| *slot = byte);
    array
}

/// Blends `from` towards `to` by `weight` out of `WEIGHT_ONE`.
///
/// Only motion is blended. A change of level, area or animation, or a frame that wrapped back
/// to the start of a loop, cannot be blended, so it takes the nearer state whole.
fn blend(from: &PlayerState, to: &PlayerState, weight: u16) -> PlayerState {
    let (a, b) = (Fields::read(from), Fields::read(to));
    let nearer = if weight < WEIGHT_ONE / 2 { a } else { b };
    let finite = a.position.iter().chain(&b.position).all(|v| v.is_finite());
    if a.place != b.place || !finite {
        return *if weight < WEIGHT_ONE / 2 { from } else { to };
    }
    // The share of `step` that `weight` covers; never larger than `step`, so it fits.
    let part = |step: i16| {
        let share = i32::from(step) * i32::from(weight) / i32::from(WEIGHT_ONE);
        i16::try_from(share).unwrap_or(step)
    };
    let fraction = f32::from(weight) / f32::from(WEIGHT_ONE);
    let mut position = a.position;
    for (value, target) in position.iter_mut().zip(b.position) {
        *value += (target - *value) * fraction;
    }
    let mut angle = a.angle;
    for (value, target) in angle.iter_mut().zip(b.angle) {
        // Angles wrap, so the wrapped difference is the short way round.
        *value = value.wrapping_add(part(target.wrapping_sub(*value)));
    }
    // On one platform throughout, where the player is on it moves too; stepping onto or off
    // one cannot be blended, so the nearer state's footing stands.
    let mut offset = nearer.offset;
    if a.platform == b.platform && a.platform != 0 {
        for ((value, from), to) in offset.iter_mut().zip(a.offset).zip(b.offset) {
            // In i32: offsets come from the network, so their difference may not fit an i16.
            let step = i32::from(to) - i32::from(from);
            let share = step * i32::from(weight) / i32::from(WEIGHT_ONE);
            *value = i16::try_from(i32::from(from) + share).unwrap_or(*value);
        }
    }
    let same_loop = a.animation == b.animation && b.frame >= a.frame;
    Fields {
        position,
        angle,
        offset,
        animation: nearer.animation,
        frame: if same_loop {
            // In i32: frames come from the network, so their difference may not fit an i16.
            let step = i32::from(b.frame) - i32::from(a.frame);
            let share = step * i32::from(weight) / i32::from(WEIGHT_ONE);
            i16::try_from(i32::from(a.frame) + share).unwrap_or(nearer.frame)
        } else {
            nearer.frame
        },
        ..nearer
    }
    .write()
}

/// The states seen for one player, oldest first, at the times they were sent.
#[derive(Debug, Default)]
struct Track {
    clock: SenderClock,
    newest: Option<u32>,
    states: VecDeque<(i64, PlayerState)>,
}

impl Track {
    /// Adds a state if it is newer than any so far. Every snapshot repeats a player's newest
    /// state until another arrives, so most repeats are ignored here.
    fn add(&mut self, sequence: u32, arrived: Instant, state: PlayerState) {
        let is_newer = self
            .newest
            .is_none_or(|newest| (1..u32::MAX / 2).contains(&sequence.wrapping_sub(newest)));
        if !is_newer {
            return;
        }
        self.newest = Some(sequence);
        self.clock.observe(sequence, arrived);
        let Some(time) = self.clock.sent_time(sequence) else {
            return;
        };
        if self.states.len() == HISTORY {
            self.states.pop_front();
        }
        self.states.push_back((time, state));
    }

    /// The state to draw at `now`: blended between the states either side, or the nearest end.
    fn at(&self, now: Instant) -> Option<PlayerState> {
        let time = self.clock.draw_time(now)?;
        let after = self.states.iter().position(|&(at, _)| at > time);
        match after {
            Some(0) => self.states.front().map(|&(_, state)| state),
            Some(index) => {
                let (from_time, from) = self.states.get(index - 1)?;
                let (to_time, to) = self.states.get(index)?;
                // Times only increase, so the span is positive and the weight below one.
                let span = to_time - from_time;
                let weight = ((time - from_time) * i64::from(WEIGHT_ONE))
                    .checked_div(span)
                    .unwrap_or(0);
                Some(blend(from, to, u16::try_from(weight).ok()?))
            }
            // Nothing newer yet: hold the newest rather than guess where it went.
            None => self.states.back().map(|&(_, state)| state),
        }
    }
}

/// Every other player's recent states, ready to draw at any moment.
#[derive(Debug, Default)]
pub struct Interpolator {
    tracks: [Option<Track>; SESSION_SIZE],
}

impl Interpolator {
    /// Adds a snapshot that arrived at `arrived`. Players not in `present` have left, and are
    /// dropped at once; present players without an update keep what they had.
    pub fn receive(&mut self, arrived: Instant, present: u16, players: &[PlayerUpdate]) {
        for update in players {
            let index = usize::from(update.player.get());
            if let Some(track) = self.tracks.get_mut(index) {
                let track = track.get_or_insert_with(Track::default);
                track.add(update.sequence, arrived, update.state);
            }
        }
        for (index, track) in (0..).zip(self.tracks.iter_mut()) {
            let is_present = PlayerId::new(index).is_some_and(|id| present & id.bit() != 0);
            if !is_present {
                *track = None;
            }
        }
    }

    /// The clock that places `player`'s send slots in time, once a state of theirs has come.
    /// The objects their game runs are drawn by it too; see `object_motion`.
    #[must_use]
    pub fn clock(&self, player: PlayerId) -> Option<&SenderClock> {
        let track = self.tracks.get(usize::from(player.get()))?.as_ref()?;
        Some(&track.clock)
    }

    /// Every player's state to draw at `now`.
    #[must_use]
    pub fn sample(&self, now: Instant) -> Vec<(PlayerId, PlayerState)> {
        (0..)
            .zip(&self.tracks)
            .filter_map(|(index, track)| Some((PlayerId::new(index)?, track.as_ref()?.at(now)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{DELAY, Fields, Interpolator, SenderClock, WEIGHT_ONE, blend};
    use b3313_protocol::{PlayerId, PlayerState, PlayerUpdate, TICK_INTERVAL};
    use std::time::{Duration, Instant};

    fn fields(x: f32, yaw: i16, animation: i16, frame: i16) -> Fields {
        Fields {
            place: [1, 1, 0, 16],
            position: [x, 50.0, -x],
            angle: [0, yaw, 0],
            animation,
            frame,
            platform: 0,
            offset: [0; 3],
            rest: [0, 0],
        }
    }

    /// Standing still at `offset` along one axis of platform `platform`.
    fn riding(platform: u16, offset: i16) -> Fields {
        Fields {
            platform,
            offset: [offset, 10, 0],
            ..fields(0.0, 0, 5, 0)
        }
    }

    #[test]
    fn a_rider_moves_across_one_platform_and_steps_between_two_at_once() {
        let across = halfway(riding(7, -100), riding(7, 300));
        assert_eq!((across.platform, across.offset), (7, [100, 10, 0]));
        // From one platform to another, or to the ground, there is no between.
        let stepping = Fields::read(&blend(
            &riding(7, -100).write(),
            &riding(8, 300).write(),
            WEIGHT_ONE / 4,
        ));
        assert_eq!((stepping.platform, stepping.offset), (7, [-100, 10, 0]));
        let off = halfway(riding(7, -100), riding(0, 0));
        assert_eq!((off.platform, off.offset), (0, [0, 10, 0]));
        // The platform and the place on it follow the animation frame in the game's layout.
        let state = riding(0x0102, 0x0304).write();
        assert_eq!(state.get(0x1A..0x20), Some(&[1, 2, 3, 4, 0, 10][..]));
    }

    fn halfway(from: Fields, to: Fields) -> Fields {
        Fields::read(&blend(&from.write(), &to.write(), WEIGHT_ONE / 2))
    }

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn fields_match_the_games_byte_layout() {
        let state = fields(1.0, 0x0102, 3, 4).write();
        // active, areaIndex, levelNum; pos[0] = 1.0; ...; angle[1]; animID; animFrame.
        assert_eq!(state.get(..8), Some(&[1, 1, 0, 16, 0x3F, 0x80, 0, 0][..]));
        assert_eq!(state.get(0x12..0x1A), Some(&[1, 2, 0, 0, 0, 3, 0, 4][..]));
        assert_eq!(Fields::read(&state), fields(1.0, 0x0102, 3, 4));
    }

    #[test]
    fn motion_blends_halfway() {
        let middle = halfway(fields(0.0, 1000, 5, 10), fields(100.0, 3000, 5, 20));
        assert_eq!(middle, fields(50.0, 2000, 5, 15));
    }

    #[test]
    fn angles_blend_the_short_way_round() {
        let middle = halfway(
            fields(0.0, i16::MAX - 99, 5, 0),
            fields(0.0, i16::MIN + 100, 5, 0),
        );
        assert!(middle.angle[1] == i16::MAX || middle.angle[1] == i16::MIN);
    }

    #[test]
    fn what_cannot_blend_takes_the_nearer_state() {
        let (from, to) = (fields(0.0, 0, 5, 30), fields(100.0, 0, 6, 2));
        let near_start = Fields::read(&blend(&from.write(), &to.write(), WEIGHT_ONE / 4));
        // A new animation: position still moves, but the animation and frame are the nearer.
        assert_eq!((near_start.animation, near_start.frame), (5, 30));
        assert!((near_start.position[0] - 25.0).abs() < 1e-3);

        let mut elsewhere = to;
        elsewhere.place = [1, 2, 0, 16];
        assert_eq!(halfway(from, elsewhere), elsewhere);
        let looped = halfway(fields(0.0, 0, 5, 30), fields(0.0, 0, 5, 2));
        assert_eq!(looped.frame, 2);
        let broken = fields(f32::NAN, 0, 5, 0);
        assert_eq!(
            Fields::read(&blend(&broken.write(), &to.write(), 1)).animation,
            5
        );
    }

    #[test]
    fn a_slot_before_the_first_heard_has_a_time_before_it() {
        let mut clock = SenderClock::default();
        clock.observe(10, Instant::now());
        let slot = i64::try_from(TICK_INTERVAL.as_micros()).unwrap_or(0);
        assert_eq!(clock.sent_time(12), Some(2 * slot));
        assert_eq!(clock.sent_time(7), Some(-3 * slot));
        // Slot numbers wrap round.
        let mut wrapped = SenderClock::default();
        wrapped.observe(1, Instant::now());
        assert_eq!(wrapped.sent_time(u32::MAX), Some(-2 * slot));
    }

    #[test]
    fn the_clock_follows_the_quickest_arrivals() {
        let start = Instant::now();
        let mut clock = SenderClock::default();
        let interval = TICK_INTERVAL;
        // State 10 is held up 20 ms on the way; state 11 is on time.
        clock.observe(10, start + Duration::from_millis(20));
        clock.observe(11, start + interval);
        clock.observe(12, start + interval * 2 + Duration::from_millis(5));

        // State 11's arrival shows the way adds nothing, so at state 12's moment the sender is
        // 66 ms past state 10, and drawing happens 100 ms before that.
        let now = start + interval * 2;
        assert_eq!(clock.draw_time(now), Some(66_000 - 100_000));
        assert_eq!(DELAY, Duration::from_millis(100));
    }

    fn state_at(x: f32) -> PlayerState {
        fields(x, 0, 5, 0).write()
    }

    fn update(player: u8, sequence: u32, x: f32) -> PlayerUpdate {
        PlayerUpdate {
            player: id(player),
            sequence,
            state: state_at(x),
        }
    }

    /// Receives player 3 moving 33 units per state, arriving `late(sequence)` after it was sent.
    fn moving(interpolator: &mut Interpolator, start: Instant, late: impl Fn(u32) -> Duration) {
        for sequence in 0..6_u32 {
            let arrived = start + TICK_INTERVAL * sequence + late(sequence);
            let x = 33.0 * f32::from(u16::try_from(sequence).unwrap_or(0));
            interpolator.receive(arrived, id(3).bit(), &[update(3, sequence, x)]);
        }
    }

    fn x_of(players: &[(PlayerId, PlayerState)]) -> Vec<(u8, f32)> {
        players
            .iter()
            .map(|(player, state)| (player.get(), Fields::read(state).position[0]))
            .collect()
    }

    #[test]
    fn players_are_drawn_between_states_and_held_at_the_newest() {
        let start = Instant::now();
        let mut interpolator = Interpolator::default();
        assert_eq!(interpolator.sample(start), []);
        moving(&mut interpolator, start, |_| Duration::ZERO);

        // Drawn 100 ms behind 165 ms: at 65 ms, so 65 units along.
        let now = start + TICK_INTERVAL * 5;
        let drawn = x_of(&interpolator.sample(now));
        assert_eq!(drawn.len(), 1);
        let x = drawn.first().map_or(0.0, |&(_, x)| x);
        assert!((x - 65.0).abs() < 0.1, "{drawn:?}");
        // Long after the last state, the newest is held.
        let later = x_of(&interpolator.sample(now + Duration::from_secs(1)));
        assert_eq!(later, [(3, 165.0)]);
    }

    #[test]
    fn states_are_timed_by_when_they_were_sent_not_when_they_arrived() {
        let start = Instant::now();
        let mut interpolator = Interpolator::default();
        // Every other state waits 25 ms longer on the way, so they arrive in pairs.
        moving(&mut interpolator, start, |sequence| {
            Duration::from_millis(u64::from(sequence % 2) * 25)
        });
        // A snapshot repeating an older state changes nothing.
        let later = start + TICK_INTERVAL * 6;
        interpolator.receive(later, id(3).bit(), &[update(3, 4, 0.0)]);

        let drawn = x_of(&interpolator.sample(start + TICK_INTERVAL * 5));
        let x = drawn.first().map_or(0.0, |&(_, x)| x);
        assert!((x - 65.0).abs() < 0.1, "{drawn:?}");
    }

    #[test]
    fn a_player_missing_from_a_snapshot_is_dropped() {
        let start = Instant::now();
        let mut interpolator = Interpolator::default();
        let both = id(0).bit() | id(4).bit();
        interpolator.receive(start, both, &[update(0, 1, 0.0), update(4, 1, 0.0)]);
        interpolator.receive(start + TICK_INTERVAL, id(4).bit(), &[update(4, 2, 1.0)]);

        let drawn = interpolator.sample(start + Duration::from_secs(1));
        assert_eq!(x_of(&drawn), [(4, 1.0)]);
    }

    #[test]
    fn a_present_player_without_an_update_is_kept() {
        let start = Instant::now();
        let mut interpolator = Interpolator::default();
        let both = id(0).bit() | id(4).bit();
        interpolator.receive(start, both, &[update(0, 1, 7.0), update(4, 1, 0.0)]);
        // Player 0 is elsewhere now, so this snapshot carries only player 4.
        interpolator.receive(start + TICK_INTERVAL, both, &[update(4, 2, 1.0)]);

        let drawn = interpolator.sample(start + Duration::from_secs(1));
        assert_eq!(x_of(&drawn), [(0, 7.0), (4, 1.0)]);
    }
}
