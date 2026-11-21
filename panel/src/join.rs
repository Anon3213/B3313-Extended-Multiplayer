//! Runs a bridge session on a thread of its own, as `b3313-bridge` does in its process, so the
//! Play page can join a server without a command line.
//!
//! The panel reads back what the session reports; neither side waits on the other.

use crate::emulator::Install;
use crate::game_setup::{self, Built, Game};
use b3313_bridge::client::resolve;
use b3313_bridge::session::{self, Event, Input, Options};
use b3313_protocol::Outfit;
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

/// What the session's thread reports.
#[derive(Debug)]
pub enum Notice {
    Event(Event),
    /// No save of the game was found to copy aside before the session, which adds to it:
    /// the emulator keeps this game's save somewhere the panel did not look, or it has none.
    SaveNotCopied,
    /// The session ended, saying why in a sentence for the player, with what the system said
    /// of it when there is more to know.
    Ended {
        reason: String,
        detail: Option<String>,
    },
}

#[derive(Debug)]
pub struct Bridge {
    stop: Arc<AtomicBool>,
    /// Whether the session's thread is still getting ready to join: reading or building the
    /// game, looking the server up, starting the emulator.
    preparing: Arc<AtomicBool>,
    notices: Receiver<Notice>,
    /// What the player types for the chat, a line at a time, and the outfits they choose.
    input: Sender<Input>,
    thread: Option<JoinHandle<()>>,
}

