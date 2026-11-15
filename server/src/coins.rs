//! Shared coins: a coin any player collects counts for, and goes from, every player in the level.
//!
//! The server cannot see a game, so it limits how fast each player's coins may count for everyone:
//! enough for any coin line or ring, but not to hand everyone a hundred coins at once.

use crate::limits::{Bucket, LogLimit};
use crate::objects::Place;
use b3313_protocol::OUTBOX_MOST;
use std::collections::HashMap;
use std::time::Instant;

/// Collections remembered in one area. A level has far fewer coins; beyond this, a coin still
/// counts and goes for everyone there, but is not kept for those who come later. As many as a
/// player who comes later can be told of at once.
const MOST_KEPT_PER_AREA: usize = OUTBOX_MOST;

/// The named coins collected in each area, for as long as anyone is in its level. A game that
/// comes to the area later still has them, so it is told to take them away, uncounted;
/// otherwise its player could collect them again, for everyone.
///
/// A name is not always one coin's alone: a coin an object spawned is named for that object and
/// a few bits of what it is, which coins of one ring can share. So every collection is kept,
/// a name as often as it was collected, and a game takes away one coin of that name for each;
/// collections are never told apart by name, which would count such coins once between them.
#[derive(Debug, Default)]
pub struct CollectedCoins {
    areas: HashMap<Place, Vec<u16>>,
}

impl CollectedCoins {
    /// Records that a coin called `name` was collected in `place`.
    pub fn collect(&mut self, place: Place, name: u16) {
        let names = self.areas.entry(place).or_default();
        if names.len() < MOST_KEPT_PER_AREA {
            names.push(name);
        }
    }

    /// The coins collected in `place`, oldest first; a name once for each of its coins.
    #[must_use]
    pub fn in_area(&self, place: Place) -> &[u16] {
        self.areas.get(&place).map_or(&[], Vec::as_slice)
    }

    /// Forgets every level `has_players` says nobody is in: its coins are back in the game of
    /// whoever enters it next.
    pub fn forget_empty_levels(&mut self, has_players: impl Fn(i16) -> bool) {
        self.areas.retain(|&(level, _), _| has_players(level));
    }
}

/// Coins' worth a player may collect at once, as in a coin ring and a blue coin together.
const ALLOWANCE: u32 = 40;
/// And this much more every second, beyond that.
const REFILL_PER_SECOND: u32 = 10;

/// How many coins' worth one player may still collect for everyone.
#[derive(Clone, Copy, Debug)]
pub struct CoinAllowance {
    worth: Bucket,
    refusals: LogLimit,
}

impl CoinAllowance {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            worth: Bucket::full(f64::from(ALLOWANCE), now),
            refusals: LogLimit::default(),
        }
    }

    /// Takes a coin worth `value`, if the allowance has room for it.
    pub fn take(&mut self, value: u8, now: Instant) -> bool {
        let (rate, most) = (f64::from(REFILL_PER_SECOND), f64::from(ALLOWANCE));
        self.worth.take(f64::from(value), rate, most, now)
    }

    /// Whether a refusal may be logged now; at most one per `LOG_INTERVAL`.
    pub fn may_log_refusal(&mut self, now: Instant) -> bool {
        self.refusals.allow(now)
    }
}

#[cfg(test)]
mod tests {
    use super::{ALLOWANCE, CoinAllowance, CollectedCoins, MOST_KEPT_PER_AREA, REFILL_PER_SECOND};
    use std::time::{Duration, Instant};

    #[test]
    fn collections_are_kept_by_area_until_their_level_is_empty() {
        let mut coins = CollectedCoins::default();
        coins.collect((4, 1), 7);
        // Two coins of one ring that share a name: both collections are kept.
        coins.collect((4, 1), 0x8185);
        coins.collect((4, 1), 0x8185);
        coins.collect((4, 2), 7);
        coins.collect((9, 1), 3);
        assert_eq!(coins.in_area((4, 1)), [7, 0x8185, 0x8185]);
        assert_eq!(coins.in_area((4, 2)), [7]);
        assert_eq!(coins.in_area((5, 1)), []);

        coins.forget_empty_levels(|level| level == 9);
        assert_eq!(coins.in_area((4, 1)), []);
        assert_eq!(coins.in_area((9, 1)), [3]);
    }

    #[test]
    fn an_area_keeps_only_so_many_collections() {
        let mut coins = CollectedCoins::default();
        let most = u16::try_from(MOST_KEPT_PER_AREA).unwrap_or(u16::MAX);
        for name in 1..=most + 5 {
            coins.collect((4, 1), name);
        }
        assert_eq!(coins.in_area((4, 1)).len(), MOST_KEPT_PER_AREA);
    }

    #[test]
    fn a_ring_of_coins_counts_but_a_flood_does_not_until_it_refills() {
        let start = Instant::now();
        let mut allowance = CoinAllowance::new(start);
        let coins = (0..ALLOWANCE).filter(|_| allowance.take(1, start)).count();
        assert_eq!(coins, usize::try_from(ALLOWANCE).unwrap_or(0));
        assert!(!allowance.take(1, start));
        let later = start + Duration::from_secs(1);
        let refilled = (0..ALLOWANCE).filter(|_| allowance.take(1, later)).count();
        assert_eq!(refilled, usize::try_from(REFILL_PER_SECOND).unwrap_or(0));
    }

    #[test]
    fn refusals_are_logged_at_most_every_two_seconds() {
        let start = Instant::now();
        let mut allowance = CoinAllowance::new(start);
        assert!(allowance.may_log_refusal(start));
        assert!(!allowance.may_log_refusal(start + Duration::from_secs(1)));
        assert!(allowance.may_log_refusal(start + Duration::from_secs(2)));
    }
}
