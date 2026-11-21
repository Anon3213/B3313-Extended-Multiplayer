//! Runs simulated bridges against a server and reports bandwidth, loss, round trips and tick cost.
//!
//! Each simulated client runs the bridge's own `Connection` at the bridge's rate, so the numbers
//! are the ones real bridges would cause, minus the game.

use b3313_bridge::client::{Connection, resolve};
use b3313_bridge::interpolation::Interpolator;
use b3313_bridge::pacing::{FRAME_INTERVAL, SEND_INTERVAL, Schedule};
use b3313_nettools::motion::{MotionMeter, MotionReport, moving_state};
use b3313_nettools::options::Options;
use b3313_protocol::metrics::{LinkReport, REPORT_INTERVAL};
use b3313_protocol::{MAX_PLAYERS, PROGRESS_SIZE, Progress, WORLD_INPUTS_SIZE};
use b3313_server::{Config, Event, StopOnDrop, serve};
use std::error::Error;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;
use std::thread;
use std::time::{Duration, Instant};

const USAGE: &str = "Usage: b3313-loadtest [--clients N] [--areas N] [--seconds S] \
[--server ADDRESS:PORT]

Without --server, it starts its own server on this PC. Point --server at b3313-netsim to measure \
a bad connection. --areas spreads the clients over that many areas of one level, as players \
exploring apart would be. Reports cover the first 10 seconds, so runs last at least that long. \
A --server with a password needs it in B3313_PASSWORD, as the bridge does.";

/// One client's results: its link, and, for the observing client, how evenly others were drawn.
type ClientResult = (LinkReport, Option<MotionReport>);

/// The server the clients join, and its session password.
#[derive(Debug)]
struct Target {
    server: SocketAddr,
    password: String,
}

/// How often a client without the session's world offers its own, as the bridge does.
const WORLD_OFFER_INTERVAL: Duration = Duration::from_secs(1);

/// Progress of one star, the first after the save's flags word.
fn one_star() -> Progress {
    let mut bytes = [0; PROGRESS_SIZE];
    if let Some(first_stars) = bytes.get_mut(4) {
        *first_stars = 1;
    }
    Progress::from_bytes(bytes)
}

/// Runs one simulated bridge until `until`, and returns its first full report.
///
/// It moves at a steady speed in area `area`, and if `observe` is set, it draws everyone else as
/// the bridge does.
fn simulate_client(
    target: &Target,
    until: Instant,
    area: u8,
    observe: bool,
) -> io::Result<ClientResult> {
    // No game: a simulated bridge joins a session whichever one it plays.
    let mut connection = Connection::join(target.server, &target.password, 0)?;
    let start = Instant::now();
    let mut interpolator = Interpolator::default();
    let mut motion = MotionMeter::new(start, area);
    let mut first_report = None;
    let mut sends = Schedule::new(start, SEND_INTERVAL);
    let mut frames = Schedule::new(start, FRAME_INTERVAL);
    // As a bridge does, so the session has a world and some progress, which the server then
    // sends on to everyone: a run without them would leave that traffic out.
    let mut world_offers = Schedule::new(start, WORLD_OFFER_INTERVAL);
    let mut has_world = false;
    while Instant::now() < until {
        let now = Instant::now();
        if sends.is_due(now) {
            connection.send_state(moving_state(area, start.elapsed().as_secs_f32()))?;
        }
        if !has_world && world_offers.is_due(now) {
            connection.offer_world(&[0; WORLD_INPUTS_SIZE])?;
        }
        connection.ping_if_due(now)?;
        if frames.is_due(now) && observe {
            motion.frame(now, &interpolator.sample(now));
        }
        let received = connection.poll(sends.next().min(frames.next()))?;
        if let Some(snapshot) = received.snapshot {
            interpolator.receive(snapshot.arrived, snapshot.present, &snapshot.players);
        }
        if received.world.is_some() && !has_world {
            has_world = true;
            if observe {
                connection.send_progress(&one_star())?;
            }
        }
        if let Some(report) = connection.report_if_due(now) {
            first_report.get_or_insert(report);
        }
    }
    connection.leave();
    let report =
        first_report.ok_or_else(|| io::Error::other("the run ended before a full report"))?;
    Ok((report, observe.then(|| motion.report())))
}

/// Runs `count` clients, dealt round the first `areas` areas in turn; client 0 observes.
fn simulate_clients(
    target: &Target,
    count: u8,
    areas: u8,
    until: Instant,
) -> io::Result<Vec<ClientResult>> {
    thread::scope(|scope| {
        let clients: Vec<_> = (0..count)
            .map(|index| {
                let area = index % areas + 1;
                scope.spawn(move || simulate_client(target, until, area, index == 0))
            })
            .collect();
        clients
            .into_iter()
            .map(|client| {
                client
                    .join()
                    .map_err(|_| io::Error::other("a client thread panicked"))?
            })
            .collect()
    })
}

