//! Command-line entry point for the bridge.
//!
//! Connects one emulator to a session server: sends the local player and shows everyone else.
//! The session itself is `b3313_bridge::session`, which the panel's Play page also runs.

use b3313_bridge::client::resolve;
use b3313_bridge::session::{self, End, Event, Input, Options};
use b3313_bridge::typing::{DEFAULT_CHAT_KEY, DEFAULT_CHAT_SECONDS, chat_key, chat_seconds};
use b3313_protocol::{Name, Outfit};
use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;

const USAGE: &str = "Usage: b3313-bridge <multiplayer ROM> <emulator PID> <server address:port>

The session password, if the server has one, goes in the B3313_PASSWORD environment variable, not
the command line, where other programs on the PC could read it. B3313_NAME is the name the other
players see, B3313_COLOR the colors they see you in (red, green and so on; /color in the chat
lists them), and B3313_HOST_PASSWORD, for the host alone, the server's host password. Lines typed
here go to the chat, and so do lines typed in the game: there, T opens the chat, or the key named
in B3313_CHAT_KEY, and \"off\" for that leaves the keyboard unread; B3313_CHAT_SECONDS is how long
the game shows a line, 8 unless given. The control panel's Play page
does all this without a command line.";
/// Where the session password comes from; unset means an open session.
const PASSWORD_VARIABLE: &str = "B3313_PASSWORD";
/// The player's name; unset leaves it to the server.
const NAME_VARIABLE: &str = "B3313_NAME";
/// The player's colors, by an outfit's name as `/color` lists them; unset leaves it to the server.
const COLOR_VARIABLE: &str = "B3313_COLOR";
/// The host's own password; unset for everyone else.
const HOST_PASSWORD_VARIABLE: &str = "B3313_HOST_PASSWORD";
/// The key that opens the chat in the game: a letter or digit; unset is T, and anything else,
/// as "off", leaves the keyboard unread.
const CHAT_KEY_VARIABLE: &str = "B3313_CHAT_KEY";
/// How many seconds the game shows chat for, from 3 to 120; unset, or anything else, is 8.
const CHAT_SECONDS_VARIABLE: &str = "B3313_CHAT_SECONDS";

fn parse_args() -> Result<(PathBuf, u32, SocketAddr), Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let (Some(rom), Some(pid), Some(server), None) =
        (args.next(), args.next(), args.next(), args.next())
    else {
        return Err(USAGE.into());
    };
    let pid = pid.to_str().ok_or(USAGE)?.parse()?;
    let server = resolve(server.to_str().ok_or(USAGE)?)?;
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
        server,
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
    };
    leave_on_ctrl_c();
    let says = chat_from_console();
    let end = session::run(&options, &STOP, &says, |event| {
        print_event(server, &event);
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
