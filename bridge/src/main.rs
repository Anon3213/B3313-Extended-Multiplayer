//! Command-line entry point for the bridge.
//!
//! Connects one emulator to a session server: sends the local player and shows everyone else.
//! The session itself is `b3313_bridge::session`, which the panel's Play page also runs.

use b3313_bridge::client::resolve;
use b3313_bridge::session::{self, End, Event, Input, Options};
use b3313_bridge::text::ChatSize;
use b3313_bridge::typing::{
    DEFAULT_CHAT_KEY, DEFAULT_CHAT_LINES, DEFAULT_CHAT_SECONDS, DEFAULT_MAP_KEY, chat_key,
    chat_lines, chat_seconds,
};
use b3313_protocol::secure::{PublicKey, split_join_code};
use b3313_protocol::{Name, Outfit};
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;

const USAGE: &str = "Usage: b3313-bridge <multiplayer ROM> <emulator PID> <server address:port, \
or its join code>

A join code is the server's address, a # and the server's key, as the server's owner gives it:
with it, only that server is joined, and nobody else who answers at its address. The key alone
can be given as B3313_SERVER_KEY. The session password, if the server has one, goes in the
B3313_PASSWORD environment variable, not
the command line, where other programs on the PC could read it. B3313_NAME is the name the other
players see, B3313_COLOR the colors they see you in (red, green and so on; /color in the chat
lists them), and B3313_HOST_PASSWORD, for the host alone, the server's host password. Lines typed
here go to the chat, and so do lines typed in the game: there, T opens the chat, or the key named
in B3313_CHAT_KEY, and \"off\" for that turns typing in the game off; B3313_CHAT_SECONDS is how long
the game shows a line, 8 unless given, B3313_CHAT_LINES how many rows it shows at once, 1 to 6
(6 unless given), B3313_CHAT_HOVER=on shows the chat while the mouse rests on it, and
B3313_CHAT_SIZE=large draws the chat at twice the size. B3313_MINIMAP=off starts with the
minimap of the walls and players around you hidden; M in the game, or the key named in
B3313_MAP_KEY (\"off\" for none), shows or hides it, as /map does.
B3313_BOT_PORT opens that port on 127.0.0.1 for a bot to play this game through; BOTS.md says how.
The control panel's Play page does all this without a command line.";
/// The key the server must have, for a server given by its address alone; unset joins
/// whatever answers there.
const SERVER_KEY_VARIABLE: &str = "B3313_SERVER_KEY";
/// Where the session password comes from; unset means an open session.
const PASSWORD_VARIABLE: &str = "B3313_PASSWORD";
/// The player's name; unset leaves it to the server.
const NAME_VARIABLE: &str = "B3313_NAME";
/// The player's colors, by an outfit's name as `/color` lists them; unset leaves it to the server.
const COLOR_VARIABLE: &str = "B3313_COLOR";
/// The host's own password; unset for everyone else.
const HOST_PASSWORD_VARIABLE: &str = "B3313_HOST_PASSWORD";
/// The key that opens the chat in the game: a letter or digit; unset is T, and anything else,
/// as "off", turns typing in the game off.
const CHAT_KEY_VARIABLE: &str = "B3313_CHAT_KEY";
/// How many seconds the game shows chat for, from 3 to 120; unset, or anything else, is 8.
const CHAT_SECONDS_VARIABLE: &str = "B3313_CHAT_SECONDS";
/// How many rows of chat the game shows at once as lines arrive, from 1 to 6; unset, or
/// anything else, is 6.
const CHAT_LINES_VARIABLE: &str = "B3313_CHAT_LINES";
/// Whether the mouse resting on the game's chat shows it: "on"; unset, or anything else, is off.
const CHAT_HOVER_VARIABLE: &str = "B3313_CHAT_HOVER";
/// How large the game draws the chat: "normal" or "large"; unset, or anything else, is normal.
const CHAT_SIZE_VARIABLE: &str = "B3313_CHAT_SIZE";
/// Whether the game shows the minimap as the session begins: "off" hides it; unset, or
/// anything else, shows it.
const MINIMAP_VARIABLE: &str = "B3313_MINIMAP";
/// The key that shows or hides the minimap in the game: a letter or digit; unset is M, and
/// anything else, as "off", leaves it to /map.
const MAP_KEY_VARIABLE: &str = "B3313_MAP_KEY";
/// The port on 127.0.0.1 a bot may connect to; unset opens none.
const BOT_PORT_VARIABLE: &str = "B3313_BOT_PORT";

/// The server as the command line gave it: where it is now, the name that was looked up for
/// that, and the key it must have, from a join code or `SERVER_KEY_VARIABLE`.
struct Server {
    address: SocketAddr,
    name: String,
    key: Option<PublicKey>,
}

fn parse_args() -> Result<(PathBuf, u32, Server), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(rom), Some(pid), Some(server), None) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        return Err(USAGE.into());
    };
    let pid = pid.to_str().ok_or(USAGE)?.parse()?;
    let (name, in_code) = split_join_code(server.to_str().ok_or(USAGE)?)?;
    let key = match (in_code, std::env::var(SERVER_KEY_VARIABLE)) {
        (Some(key), _) => Some(key),
        (None, Ok(key)) => Some(key.trim().parse()?),
        (None, Err(_)) => None,
    };
    let server = Server {
        address: resolve(name)?,
        name: name.to_owned(),
        key,
    };
    Ok((PathBuf::from(rom), pid, server))
}

