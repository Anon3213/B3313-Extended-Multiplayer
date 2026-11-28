//! Games beside the player's own for bots on this PC to play, each a player of its own in the
//! player's session: started from the Play page while the player is in one, and closed with
//! it. Each is a copy of the player's ROM in a muted emulator of its own (`Install::start_bot`)
//! with a bridge on a thread of its own (`join::Bridge`), whose bot API (`b3313_bridge::bots`)
//! a bot connects to, at a port of its own.

use crate::emulator;
use crate::join::{Bridge, Hosting, Notice, Request};
use b3313_bridge::assist::BotNotice;
use b3313_bridge::bots;
use b3313_bridge::session::Event;
use b3313_bridge::text::ChatSize;
use b3313_bridge::typing::{DEFAULT_CHAT_LINES, DEFAULT_CHAT_SECONDS};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The most bot games started at once. Each costs about a tenth of a processor core and
/// 600 MB of memory (measured in level 9 on 2026-11-27), and takes a player's place.
pub const MOST: u8 = 8;

/// Where the session of one bot's game is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Copying the game and starting its emulator.
    Starting,
    WaitingForGame,
    Joining,
    Playing(u8),
    Rejoining,
    /// Ended, and why; its emulator is closed.
    Ended(String),
}

impl Status {
    /// The status in a few words.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Starting => "Starting its game".to_owned(),
            Self::WaitingForGame => "Waiting for its game".to_owned(),
            Self::Joining => "Joining".to_owned(),
            Self::Playing(player) => format!("In the session as player {player}"),
            Self::Rejoining => "Joining again".to_owned(),
            Self::Ended(why) => why.clone(),
        }
    }
}

/// One bot's game: its emulator and the bridge that has it in the session.
#[derive(Debug)]
pub struct BotGame {
    pub name: String,
    /// Where a bot connects to play it, once its bridge is listening there.
    pub address: Option<SocketAddr>,
    /// Whether a bot is connected.
    pub has_bot: bool,
    pub status: Status,
    bridge: Option<Bridge>,
    /// The emulator its session started, once it has.
    emulator: Option<u32>,
    /// The player its Mario is to follow once it is in the session, until then: told to its
    /// bridge as `/follow` in its own chat, which presses through the game's menus first.
    follow: Option<String>,
    /// The player it was sent to follow, once it has been: what its row says it does.
    pub following: Option<String>,
}

impl BotGame {
    /// Starts bot game number `number` (from 1) in the session `joined` is the player's
    /// request for, with its bot API at `port`; its Mario follows the player named `follow`,
    /// if one is, until a bot takes the controller.
    ///
    /// # Errors
    /// Fails, with a message for the player, if its session's thread cannot start.
    pub fn start(
        joined: &Request,
        number: u8,
        port: u16,
        follow: Option<String>,
    ) -> Result<Self, String> {
        let request = request(joined, number, port);
        let name = request.name.clone();
        Ok(Self {
            name,
            address: None,
            has_bot: false,
            status: Status::Starting,
            bridge: Some(Bridge::start(request)?),
            emulator: None,
            follow,
            following: None,
        })
    }

    /// Takes in what its session reported; whether anything changed.
    pub fn poll(&mut self) -> bool {
        let notices = self.bridge.as_ref().map(Bridge::drain).unwrap_or_default();
        let changed = !notices.is_empty();
        for notice in notices {
            self.take(notice);
        }
        changed
    }

