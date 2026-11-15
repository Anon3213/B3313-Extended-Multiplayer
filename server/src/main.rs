//! Command-line entry point for the session server.
//!
//! The server's owner types commands into its console; `console::HELP` lists them.

use b3313_server::bans::{BanList, read_ban_file};
use b3313_server::console::HELP;
use b3313_server::{Command, Config, Event, serve_with_console};
use std::error::Error;
use std::io::BufRead;
use std::net::UdpSocket;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;

const USAGE: &str = "Usage: b3313-server <address:port to listen on, such as 127.0.0.1:41313> \
[--config <file>]

Without --config, every setting has its default; server/server.example.conf lists them.";

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // No default address: listening beyond this PC must be a deliberate choice.
    let (address, mut config) = match args.as_slice() {
        [address] => (address, Config::default()),
        [address, flag, path] if flag == "--config" => {
            let text = std::fs::read_to_string(path)
                .map_err(|error| format!("cannot read {path}: {error}"))?;
            (address, Config::parse(&text)?)
        }
        _ => return Err(USAGE.into()),
    };
    if let Some(path) = &config.ban_file {
        config.bans = read_ban_file(path)?;
    }
    let mut bans = BanList::new(config.ban_file.clone(), config.bans.clone());

    let socket = UdpSocket::bind(address)?;
    println!("Listening on {}. {HELP}", socket.local_addr()?);
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
    serve_with_console(&socket, config, &AtomicBool::new(false), &received, log)?;
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
