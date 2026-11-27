//! Runs a bridge session on a thread of its own, as `b3313-bridge` does in its process, so the
//! Play page can join a server without a command line.
//!
//! The panel reads back what the session reports; neither side waits on the other.

use crate::emulator::Install;
use crate::game_setup::{self, Built, Game};
use b3313_bridge::client::{WrongServer, resolve, wrong_server};
use b3313_bridge::session::{self, Event, Input, Options};
use b3313_bridge::text::ChatSize;
use b3313_protocol::Outfit;
use b3313_protocol::secure::{PublicKey, split_join_code};
use std::io;
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
    /// The session started the game in the emulator, as it does when none was running it.
    GameStarted,
    /// The session ended, saying what happened in a few words for the sidebar and why in a
    /// sentence for the player, with what the system said of it when there is more to know.
    Ended {
        headline: &'static str,
        reason: String,
        detail: Option<String>,
    },
}

/// What happened to a session that the panel itself ended or never started, in the few
/// words the sidebar has room for; `b3313_bridge::session::End::headline` has the bridge's.
mod headline {
    pub const PANEL_FAULT: &str = "Panel fault";
    pub const DID_NOT_JOIN: &str = "Did not join";
    pub const WRONG_PASSWORD: &str = "Wrong password";
    pub const COULD_NOT_JOIN: &str = "Could not join";
    pub const NOT_A_KEY: &str = "Not a server's key";
    pub const SERVER_NOT_FOUND: &str = "Server not found";
    pub const GAME_NOT_READY: &str = "Game not ready";
    pub const LEFT: &str = "Left";
    pub const SAVE_NOT_COPIED: &str = "Save not copied";
    pub const EMULATOR_NOT_STARTED: &str = "Emulator not started";

    #[cfg(test)]
    pub const ALL: [&str; 10] = [
        PANEL_FAULT,
        DID_NOT_JOIN,
        WRONG_PASSWORD,
        COULD_NOT_JOIN,
        NOT_A_KEY,
        SERVER_NOT_FOUND,
        GAME_NOT_READY,
        LEFT,
        SAVE_NOT_COPIED,
        EMULATOR_NOT_STARTED,
    ];
}

/// Why a session did not start: what happened in a few words, and why in a sentence.
type Failure = (&'static str, String);

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
                headline: headline::PANEL_FAULT,
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
    let (headline, reason, detail) = match options {
        Ok(options) => match session::run(&options, stop, typed, report) {
            Ok(end) => (end.headline(), end.describe(), end.detail()),
            Err(error) => not_joined(&error),
        },
        Err((headline, reason)) => (headline, reason, None),
    };
    let _ = notify.send(Notice::Ended {
        headline,
        reason,
        detail,
    });
}

/// How a join that failed with `error` is told: in a few words, in a sentence, and with
/// what more there is to know.
fn not_joined(error: &io::Error) -> (&'static str, String, Option<String>) {
    if let Some(wrong) = wrong_server(error) {
        let (reason, keys) = wrong_server_reason(&wrong);
        return (headline::DID_NOT_JOIN, reason, keys);
    }
    let headline = if error.kind() == io::ErrorKind::PermissionDenied {
        headline::WRONG_PASSWORD
    } else {
        headline::COULD_NOT_JOIN
    };
    (headline, format!("Could not join: {error}."), None)
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
    /// As typed or pasted: a name or an address, with its port, and after a `#` the server's
    /// key if it came from an invite. With a key, only that server is joined.
    pub server: String,
    pub password: String,
    /// The server's host password, if its owner gave the player one: whoever joins with it
    /// is the host. Empty for most players. It goes to a server that is not this panel's
    /// own; see `host_password_for`.
    pub host_password: String,
    /// The name the player chose; empty leaves it to the server.
    pub name: String,
    /// The outfit the player chose, if they have.
    pub outfit: Option<Outfit>,
    /// The key that opens the chat in the game, if the player types it there.
    pub chat_key: Option<char>,
    /// How many seconds the game shows the chat's lines for.
    pub chat_seconds: u8,
    /// How large the game draws the chat.
    pub chat_size: ChatSize,
    /// The port a bot on this PC may play the game through, if the player lets one.
    pub bot_port: Option<u16>,
    /// The server this panel is hosting, if it is: where it listens, its key, and its host
    /// password, which goes to that server alone, so the player hosting is the host.
    pub hosting: Option<Hosting>,
}

/// The server the panel is hosting, as a session joining it needs to know it.
#[derive(Clone, Debug)]
pub struct Hosting {
    pub address: SocketAddr,
    pub key: PublicKey,
    pub host_password: String,
}

/// Where the server the player gave is, and its key if they gave one after a `#`.
///
/// # Errors
/// Fails, with a message for the player, if what follows the `#` is no key: it is never
/// taken for an address with none, which would join whatever answers there.
pub fn read_server(server: &str) -> Result<(&str, Option<PublicKey>), String> {
    split_join_code(server).map_err(|error| {
        format!(
            "What follows the # in the server's address is not a server's key ({error}). \
             Paste the invite again, or type the address without it."
        )
    })
}