    fn take(&mut self, notice: Notice) {
        match notice {
            Notice::GameStarted(pid) => self.emulator = Some(pid),
            Notice::Event(Event::WaitingForGame) => self.status = Status::WaitingForGame,
            Notice::Event(Event::Joining) => self.status = Status::Joining,
            Notice::Event(Event::Joined(player)) => {
                self.status = Status::Playing(player.get());
                if let (Some(name), Some(bridge)) = (self.follow.take(), &self.bridge) {
                    bridge.say(format!("/follow {name}"));
                    self.following = Some(name);
                }
            }
            Notice::Event(Event::Reconnecting(_)) => self.status = Status::Rejoining,
            Notice::Event(Event::Bot(notice)) => match notice {
                BotNotice::Listening(address) => self.address = Some(address),
                BotNotice::Unavailable(why) => self.status = Status::Ended(why),
                BotNotice::Connected => {
                    self.has_bot = true;
                    // A bot that takes the controller ends the following; one that only
                    // watches does not, but the row cannot tell which.
                    self.following = None;
                }
                BotNotice::Left => self.has_bot = false,
            },
            Notice::Ended { reason, .. } => {
                // The thread has finished; this only collects it.
                drop(self.bridge.take());
                self.close_emulator();
                self.has_bot = false;
                self.status = Status::Ended(reason);
            }
            Notice::SaveNotCopied | Notice::Event(_) => {}
        }
    }

    /// Takes the game out of the session and closes its emulator.
    fn stop(&mut self) {
        if let Some(bridge) = self.bridge.take() {
            bridge.ask_to_leave();
            // An emulator started since the last look is closed too.
            for notice in bridge.drain() {
                if let Notice::GameStarted(pid) = notice {
                    self.emulator = Some(pid);
                }
            }
            // Waits for the session to leave, which it does within a frame or two, so that
            // the game is out of it before its emulator closes.
            drop(bridge);
        }
        self.close_emulator();
    }

    fn close_emulator(&mut self) {
        if let Some(pid) = self.emulator.take() {
            // One that cannot be closed is left to the player, whose window it is.
            let _ = emulator::close(pid);
        }
    }
}

impl Drop for BotGame {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The request for bot game number `number`: the player's session, server and password,
/// with a name, a ROM and a bot port of its own, and never the host password, which would
/// make the bot the host.
fn request(joined: &Request, number: u8, port: u16) -> Request {
    Request {
        rom: rom_copy(&joined.rom, number),
        sources: None,
        pid: None,
        server: joined.server.clone(),
        password: joined.password.clone(),
        host_password: String::new(),
        name: format!("Bot {number}"),
        outfit: None,
        chat_key: None,
        chat_seconds: DEFAULT_CHAT_SECONDS,
        chat_lines: DEFAULT_CHAT_LINES,
        chat_on_hover: false,
        chat_size: ChatSize::Normal,
        bot_port: Some(port),
        hosting: joined.hosting.as_ref().map(|hosting| Hosting {
            host_password: String::new(),
            ..hosting.clone()
        }),
        bot_of: Some(joined.rom.clone()),
    }
}

/// Where bot game number `number` keeps its copy of `rom`, which the emulator names its save
/// after: among the panel's own files, or beside `rom` where the panel has no folder.
fn rom_copy(rom: &Path, number: u8) -> PathBuf {
    let folder = crate::preferences::path()
        .and_then(|preferences| Some(preferences.parent()?.join("bot-games")))
        .or_else(|| Some(rom.parent()?.join("bot-games")))
        .unwrap_or_default();
    let stem = rom.file_stem().unwrap_or_default().to_string_lossy();
    let extension = rom.extension().unwrap_or_default().to_string_lossy();
    folder.join(format!("{stem}-bot{number}.{extension}"))
}

/// The bot port of bot game number `number`: those after the one the player's own game would
/// take, `own`, which is the usual one unless the player chose another.
#[must_use]
pub fn port(own: Option<u16>, number: u8) -> Option<u16> {
    own.unwrap_or(bots::DEFAULT_PORT)
        .checked_add(u16::from(number))
}

#[cfg(test)]
mod tests {
    use super::{port, request, rom_copy};
    use crate::join::{Hosting, Request};
    use b3313_protocol::secure::{SecureError, ServerKey};
    use std::path::{Path, PathBuf};

