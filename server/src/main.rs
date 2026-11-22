//! Command-line entry point for the session server.
//!
//! The server's owner types commands into its console; `console::HELP` lists them.

use b3313_protocol::secure::{PublicKey, join_code};
use b3313_server::bans::{BanList, read_ban_file};
use b3313_server::console::HELP;
use b3313_server::key_file::{beside, load_or_create};
use b3313_server::{Command, Config, Event, serve_with_console};
use std::error::Error;
use std::io::BufRead;
use std::net::UdpSocket;
use std::path::Path;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;

/// Set when the server is stopped from outside: it then tells every player's bridge that
/// the session has ended, as `end` does, where a server simply gone leaves them to wait out
/// the silence and try to join again for two minutes.
static STOP: AtomicBool = AtomicBool::new(false);

/// Has Ctrl+C and Ctrl+Break set `STOP`, where they would end the program at once.
#[cfg(windows)]
fn stop_in_order() {
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
        eprintln!("Ctrl+C will end the server at once, without telling the players.");
    }
}

/// Has the signals that ask a program to stop, Ctrl+C's and a service manager's, set `STOP`.
#[cfg(unix)]
fn stop_in_order() {
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }

    let handler: extern "C" fn(libc::c_int) = on_signal;
    #[expect(
        clippy::as_conversions,
        reason = "signal takes its handler as a number, and a function's address is one"
    )]
    let handler = handler as libc::sighandler_t;
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: the handler lives as long as the program, and all it does is store to an
        // atomic, which a signal handler may.
        let before = unsafe { libc::signal(signal, handler) };
        if before == libc::SIG_ERR {
            eprintln!("Signal {signal} will end the server at once, without telling the players.");
        }
    }
}

#[cfg(not(any(windows, unix)))]
fn stop_in_order() {}

const USAGE: &str = "Usage: b3313-server <address:port to listen on, such as 127.0.0.1:41313> \
[--config <file>]

Without --config, every setting has its default; server/server.example.conf lists them. The
server's key is then kept in server.key, in the folder the server is run from.";

/// What the server's owner is told of its key as it starts: what to give players, and
/// where the private half is.
fn key_notice(key: &PublicKey, kept_in: &Path) -> String {
    let code = join_code("<the address players reach this server at>", key);
    format!(
        "Give players this join code, which lets their programs check that they reached this \
         server and no other:\n  {code}\nThis server's key is kept in {}: show that file to \
         nobody.",
        kept_in.display()
    )
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // No default address: listening beyond this PC must be a deliberate choice.
    let (address, mut config, settings) = match args.as_slice() {
        [address] => (address, Config::default(), None),
        [address, flag, path] if flag == "--config" => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read {path}: {error}"))?;
            (address, Config::parse(&text)?, Some(Path::new(path)))
        }
        _ => return Err(USAGE.into()),
    };
    if let Some(path) = &config.ban_file {
        config.bans = read_ban_file(path)?;
    }
    let mut bans = BanList::new(config.ban_file.clone(), config.bans.clone());
    // Kept where the settings say, or beside them, or with no settings in the folder the
    // server is run from: a server started again must be the same server, or its players'
    // bridges, which join again only the server they were in, would refuse it.
    let key_file = config
        .key_file
        .clone()
        .unwrap_or_else(|| beside(settings.unwrap_or_else(|| Path::new(""))));
    let key = load_or_create(&key_file)?;
    config.key = Some(key.clone());

    let socket = UdpSocket::bind(address)?;
    println!("Listening on {}. {HELP}", socket.local_addr()?);
    println!("{}", key_notice(&key.public(), &key_file));
    let (commands, received) = mpsc::channel();
    // Reads the console for as long as the server runs; it ends with the process.
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            match Command::parse(&line) {
                Ok(command) => {
                    if commands.send(command).is_err() {
                        return;
                    }
                }
                Err(message) => println!("{message}"),
            }
        }
    });
    let log = |event: Event| {
        if let Err(message) = bans.update(&event) {
            // The ban holds until the server stops either way.
            println!("{message}");
        }
        println!("{event}");
    };
    stop_in_order();
    serve_with_console(&socket, config, &STOP, &received, log)?;
    Ok(())
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
