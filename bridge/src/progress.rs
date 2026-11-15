//! Shared progress on the bridge: what this game got during the session, and the session's.

use crate::client::{Connection, Received};
use crate::mailbox::Mailbox;
use crate::ram::ProcessMemory;
use b3313_protocol::Progress;
use std::io;
use std::time::{Duration, Instant};

/// Progress the server has not taken yet is sent again this often, as UDP may lose it.
pub const RESEND_INTERVAL: Duration = Duration::from_secs(1);

/// What this player has to share, and what the session has.
#[derive(Debug, Default)]
pub struct ProgressSync {
    /// The save file played and its progress when first seen: what the player brought, which is
    /// theirs, not the session's.
    baseline: Option<(u8, Progress)>,
    session: Progress,
    /// The last report, and when it went.
    sent: Option<(Progress, Instant)>,
}

impl ProgressSync {
    /// What the game got since joining that the session does not have yet, from its current
    /// progress in `file`. The first sight of a file only sets the baseline.
    pub fn gains(&mut self, file: u8, progress: &Progress) -> Progress {
        match self.baseline {
            Some((known_file, baseline)) if known_file == file => {
                progress.without(&baseline).without(&self.session)
            }
            _ => {
                // A new file, or another one: everything in it was there before.
                self.baseline = Some((file, *progress));
                Progress::default()
            }
        }
    }

    /// Begins anew with a server joined again, which may know nothing of the session's
    /// progress. What the player brought is still theirs alone, so what the game got since
    /// is offered again.
    pub fn start_again(&mut self) {
        *self = Self {
            baseline: self.baseline,
            ..Self::default()
        };
    }

    /// Whether `gains` should be sent now: they are new, or the last report is due again.
    fn is_due(&self, gains: &Progress, now: Instant) -> bool {
        !gains.is_empty()
            && self.sent.is_none_or(|(sent, at)| {
                sent != *gains || now.saturating_duration_since(at) >= RESEND_INTERVAL
            })
    }

    /// Reports what the game got during the session, if anything is new or due again. Call it once
    /// per send slot.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read, or the socket fails.
    pub fn send<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        connection: &mut Connection,
        now: Instant,
    ) -> io::Result<()> {
        let Some((file, progress)) = mailbox.read_progress_out()? else {
            return Ok(());
        };
        let gains = self.gains(file, &progress);
        if self.is_due(&gains, now) {
            connection.send_progress(&gains)?;
            self.sent = Some((gains, now));
        }
        Ok(())
    }

    /// Takes the session's progress, if the server sent it, and gives it to the game.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn receive<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        received: &Received,
    ) -> io::Result<()> {
        let Some(session) = &received.progress else {
            return Ok(());
        };
        self.session = **session;
        mailbox.write_session_progress(session)
    }
}

#[cfg(test)]
mod tests {
    use super::{ProgressSync, RESEND_INTERVAL};
    use b3313_protocol::{PROGRESS_SIZE, Progress};
    use std::time::Instant;

    fn stars(byte: u8) -> Progress {
        let mut bytes = [0; PROGRESS_SIZE];
        bytes[4] = byte;
        Progress::from_bytes(bytes)
    }

    #[test]
    fn only_what_was_got_during_the_session_is_shared() {
        let mut sync = ProgressSync::default();
        // The player brought one star.
        assert!(sync.gains(1, &stars(0b001)).is_empty());
        assert_eq!(sync.gains(1, &stars(0b011)), stars(0b010));
        // The session has it now, and another player's star came in through it.
        sync.session = stars(0b110);
        assert!(sync.gains(1, &stars(0b111)).is_empty());
        // Another file brings its own progress, which is not news either.
        assert!(sync.gains(2, &stars(0b1111)).is_empty());
    }

    #[test]
    fn a_server_joined_again_is_offered_what_the_game_got_before_it() {
        let mut sync = ProgressSync::default();
        assert!(sync.gains(1, &stars(0b001)).is_empty());
        // Got during the session, and the server that had it is gone.
        sync.session = stars(0b010);
        sync.sent = Some((stars(0b010), Instant::now()));

        sync.start_again();

        assert_eq!(sync.gains(1, &stars(0b011)), stars(0b010));
        assert!(sync.sent.is_none());
    }

    #[test]
    fn a_report_goes_when_it_changes_and_again_until_taken() {
        let start = Instant::now();
        let mut sync = ProgressSync::default();
        assert!(sync.is_due(&stars(1), start));
        sync.sent = Some((stars(1), start));
        assert!(!sync.is_due(&stars(1), start));
        assert!(sync.is_due(&stars(3), start));
        assert!(sync.is_due(&stars(1), start + RESEND_INTERVAL));
        assert!(!sync.is_due(&Progress::default(), start));
    }
}
