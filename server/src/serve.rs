//! A session on a socket: every datagram through the gate to the session, a tick's packets
//! out thirty times a second, and the commands of the server's owner and of the chat carried
//! out between.

use crate::gate::Gate;
use crate::{Command, Config, Event, Session, TICK_INTERVAL};
use b3313_protocol::{MAX_DATAGRAM_SIZE, Packet, PlayerId, RemoveReason, is_oversized_datagram};
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// How often `serve` checks its socket between ticks while anyone is playing. Not a socket
/// timeout: on Windows those round up to the 15.6 ms system timer, which made ticks late by up
/// to that much.
const POLL_STEP: Duration = Duration::from_millis(1);
/// How often it checks while nobody is playing, when there is no one for a late answer to
/// keep waiting but a stranger joining, whom this costs a twentieth of a second at most over
/// the handshake's two exchanges. Waking a thousand times a second for nobody took 1.4% of a
/// processor core, more than sixteen players add to it.
const IDLE_POLL_STEP: Duration = Duration::from_millis(20);
/// Datagrams taken in one turn of the serve loop before it looks whether a tick is due.
/// Sixteen bridges send far fewer in a turn; only a flood reaches it.
const MOST_RECEIVED_PER_TURN: usize = 256;

/// Sets a `serve` stop flag when dropped. Code that runs `serve` on another thread holds one, so
/// that if it panics, the server stops instead of keeping the thread, and the panic, waiting.
#[derive(Debug)]
pub struct StopOnDrop<'a>(pub &'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Runs a session on `socket` until `stop` is set, reporting events to `log`.
///
/// # Errors
/// Fails if the socket stops working. A single failed send does not stop the server.
pub fn serve(
    socket: &UdpSocket,
    config: Config,
    stop: &AtomicBool,
    log: impl FnMut(Event),
) -> io::Result<()> {
    let (_, no_commands) = mpsc::channel();
    serve_with_console(socket, config, stop, &no_commands, log)
}

/// Tells the bridge at `address` that its player is out of the session, and why. The notice is
/// sent a few times, since UDP may lose one; a bridge that misses them all times out instead.
fn notify_removed(socket: &UdpSocket, gate: &mut Gate, address: SocketAddr, reason: RemoveReason) {
    /// How many times the notice goes. It cannot be sent until acknowledged, as the player's
    /// channel is forgotten with them.
    const NOTICES: u8 = 3;
    for _ in 0..NOTICES {
        if let Some(notice) = gate.seal(address, &Packet::Removed(reason)) {
            let _ = socket.send_to(&notice, address);
        }
    }
}

/// Removes a player and tells their bridge why.
fn remove_player(
    socket: &UdpSocket,
    session: &mut Session,
    gate: &mut Gate,
    player: PlayerId,
    reason: RemoveReason,
) -> Option<SocketAddr> {
    let address = session.kick(player, reason)?;
    notify_removed(socket, gate, address, reason);
    gate.forget(address);
    Some(address)
}

/// Bans `ip`, and removes everyone playing from it, each told why: a ban is of an address,
/// and the gate hears nobody from it afterwards, whoever of them was meant.
fn ban_address(socket: &UdpSocket, session: &mut Session, gate: &mut Gate, ip: IpAddr) {
    let ip = ip.to_canonical();
    for (player, address) in session.players() {
        if address.ip().to_canonical() == ip {
            remove_player(socket, session, gate, player, RemoveReason::Banned);
        }
    }
    gate.ban(ip);
}

/// Carries out one console command.
fn obey(
    command: Command,
    socket: &UdpSocket,
    session: &mut Session,
    gate: &mut Gate,
) -> Option<Event> {
    match command {
        Command::List => Some(Event::Players(session.players())),
        Command::Kick(player) => {
            let removed = remove_player(socket, session, gate, player, RemoveReason::Kicked);
            // The session reports a kick itself.
            removed.is_none().then(|| no_player(player))
        }
        Command::Ban(player) => {
            let playing = session.players();
            let address = playing.iter().find(|&&(other, _)| other == player);
            let address = address.map(|&(_, address)| address);
            match address {
                Some(address) => {
                    ban_address(socket, session, gate, address.ip());
                    Some(Event::Banned(player, address.ip()))
                }
                None => Some(no_player(player)),
            }
        }
        Command::BanAddress(ip) => {
            ban_address(socket, session, gate, ip);
            Some(Event::AddressBanned(ip))
        }
        Command::Unban(ip) => Some(if gate.unban(ip) {
            Event::Unbanned(ip)
        } else {
            Event::CommandFailed(format!("{ip} is not banned"))
        }),
        // The session reports a change of role, and what the server's owner says, itself.
        Command::Mod(player) => session
            .set_moderator(player, true)
            .err()
            .map(Event::CommandFailed),
        Command::Unmod(player) => session
            .set_moderator(player, false)
            .err()
            .map(Event::CommandFailed),
        Command::Host(player) => session.make_host(player).err().map(Event::CommandFailed),
        Command::Say(text) => {
            session.say_as_server(&text);
            None
        }
        // Only reported here: `serve_with_console`, which stops for it, tells the players.
        Command::End => Some(Event::Ended),
        // Whoever starts the server again says so.
        Command::Restart => None,
    }
}

