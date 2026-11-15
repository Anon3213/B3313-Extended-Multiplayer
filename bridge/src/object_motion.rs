//! Draws the objects other games run on the same delayed timeline as their players.
//!
//! An owner's bridge reads its objects in the send slot it reads its Mario in, and says which,
//! so each object state has a place on that owner's timeline (`interpolation::SenderClock`).
//! Drawing an object at the moment its owner's Mario is drawn keeps the two together: a player
//! seen landing on an enemy is seen doing so as the enemy is squashed, not a tenth of a second
//! apart. It also frees an object's motion from how fast the receiving game runs, which is not
//! the same in every emulator.
//!
//! Unlike a player, an object that is not changing is sent only once a second. So a state is
//! never blended from one much older than itself: the object stood still until just before it.

use crate::interpolation::SenderClock;
use b3313_protocol::objects::{is_child_name, object_id};
use b3313_protocol::{ObjectState, ObjectUpdate, PlayerId, TICK_INTERVAL};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// States kept per object, well beyond the delay players are drawn at.
const HISTORY: usize = 8;
/// The longest a blend from one state to the next takes: three send slots, as an owner with
/// more moving objects than a packet holds sends each every second or third slot.
const BLEND_SPAN: Duration = Duration::from_millis(3 * 33);
const _: () = assert!(BLEND_SPAN.as_millis() == 3 * TICK_INTERVAL.as_millis());
/// Blend weights are fixed point out of this, so integer fields need no float conversions.
const WEIGHT_ONE: u16 = 4096;
/// An object unheard of for this long is forgotten: it is gone, or this game runs it now.
const FORGET_AFTER: Duration = Duration::from_secs(5);

/// Where the blended fields sit in a state's network part (`struct MpObjectState` from `id`).
const POSITION: [usize; 3] = [4, 8, 12];
const ANGLES: [usize; 4] = [24, 26, 28, 30];
const ANIMATION: usize = 42;
const ANIMATION_FRAME: usize = 44;

fn micros(duration: Duration) -> i64 {
    i64::try_from(duration.as_micros()).unwrap_or(i64::MAX)
}

fn float_at(state: &ObjectState, at: usize) -> f32 {
    let bytes = state
        .get(at..at + 4)
        .and_then(|bytes| bytes.try_into().ok());
    bytes.map_or(f32::NAN, f32::from_be_bytes)
}

fn short_at(state: &ObjectState, at: usize) -> i16 {
    let bytes = state
        .get(at..at + 2)
        .and_then(|bytes| bytes.try_into().ok());
    bytes.map_or(0, i16::from_be_bytes)
}

fn put(state: &mut ObjectState, at: usize, bytes: &[u8]) {
    if let Some(field) = state.get_mut(at..at + bytes.len()) {
        field.copy_from_slice(bytes);
    }
}

/// Blends `from` towards `to` by `weight` out of `WEIGHT_ONE`.
///
/// Only where the object is and how it is turned are blended, and the frame of an animation
/// that plays on. What it is doing, and how it looks, change at once: they are the nearer
/// state's.
fn blend(from: &ObjectState, to: &ObjectState, weight: u16) -> ObjectState {
    let mut state = if weight < WEIGHT_ONE / 2 { *from } else { *to };
    let fraction = f32::from(weight) / f32::from(WEIGHT_ONE);
    for at in POSITION {
        let (a, b) = (float_at(from, at), float_at(to, at));
        if a.is_finite() && b.is_finite() {
            put(&mut state, at, &(a + (b - a) * fraction).to_be_bytes());
        }
    }
    // The share of `step` that `weight` covers; never larger than `step`, so it fits.
    let part = |step: i32| {
        let share = step * i32::from(weight) / i32::from(WEIGHT_ONE);
        i16::try_from(share).unwrap_or_default()
    };
    for at in ANGLES {
        let a = short_at(from, at);
        // Angles wrap, so the wrapped difference is the short way round.
        let step = short_at(to, at).wrapping_sub(a);
        let turned = a.wrapping_add(part(i32::from(step)));
        put(&mut state, at, &turned.to_be_bytes());
    }
    let (frame_from, frame_to) = (
        short_at(from, ANIMATION_FRAME),
        short_at(to, ANIMATION_FRAME),
    );
    if from.get(ANIMATION) == to.get(ANIMATION) && frame_to >= frame_from {
        // In i32: frames come from the network, so their difference may not fit an i16.
        let step = i32::from(frame_to) - i32::from(frame_from);
        let frame = frame_from.saturating_add(part(step));
        put(&mut state, ANIMATION_FRAME, &frame.to_be_bytes());
    }
    state
}