/// Runs the clients against a server of its own, and returns the server's events too.
fn with_own_server(
    count: u8,
    areas: u8,
    until: Instant,
) -> io::Result<(Vec<ClientResult>, Vec<Event>)> {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
    let server = socket.local_addr()?;
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        let serving = scope.spawn(|| {
            let mut events = Vec::new();
            let record = |event| events.push(event);
            serve(&socket, Config::default(), &stop, record).map(|()| events)
        });
        let stopping = StopOnDrop(&stop);
        let target = Target {
            server,
            password: String::new(),
        };
        let clients = simulate_clients(&target, count, areas, until);
        drop(stopping);
        let events = serving
            .join()
            .map_err(|_| io::Error::other("the server thread panicked"))??;
        Ok((clients?, events))
    })
}

fn kilobytes(bytes_per_second: f64) -> String {
    format!("{:.1} KB/s", bytes_per_second / 1000.0)
}

fn millis(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.0)
}

/// Sums (sent, received) byte rates.
fn total_rates(rates: impl Iterator<Item = (f64, f64)>) -> (f64, f64) {
    rates.fold((0.0, 0.0), |(all_sent, all_received), (sent, received)| {
        (all_sent + sent, all_received + received)
    })
}

fn print_clients(results: &[ClientResult]) {
    println!("Each client, as its bridge saw it:");
    for (index, (report, _)) in results.iter().enumerate() {
        println!("  client {index}: {report}");
    }
    for motion in results.iter().filter_map(|(_, motion)| motion.as_ref()) {
        println!("Motion, as client 0 drew the others: {motion}");
    }
    let reports: Vec<LinkReport> = results.iter().map(|(report, _)| *report).collect();
    let (sent, received) = total_rates(reports.iter().map(LinkReport::byte_rates));
    let worst_loss = reports
        .iter()
        .map(|report| report.losses.rate())
        .fold(0.0, f64::max);
    println!(
        "Clients in total: sent {}, received {}; worst loss {:.1}%",
        kilobytes(sent),
        kilobytes(received),
        worst_loss * 100.0
    );
    let means: Vec<Duration> = reports
        .iter()
        .filter_map(|report| report.round_trips.mean())
        .collect();
    let worst = reports
        .iter()
        .map(|report| report.round_trips.max)
        .max()
        .unwrap_or_default();
    if let Some(mean) = u32::try_from(means.len())
        .ok()
        .and_then(|count| means.iter().sum::<Duration>().checked_div(count))
    {
        println!(
            "Round trips: {} on average, {} at worst",
            millis(mean),
            millis(worst)
        );
    }
}

fn print_server(events: &[Event]) {
    let ticks = events.iter().find_map(|event| match event {
        Event::Ticks(report) => Some(*report),
        _ => None,
    });
    let (sent, received) = total_rates(events.iter().filter_map(|event| match event {
        Event::Link(_, report) => Some(report.byte_rates()),
        _ => None,
    }));
    let ticks = ticks.map_or_else(|| "no tick report".to_owned(), |t| t.to_string());
    println!(
        "Server: sent {}, received {}; {ticks}",
        kilobytes(sent),
        kilobytes(received),
    );
}

fn run() -> Result<(), Box<dyn Error>> {
    let names = ["clients", "areas", "seconds", "server"];
    let options = Options::parse(std::env::args().skip(1), &names)?;
    if !options.positional.is_empty() {
        return Err(USAGE.into());
    }
    let clients: u8 = options.get("clients", MAX_PLAYERS)?;
    let areas: u8 = options.get("areas", 1)?;
    let seconds: u64 = options.get("seconds", REPORT_INTERVAL.as_secs() + 1)?;
    if !(1..=MAX_PLAYERS).contains(&clients) {
        return Err(format!("--clients must be from 1 to {MAX_PLAYERS}").into());
    }
    if !(1..=clients).contains(&areas) {
        return Err("--areas must be from 1 to the number of clients".into());
    }
    if seconds <= REPORT_INTERVAL.as_secs() {
        return Err(format!("--seconds must be over {}", REPORT_INTERVAL.as_secs()).into());
    }
    let until = Instant::now() + Duration::from_secs(seconds);

    if options.has("server") {
        let server: String = options.get("server", String::new())?;
        // As a bridge does: for "localhost" Windows gives the IPv6 address first, where a
        // server listening on IPv4 is not.
        let server = resolve(&server)?;
        println!("{clients} clients in {areas} areas for {seconds} s against {server}");
        let target = Target {
            server,
            password: std::env::var("B3313_PASSWORD").unwrap_or_default(),
        };
        print_clients(&simulate_clients(&target, clients, areas, until)?);
    } else {
        println!("{clients} clients in {areas} areas for {seconds} s against a server on this PC");
        let (reports, events) = with_own_server(clients, areas, until)?;
        print_clients(&reports);
        print_server(&events);
    }
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
