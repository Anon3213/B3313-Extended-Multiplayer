//! Object sync: which player runs each synced object, and which object states each player needs.
//!
//! Each synced object is run by one player's game, its owner, and every other game in the area
//! follows that owner's states. The owner is the player nearest the object, because an object only
//! updates near the Mario in the game that runs it, and reacts only to that Mario. To stop
//! ownership flipping back and forth, it moves only to a player nearer by `HANDOVER_MARGIN`.
//! A player whose Mario attacks an object or picks it up becomes its owner at once, whoever is
//! nearer, so the touch needs no round trip and lands as that player made it.
//! Objects whose position the server has not heard yet belong to the area's longest-present
//! player, whose game reports them.
//!
//! Only an object's owner may send its state or report it gone, so a tampered client can move or
//! remove only objects near its own player.
//!
//! An enemy a numbered object spawned is run by whoever runs its spawner, under a name every game
//! gives it alike. Which of them are alive is state, as the gone set is: the server sends each
//! player the whole list for its area, and a game removes its copy of another game's enemy that
//! is not in it, or makes one that is and that its own spawner has not spawned. An enemy is alive
//! while its owner keeps sending its state, and not once the owner says it has gone.

use b3313_protocol::objects::{is_child_name, object_id, object_position, owning_number};
use b3313_protocol::{
    MAX_LIVE_CHILDREN, MAX_OBJECT_ID, MAX_OBJECTS_PER_PACKET, ObjectSet, ObjectState, ObjectUpdate,
    Packet, PlayerId, SESSION_SIZE, Whereabouts,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How much nearer than the owner another player must be to take an object over, in world units:
/// the width of a room, so two players moving about together do not keep handing objects back and
/// forth. In play, 300 was too little: within about 1000 units of each other, ownership flipped
/// every few seconds.
pub const HANDOVER_MARGIN: f32 = 600.0;

/// How long an object a player attacked or picked up stays theirs though another player is
/// nearer: long enough to finish a fight with it, after which the nearest player has it again.
pub const CLAIM_HOLD: Duration = Duration::from_secs(3);

/// A level and area.
pub type Place = (i16, u8);

/// A spawned enemy's state is dropped once its owner has not sent one for this long. Owners refresh
/// what they run every second, so the enemy is gone or unloaded, and its last state must not land
/// on one its spawner spawns again later.
pub const CHILD_STATE_LIFETIME: Duration = Duration::from_secs(3);
/// A player is refreshed every time one is asked for until its owned and gone sets have gone this
/// many refreshes without changing, three seconds' worth, and after that every
/// `QUIET_REFRESH_EVERY`th time: a lost packet matters while who runs what is changing, and a
/// refresh is 100 bytes that say nothing new once it has settled.
const SETTLING_REFRESHES: u32 = 9;
const QUIET_REFRESH_EVERY: u32 = 3;

/// One player in the session, as object sync sees them.
#[derive(Clone, Copy, Debug)]
pub struct Present {
    pub player: PlayerId,
    pub whereabouts: Whereabouts,
    /// Order of joining; the lowest in an area owns the objects nobody has reported yet.
    pub joined: u64,
    /// Whether the player's game is running its objects: not while it is paused or at a
    /// menu, when its bridge sends no state. Only a running game is given objects to run.
    pub running: bool,
}

impl Present {
    const fn place(&self) -> Place {
        (self.whereabouts.level, self.whereabouts.area)
    }

    /// How far this player is from `position`; a malformed position is never nearest.
    fn distance_to(&self, position: [f32; 3]) -> f32 {
        let squares: f32 = self
            .whereabouts
            .position
            .iter()
            .zip(position)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        let distance = squares.sqrt();
        if distance.is_nan() {
            f32::INFINITY
        } else {
            distance
        }
    }
}

/// An object's newest state as its owner sent it, with the version it was stored under and when.
#[derive(Clone, Copy, Debug)]
struct Stored {
    update: ObjectUpdate,
    version: u64,
    at: Instant,
}

#[derive(Debug, Default)]
struct Area {
    /// Each object's newest state.
    states: HashMap<u16, Stored>,
    /// Owners of numbered objects; a spawned enemy's is its spawner's.
    owners: HashMap<u16, PlayerId>,
    gone: ObjectSet,
    /// Numbered objects a player claimed by attacking or picking up, and until when the claim
    /// holds against a nearer player.
    claims: HashMap<u16, Instant>,
}

impl Area {
    /// Whether a claim on numbered object `id` still holds at `now`.
    fn is_claimed(&self, id: u16, now: Instant) -> bool {
        self.claims.get(&id).is_some_and(|&until| now < until)
    }

    /// Who runs object `id`: its owner, or its spawner's.
    fn owner_of(&self, id: u16) -> Option<PlayerId> {
        self.owners.get(&owning_number(id)).copied()
    }

    /// The spawned enemies alive here, in order of name: those whose owners still send their
    /// states. None if there are more than a packet lists, so that no game removes one for
    /// being left out.
    fn live_children(&self) -> Option<Vec<u16>> {
        let mut names: Vec<u16> = self
            .states
            .keys()
            .copied()
            .filter(|&id| is_child_name(id))
            .collect();
        names.sort_unstable();
        (names.len() <= MAX_LIVE_CHILDREN).then_some(names)
    }
}

/// What one player has been sent: about which place, the version of each object's state, and
/// the owned and gone sets and the list of spawned enemies alive last sent.
#[derive(Debug, Default)]
struct Sent {
    place: Option<Place>,
    versions: HashMap<u16, u64>,
    owned: ObjectSet,
    gone: ObjectSet,
    children: Option<Vec<u16>>,
    /// Refreshes asked for since the sets or the list last changed.
    unchanged_refreshes: u32,
}

#[derive(Debug, Default)]
pub struct Objects {
    areas: HashMap<Place, Area>,
    /// Raised for every state stored, so newer states have higher versions.
    version: u64,
    sent: [Sent; SESSION_SIZE],
}

impl Objects {
    /// Takes a player's report on the objects of `place`, where that player is now. States and
    /// gone reports count only for objects the player owns.
    pub fn receive(&mut self, from: PlayerId, place: Place, report: &Report, now: Instant) {
        let Some(area) = self.areas.get_mut(&place) else {
            return;
        };
        for state in report.states {
            let id = object_id(state);
            if area.owner_of(id) == Some(from) && !area.gone.contains(id) {
                self.version += 1;
                let update = ObjectUpdate {
                    owner: from,
                    sequence: report.sequence,
                    state: *state,
                };
                let stored = Stored {
                    update,
                    version: self.version,
                    at: now,
                };
                area.states.insert(id, stored);
            }
        }
        for id in report.gone.ids() {
            if area.owner_of(id) == Some(from) {
                area.gone.insert(id);
                area.states.remove(&id);
            }
        }
        for &name in report.gone_children {
            // Gone from the list of those alive at once, rather than when its state lapses.
            if is_child_name(name) && area.owner_of(name) == Some(from) {
                area.states.remove(&name);
            }
        }
    }

    /// Decides who owns each object, for the players now in each place, and forgets places nobody
    /// is in: everything there starts afresh when a player arrives, as it does in the game. Drops
    /// spawned enemies' states that have not been refreshed lately.
    pub fn assign(&mut self, present: &[Present], now: Instant) {
        self.areas
            .retain(|place, _| present.iter().any(|p| p.place() == *place));
        for player in present {
            self.areas.entry(player.place()).or_default();
        }
        for (place, area) in &mut self.areas {
            area.states.retain(|&id, stored| {
                !is_child_name(id)
                    || now.saturating_duration_since(stored.at) < CHILD_STATE_LIFETIME
            });
            let here: Vec<Present> = present
                .iter()
                .filter(|p| p.place() == *place)
                .copied()
                .collect();
            area.assign(&here, now);
        }
    }

    /// Makes `player` the owner of object `id` of `place`, which their Mario attacked or picked
    /// up, so that their own game runs it and the touch lands at once, as they made it. The
    /// claim holds for `CLAIM_HOLD` against players who are merely nearer; another player's
    /// touch takes it over. A spawned enemy goes with its spawner, and so do its siblings.
    ///
    /// The caller checks that the player is where the object is.
    pub fn claim(&mut self, place: Place, id: u16, player: PlayerId, now: Instant) {
        if let Some(area) = self.areas.get_mut(&place) {
            let number = owning_number(id);
            area.owners.insert(number, player);
            area.claims.insert(number, now + CLAIM_HOLD);
        }
    }

    /// What `player`, now in `place`, needs to hear: what it owns, what is gone, and the newest
    /// states from other owners it has not been sent, oldest first, up to a packet's worth.
    ///
    /// Nothing, unless there are states to send, either set has changed since the last packet, or
    /// `refresh` asks for one anyway, so a lost packet is made good.
    pub fn updates_for(&mut self, player: PlayerId, place: Place, refresh: bool) -> Option<Packet> {
        let area = self.areas.get(&place)?;
        let sent = self.sent.get_mut(usize::from(player.get()))?;
        if sent.place != Some(place) {
            *sent = Sent {
                place: Some(place),
                ..Sent::default()
            };
        }
        let mut owned = ObjectSet::default();
        for (&id, _) in area.owners.iter().filter(|&(_, &owner)| owner == player) {
            owned.insert(id);
        }
        // Each with the version this player last had of it, none counting as the oldest.
        let mut due: Vec<(u64, u64, ObjectUpdate)> = area
            .states
            .iter()
            .filter(|&(&id, _)| area.owner_of(id) != Some(player))
            .filter_map(|(&id, stored)| {
                let seen = sent.versions.get(&id).copied().unwrap_or(0);
                (seen < stored.version).then_some((seen, stored.version, stored.update))
            })
            .collect();
        let children = area.live_children();
        let is_changed = sent.owned != owned || sent.gone != area.gone || sent.children != children;
        if is_changed {
            sent.unchanged_refreshes = 0;
        } else if refresh {
            sent.unchanged_refreshes = sent.unchanged_refreshes.saturating_add(1);
        }
        let refresh = refresh
            && (sent.unchanged_refreshes <= SETTLING_REFRESHES
                || sent.unchanged_refreshes.is_multiple_of(QUIET_REFRESH_EVERY));
        if due.is_empty() && !refresh && !is_changed {
            return None;
        }
        // The copies this player has had no news of for longest go first. By each state's own
        // age alone, an area with more news than a packet holds would send the same objects
        // every time, those whose owner's packets come first, and the rest would never move.
        due.sort_unstable_by_key(|&(seen, version, _)| (seen, version));
        due.truncate(MAX_OBJECTS_PER_PACKET);
        for (_, version, update) in &due {
            sent.versions.insert(object_id(&update.state), *version);
        }
        sent.owned = owned;
        sent.gone = area.gone;
        sent.children.clone_from(&children);
        Some(Packet::ObjectUpdates {
            level: place.0,
            area: place.1,
            owned,
            gone: area.gone,
            children,
            states: due.into_iter().map(|(_, _, update)| update).collect(),
        })
    }

    /// Who runs object `id` of `place`, and where it last was, if the server has heard.
    #[must_use]
    pub fn owner_and_position(&self, place: Place, id: u16) -> Option<(PlayerId, [f32; 3])> {
        let area = self.areas.get(&place)?;
        let owner = area.owner_of(id)?;
        let stored = area.states.get(&id)?;
        Some((owner, object_position(&stored.update.state)))
    }

    /// Forgets what a departed player was sent, so whoever takes their number starts afresh.
    pub fn forget(&mut self, player: PlayerId) {
        if let Some(sent) = self.sent.get_mut(usize::from(player.get())) {
            *sent = Sent::default();
        }
    }
}

/// One player's report on the objects where they are, as an `Objects` packet carries it.
#[derive(Clone, Copy, Debug)]
pub struct Report<'a> {
    /// The send slot the player's bridge read the states in.
    pub sequence: u32,
    pub gone: &'a ObjectSet,
    pub gone_children: &'a [u16],
    pub states: &'a [ObjectState],
}