impl Bridge {
    /// Starts a session as `request` says; it attaches to the game as soon as it runs, then
    /// joins. Reading the ROM and looking up the server's name happen on the session's thread,
    /// so the panel never waits on them; what fails there ends the session with a message.
    ///
    /// # Errors
    /// Fails, with a message for the player, if the session's thread cannot start.
    pub fn start(request: Request) -> Result<Self, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (notify, notices) = mpsc::channel();
        let (input, typed) = mpsc::channel();
        let preparing = Arc::new(AtomicBool::new(true));
        let thread = {
            let (stop, preparing) = (Arc::clone(&stop), Arc::clone(&preparing));
            thread::Builder::new()
                .name("b3313-bridge".to_owned())
                .spawn(move || play(request, (&stop, &preparing), &typed, &notify))
                .map_err(|error| format!("cannot start the bridge: {error}"))?
        };
        Ok(Self {
            stop,
            preparing,
            notices,
            input,
            thread: Some(thread),
        })
    }

    /// Sends a line the player typed to the chat; the server sends it back when it has it.
    pub fn say(&self, text: String) {
        // Fails only once the session has ended, which `drain` reports.
        let _ = self.input.send(Input::Say(text));
    }

    /// Has the player wear an outfit from now on; the roster shows it once the server has it.
    pub fn wear(&self, outfit: Outfit) {
        // Fails only once the session has ended, which `drain` reports.
        let _ = self.input.send(Input::Wear(outfit));
    }

    /// Everything reported since the last call.
    #[must_use]
    pub fn drain(&self) -> Vec<Notice> {
        self.notices.try_iter().collect()
    }

    /// Asks the session to leave. Its thread notices within a frame or two, or a quarter of
    /// a second while waiting for the game or for the server to answer, or once the game is
    /// built if it is building it; `drain` then reports the end.
    pub fn ask_to_leave(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    fn shut_down(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // A session still getting ready, which may be building the game for a while yet,
        // has nothing in a game or on a server to take back, and is not waited for: a
        // window being closed would stand still until it had. It ends when it looks at
        // `stop`, or with the panel.
        if self.preparing.load(Ordering::Relaxed) {
            return;
        }
        if let Some(thread) = self.thread.take() {
            // A panic on that thread has already been reported by the panic hook.
            let _ = thread.join();
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// Says the session ended if its thread panics, which nothing else would: the Play page
/// would show it joined for good.
struct EndedOnPanic<'a>(&'a Sender<Notice>);

impl Drop for EndedOnPanic<'_> {
    fn drop(&mut self) {
        if thread::panicking() {
            let reason = "The session stopped for a fault in the panel; a crash report was \
                          written."
                .to_owned();
            let _ = self.0.send(Notice::Ended {
                reason,
                detail: None,
            });
        }
    }
}

/// The session's thread. `preparing` is cleared once it has what it joins with, and from
/// then on is waited for when it is to stop: it has a game to take the session out of.
fn play(
    request: Request,
    (stop, preparing): (&AtomicBool, &AtomicBool),
    typed: &Receiver<Input>,
    notify: &Sender<Notice>,
) {
    let _ended = EndedOnPanic(notify);
    let report = |event| {
        let _ = notify.send(Notice::Event(event));
    };
    let options = request.options(stop, notify);
    preparing.store(false, Ordering::Relaxed);
    let (reason, detail) = match options {
        Ok(options) => match session::run(&options, stop, typed, report) {
            Ok(end) => (end.describe(), end.detail()),
            Err(error) => (format!("Could not join: {error}."), None),
        },
        Err(message) => (message, None),
    };
    let _ = notify.send(Notice::Ended { reason, detail });
}

/// What the Play page asks to join with, as the player gave it.
#[derive(Clone, Debug)]
pub struct Request {
    pub rom: PathBuf,
    /// The player's Super Mario 64 ROM and the B3313 patch, if they are known: what the ROM
    /// is built from again when this panel is newer than it.
    pub sources: Option<(PathBuf, PathBuf)>,
    /// The emulator already running the game, or none to start it.
    pub pid: Option<u32>,
    /// As typed: a name or an address, with its port.
    pub server: String,
    pub password: String,
    /// The name the player chose; empty leaves it to the server.
    pub name: String,
    /// The outfit the player chose, if they have.
    pub outfit: Option<Outfit>,
    /// The key that opens the chat in the game, if the player types it there.
    pub chat_key: Option<char>,
    /// How many seconds the game shows the chat's lines for.
    pub chat_seconds: u8,
    /// The server this panel is hosting, if it is: where it listens, and its host password,
    /// which goes to that server alone, so the player hosting is the host.
    pub hosting: Option<(SocketAddr, String)>,
}

/// Whether `server` is the one this panel hosts at `hosting`: the same port, on this PC. Any
/// other server must never be sent the host password, which would let its owner join as host.
fn is_own_server(server: SocketAddr, hosting: SocketAddr) -> bool {
    // Only an address of this PC can be bound to.
    let is_this_pc = server.ip().is_loopback()
        || server.ip() == hosting.ip()
        || UdpSocket::bind((server.ip(), 0)).is_ok();
    server.port() == hosting.port() && is_this_pc
}

/// The bytes of the ROM at `rom`, to join with. One the panel built, as the game `built_here`,
/// is checked against the one this panel's bridge was made for, and built again with
/// `rebuild`, from the player's two files in `sources`, if an older panel made it; one the
/// player chose themself is taken as it is. `rebuild` is `game_setup::set_up`, but for tests.
fn current_rom(
    rom: &Path,
    built_here: Option<&'static Game>,
    sources: Option<&(PathBuf, PathBuf)>,
    rebuild: impl FnOnce(&Path, &Path) -> Result<Built, String>,
) -> Result<Vec<u8>, String> {
    let read = || {
        std::fs::read(rom)
            .map_err(|error| format!("Could not read the ROM {}: {error}.", rom.display()))
    };
    match read() {
        Ok(bytes) if built_here.is_none_or(|game| game.is_current(&bytes)) => Ok(bytes),
        outdated => {
            let Some((game, (sm64, patch))) = built_here.zip(sources) else {
                return outdated.and(Err(
                    "This panel is newer than its game. Set the game up again on the Play page."
                        .to_owned(),
                ));
            };
            // The player chose no file just now, so what went wrong with one needs saying.
            let built = rebuild(sm64, patch).map_err(|error| {
                format!(
                    "This panel is newer than its game and could not build it again: {error} \
                     Set the game up again on the Play page."
                )
            })?;
            // The patch chosen may be another version's than the ROM joined with, and
            // building that one leaves this ROM as old as it was.
            if built.rom != rom {
                return Err(format!(
                    "This panel is newer than its game, B3313 {}, and the patch chosen on the \
                     Play page is for B3313 {}. Choose the patch of the version you play there.",
                    game.name, built.game.name
                ));
            }
            read()
        }
    }
}

impl Request {
    /// The ROM's bytes, brought up to date if the panel built it; see `current_rom`.
    fn current_rom(&self) -> Result<Vec<u8>, String> {
        current_rom(
            &self.rom,
            game_setup::built_at(&self.rom),
            self.sources.as_ref(),
            game_setup::set_up,
        )
    }

    /// The emulator to join with: the one named, or one started now with the ROM.
    fn emulator(&self) -> Result<u32, String> {
        if let Some(pid) = self.pid {
            return Ok(pid);
        }
        let install = Install::find().ok_or(
            "Parallel Launcher's emulator was not found. Install Parallel Launcher and start \
             any game in it once, or start the multiplayer ROM yourself.",
        )?;
        install
            .start(&self.rom, self.chat_key.is_some())
            .map_err(|error| format!("Could not start the emulator: {error}."))
    }

    /// Copies the game's save aside before a session adds to it, where the emulator is the
    /// one whose saves the panel can find.
    fn back_up_save(&self) -> Result<usize, String> {
        let (Some(install), Some(backups)) = (Install::find(), game_setup::backups_path()) else {
            return Ok(0);
        };
        install
            .back_up_saves(&self.rom, &backups)
            .map_err(|error| format!("Could not copy your save aside before joining: {error}."))
    }

    /// What the session needs, read and looked up; or why that failed, in a sentence.
    fn options(self, stop: &AtomicBool, notify: &Sender<Notice>) -> Result<Options, String> {
        // Before the emulator is started, which a mistyped name should not do.
        let server = resolve(&self.server)
            .map_err(|error| format!("Could not find the server {}: {error}.", self.server))?;
        let rom = self.current_rom()?;
        // Looking the name up and building the game take a while: a player who left
        // meanwhile is not to have the game start in front of them.
        if stop.load(Ordering::Relaxed) {
            return Err("You left the session.".to_owned());
        }
        if self.back_up_save()? == 0 {
            // Said, not passed over: the session changes a save that then has no copy.
            let _ = notify.send(Notice::SaveNotCopied);
        }
        let pid = self.emulator()?;
        let host_password = self
            .hosting
            .filter(|&(hosting, _)| is_own_server(server, hosting))
            .map(|(_, password)| password)
            .unwrap_or_default();
        Ok(Options {
            rom,
            pid,
            server,
            password: self.password,
            outfit: self.outfit,
            name: self.name,
            host_password,
            chat_key: self.chat_key,
            chat_seconds: self.chat_seconds,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{current_rom, is_own_server};
    use crate::game_setup::{Built, GAMES, Game};
    use std::net::SocketAddr;
    use std::path::{Path, PathBuf};

    /// A folder of this test's own, empty, with the player's two files named in it. Nothing
    /// reads those two here: what rebuilds the game is the test's.
    fn scratch(name: &str) -> (PathBuf, (PathBuf, PathBuf)) {
        let folder = std::env::temp_dir().join(format!("b3313-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::create_dir_all(&folder);
        let sources = (folder.join("sm64.z64"), folder.join("b3313.bps"));
        (folder, sources)
    }

    /// Two of the games the panel builds: the one a test's ROM is, and another.
    fn two_games() -> (&'static Game, &'static Game) {
        match GAMES.as_slice() {
            [first, second, ..] => (first, second),
            _ => unreachable!("the panel builds more than one game"),
        }
    }

    fn never_rebuilt(_: &Path, _: &Path) -> Result<Built, String> {
        unreachable!("this ROM is not one to build again")
    }

    #[test]
    fn a_rom_the_player_chose_themself_is_taken_as_it_is() -> std::io::Result<()> {
        let (folder, sources) = scratch("own-rom");
        let rom = folder.join("mine.z64");
        std::fs::write(&rom, b"whatever the player built")?;
        let taken = current_rom(&rom, None, Some(&sources), never_rebuilt);
        assert_eq!(
            taken.as_deref(),
            Ok(b"whatever the player built".as_slice())
        );
        Ok(())
    }

    #[test]
    fn an_older_panels_game_is_built_again_and_the_new_one_played() -> std::io::Result<()> {
        let (folder, sources) = scratch("rebuilt");
        let (game, _) = two_games();
        let rom = folder.join("game.z64");
        std::fs::write(&rom, b"built by an older panel")?;
        let rebuild = |sm64: &Path, patch: &Path| {
            // From the player's two files, and over the old game.
            assert_eq!((sm64, patch), (sources.0.as_path(), sources.1.as_path()));
            std::fs::write(&rom, b"built anew").map_err(|error| error.to_string())?;
            Ok(Built {
                rom: rom.clone(),
                game,
            })
        };
        let taken = current_rom(&rom, Some(game), Some(&sources), rebuild);
        assert_eq!(taken.as_deref(), Ok(b"built anew".as_slice()));
        Ok(())
    }

    #[test]
    fn a_game_that_another_versions_patch_cannot_renew_is_not_played() -> std::io::Result<()> {
        let (folder, sources) = scratch("other-patch");
        let (game, other) = two_games();
        let rom = folder.join("game.z64");
        std::fs::write(&rom, b"built by an older panel")?;
        // The patch chosen is the other version's, so that is the game built, elsewhere.
        let rebuild = |_: &Path, _: &Path| {
            let built = folder.join("other game.z64");
            std::fs::write(&built, b"the other version").map_err(|error| error.to_string())?;
            Ok(Built {
                rom: built,
                game: other,
            })
        };
        let Err(refusal) = current_rom(&rom, Some(game), Some(&sources), rebuild) else {
            unreachable!("the old game must not be played as though it were new");
        };
        // Both versions are named, so the player knows which patch to choose.
        assert!(refusal.contains(game.name), "{refusal}");
        assert!(refusal.contains(other.name), "{refusal}");
        Ok(())
    }

    #[test]
    fn an_older_panels_game_with_nothing_to_build_it_from_is_refused() -> std::io::Result<()> {
        let (folder, _) = scratch("no-sources");
        let (game, _) = two_games();
        let rom = folder.join("game.z64");
        std::fs::write(&rom, b"built by an older panel")?;
        let refused = current_rom(&rom, Some(game), None, never_rebuilt);
        assert!(refused.is_err_and(|message| message.contains("Set the game up again")));
        Ok(())
    }

    fn address(text: &str) -> SocketAddr {
        text.parse()
            .unwrap_or_else(|_| unreachable!("the test's address"))
    }

    #[test]
    fn only_this_pcs_own_server_is_sent_the_host_password() {
        let hosting = address("0.0.0.0:41313");
        assert!(is_own_server(address("127.0.0.1:41313"), hosting));
        assert!(is_own_server(address("[::1]:41313"), hosting));
        // Another server on this PC, and one on another PC at the same port: 203.0.113.0/24 is
        // kept for documentation, so it is no address of this PC.
        assert!(!is_own_server(address("127.0.0.1:41314"), hosting));
        assert!(!is_own_server(address("203.0.113.7:41313"), hosting));
    }
}