/// One object's states, oldest first, at the times its owner's bridge read them.
#[derive(Debug)]
struct Track {
    owner: PlayerId,
    newest: u32,
    states: VecDeque<(i64, ObjectState)>,
    heard: Instant,
    /// Whether a state has come since the game was last given this object.
    is_news: bool,
    /// What the game was last given.
    given: Option<ObjectState>,
}

impl Track {
    /// The state to draw at `time` on the owner's timeline, or the newest if that is unknown.
    fn at(&self, time: Option<i64>) -> Option<ObjectState> {
        let newest = self.states.back().map(|&(_, state)| state);
        let Some(time) = time else {
            return newest;
        };
        let after = self.states.iter().position(|&(at, _)| at > time);
        match after {
            Some(0) => self.states.front().map(|&(_, state)| state),
            Some(index) => {
                let (from_time, from) = self.states.get(index - 1)?;
                let (to_time, to) = self.states.get(index)?;
                // The object was as `from` has it until shortly before `to` was read.
                let start = (*from_time).max(to_time - micros(BLEND_SPAN));
                let span = to_time - start;
                if time <= start || span <= 0 {
                    return Some(*from);
                }
                // `time` is before `to_time`, so the weight is below one.
                let weight = ((time - start) * i64::from(WEIGHT_ONE))
                    .checked_div(span)
                    .unwrap_or(0);
                Some(blend(from, to, u16::try_from(weight).ok()?))
            }
            // Nothing newer yet: hold the newest rather than guess where it went.
            None => newest,
        }
    }
}

/// The recent states of every object another game runs, where this game's player is.
#[derive(Debug, Default)]
pub struct ObjectMotion {
    tracks: HashMap<u16, Track>,
    /// Where, among the objects due, the next draw begins when they are too many for one.
    turn: usize,
}

/// The objects one draw gives the game at most: a third of what its ring holds. The game
/// empties the ring once in each of its frames, between which two draws come, and now and
/// then three; with more, it takes the newest and never sees the rest.
const MOST_GIVEN: usize = crate::layout::len::OBJECTS_IN_ENTRIES / 3;

impl ObjectMotion {
    /// Forgets every object, as when the player goes to another place.
    pub fn clear(&mut self) {
        self.tracks.clear();
    }

    /// Forgets the object named `name`, as when this game comes to run it or it is gone.
    pub fn forget(&mut self, name: u16) {
        self.tracks.remove(&name);
    }

    /// Forgets every spawned enemy that is not in `alive`, the server's list of those there are.
    pub fn forget_children_but(&mut self, alive: &[u16]) {
        self.tracks
            .retain(|name, _| !is_child_name(*name) || alive.contains(name));
    }

    /// Adds a state that arrived at `arrived`. `clock` is its owner's, if their player has been
    /// seen: without one the state has no place in time, and is drawn as soon as it comes.
    pub fn receive(
        &mut self,
        update: &ObjectUpdate,
        clock: Option<&SenderClock>,
        arrived: Instant,
    ) {
        let name = object_id(&update.state);
        let time = clock.and_then(|clock| clock.sent_time(update.sequence));
        let track = self.tracks.entry(name).or_insert_with(|| Track {
            owner: update.owner,
            newest: update.sequence,
            states: VecDeque::with_capacity(HISTORY),
            heard: arrived,
            is_news: true,
            given: None,
        });
        if track.owner != update.owner {
            // Another game runs it now, on another timeline.
            track.owner = update.owner;
            track.states.clear();
        } else if !track.states.is_empty()
            && !(1..u32::MAX / 2).contains(&update.sequence.wrapping_sub(track.newest))
        {
            return;
        }
        track.newest = update.sequence;
        track.heard = arrived;
        track.is_news = true;
        // With no clock the time does not matter: `at` is not asked for one either.
        let time = time.unwrap_or(0);
        // Times must only rise, or a state out of place would be drawn in place of every later
        // one. They do, unless the owner's clock started, or started again, in between.
        track.states.retain(|&(at, _)| at < time);
        if track.states.len() == HISTORY {
            track.states.pop_front();
        }
        track.states.push_back((time, update.state));
    }

