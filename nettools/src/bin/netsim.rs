//! Runs the bad-network proxy between bridges and a server.

use b3313_nettools::netsim::{Impairment, Proxy};
use b3313_nettools::options::Options;
use std::error::Error;
use std::net::{SocketAddr, UdpSocket};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USAGE: &str = "Usage: b3313-netsim <address:port to listen on> <server address:port> \
[--delay MS] [--jitter MS] [--loss PERCENT] [--seed N]

Each packet, in each direction, waits the delay plus up to the jitter, and is dropped at the loss \
rate. Point bridges at the listening address. The same seed repeats the same drops and delays.";

fn run() -> Result<(), Box<dyn Error>> {
    let options = Options::parse(
        std::env::args().skip(1),
        &["delay", "jitter", "loss", "seed"],
    )?;
    let [listen, server] = options.positional.as_slice() else {
        return Err(USAGE.into());
    };
    // As a bridge does: for "localhost" Windows gives the IPv6 address first, where a server
    // listening on IPv4 is not.
    let server: SocketAddr = b3313_bridge::client::resolve(server)?;
    let loss_percent: f64 = options.get("loss", 0.0)?;
    if !(0.0..=100.0).contains(&loss_percent) {
        return Err("--loss must be from 0 to 100".into());
    }
    let impairment = Impairment {
        delay: Duration::from_millis(options.get("delay", 0)?),
        jitter: Duration::from_millis(options.get("jitter", 0)?),
        loss: loss_percent / 100.0,
    };
    let clock_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let seed = options.get("seed", clock_seed)?;

    let socket = UdpSocket::bind(listen)?;
    println!(
        "Forwarding {} to {server} with {} ms delay, up to {} ms jitter and {loss_percent}% loss \
         (seed {seed}).",
        socket.local_addr()?,
        impairment.delay.as_millis(),
        impairment.jitter.as_millis(),
    );
    Proxy::new(socket, server, impairment, seed)?.run(&AtomicBool::new(false))?;
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