    fn players_request() -> Result<Request, SecureError> {
        Ok(Request {
            rom: PathBuf::from(r"C:\b3313\game\a2ae-multiplayer.z64"),
            sources: None,
            pid: Some(1),
            server: "203.0.113.9:41313".to_owned(),
            password: "in".to_owned(),
            host_password: "crown".to_owned(),
            name: "Luigi".to_owned(),
            outfit: None,
            chat_key: Some('t'),
            chat_seconds: 8,
            chat_lines: 6,
            chat_on_hover: true,
            chat_size: b3313_bridge::text::ChatSize::Large,
            bot_port: Some(40000),
            hosting: Some(Hosting {
                address: ([127, 0, 0, 1], 41313).into(),
                key: ServerKey::generate()?.public(),
                host_password: "crown".to_owned(),
            }),
            bot_of: None,
        })
    }

    #[test]
    fn a_bots_game_joins_the_players_session_and_is_never_the_host() -> Result<(), SecureError> {
        let joined = players_request()?;
        let bot = request(&joined, 2, 40002);
        assert_eq!(
            (bot.server.as_str(), bot.password.as_str()),
            ("203.0.113.9:41313", "in")
        );
        assert_eq!(bot.name, "Bot 2");
        assert_eq!(bot.bot_port, Some(40002));
        assert_eq!(bot.bot_of.as_deref(), Some(joined.rom.as_path()));
        assert_ne!(bot.rom, joined.rom);
        // Neither password that makes the host goes with it, to the panel's own server or any.
        assert_eq!(bot.host_password, "");
        assert!(
            bot.hosting
                .is_some_and(|hosting| hosting.host_password.is_empty())
        );
        // Nobody types in it, nor starts the game already running for the player.
        assert_eq!(
            (bot.chat_key, bot.pid, bot.chat_on_hover),
            (None, None, false)
        );
        Ok(())
    }

    /// A bot connected to a bot game's API, for the live test.
    struct TestBot {
        stream: std::net::TcpStream,
        reader: std::io::BufReader<std::net::TcpStream>,
    }

    impl TestBot {
        fn connect(address: std::net::SocketAddr) -> std::io::Result<Self> {
            let stream = std::net::TcpStream::connect(address)?;
            stream.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;
            let reader = std::io::BufReader::new(stream.try_clone()?);
            Ok(Self { stream, reader })
        }

        fn send(&mut self, line: &str) -> std::io::Result<()> {
            use std::io::Write as _;
            self.stream.write_all(format!("{line}\n").as_bytes())
        }

        /// The first line within `seconds` that `wanted` takes, if one comes.
        fn wait_for(
            &mut self,
            seconds: u64,
            wanted: impl Fn(&b3313_bridge::json::Value) -> bool,
        ) -> Option<b3313_bridge::json::Value> {
            use std::io::BufRead as _;
            let until = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
            let mut line = String::new();
            while std::time::Instant::now() < until {
                line.clear();
                if self.reader.read_line(&mut line).is_ok_and(|read| read > 0)
                    && let Ok(value) = b3313_bridge::json::parse(&line)
                    && wanted(&value)
                {
                    return Some(value);
                }
            }
            None
        }
    }