fn print_event(server: SocketAddr, event: &Event) {
    match event {
        Event::WaitingForGame => println!("Waiting for the game to start."),
        Event::Joining => {}
        Event::Reconnecting(limit) => println!(
            "Lost {server}. Joining again, for {} seconds at most.",
            limit.as_secs()
        ),
        Event::Joined(player) => println!("Joined {server} as player {}.", player.get()),
        Event::Server(key) => println!(
            "This server's key is {key}. Give it as {SERVER_KEY_VARIABLE}, or after a # at \
             the end of the server's address, to join this server and no other."
        ),
        Event::WrongRom(found) => eprintln!("{}", session::describe_wrong_rom(*found)),
        Event::Hit(target, kind) => println!("Hit player {} ({kind:?}).", target.get()),
        Event::HitBy(attacker, kind) => println!("Hit by player {} ({kind:?}).", attacker.get()),
        Event::Link(report) => println!("Network: {report}"),
        // Empty while the server is being joined again, when there is nobody to name.
        Event::Roster(roster) if roster.is_empty() => {}
        Event::Roster(roster) => {
            let names: Vec<&str> = roster.iter().map(|entry| entry.name.as_str()).collect();
            println!("In the session: {}.", names.join(", "));
        }
        Event::Said {
            from: Some(_),
            name,
            text,
        } => println!("<{name}> {text}"),
        Event::Said {
            from: None, text, ..
        } => println!("* {text}"),
        Event::Bot(notice) => println!("{}", notice.describe()),
    }
}

/// Sends each line typed into the console to the chat, until the console closes.
fn chat_from_console() -> Receiver<Input> {
    let (lines, says) = mpsc::channel();
    thread::spawn(move || {
        for line in std::io::stdin().lines().map_while(Result::ok) {
            if lines.send(Input::Say(line)).is_err() {
                break;
            }
        }
    });
    says
}

/// Set by Ctrl+C. The session then ends as the panel's Leave ends one: the server is told,
/// and the session is taken out of the game, where a killed bridge leaves both to time out.
static STOP: AtomicBool = AtomicBool::new(false);