    /// The objects to give the game at `now`, each as it is to be drawn: those that have moved
    /// since they were last given, and those a state has come for, which tells the game its
    /// owner still runs it. `clock_of` gives a player's clock.
    pub fn sample<'a>(
        &mut self,
        now: Instant,
        clock_of: impl Fn(PlayerId) -> Option<&'a SenderClock>,
    ) -> Vec<ObjectState> {
        self.tracks
            .retain(|_, track| now.saturating_duration_since(track.heard) < FORGET_AFTER);
        let mut due = Vec::new();
        for (&name, track) in &self.tracks {
            let time = clock_of(track.owner).and_then(|clock| clock.draw_time(now));
            let Some(state) = track.at(time) else {
                continue;
            };
            if track.is_news || track.given != Some(state) {
                due.push((name, state));
            }
        }
        if due.len() > MOST_GIVEN {
            // More than the game takes in between two draws. Each draw goes on where the
            // one before left off, so every object has its turn.
            due.sort_unstable_by_key(|&(name, _)| name);
            let first = self.turn % due.len();
            due.rotate_left(first);
            due.truncate(MOST_GIVEN);
            self.turn = self.turn.wrapping_add(MOST_GIVEN);
        }
        for (name, state) in &due {
            if let Some(track) = self.tracks.get_mut(name) {
                track.is_news = false;
                track.given = Some(*state);
            }
        }
        due.into_iter().map(|(_, state)| state).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{ANIMATION_FRAME, BLEND_SPAN, FORGET_AFTER, ObjectMotion, float_at, short_at};
    use crate::interpolation::{DELAY, SenderClock};
    use b3313_protocol::{OBJECT_STATE_SIZE, ObjectState, ObjectUpdate, PlayerId, TICK_INTERVAL};
    use std::time::{Duration, Instant};

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    /// Object 7 at `x`, turned `yaw`, on frame `frame` of animation 2, doing `action`.
    fn object(x: f32, yaw: i16, frame: i16, action: u8) -> ObjectState {
        let mut state = [0; OBJECT_STATE_SIZE];
        state[1] = 7;
        state[3] = action;
        state[4..8].copy_from_slice(&x.to_be_bytes());
        state[24..26].copy_from_slice(&yaw.to_be_bytes());
        state[42] = 2;
        state[ANIMATION_FRAME..ANIMATION_FRAME + 2].copy_from_slice(&frame.to_be_bytes());
        state
    }

    fn update(sequence: u32, state: ObjectState) -> ObjectUpdate {
        ObjectUpdate {
            owner: id(1),
            sequence,
            state,
        }
    }

    /// Player 1's clock, which has seen send slots 1 to `slots`, each on time from `start`.
    fn clock(start: Instant, slots: u32) -> SenderClock {
        let mut clock = SenderClock::default();
        for slot in 1..=slots {
            clock.observe(slot, start + TICK_INTERVAL * (slot - 1));
        }
        clock
    }

    /// When slot `slot` is drawn: `DELAY` after it was sent.
    fn drawn(start: Instant, slot: u32) -> Instant {
        start + TICK_INTERVAL * (slot - 1) + DELAY
    }

    #[test]
    fn more_objects_than_the_games_ring_holds_take_turns() {
        let now = Instant::now();
        let mut motion = ObjectMotion::default();
        let mut given = std::collections::HashSet::new();
        // A hundred objects, every one with news at every draw.
        for sequence in 1..=5 {
            for name in 1..=100 {
                let mut state = object(0.0, 0, 0, 0);
                state[1] = name;
                motion.receive(&update(sequence, state), None, now);
            }
            let states = motion.sample(now, |_| None);
            assert_eq!(states.len(), super::MOST_GIVEN);
            given.extend(states.iter().map(|state| state[1]));
        }
        assert_eq!(given.len(), 100, "none is passed over draw after draw");
    }

    fn x_at(motion: &mut ObjectMotion, clock: &SenderClock, now: Instant) -> Option<f32> {
        let states = motion.sample(now, |_| Some(clock));
        states.first().map(|state| float_at(state, 4))
    }

    #[test]
    fn an_object_is_drawn_between_its_states_at_the_moment_its_owners_player_is() {
        let start = Instant::now();
        let clock = clock(start, 10);
        let mut motion = ObjectMotion::default();
        motion.receive(&update(4, object(0.0, 0, 0, 1)), Some(&clock), start);
        motion.receive(
            &update(5, object(100.0, 0x4000, 10, 2)),
            Some(&clock),
            start,
        );

        assert_eq!(x_at(&mut motion, &clock, drawn(start, 4)), Some(0.0));
        let halfway = drawn(start, 4) + TICK_INTERVAL / 2;
        let state = motion.sample(halfway, |_| Some(&clock));
        let state = state.first().copied().unwrap_or([0; OBJECT_STATE_SIZE]);
        assert!((float_at(&state, 4) - 50.0).abs() < 1.0);
        assert!((short_at(&state, 24) - 0x2000).abs() < 0x100);
        assert!((short_at(&state, ANIMATION_FRAME) - 5).abs() <= 1);
        // What it is doing is not blended: the nearer state's.
        assert_eq!(state[3], 2);
        assert_eq!(x_at(&mut motion, &clock, drawn(start, 6)), Some(100.0));
    }

    #[test]
    fn an_object_that_stood_still_does_not_glide_through_the_time_it_stood() {
        let start = Instant::now();
        let clock = clock(start, 60);
        let mut motion = ObjectMotion::default();
        // Still for a second, then moved in slot 40.
        motion.receive(&update(10, object(0.0, 0, 0, 1)), Some(&clock), start);
        motion.receive(&update(40, object(90.0, 0, 0, 1)), Some(&clock), start);
        assert_eq!(x_at(&mut motion, &clock, drawn(start, 20)), Some(0.0));
        let before = drawn(start, 40).checked_sub(BLEND_SPAN + Duration::from_millis(1));
        assert_eq!(before.and_then(|at| x_at(&mut motion, &clock, at)), None);
        // It moves only over the last blend span before the new state.
        let late = drawn(start, 40).checked_sub(BLEND_SPAN / 3);
        let x = late.and_then(|at| x_at(&mut motion, &clock, at));
        assert!(x.is_some_and(|x| (x - 60.0).abs() < 2.0), "{x:?}");
    }

    #[test]
    fn a_state_read_before_its_owners_clock_started_does_not_hold_the_object_there() {
        let start = Instant::now();
        // This bridge first heard player 1 in slot 30; the object's first state is from slot 20.
        let mut clock = SenderClock::default();
        for slot in 30..=60 {
            clock.observe(slot, start + TICK_INTERVAL * (slot - 30));
        }
        let mut motion = ObjectMotion::default();
        motion.receive(&update(20, object(0.0, 0, 0, 1)), Some(&clock), start);
        motion.receive(&update(40, object(90.0, 0, 0, 1)), Some(&clock), start);
        let after = start + TICK_INTERVAL * 10 + DELAY;
        assert_eq!(x_at(&mut motion, &clock, after), Some(90.0));
    }

    #[test]
    fn a_state_timed_before_those_kept_replaces_them() {
        let start = Instant::now();
        let clock = clock(start, 60);
        let mut motion = ObjectMotion::default();
        // The first came before this bridge had the owner's clock, so it has no time.
        motion.receive(&update(50, object(0.0, 0, 0, 1)), Some(&clock), start);
        motion.receive(&update(51, object(90.0, 0, 0, 1)), None, start);
        assert_eq!(x_at(&mut motion, &clock, drawn(start, 40)), Some(90.0));
    }

    #[test]
    fn the_game_is_given_an_object_when_it_moves_or_its_owner_is_heard_from() {
        let start = Instant::now();
        let clock = clock(start, 60);
        let mut motion = ObjectMotion::default();
        motion.receive(&update(4, object(0.0, 0, 0, 1)), Some(&clock), start);
        let later = drawn(start, 9);
        assert_eq!(motion.sample(later, |_| Some(&clock)).len(), 1);
        // Nothing new, and it has not moved.
        assert_eq!(motion.sample(later, |_| Some(&clock)).len(), 0);
        // The owner's refresh, a second on, says nothing new; the game hears of it all the same.
        motion.receive(&update(34, object(0.0, 0, 0, 1)), Some(&clock), start);
        assert_eq!(motion.sample(drawn(start, 40), |_| Some(&clock)).len(), 1);
        // An older state arriving late changes nothing.
        motion.receive(&update(30, object(5.0, 0, 0, 1)), Some(&clock), start);
        assert_eq!(motion.sample(drawn(start, 41), |_| Some(&clock)).len(), 0);
    }

    #[test]
    fn a_state_without_a_clock_is_drawn_at_once_and_a_silent_object_is_forgotten() {
        let start = Instant::now();
        let mut motion = ObjectMotion::default();
        motion.receive(&update(4, object(0.0, 0, 0, 1)), None, start);
        motion.receive(&update(5, object(100.0, 0, 0, 1)), None, start);
        let states = motion.sample(start, |_| None);
        assert_eq!(states.first().map(|state| float_at(state, 4)), Some(100.0));

        // Another game takes the object over: its states start afresh.
        let mut other = update(1, object(7.0, 0, 0, 1));
        other.owner = id(2);
        motion.receive(&other, None, start);
        let states = motion.sample(start, |_| None);
        assert_eq!(states.first().map(|state| float_at(state, 4)), Some(7.0));

        motion.receive(&update(9, object(1.0, 0, 0, 1)), None, start);
        assert_eq!(motion.sample(start + FORGET_AFTER, |_| None).len(), 0);
        motion.forget(7);
        motion.clear();
    }
}