    /// Polls `games` until `done` says so of them, for at most `seconds`.
    fn poll_until(
        games: &mut [&mut super::BotGame],
        seconds: u64,
        done: impl Fn(&[&mut super::BotGame]) -> bool,
    ) -> bool {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        while std::time::Instant::now() < until {
            for game in games.iter_mut() {
                game.poll();
            }
            if done(games) {
                return true;
            }
            if games
                .iter()
                .any(|game| matches!(game.status, super::Status::Ended(_)))
            {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        false
    }

    #[test]
    #[ignore = "starts two emulators: needs Parallel Launcher and Bee/a2ae-multiplayer.z64"]
    fn two_bot_games_join_get_into_a_level_from_the_title_screen_and_close() -> Result<(), String> {
        use super::{BotGame, Status};
        let rom = Path::new(env!("CARGO_MANIFEST_DIR")).join("../Bee/a2ae-multiplayer.z64");
        let host = crate::host::Host::start("127.0.0.1:0", b3313_server::Config::default())?;
        let joined = Request {
            rom,
            server: host.address().to_string(),
            password: String::new(),
            hosting: Some(Hosting {
                address: host.address(),
                key: host.key(),
                host_password: host.host_password().to_owned(),
            }),
            ..players_request().map_err(|error| error.to_string())?
        };
        let mut first = BotGame::start(&joined, 1, 33191, None)?;
        let playing = |game: &BotGame| matches!(game.status, Status::Playing(_));
        let both = poll_until(&mut [&mut first], 120, |games| {
            games
                .iter()
                .all(|game| playing(game) && game.address.is_some())
        });
        assert!(both, "the first bot game did not join: {:?}", first.status);

        // A bot asks for a warp at the title screen: the bridge presses into a level first.
        let address = first.address.ok_or("no bot port")?;
        let mut bot = TestBot::connect(address).map_err(|error| error.to_string())?;
        bot.send(r#"{"type":"hello","id":1}"#)
            .map_err(|error| error.to_string())?;
        let welcome = bot.wait_for(10, |value| {
            value.get("type").and_then(|kind| kind.as_str()) == Some("welcome")
        });
        assert!(welcome.is_some(), "no welcome");
        bot.send(r#"{"type":"warp","level":6,"id":2}"#)
            .map_err(|error| error.to_string())?;
        let done = bot.wait_for(240, |value| {
            value.get("type").and_then(|kind| kind.as_str()) == Some("done")
        });
        let arrived = done
            .as_ref()
            .and_then(|done| done.get("ok"))
            .and_then(b3313_bridge::json::Value::as_bool);
        assert_eq!(arrived, Some(true), "the warp ended: {done:?}");

        // A second, told to follow the first, gets into the game by itself and after it.
        let mut second = BotGame::start(&joined, 2, 33192, Some(first.name.clone()))?;
        let joined_too = poll_until(&mut [&mut first, &mut second], 120, |games| {
            games.iter().all(|game| playing(game))
        });
        assert!(
            joined_too,
            "the second bot game did not join: {:?}",
            second.status
        );
        let second_address = second.address.ok_or("no second bot port")?;
        let mut watcher = TestBot::connect(second_address).map_err(|error| error.to_string())?;
        watcher
            .send(r#"{"type":"hello"}"#)
            .map_err(|error| error.to_string())?;
        watcher
            .send(r#"{"type":"watch","every":1}"#)
            .map_err(|error| error.to_string())?;
        let following = watcher.wait_for(240, |value| {
            value.get("type").and_then(|kind| kind.as_str()) == Some("state")
                && value
                    .get("playing")
                    .and_then(b3313_bridge::json::Value::as_bool)
                    == Some(true)
                && value
                    .get("level")
                    .and_then(b3313_bridge::json::Value::as_f64)
                    == Some(6.0)
        });
        assert!(following.is_some(), "the second never reached level 6");
        drop(watcher);

        // Stopped, both leave and their emulators close.
        let emulators: Vec<u32> = [first.emulator, second.emulator]
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(emulators.len(), 2);
        drop((first, second));
        // A process killed ends a moment after it is told to.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let running = b3313_bridge::process::find_processes(b3313_bridge::process::EMULATOR)
                .map_err(|error| error.to_string())?;
            if emulators.iter().all(|pid| !running.contains(pid)) {
                return Ok(());
            }
            assert!(std::time::Instant::now() < until, "an emulator stayed open");
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    }

    #[test]
    fn each_bot_game_has_a_rom_of_its_own_and_the_ports_after_the_players() {
        let rom = Path::new(r"C:\b3313\game\a2ae-multiplayer.z64");
        let (first, second) = (rom_copy(rom, 1), rom_copy(rom, 2));
        assert_ne!(first, second);
        assert!(first.ends_with("a2ae-multiplayer-bot1.z64"));
        assert_eq!(port(None, 1), Some(b3313_bridge::bots::DEFAULT_PORT + 1));
        assert_eq!(port(Some(40000), 3), Some(40003));
        assert_eq!(port(Some(u16::MAX), 1), None);
    }
}
