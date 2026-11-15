//! Shared progress: what every player got during the session, which every game adds to its save.
//!
//! The server cannot see a game, so it cannot check that a star was really collected. It limits
//! how fast each player may add progress instead, and logs every addition with who made it, so the
//! server's owner, the host or a moderator can kick or ban a player who hands everyone stars.

use crate::limits::Bucket;
use b3313_protocol::Progress;
use std::time::{Duration, Instant};

/// Progress a player may add at once: a star, then its door, say, and a switch, with room to spare.
const ALLOWANCE: u32 = 8;
/// And one more after this long, beyond that.
const REFILL_INTERVAL: Duration = Duration::from_secs(15);

/// How much progress one player may still add.
#[derive(Clone, Copy, Debug)]
pub struct Allowance(Bucket);

impl Allowance {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self(Bucket::full(f64::from(ALLOWANCE), now))
    }

    /// Takes as much of `count` as there is, as each new star, key, switch or door costs one,
    /// and says how much that was.
    fn take(&mut self, count: u32, now: Instant) -> u32 {
        let rate = REFILL_INTERVAL.as_secs_f64().recip();
        self.0.take_up_to(count, rate, f64::from(ALLOWANCE), now)
    }
}

/// What came of a player's report of their progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing the session lacked.
    Known,
    /// This much new progress, now the session's.
    Added(u32),
    /// This much, while the player's allowance has nothing left; the session did not take it.
    Refused(u32),
}

/// The session's progress: everything each player got during it.
#[derive(Debug, Default)]
pub struct SessionProgress {
    progress: Progress,
    /// Raised whenever the progress grows, so each player is sent it once more.
    version: u64,
}

impl SessionProgress {
    /// Takes a player's report of what their game got during the session: as much of it as
    /// their allowance has room for. The bridge reports the rest again, so it is taken as the
    /// allowance comes back, and none of it is refused for good.
    pub fn receive(
        &mut self,
        report: &Progress,
        allowance: &mut Allowance,
        now: Instant,
    ) -> Outcome {
        let new = report.without(&self.progress);
        if new.is_empty() {
            return Outcome::Known;
        }
        let taken = allowance.take(new.count(), now);
        if taken == 0 {
            return Outcome::Refused(new.count());
        }
        self.progress.add(&new.first(taken));
        self.version += 1;
        Outcome::Added(taken)
    }

    #[must_use]
    pub const fn progress(&self) -> &Progress {
        &self.progress
    }

    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }
}

#[cfg(test)]
mod tests {
    use super::{ALLOWANCE, Allowance, Outcome, REFILL_INTERVAL, SessionProgress};
    use b3313_protocol::{PROGRESS_SIZE, Progress};
    use std::time::Instant;

    /// A report with the first `stars` stars of the star bytes.
    fn stars(stars: usize) -> Progress {
        let mut bytes = [0; PROGRESS_SIZE];
        for star in 0..stars {
            if let Some(byte) = bytes.get_mut(4 + star / 8) {
                *byte |= 1 << (star % 8);
            }
        }
        Progress::from_bytes(bytes)
    }

    /// One player's session, with their allowance, from `start`.
    struct Player {
        session: SessionProgress,
        allowance: Allowance,
        start: Instant,
    }

    impl Player {
        fn new() -> Self {
            let start = Instant::now();
            Self {
                session: SessionProgress::default(),
                allowance: Allowance::new(start),
                start,
            }
        }

        /// Reports the first `count` stars, `waited` refill intervals after the start.
        fn report(&mut self, count: usize, waited: u32) -> Outcome {
            let now = self.start + REFILL_INTERVAL * waited;
            self.session
                .receive(&stars(count), &mut self.allowance, now)
        }
    }

    #[test]
    fn new_progress_is_added_once_and_known_progress_is_not_news() {
        let mut player = Player::new();
        assert_eq!(player.report(2, 0), Outcome::Added(2));
        assert_eq!(player.session.version(), 1);
        assert_eq!(player.report(2, 0), Outcome::Known);
        assert_eq!(player.report(3, 0), Outcome::Added(1));
        assert_eq!(player.session.progress().count(), 3);
    }

    #[test]
    fn more_than_the_allowance_is_taken_only_as_it_refills() {
        let mut player = Player::new();
        // A report the allowance could never hold whole is still taken, a part at a time: a
        // player who got ahead of it must not be shut out for the rest of the session.
        assert_eq!(player.report(40, 0), Outcome::Added(ALLOWANCE));
        assert_eq!(player.session.progress().count(), ALLOWANCE);
        assert_eq!(player.report(40, 0), Outcome::Refused(40 - ALLOWANCE));

        assert_eq!(player.report(40, 3), Outcome::Added(3));
        assert_eq!(player.session.progress().count(), ALLOWANCE + 3);
    }
}