/// Has Ctrl+C and Ctrl+Break set `STOP`, where they would end the program at once.
#[cfg(windows)]
fn leave_on_ctrl_c() {
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
    };

    /// Called by Windows on a thread of its own; says whether the key was dealt with.
    unsafe extern "system" fn on_control(kind: u32) -> windows_sys::core::BOOL {
        let is_ours = kind == CTRL_C_EVENT || kind == CTRL_BREAK_EVENT;
        if is_ours {
            STOP.store(true, Ordering::Relaxed);
        }
        windows_sys::core::BOOL::from(is_ours)
    }

    // SAFETY: `on_control` has the signature the call asks for and lives as long as the
    // program, and all it does is store to an atomic, which any thread may.
    let added = unsafe { SetConsoleCtrlHandler(Some(on_control), 1) };
    if added == 0 {
        eprintln!("Ctrl+C will end the bridge at once, without leaving the session in order.");
    }
}

#[cfg(not(windows))]
fn leave_on_ctrl_c() {}

/// The port `BOT_PORT_VARIABLE` names, if it is set.
fn bot_port() -> Result<Option<u16>, Box<dyn Error>> {
    match std::env::var(BOT_PORT_VARIABLE) {
        Ok(port) => match port.trim().parse() {
            Ok(port) => Ok(Some(port)),
            Err(_) => Err(format!("{BOT_PORT_VARIABLE} is not a port number ({port}).").into()),
        },
        Err(_) => Ok(None),
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let (rom_path, pid, server) = parse_args()?;
    let name = std::env::var(NAME_VARIABLE).unwrap_or_default();
    if !name.is_empty() && Name::new(&name).is_none() {
        eprintln!("{NAME_VARIABLE} has no plain letters in it; the server will make a name up.");
    }
    let color = std::env::var(COLOR_VARIABLE).ok();
    let outfit = color.as_deref().and_then(Outfit::named);
    if let (Some(color), None) = (&color, outfit) {
        eprintln!("{COLOR_VARIABLE} names no colors ({color}); /color in the chat lists them.");
    }
    let options = Options {
        rom: std::fs::read(rom_path)?,
        pid,
        server: server.address,
        server_name: Some(server.name),
        server_key: server.key,
        password: std::env::var(PASSWORD_VARIABLE).unwrap_or_default(),
        name,
        outfit,
        host_password: std::env::var(HOST_PASSWORD_VARIABLE).unwrap_or_default(),
        chat_key: std::env::var(CHAT_KEY_VARIABLE)
            .map_or(Some(DEFAULT_CHAT_KEY), |named| chat_key(&named)),
        chat_seconds: std::env::var(CHAT_SECONDS_VARIABLE)
            .ok()
            .and_then(|named| chat_seconds(&named))
            .unwrap_or(DEFAULT_CHAT_SECONDS),
        chat_lines: std::env::var(CHAT_LINES_VARIABLE)
            .ok()
            .and_then(|named| chat_lines(&named))
            .unwrap_or(DEFAULT_CHAT_LINES),
        chat_on_hover: std::env::var(CHAT_HOVER_VARIABLE)
            .is_ok_and(|named| named.trim().eq_ignore_ascii_case("on")),
        chat_size: std::env::var(CHAT_SIZE_VARIABLE)
            .ok()
            .and_then(|named| ChatSize::named(&named))
            .unwrap_or_default(),
        minimap: !std::env::var(MINIMAP_VARIABLE)
            .is_ok_and(|named| named.trim().eq_ignore_ascii_case("off")),
        map_key: std::env::var(MAP_KEY_VARIABLE)
            .map_or(Some(DEFAULT_MAP_KEY), |named| chat_key(&named)),
        bot_port: bot_port()?,
    };
    leave_on_ctrl_c();
    let says = chat_from_console();
    let address = options.server;
    let end = session::run(&options, &STOP, &says, |event| {
        print_event(address, &event);
    })?;
    match end {
        // Closing the game is the usual way to finish.
        End::GameClosed(_) | End::Left => {
            println!("{}", end.describe());
            Ok(())
        }
        End::ServerSilent | End::Removed(_) | End::Refused(_) | End::NetworkFailing(_) => {
            Err(end.describe().into())
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