/// What the player is told when the server that answered is not the one whose key they had,
/// from an invite or from joining it before, and the two keys for whoever wants them: they
/// are long, and tell the player nothing they can act on.
fn wrong_server_reason(wrong: &WrongServer) -> (String, Option<String>) {
    let reason = "Did not join: the server at this address is not the one you joined before \
                  or were invited to. This is usual when its owner set it up anew or moved it \
                  to another PC: ask them for a new invite and paste it. To join what is there \
                  now anyway, forget this server under Recent and join again."
        .to_owned();
    let keys = format!(
        "The server that answered has the key {}; the one expected was {}.",
        wrong.met, wrong.expected
    );
    (reason, Some(keys))
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

/// The host password to join with: the one this panel's own server was started with, if that
/// is the server joined, `own`, so that the player hosting is its host whatever is in the
/// field; for any other server, what the player `typed` for it.
fn host_password_for(own: Option<Hosting>, typed: String) -> String {
    own.map_or(typed, |hosting| hosting.host_password)
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

    /// The emulator to join with: the one named, or one started now with the ROM, and
    /// whether it was started now.
    fn emulator(&self) -> Result<(u32, bool), String> {
        if let Some(pid) = self.pid {
            return Ok((pid, false));
        }
        let install = Install::find().ok_or(
            "Parallel Launcher's emulator was not found. Install Parallel Launcher and start \
             any game in it once, or start the multiplayer ROM yourself.",
        )?;
        install
            .start(&self.rom, self.chat_key.is_some())
            .map(|pid| (pid, true))
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

    /// What the session needs, read and looked up; or why that failed.
    fn options(self, stop: &AtomicBool, notify: &Sender<Notice>) -> Result<Options, Failure> {
        let failed = |headline: &'static str| move |reason: String| (headline, reason);
        // Before the emulator is started, which a mistyped name or key should not do.
        let (name, key) = read_server(&self.server).map_err(failed(headline::NOT_A_KEY))?;
        let server = resolve(name)
            .map_err(|error| format!("Server not found: {name} could not be looked up. {error}."))
            .map_err(failed(headline::SERVER_NOT_FOUND))?;
        let name = name.to_owned();
        let rom = self
            .current_rom()
            .map_err(failed(headline::GAME_NOT_READY))?;
        // Looking the name up and building the game take a while: a player who left
        // meanwhile is not to have the game start in front of them.
        if stop.load(Ordering::Relaxed) {
            return Err((headline::LEFT, "You left the session.".to_owned()));
        }
        let copied = self
            .back_up_save()
            .map_err(failed(headline::SAVE_NOT_COPIED))?;
        if copied == 0 {
            // Said, not passed over: the session changes a save that then has no copy.
            let _ = notify.send(Notice::SaveNotCopied);
        }
        let (pid, started) = self
            .emulator()
            .map_err(failed(headline::EMULATOR_NOT_STARTED))?;
        if started {
            let _ = notify.send(Notice::GameStarted);
        }
        let own = self
            .hosting
            .filter(|hosting| is_own_server(server, hosting.address));
        // This panel's own server is known by the key the panel runs it with, whatever key
        // was kept for its address: one kept from before the server was set up anew would
        // refuse its own owner.
        let server_key = own.as_ref().map(|hosting| hosting.key).or(key);
        let host_password = host_password_for(own, self.host_password);
        Ok(Options {
            rom,
            pid,
            server,
            server_name: Some(name),
            server_key,
            password: self.password,
            outfit: self.outfit,
            name: self.name,
            host_password,
            chat_key: self.chat_key,
            chat_seconds: self.chat_seconds,
            chat_size: self.chat_size,
            bot_port: self.bot_port,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Hosting, current_rom, headline, host_password_for, is_own_server, not_joined};
    use crate::game_setup::{Built, GAMES, Game};
    use b3313_bridge::session::HEADLINE_MOST;
    use b3313_protocol::secure::{SecureError, ServerKey};
    use std::io;
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
    fn what_happened_to_a_session_is_said_in_few_enough_words_for_the_sidebar() {
        for headline in headline::ALL {
            let length = headline.chars().count();
            assert!((1..=HEADLINE_MOST).contains(&length), "{headline}");
        }
        // A join that failed is told by what failed, where the panel can tell.
        let wrong_password = io::Error::new(io::ErrorKind::PermissionDenied, "wrong");
        assert_eq!(not_joined(&wrong_password).0, headline::WRONG_PASSWORD);
        let full = io::Error::other("the session is full");
        let (headline, reason, _) = not_joined(&full);
        assert_eq!(headline, headline::COULD_NOT_JOIN);
        assert_eq!(reason, "Could not join: the session is full.");
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

    #[test]
    fn the_host_password_typed_goes_to_any_server_but_this_panels_own() -> Result<(), SecureError> {
        let typed = || "given by its owner".to_owned();
        // Someone else's server: what the player typed, or nothing if they typed nothing.
        assert_eq!(host_password_for(None, typed()), "given by its owner");
        assert_eq!(host_password_for(None, String::new()), "");
        // This panel's own: the password it was started with, whatever is in the field.
        let own = Hosting {
            address: address("127.0.0.1:41313"),
            key: ServerKey::generate()?.public(),
            host_password: "made up by the panel".to_owned(),
        };
        assert_eq!(
            host_password_for(Some(own), typed()),
            "made up by the panel"
        );
        Ok(())
    }
}