fn no_player(player: PlayerId) -> Event {
    Event::CommandFailed(format!("there is no player {}", player.get()))
}

/// Runs a session as `serve` does, also carrying out the console's commands as they arrive.
/// It runs until `stop` is set or the session is ended, from the console or the chat;
/// either way every player's bridge is told the session has ended, so none waits out the
/// silence to learn it.
///
/// # Errors
/// Fails if the socket stops working. A single failed send does not stop the server.
pub fn serve_with_console(
    socket: &UdpSocket,
    config: Config,
    stop: &AtomicBool,
    commands: &Receiver<Command>,
    mut log: impl FnMut(Event),
) -> io::Result<()> {
    let mut gate = Gate::new(&config, Instant::now()).map_err(io::Error::other)?;
    let mut session = Session::with_config(config);
    let mut buffer = [0; MAX_DATAGRAM_SIZE + 1];
    socket.set_nonblocking(true)?;
    let mut next_tick = Instant::now() + TICK_INTERVAL;
    // What the players' bridges are told as the server stops: that the session is over, unless
    // the server is to be started again, when they are to come back.
    let mut parting = RemoveReason::Ended;
    let mut ended = false;
    while !ended && !stop.load(Ordering::Relaxed) {
        // Until nothing waits, and so many at most: a flood must not put the ticks off. The
        // turn's first look is never skipped, or what came in just before a tick would wait
        // for the tick after.
        for _ in 0..MOST_RECEIVED_PER_TURN {
            let (length, from) = match socket.recv_from(&mut buffer) {
                Ok(received) => received,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                // Windows reports an earlier send to a closed port as a failed receive.
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => continue,
                // Anyone can send one; it is no reason to stop.
                Err(error) if is_oversized_datagram(&error) => continue,
                Err(error) => return Err(error),
            };
            let data = buffer.get(..length).unwrap_or_default();
            if let Some(reply) = gate.receive(&mut session, from, data, Instant::now()) {
                // UDP gives no delivery promise anyway; the client resends Hello.
                let _ = socket.send_to(&reply, from);
                session.record_sent(from, reply.len());
            }
        }
        let now = Instant::now();
        if now >= next_tick {
            session.record_rate_limited(gate.take_rate_limited());
            for (address, packet) in session.tick(now) {
                if let Some(sealed) = gate.seal(address, &packet) {
                    let _ = socket.send_to(&sealed, address);
                    session.record_sent(address, sealed.len());
                }
            }
            gate.forget_departed(&session, now);
            session.record_tick_time(now.saturating_duration_since(next_tick), now.elapsed());
            // From the schedule, not from now, so small delays do not add up over time. After a
            // long stall, such as the PC sleeping, it starts afresh instead of sending a burst.
            next_tick += TICK_INTERVAL;
            if next_tick <= now {
                next_tick = now + TICK_INTERVAL;
            }
        }
        // The console's commands, then what the host or a moderator asked for in the chat.
        let orders = session.take_orders();
        for command in commands.try_iter().chain(orders) {
            if command == Command::Restart {
                parting = RemoveReason::Restarting;
            }
            ended |= matches!(command, Command::End | Command::Restart);
            let outcome = obey(command, socket, &mut session, &mut gate);
            // What the command did to the session, such as removing a banned player, comes
            // before the command's own report, as it happened.
            session.take_events().into_iter().for_each(&mut log);
            if let Some(event) = outcome {
                log(event);
            }
        }
        session.take_events().into_iter().for_each(&mut log);
        let longest_wait = if session.has_players() {
            POLL_STEP
        } else {
            IDLE_POLL_STEP
        };
        thread::sleep(
            next_tick
                .saturating_duration_since(Instant::now())
                .min(longest_wait),
        );
    }
    for (_, address) in session.players() {
        notify_removed(socket, &mut gate, address, parting);
    }
    Ok(())
}