impl Area {
    /// Gives each object an owner among those of `here`, the players in this area, whose
    /// games are running. One a player has claimed stays theirs while the claim holds and
    /// they are here and running. With nobody running, everything stays whose it was: there
    /// is nobody to run it, nor to see it stand still.
    fn assign(&mut self, here: &[Present], now: Instant) {
        let here: Vec<Present> = here.iter().filter(|p| p.running).copied().collect();
        let here = here.as_slice();
        let Some(anchor) = here.iter().min_by_key(|p| p.joined) else {
            return;
        };
        self.claims.retain(|_, until| now < *until);
        for id in 1..=MAX_OBJECT_ID {
            let current = self
                .owners
                .get(&id)
                .and_then(|owner| here.iter().find(|p| p.player == *owner));
            if current.is_some() && self.is_claimed(id, now) {
                continue;
            }
            let position = self
                .states
                .get(&id)
                .map(|stored| object_position(&stored.update.state))
                .filter(|position| position.iter().all(|value| value.is_finite()));
            let owner = match position {
                Some(position) => nearest_keeping(here, anchor, position, current),
                None => current.unwrap_or(anchor),
            };
            self.owners.insert(id, owner.player);
        }
    }
}

/// The player in `here` nearest `position`, unless `current` is within `HANDOVER_MARGIN` of being
/// as near. `fallback` is one of `here`, so there is always an answer.
fn nearest_keeping<'a>(
    here: &'a [Present],
    fallback: &'a Present,
    position: [f32; 3],
    current: Option<&'a Present>,
) -> &'a Present {
    let nearest = here
        .iter()
        .min_by(|a, b| a.distance_to(position).total_cmp(&b.distance_to(position)))
        .unwrap_or(fallback);
    match current {
        Some(owner)
            if owner.distance_to(position) <= nearest.distance_to(position) + HANDOVER_MARGIN =>
        {
            owner
        }
        _ => nearest,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CHILD_STATE_LIFETIME, CLAIM_HOLD, HANDOVER_MARGIN, Objects, Place, Present,
        QUIET_REFRESH_EVERY, Report, SETTLING_REFRESHES,
    };
    use b3313_protocol::objects::object_id;
    use b3313_protocol::{
        MAX_OBJECTS_PER_PACKET, OBJECT_STATE_SIZE, ObjectSet, ObjectState, Packet, PlayerId,
        Whereabouts,
    };
    use std::time::Instant;

    const HALL: Place = (16, 1);

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    fn assign(objects: &mut Objects, present: &[Present]) {
        objects.assign(present, Instant::now());
    }

    /// A report on the hall from player `from` of numbered objects gone and states, and no spawned
    /// enemy gone.
    fn receive(objects: &mut Objects, from: u8, gone: &ObjectSet, states: &[ObjectState]) {
        let report = Report {
            sequence: 0,
            gone,
            gone_children: &[],
            states,
        };
        objects.receive(id(from), HALL, &report, Instant::now());
    }

    fn none() -> ObjectSet {
        ObjectSet::default()
    }

    /// Spawner `spawner`'s child with key `key`, named as the game names it.
    fn child(spawner: u16, key: u16) -> u16 {
        0x8000 | (spawner << 7) | key
    }

    /// The spawned enemies `player` is told are alive, and the states it is sent, by name.
    fn child_news(objects: &mut Objects, player: u8) -> (Option<Vec<u16>>, Vec<u16>) {
        match objects.updates_for(id(player), HALL, true) {
            Some(Packet::ObjectUpdates {
                children, states, ..
            }) => {
                let names = states.iter().map(|update| object_id(&update.state));
                (children, names.collect())
            }
            _ => (None, vec![]),
        }
    }

    #[test]
    fn a_spawned_enemy_is_run_by_whoever_runs_its_spawner() {
        let mut objects = Objects::default();
        // Player 1 is near spawner 3, at 5000, which player 0 reports, there first.
        let players = [at(0, 0.0, 0), at(1, 5000.0, 1)];
        assign(&mut objects, &players);
        receive(&mut objects, 0, &none(), &[state(3, 5000.0, 0)]);
        assign(&mut objects, &players);
        // Player 0 no longer runs the spawner, so its state for the spawner's child does not count.
        let goomba = child(3, 0x11);
        receive(&mut objects, 0, &none(), &[state(goomba, 5000.0, 1)]);
        assert!(!child_news(&mut objects, 0).1.contains(&goomba));
        receive(&mut objects, 1, &none(), &[state(goomba, 5000.0, 2)]);
        assert!(child_news(&mut objects, 0).1.contains(&goomba));
        // Its owner is not sent its own.
        assert!(!child_news(&mut objects, 1).1.contains(&goomba));
    }

    #[test]
    fn the_spawned_enemies_alive_are_those_their_owner_sends_and_has_not_said_are_gone() {
        let mut objects = Objects::default();
        let players = [at(0, 0.0, 0), at(1, 0.0, 1)];
        assign(&mut objects, &players);
        let (goomba, other) = (child(3, 0x11), child(4, 1));
        assert_eq!(child_news(&mut objects, 1).0, Some(vec![]));
        receive(
            &mut objects,
            0,
            &none(),
            &[state(other, 0.0, 1), state(goomba, 0.0, 1)],
        );
        // Everyone is sent the whole list, in order of name, every time: a lost packet is made
        // good by the next.
        for player in [0, 1, 1] {
            assert_eq!(
                child_news(&mut objects, player).0,
                Some(vec![goomba, other])
            );
        }
        let report = Report {
            sequence: 0,
            gone: &ObjectSet::default(),
            gone_children: &[goomba],
            states: &[],
        };
        // Player 1 runs nothing, so its word that the goomba is gone does not count.
        objects.receive(id(1), HALL, &report, Instant::now());
        assert_eq!(child_news(&mut objects, 1).0, Some(vec![goomba, other]));
        objects.receive(id(0), HALL, &report, Instant::now());
        assert_eq!(child_news(&mut objects, 1).0, Some(vec![other]));
        // A change of the list is news by itself, sent without waiting for a refresh.
        receive(&mut objects, 0, &none(), &[state(goomba, 0.0, 2)]);
        objects.updates_for(id(1), HALL, false);
        let report = Report {
            gone_children: &[other],
            ..report
        };
        objects.receive(id(0), HALL, &report, Instant::now());
        assert!(objects.updates_for(id(1), HALL, false).is_some());
        assert!(objects.updates_for(id(1), HALL, false).is_none());
    }

    #[test]
    fn more_spawned_enemies_than_a_packet_lists_are_sent_as_no_list() {
        let mut objects = Objects::default();
        assign(&mut objects, &[at(0, 0.0, 0), at(1, 0.0, 1)]);
        let most = u16::try_from(b3313_protocol::MAX_LIVE_CHILDREN).unwrap_or(u16::MAX);
        let states: Vec<ObjectState> = (0..=most).map(|key| state(child(3, key), 0.0, 0)).collect();
        for chunk in states.chunks(8) {
            receive(&mut objects, 0, &none(), chunk);
        }
        assert_eq!(child_news(&mut objects, 1).0, None);
    }

    #[test]
    fn a_spawned_enemys_state_lapses_when_not_refreshed() {
        let mut objects = Objects::default();
        let players = [at(0, 0.0, 0), at(1, 0.0, 1)];
        let start = Instant::now();
        objects.assign(&players, start);
        let goomba = child(3, 0x11);
        let report = Report {
            sequence: 0,
            gone: &ObjectSet::default(),
            gone_children: &[],
            states: &[state(goomba, 0.0, 1), state(3, 0.0, 1)],
        };
        objects.receive(id(0), HALL, &report, start);
        objects.assign(&players, start + CHILD_STATE_LIFETIME);
        // A newcomer to the hall gets the spawner's state, but not the lapsed child's, which is
        // no longer listed as alive either.
        assert_eq!(child_news(&mut objects, 1), (Some(vec![]), vec![3]));
    }

    fn at(player: u8, x: f32, joined: u64) -> Present {
        Present {
            player: id(player),
            whereabouts: Whereabouts {
                level: HALL.0,
                area: HALL.1,
                position: [x, 0.0, 0.0],
            },
            joined,
            running: true,
        }
    }

    /// Object `object` at `x`, with `marker` in its last byte to tell states apart.
    fn state(object: u16, x: f32, marker: u8) -> ObjectState {
        let mut state = [0; OBJECT_STATE_SIZE];
        state[..2].copy_from_slice(&object.to_be_bytes());
        state[4..8].copy_from_slice(&x.to_be_bytes());
        state[OBJECT_STATE_SIZE - 1] = marker;
        state
    }

    /// Returns what `player` owns and the states it is sent, as (object, marker).
    fn updates(objects: &mut Objects, player: u8) -> (Vec<u16>, Vec<(u16, u8)>, Vec<u16>) {
        match objects.updates_for(id(player), HALL, true) {
            Some(Packet::ObjectUpdates {
                owned,
                gone,
                states,
                ..
            }) => (
                owned.ids().filter(|&object| object <= 3).collect(),
                states
                    .iter()
                    .map(|update| update.state)
                    .map(|s| (object_id(&s), s[OBJECT_STATE_SIZE - 1]))
                    .collect(),
                gone.ids().collect(),
            ),
            _ => (vec![], vec![], vec![]),
        }
    }

    #[test]
    fn unreported_objects_go_to_the_longest_present_then_to_the_nearest() {
        let mut objects = Objects::default();
        let players = [at(0, 0.0, 5), at(1, 5000.0, 2)];
        assign(&mut objects, &players);
        // Player 1 joined first, so it reports everything until positions are known.
        assert_eq!(updates(&mut objects, 1).0, [1, 2, 3]);
        assert_eq!(updates(&mut objects, 0).0, []);

        // Object 2 is next to player 0.
        receive(&mut objects, 1, &none(), &[state(2, 100.0, 1)]);
        assign(&mut objects, &players);
        assert_eq!(updates(&mut objects, 0).0, [2]);
        assert_eq!(updates(&mut objects, 1).0, [1, 3]);
    }

    /// A paused game runs no objects: the enemy beside its player would stand still for
    /// everyone else in the area for as long as the pause lasts.
    #[test]
    fn a_game_that_is_not_running_is_given_no_objects_to_run() {
        let mut objects = Objects::default();
        let both = [at(0, 0.0, 0), at(1, 1000.0, 1)];
        assign(&mut objects, &both);
        receive(&mut objects, 0, &none(), &[state(1, 0.0, 1)]);
        assign(&mut objects, &both);
        assert!(updates(&mut objects, 0).0.contains(&1), "beside player 0");

        let paused = Present {
            running: false,
            ..at(0, 0.0, 0)
        };
        assign(&mut objects, &[paused, at(1, 1000.0, 1)]);
        assert!(updates(&mut objects, 1).0.contains(&1), "far, but running");
        assert!(!updates(&mut objects, 0).0.contains(&1));

        // On again, and still the nearest: its own to run once more.
        assign(&mut objects, &both);
        assert!(updates(&mut objects, 0).0.contains(&1));
    }

    #[test]
    fn ownership_moves_only_to_a_clearly_nearer_player() {
        let mut objects = Objects::default();
        assign(&mut objects, &[at(0, 0.0, 0), at(1, 1000.0, 1)]);
        receive(&mut objects, 0, &none(), &[state(1, 0.0, 1)]);
        assign(&mut objects, &[at(0, 0.0, 0), at(1, 1000.0, 1)]);
        assert!(updates(&mut objects, 0).0.contains(&1));

        // Player 1 comes nearer than player 0, but not by the margin.
        let close = HANDOVER_MARGIN / 2.0;
        assign(&mut objects, &[at(0, close, 0), at(1, -close / 2.0, 1)]);
        assert!(updates(&mut objects, 0).0.contains(&1));

        let apart = [at(0, 2.0 * HANDOVER_MARGIN, 0), at(1, 0.0, 1)];
        assign(&mut objects, &apart);
        assert!(updates(&mut objects, 1).0.contains(&1));
        assert!(!updates(&mut objects, 0).0.contains(&1));
    }

    #[test]
    fn a_claimed_object_is_the_claimers_until_the_claim_lapses_or_another_claims_it() {
        let mut objects = Objects::default();
        // Object 1 is beside player 0; player 1 is far from it.
        let players = [at(0, 0.0, 0), at(1, 5000.0, 1)];
        let start = Instant::now();
        objects.assign(&players, start);
        receive(&mut objects, 0, &none(), &[state(1, 0.0, 1)]);
        objects.assign(&players, start);
        assert!(updates(&mut objects, 0).0.contains(&1));

        // Player 1 attacks it: theirs at once, and their states are the ones that count.
        objects.claim(HALL, 1, id(1), start);
        assert!(updates(&mut objects, 1).0.contains(&1));
        assert!(!updates(&mut objects, 0).0.contains(&1));
        receive(&mut objects, 0, &none(), &[state(1, 0.0, 2)]);
        receive(&mut objects, 1, &none(), &[state(1, 0.0, 3)]);
        assert_eq!(updates(&mut objects, 0).1, [(1, 3)]);

        // It stays theirs though player 0 is far nearer, while the claim holds.
        objects.assign(&players, start + CLAIM_HOLD / 2);
        assert!(updates(&mut objects, 1).0.contains(&1));
        // Player 0 hitting back takes it over.
        objects.claim(HALL, 1, id(0), start + CLAIM_HOLD / 2);
        assert!(updates(&mut objects, 0).0.contains(&1));

        // A claim that has lapsed leaves the object to the nearest player again.
        objects.claim(HALL, 1, id(1), start);
        objects.assign(&players, start + CLAIM_HOLD);
        assert!(updates(&mut objects, 0).0.contains(&1));
    }

    #[test]
    fn a_claim_on_a_spawned_enemy_takes_its_spawner_and_a_claimer_who_leaves_loses_it() {
        let mut objects = Objects::default();
        let players = [at(0, 0.0, 0), at(1, 0.0, 1)];
        let start = Instant::now();
        objects.assign(&players, start);
        objects.claim(HALL, child(3, 0x11), id(1), start);
        assert!(updates(&mut objects, 1).0.contains(&3));

        // Player 1 goes; the claim does not keep the spawner from player 0.
        objects.assign(&players[..1], start);
        assert!(updates(&mut objects, 0).0.contains(&3));
    }

    #[test]
    fn only_the_owners_states_and_gone_reports_count() {
        let mut objects = Objects::default();
        let players = [at(0, 0.0, 0), at(1, 0.0, 1)];
        assign(&mut objects, &players);
        let mut gone = ObjectSet::default();
        gone.insert(2);
        // Player 1 owns nothing yet, so neither its state nor its report counts.
        receive(&mut objects, 1, &gone, &[state(1, 0.0, 7)]);
        assert_eq!(updates(&mut objects, 0).1, []);
        assert_eq!(updates(&mut objects, 0).2, []);

        receive(&mut objects, 0, &gone, &[state(1, 0.0, 8)]);
        let (_, states, gone) = updates(&mut objects, 1);
        assert_eq!(states, [(1, 8)]);
        assert_eq!(gone, [2]);
        // The owner is not sent its own objects' states.
        assert_eq!(updates(&mut objects, 0).1, []);
    }

    #[test]
    fn each_state_is_sent_once_and_again_after_a_change_of_place() {
        let mut objects = Objects::default();
        let players = [at(0, 0.0, 0), at(1, 0.0, 1)];
        assign(&mut objects, &players);
        receive(&mut objects, 0, &none(), &[state(1, 0.0, 1)]);
        assert_eq!(updates(&mut objects, 1).1, [(1, 1)]);
        assert_eq!(updates(&mut objects, 1).1, []);
        receive(&mut objects, 0, &none(), &[state(1, 0.0, 2)]);
        assert_eq!(updates(&mut objects, 1).1, [(1, 2)]);

        // Player 1 goes to another area and comes back: the hall is new to it again.
        let mut elsewhere = at(1, 0.0, 1);
        elsewhere.whereabouts.area = 2;
        assign(&mut objects, &[players[0], elsewhere]);
        assert!(objects.updates_for(id(1), (16, 2), true).is_some());
        assign(&mut objects, &players);
        assert_eq!(updates(&mut objects, 1).1, [(1, 2)]);
    }

    #[test]
    fn nothing_is_sent_without_news_unless_a_refresh_is_due() {
        let mut objects = Objects::default();
        assign(&mut objects, &[at(0, 0.0, 0)]);
        // The first packet tells player 0 what it owns.
        assert!(objects.updates_for(id(0), HALL, false).is_some());
        assert!(objects.updates_for(id(0), HALL, false).is_none());
        assert!(objects.updates_for(id(0), HALL, true).is_some());

        // An object gone is news.
        let mut gone = ObjectSet::default();
        gone.insert(2);
        receive(&mut objects, 0, &gone, &[]);
        assert!(objects.updates_for(id(0), HALL, false).is_some());
        assert!(objects.updates_for(id(0), HALL, false).is_none());
    }

    #[test]
    fn refreshes_thin_out_once_who_runs_what_has_settled_and_return_when_it_changes() {
        let mut objects = Objects::default();
        assign(&mut objects, &[at(0, 0.0, 0)]);
        assert!(objects.updates_for(id(0), HALL, false).is_some());
        let refreshed = |objects: &mut Objects, times: u32| -> u32 {
            (0..times)
                .map(|_| u32::from(objects.updates_for(id(0), HALL, true).is_some()))
                .sum()
        };
        // Every refresh while it settles, then one in three.
        assert_eq!(
            refreshed(&mut objects, SETTLING_REFRESHES),
            SETTLING_REFRESHES
        );
        assert_eq!(refreshed(&mut objects, 6 * QUIET_REFRESH_EVERY), 6);

        // A change is sent at once, and every refresh after it again.
        let mut gone = ObjectSet::default();
        gone.insert(2);
        receive(&mut objects, 0, &gone, &[]);
        assert!(objects.updates_for(id(0), HALL, false).is_some());
        assert_eq!(
            refreshed(&mut objects, SETTLING_REFRESHES),
            SETTLING_REFRESHES
        );
    }

    #[test]
    fn a_place_nobody_is_in_is_forgotten() {
        let mut objects = Objects::default();
        assign(&mut objects, &[at(0, 0.0, 0)]);
        receive(&mut objects, 0, &none(), &[state(1, 0.0, 1)]);
        assign(&mut objects, &[]);
        assign(&mut objects, &[at(0, 0.0, 0), at(1, 0.0, 1)]);
        assert_eq!(updates(&mut objects, 1).1, []);
    }

    #[test]
    fn a_packets_worth_goes_at_a_time_oldest_first() {
        let mut objects = Objects::default();
        let players = [at(0, 0.0, 0), at(1, 0.0, 1)];
        assign(&mut objects, &players);
        let states: Vec<ObjectState> = (1..=15).map(|object| state(object, 0.0, 0)).collect();
        for chunk in states.chunks(5) {
            receive(&mut objects, 0, &none(), chunk);
        }
        let full = u16::try_from(MAX_OBJECTS_PER_PACKET).unwrap_or(u16::MAX);
        let first: Vec<u16> = updates(&mut objects, 1).1.iter().map(|&(o, _)| o).collect();
        assert_eq!(first, (1..=full).collect::<Vec<_>>());
        let rest: Vec<u16> = updates(&mut objects, 1).1.iter().map(|&(o, _)| o).collect();
        assert_eq!(rest, (full + 1..=15).collect::<Vec<_>>());
    }

    /// Ten objects that all move in every tick are more than a packet holds. Each must still
    /// reach the other player in turn, not the same eight every time.
    #[test]
    fn with_more_news_than_a_packet_holds_every_object_still_has_its_turn() {
        let mut objects = Objects::default();
        let players = [at(0, 0.0, 0), at(1, 0.0, 1)];
        assign(&mut objects, &players);
        let mut told = std::collections::HashSet::new();
        for round in 1..=3 {
            let moved: Vec<ObjectState> =
                (1..=10).map(|object| state(object, 0.0, round)).collect();
            receive(&mut objects, 0, &none(), &moved);
            told.extend(updates(&mut objects, 1).1.iter().map(|&(object, _)| object));
        }
        assert_eq!(told.len(), 10, "told of {told:?}");
    }
}
