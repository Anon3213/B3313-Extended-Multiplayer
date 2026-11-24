//! The bridge's connection to the session server, and how a snapshot reaches the game.

use crate::mailbox::{
    Mailbox, ObjectsOut, REMOTE_SLOT_COUNT, REMOTE_SLOTS, RemoteSlot, RingCoin, RingInteraction,
};
use crate::objects::ObjectNews;
use crate::ram::ProcessMemory;
use b3313_protocol::metrics::{LinkMeter, LinkReport};
use b3313_protocol::secure::{Channel, Initiator, PublicKey, Salt, derive_key};
use b3313_protocol::{
    Ack, ChatLine, Coin, DecodeError, Hit, HitKind, Hurt, Inbox, Interaction, MAX_COIN_VALUE,
    MAX_COINS_PER_PACKET, MAX_DATAGRAM_SIZE, MAX_HITS_PER_PACKET, MAX_INTERACTIONS_PER_PACKET,
    Message, Name, ObjectState, Outbox, Outfit, PROTOCOL_VERSION, Packet, PlayerId, PlayerState,
    PlayerUpdate, Progress, RejectReason, RemoveReason, RosterEntry, Stream, WorldInputs, game,
    is_oversized_datagram,
};
use std::collections::VecDeque;
use std::fmt;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long to keep asking to join before giving up.
pub const JOIN_TIMEOUT: Duration = Duration::from_secs(10);
const HELLO_INTERVAL: Duration = Duration::from_millis(250);
const PING_INTERVAL: Duration = Duration::from_secs(1);
/// Pings older than this many are taken as lost and forgotten.
const PINGS_IN_FLIGHT: usize = 8;
/// How often `poll` checks the socket while it waits. Not a socket timeout: on Windows those round
/// up to the 15.6 ms system timer, which turned a 33 ms wait into 47.
const POLL_STEP: Duration = Duration::from_millis(1);
/// The server sends a snapshot every tick, so this much silence means it is gone.
pub const SERVER_TIMEOUT: Duration = Duration::from_secs(5);

/// The game slot that shows player `other`, as seen by player `me`, who has no slot of their own.
///
/// Stable while both stay connected, so a puppet does not jump between slots.
#[must_use]
pub fn slot_for(me: PlayerId, other: PlayerId) -> Option<RemoteSlot> {
    match other.get().cmp(&me.get()) {
        std::cmp::Ordering::Less => RemoteSlot::new(other.get()),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => RemoteSlot::new(other.get() - 1),
    }
}

/// The player that game slot `slot` shows, as seen by player `me`: the reverse of `slot_for`.
#[must_use]
pub fn player_for(me: PlayerId, slot: RemoteSlot) -> Option<PlayerId> {
    if slot.index() < me.get() {
        PlayerId::new(slot.index())
    } else {
        PlayerId::new(slot.index() + 1)
    }
}

/// A player is written again after this many draws even if nothing about them changed: a
/// game whose slots were cleared under the bridge, as when its lease ran out while the PC
/// slept, has everyone back within a second.
const REFRESH_DRAWS: u8 = 60;

/// What a remote slot was last given, and how many draws ago.
#[derive(Clone, Copy, Debug)]
struct Shown {
    state: PlayerState,
    draws_since: u8,
}

/// What each remote slot shows, so departures can be cleared, and a player who has not
/// changed since the last draw is not written into the game again: each write is four calls
/// into the emulator's process, sixty times a second for every player.
#[derive(Debug, Default)]
pub struct ShownPlayers([Option<Shown>; REMOTE_SLOT_COUNT]);

impl ShownPlayers {
    fn slots(&self) -> impl Iterator<Item = RemoteSlot> + '_ {
        (0..REMOTE_SLOTS)
            .zip(self.0)
            .filter_map(|(index, shown)| shown.and(RemoteSlot::new(index)))
    }
}

/// Writes every player in a snapshot into the game, and clears slots of players who left.
///
/// # Errors
/// Fails if the game's memory cannot be written.
pub fn apply_snapshot<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    me: PlayerId,
    players: &[(PlayerId, PlayerState)],
    shown: &mut ShownPlayers,
) -> io::Result<()> {
    let mut present = ShownPlayers::default();
    for (other, state) in players {
        let Some(slot) = slot_for(me, *other) else {
            continue;
        };
        let index = usize::from(slot.index());
        let now_shown = match shown.0.get(index).copied().flatten() {
            Some(last) if last.state == *state && last.draws_since < REFRESH_DRAWS => Shown {
                draws_since: last.draws_since + 1,
                ..last
            },
            _ => {
                mailbox.write_remote_player(slot, state)?;
                Shown {
                    state: *state,
                    draws_since: 0,
                }
            }
        };
        if let Some(entry) = present.0.get_mut(index) {
            *entry = Some(now_shown);
        }
    }
    for slot in shown.slots() {
        let is_present = present.0.get(usize::from(slot.index()));
        if is_present.is_none_or(Option::is_none) {
            mailbox.clear_remote_player(slot)?;
        }
    }
    *shown = present;
    Ok(())
}

/// Removes every player this bridge put into the game.
///
/// # Errors
/// Fails if the game's memory cannot be written.
pub fn clear_all<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    shown: &mut ShownPlayers,
) -> io::Result<()> {
    for slot in shown.slots() {
        mailbox.clear_remote_player(slot)?;
    }
    *shown = ShownPlayers::default();
    Ok(())
}

/// Every other player's state at one server tick.
#[derive(Debug)]
pub struct Snapshot {
    pub tick: u32,
    pub arrived: Instant,
    /// Everyone else in the session, one bit each; players elsewhere get no state every tick.
    pub present: u16,
    pub players: Vec<PlayerUpdate>,
}

/// What the server sent since the last poll.
#[derive(Debug, Default)]
pub struct Received {
    /// The newest snapshot of everyone else, if it is newer than any before it.
    pub snapshot: Option<Snapshot>,
    /// The session's world, once it has one.
    pub world: Option<Box<WorldInputs>>,
    /// Hits on this player that the server accepted, each once, oldest first.
    pub hurts: Vec<Hurt>,
    /// Set if the server removed this player from the session.
    pub removed: Option<RemoveReason>,
    /// What the server said about the objects where this player is.
    pub objects: Option<ObjectNews>,
    /// The session's progress, if the server sent it.
    pub progress: Option<Box<Progress>>,
    /// Coins other players in this level collected, each once, oldest first.
    pub coins: Vec<RingCoin>,
    /// Everyone in the session with their roles and names, if the server sent it.
    pub roster: Option<Vec<RosterEntry>>,
    /// Chat lines for this player, each once, oldest first.
    pub said: Vec<ChatLine>,
}

#[derive(Debug)]
pub struct Connection {
    socket: UdpSocket,
    player: PlayerId,
    /// The key the server proved it holds in the handshake.
    server_key: PublicKey,
    /// Seals every packet sent and opens every packet received after the handshake.
    channel: Channel,
    sequence: u32,
    last_heard: Instant,
    /// Snapshot ticks arrive in order unless lost, so the meter's losses are the server's.
    meter: LinkMeter,
    /// Pings awaiting a Pong, oldest first.
    pings: VecDeque<(u32, Instant)>,
    next_ping: u32,
    last_ping: Option<Instant>,
    /// Hits landed, each sent until the server acknowledges it; and which hurts have been taken,
    /// so repeats are dropped.
    hits: Outbox<Hit>,
    hurts: Inbox,
    /// The objects others' games run that this Mario attacked or picked up.
    interactions: Outbox<Interaction>,
    /// Coins this Mario collected, and others' coins.
    coins: Outbox<Coin>,
    coins_collected: Inbox,
    /// What this player typed, and the chat lines for them.
    says: Outbox<(u16, Message)>,
    said: Inbox,
}

/// Which of a name's `addresses` to join at: the first IPv4 one, if it has any. A server
/// listens on IPv4 unless told otherwise, and a name may put an IPv6 address first, as
/// Windows does for `localhost`, where that server hears nothing.
fn preferred(addresses: &[SocketAddr]) -> Option<SocketAddr> {
    let ipv4 = addresses.iter().find(|address| address.is_ipv4());
    ipv4.or(addresses.first()).copied()
}

/// The address of the server the player gave as `server`: a name or an address, with its port.
///
/// # Errors
/// Fails if the name cannot be looked up or has no address.
pub fn resolve(server: &str) -> io::Result<SocketAddr> {
    let addresses: Vec<SocketAddr> = server.to_socket_addrs()?.collect();
    preferred(&addresses)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "the name has no address"))
}

/// Looks `server` up as `resolve` does, giving up after `limit`: for a bridge with a game to
/// keep going meanwhile, as a name's lookup can take many seconds to fail. None if there was
/// no answer in time, or no address.
///
/// An address written out needs no lookup and is answered at once. A lookup given up on runs
/// to its end on a thread of its own, and its answer is dropped.
#[must_use]
pub fn resolve_within(server: &str, limit: Duration) -> Option<SocketAddr> {
    if let Ok(address) = server.parse() {
        return Some(address);
    }
    let (answer, answered) = mpsc::channel();
    let name = server.to_owned();
    thread::spawn(move || {
        // Nobody waits any more if the limit has passed.
        let _ = answer.send(resolve(&name).ok());
    });
    answered.recv_timeout(limit).ok().flatten()
}

/// Why a join was given up though the server answered and knew the password: it is not the
/// server the player meant, whose key they had from a join code or an earlier join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrongServer {
    pub expected: PublicKey,
    pub met: PublicKey,
}

impl fmt::Display for WrongServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the server that answered is not the one expected: its key is {}, and the one \
             expected is {}. If its owner set it up anew or moved it to another PC, ask them \
             for its new join code",
            self.met, self.expected
        )
    }
}

impl std::error::Error for WrongServer {}

/// The two keys, if `error` is a join given up because another server answered.
#[must_use]
pub fn wrong_server(error: &io::Error) -> Option<WrongServer> {
    error.get_ref()?.downcast_ref().copied()
}

/// What a player is told whose game is the version of B3313 numbered `own`, on being refused
/// by a session that plays the one numbered `session`.
fn wrong_game(session: u8, own: u8) -> String {
    format!(
        "this session is playing {}, and your game is {}; everyone in a session must play the \
         same version of B3313",
        game::describe(session),
        game::describe(own)
    )
}

fn is_quiet(error: &io::Error) -> bool {
    // Windows reports an earlier send to a closed port as a failed receive. The server timeout
    // covers that case, so here it means only that no packet arrived. Nor is a datagram too
    // long to be a packet one.
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset
    ) || is_oversized_datagram(error)
}

impl Connection {
    /// Joins the session at `server` with the session's `password`, empty for an open session.
    /// `game` is the version of B3313 the player's game is, as `b3313_protocol::game` numbers
    /// them, or 0 if it is not known: a session takes only players of the game it plays.
    ///
    /// The server first challenges the client to prove it receives at its address, and sends the
    /// salt for the password's key; the client then completes the encrypted handshake.
    ///
    /// Whatever server answers there is joined: `join_unless` takes the key it must have.
    ///
    /// # Errors
    /// Fails if the password is wrong, the server is full, plays another version of B3313, runs
    /// another protocol version, or does not answer.
    pub fn join(server: SocketAddr, password: &str, game: u8) -> io::Result<Self> {
        Self::join_within(server, password, game, JOIN_TIMEOUT)
    }

    /// Joins as `join` does, giving up after `limit` if the server has not answered: for a
    /// bridge that is joining again, and has a game to keep going between its tries.
    ///
    /// # Errors
    /// As `join`; a server that did not answer in time is `io::ErrorKind::TimedOut`.
    pub fn join_within(
        server: SocketAddr,
        password: &str,
        game: u8,
        limit: Duration,
    ) -> io::Result<Self> {
        Self::join_unless(
            (server, None),
            password,
            game,
            limit,
            &AtomicBool::new(false),
        )
    }

    /// Joins as `join_within` does, giving up as soon as `stop` is set: for a session that may
    /// be left while it is still joining, which must not wait out a server that never answers.
    /// The server is its address and, if the player knows it, the key it must have: an
    /// address may come to be another server's, and the password is known to every player,
    /// so only the key says that this is the server meant.
    ///
    /// # Errors
    /// As `join_within`; one given up for `stop` is `io::ErrorKind::Interrupted`, and a
    /// server with another key than the one given is told by `wrong_server`.
    pub fn join_unless(
        (server, key): (SocketAddr, Option<PublicKey>),
        password: &str,
        game: u8,
        limit: Duration,
        stop: &AtomicBool,
    ) -> io::Result<Self> {
        let local: SocketAddr = if server.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let socket = UdpSocket::bind(local)?;
        // A connected socket drops datagrams from anyone but the server.
        socket.connect(server)?;
        socket.set_read_timeout(Some(HELLO_INTERVAL))?;

        let deadline = Instant::now() + limit;
        let mut buffer = [0; MAX_DATAGRAM_SIZE + 1];
        let mut cookie = 0;
        // Started once the server's salt is known, and again if a restarted server sends another.
        let mut handshake: Option<(Salt, Initiator, Vec<u8>)> = None;
        while Instant::now() < deadline {
            // Looked at every `HELLO_INTERVAL`, which is as long as a receive waits.
            if stop.load(Ordering::Relaxed) {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let first = handshake.as_ref().map(|(_, _, message)| message.clone());
            let hello = Packet::Hello {
                cookie,
                game,
                handshake: first.unwrap_or_default(),
            };
            let asked = Instant::now();
            socket.send(&hello.encode())?;
            let length = match socket.recv(&mut buffer) {
                Ok(length) => length,
                Err(error) if is_quiet(&error) => {
                    // A port nobody listens at is said at once, where a silent server is
                    // waited out: either way the next Hello goes an interval after this one.
                    thread::sleep(HELLO_INTERVAL.saturating_sub(asked.elapsed()));
                    continue;
                }
                Err(error) => return Err(error),
            };
            match Packet::decode(buffer.get(..length).unwrap_or_default()) {
                Ok(Packet::Challenge {
                    cookie: given,
                    salt,
                }) => {
                    cookie = given;
                    let is_new_salt = handshake.as_ref().is_none_or(|(known, ..)| *known != salt);
                    if is_new_salt {
                        // Argon2 runs here, once: tens of milliseconds by design.
                        let key = derive_key(password, &salt).map_err(io::Error::other)?;
                        let (initiator, message) =
                            Initiator::start(&key).map_err(io::Error::other)?;
                        handshake = Some((salt, initiator, message));
                    }
                }
                Ok(Packet::Accept { handshake: reply }) => {
                    let Some((_, initiator, _)) = handshake.take() else {
                        continue;
                    };
                    let (channel, payload, met) = initiator.finish(&reply).map_err(|_| {
                        io::Error::other("the server's handshake did not check out")
                    })?;
                    return match Packet::decode(&payload) {
                        Ok(Packet::Welcome { player }) => {
                            let mut joined = Self::joined(socket, player, channel, met)?;
                            match key {
                                Some(expected) if expected != met => {
                                    // It has this player down as joined: free the place.
                                    joined.leave();
                                    let wrong = WrongServer { expected, met };
                                    Err(io::Error::new(io::ErrorKind::InvalidData, wrong))
                                }
                                _ => Ok(joined),
                            }
                        }
                        Ok(Packet::Rejected(RejectReason::Full)) => {
                            Err(io::Error::other("the session is full"))
                        }
                        Ok(Packet::Rejected(RejectReason::WrongGame(session))) => {
                            Err(io::Error::other(wrong_game(session, game)))
                        }
                        _ => Err(io::Error::other("the server's answer made no sense")),
                    };
                }
                Ok(Packet::Rejected(RejectReason::WrongPassword)) => {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "the session password is wrong",
                    ));
                }
                // A server of another version rejects in its own version's packets, which do
                // not decode here; their version number is what says so.
                Err(DecodeError::WrongVersion(theirs)) => {
                    return Err(io::Error::other(if theirs > PROTOCOL_VERSION {
                        "the server runs a newer version of B3313 Extended Multiplayer than \
                         yours; update yours to join"
                    } else {
                        "the server runs an older version of B3313 Extended Multiplayer than \
                         yours; its owner needs to update it"
                    }));
                }
                // Noise, or a late answer to an earlier Hello; keep asking.
                Ok(_) | Err(_) => {}
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the server did not answer",
        ))
    }

    fn joined(
        socket: UdpSocket,
        player: PlayerId,
        channel: Channel,
        server_key: PublicKey,
    ) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        let now = Instant::now();
        Ok(Self {
            socket,
            player,
            server_key,
            channel,
            sequence: 0,
            last_heard: now,
            meter: LinkMeter::new(now),
            pings: VecDeque::with_capacity(PINGS_IN_FLIGHT),
            next_ping: 0,
            last_ping: None,
            hits: Outbox::default(),
            hurts: Inbox::default(),
            interactions: Outbox::default(),
            coins: Outbox::default(),
            coins_collected: Inbox::default(),
            says: Outbox::default(),
            said: Inbox::default(),
        })
    }

    /// Tells the server who this player is: the name they chose, if any, and the host password,
    /// if this bridge has one. Sent until the server's roster lists this player.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn introduce(
        &mut self,
        name: Option<&Name>,
        outfit: Option<Outfit>,
        host_password: &str,
    ) -> io::Result<()> {
        self.send(&Packet::Introduce {
            name: name.cloned(),
            outfit,
            host_password: host_password.to_owned(),
        })
    }

    /// Tells the server the outfit this player now chooses. Nothing acknowledges it but the
    /// roster, so send it until that shows the outfit worn.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn wear(&mut self, outfit: Outfit) -> io::Result<()> {
        self.send(&Packet::Wear(outfit))
    }

    /// Queues something this player typed; `send_says` carries it until the server has it.
    pub fn queue_say(&mut self, text: Message) {
        self.says.push(|id| (id, text));
    }

    /// How many typed messages the server has yet to acknowledge.
    #[must_use]
    pub fn says_waiting(&self) -> usize {
        self.says.len()
    }

    /// Sends the oldest message the server has not acknowledged, if any. Call it once per send
    /// slot.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn send_says(&mut self) -> io::Result<()> {
        match self.says.batch(1).pop() {
            Some((id, text)) => self.send(&Packet::Say { id, text }),
            None => Ok(()),
        }
    }

    /// Takes the chat lines the server sent, each once.
    fn take_said(&mut self, lines: Vec<ChatLine>, received: &mut Received) {
        for line in lines {
            if self.said.take(line.id) {
                received.said.push(line);
            }
        }
    }

    /// Tells the server how far this bridge has taken the events of `stream`, so it stops
    /// sending those again. The server repeats what is not acknowledged, so a lost one costs
    /// nothing.
    fn acknowledge(&mut self, stream: Stream) {
        let inbox = match stream {
            Stream::Attacks => self.hurts,
            // The server sends none of these: it answers a claim with who owns what.
            Stream::Interactions => return,
            Stream::Coins => self.coins_collected,
            Stream::Chat => self.said,
        };
        if let Some(ack) = inbox.ack(stream) {
            // A socket that fails here fails the next send too, which reports it.
            let _ = self.send(&Packet::Acks(vec![ack]));
        }
    }

    /// Takes the server's word on how far it has taken this bridge's events.
    fn acknowledged(&mut self, acks: &[Ack]) {
        for ack in acks {
            match ack.stream {
                Stream::Attacks => self.hits.acknowledge(ack.id),
                Stream::Interactions => self.interactions.acknowledge(ack.id),
                Stream::Coins => self.coins.acknowledge(ack.id),
                Stream::Chat => self.says.acknowledge(ack.id),
            }
        }
    }

    #[must_use]
    pub const fn player(&self) -> PlayerId {
        self.player
    }

    /// The key of the server joined, which it proved it holds: what to know it by next time.
    #[must_use]
    pub const fn server_key(&self) -> PublicKey {
        self.server_key
    }

    /// Sends this player's latest state, in this send slot.
    ///
    /// Call this or `keep_alive` once per `SEND_INTERVAL`: other bridges time this player's states
    /// by their sequence numbers.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn send_state(&mut self, state: PlayerState) -> io::Result<()> {
        self.sequence = self.sequence.wrapping_add(1);
        self.send(&Packet::State {
            sequence: self.sequence,
            state,
        })
    }

    /// Keeps this player in the session while the game has no state to send, as in menus.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn keep_alive(&mut self, away: bool) -> io::Result<()> {
        self.sequence = self.sequence.wrapping_add(1);
        self.send(&Packet::Alive {
            sequence: self.sequence,
            away,
        })
    }

    fn send(&mut self, packet: &Packet) -> io::Result<()> {
        let (nonce, sealed) = self
            .channel
            .seal(&packet.encode())
            .map_err(io::Error::other)?;
        let encoded = Packet::Sealed { nonce, sealed }.encode();
        match self.socket.send(&encoded) {
            Err(error) if !is_quiet(&error) => Err(error),
            _ => {
                self.meter.sent(encoded.len());
                Ok(())
            }
        }
    }

    /// Offers this game's world to the session, which adopts it if this player is the owner.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn offer_world(&mut self, world: &WorldInputs) -> io::Result<()> {
        self.send(&Packet::OfferWorld(Box::new(*world)))
    }

    /// Sends a ping once a `PING_INTERVAL` has passed since the last one.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn ping_if_due(&mut self, now: Instant) -> io::Result<()> {
        if self
            .last_ping
            .is_some_and(|sent| now.saturating_duration_since(sent) < PING_INTERVAL)
        {
            return Ok(());
        }
        let id = self.next_ping;
        self.next_ping = id.wrapping_add(1);
        self.last_ping = Some(now);
        if self.pings.len() == PINGS_IN_FLIGHT {
            self.pings.pop_front();
        }
        self.pings.push_back((id, now));
        self.send(&Packet::Ping { id })
    }

    /// Returns a report of this link once per `REPORT_INTERVAL`.
    pub fn report_if_due(&mut self, now: Instant) -> Option<LinkReport> {
        self.meter.report_if_due(now)
    }

    /// Waits until `until` and returns what arrived: the newest snapshot and the session's world.
    ///
    /// It checks the socket every `POLL_STEP` while waiting, so each Pong is timed within that of
    /// its arrival. When `until` has passed, it still takes what is already waiting.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn poll(&mut self, until: Instant) -> io::Result<Received> {
        let mut received = Received::default();
        let mut buffer = [0; MAX_DATAGRAM_SIZE + 1];
        loop {
            loop {
                let length = match self.socket.recv(&mut buffer) {
                    Ok(length) => length,
                    Err(error) if is_quiet(&error) => break,
                    Err(error) => return Err(error),
                };
                let data = buffer.get(..length).unwrap_or_default();
                self.receive(data, Instant::now(), &mut received);
            }
            let remaining = until.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(received);
            }
            thread::sleep(remaining.min(POLL_STEP));
        }
    }

    fn receive(&mut self, data: &[u8], arrived: Instant, received: &mut Received) {
        // Only what the server sealed on this channel counts; anything else is noise or forged.
        let Ok(Packet::Sealed { nonce, sealed }) = Packet::decode(data) else {
            return;
        };
        let Some(inner) = self.channel.open(nonce, &sealed) else {
            return;
        };
        let Ok(
            packet @ (Packet::Snapshot { .. }
            | Packet::World(_)
            | Packet::Pong { .. }
            | Packet::Hurts(_)
            | Packet::Removed(_)
            | Packet::ObjectUpdates { .. }
            | Packet::SessionProgress(_)
            | Packet::CoinsCollected(_)
            | Packet::Roster(_)
            | Packet::Said(_)
            | Packet::Acks(_)),
        ) = Packet::decode(&inner)
        else {
            return;
        };
        self.last_heard = arrived;
        self.meter.received(data.len());
        match packet {
            Packet::Snapshot {
                tick,
                present,
                players,
            } => {
                if self.meter.arrived(tick, arrived) {
                    received.snapshot = Some(Snapshot {
                        tick,
                        arrived,
                        present,
                        players,
                    });
                }
            }
            Packet::World(world) => received.world = Some(world),
            Packet::Pong { id } => {
                if let Some(index) = self.pings.iter().position(|&(sent, _)| sent == id) {
                    // Pings sent before this one and still unanswered are lost.
                    if let Some((_, sent)) = self.pings.drain(..=index).next_back() {
                        self.meter
                            .round_trip(arrived.saturating_duration_since(sent));
                    }
                }
            }
            Packet::Removed(reason) => received.removed = Some(reason),
            Packet::SessionProgress(progress) => received.progress = Some(progress),
            Packet::CoinsCollected(coins) => {
                self.take_coins(coins, received);
                self.acknowledge(Stream::Coins);
            }
            Packet::Roster(roster) => received.roster = Some(roster),
            Packet::Said(lines) => {
                self.take_said(lines, received);
                self.acknowledge(Stream::Chat);
            }
            Packet::Acks(acks) => self.acknowledged(&acks),
            Packet::ObjectUpdates {
                level,
                area,
                owned,
                gone,
                children,
                states,
            } => {
                let news = ObjectNews {
                    place: (level, area),
                    owned,
                    gone,
                    children,
                    states,
                };
                match &mut received.objects {
                    Some(earlier) => earlier.merge(news),
                    None => received.objects = Some(news),
                }
            }
            Packet::Hurts(hurts) => {
                // Each hurt comes until acknowledged; only ids newer than any taken are new.
                for hurt in hurts {
                    if self.hurts.take(hurt.id) {
                        received.hurts.push(hurt);
                    }
                }
                self.acknowledge(Stream::Attacks);
            }
            _ => {}
        }
    }

    /// Queues a hit this player landed on `target`; `send_hits` carries it until the server
    /// has it.
    pub fn queue_hit(&mut self, target: PlayerId, kind: HitKind) {
        self.hits.push(|id| Hit { target, kind, id });
    }

    /// Sends the hits the server has not acknowledged, if any. Call it once per send slot.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn send_hits(&mut self) -> io::Result<()> {
        if self.hits.is_empty() {
            return Ok(());
        }
        let batch = self.hits.batch(MAX_HITS_PER_PACKET);
        self.send(&Packet::Hits(batch))
    }

    /// Reports the game's synced objects: which are gone, and `states` of those it owns.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn send_objects(
        &mut self,
        out: &ObjectsOut,
        states: Vec<ObjectState>,
        gone_children: Vec<u16>,
    ) -> io::Result<()> {
        let (level, area) = out.place;
        self.send(&Packet::Objects {
            level,
            area,
            // The slot this player's state went in just now: both were read together.
            sequence: self.sequence,
            gone: out.gone,
            gone_children,
            states,
        })
    }

    /// Takes the coins others in this level collected, each once.
    fn take_coins(&mut self, coins: Vec<Coin>, received: &mut Received) {
        for coin in coins {
            if self.coins_collected.take(coin.id) {
                received.coins.push(RingCoin {
                    place: (coin.level, coin.area),
                    name: coin.name,
                    value: coin.value,
                });
            }
        }
    }

    /// Queues a coin this Mario collected; `send_coins` carries it until the server has it.
    /// One the protocol could not carry, worth nothing or too much, is dropped.
    pub fn queue_coin(&mut self, coin: &RingCoin) {
        if !(1..=MAX_COIN_VALUE).contains(&coin.value) {
            return;
        }
        let (level, area) = coin.place;
        let (name, value) = (coin.name, coin.value);
        self.coins.push(|id| Coin {
            level,
            area,
            name,
            value,
            id,
        });
    }

    /// Sends the coins the server has not acknowledged, if any. Call it once per send slot.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn send_coins(&mut self) -> io::Result<()> {
        if self.coins.is_empty() {
            return Ok(());
        }
        let batch = self.coins.batch(MAX_COINS_PER_PACKET);
        self.send(&Packet::Coins(batch))
    }

    /// Reports what this game got during the session.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn send_progress(&mut self, gains: &Progress) -> io::Result<()> {
        self.send(&Packet::Progress(Box::new(*gains)))
    }

    /// Queues this game's claim on an object another game runs, which its Mario attacked or
    /// picked up; `send_interactions` carries it until the server has it.
    pub fn queue_interaction(&mut self, interaction: &RingInteraction) {
        let (level, area) = interaction.place;
        let (object, status) = (interaction.object, interaction.status);
        self.interactions.push(|id| Interaction {
            level,
            area,
            object,
            status,
            id,
        });
    }

    /// Sends the interactions the server has not acknowledged, if any. Call it once per send
    /// slot.
    ///
    /// # Errors
    /// Fails if the socket fails.
    pub fn send_interactions(&mut self) -> io::Result<()> {
        if self.interactions.is_empty() {
            return Ok(());
        }
        let batch = self.interactions.batch(MAX_INTERACTIONS_PER_PACKET);
        self.send(&Packet::Interactions(batch))
    }

    #[must_use]
    pub fn is_server_alive(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_heard) < SERVER_TIMEOUT
    }

    /// Tells the server this player is leaving. Best effort: the server times out silent players.
    pub fn leave(&mut self) {
        let _ = self.send(&Packet::Bye);
    }
}

#[cfg(test)]
mod tests {
    use super::{Connection, ShownPlayers, apply_snapshot, clear_all, player_for, slot_for};
    use crate::layout::len;
    use crate::mailbox::testing::{game, raw};
    use crate::mailbox::{Mailbox, RemoteSlot};
    use crate::ram::N64Ram;
    use b3313_protocol::{PLAYER_STATE_SIZE, PlayerId};
    use b3313_protocol::{PROTOCOL_VERSION, Packet, RejectReason};
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    #[test]
    fn a_name_with_both_kinds_of_address_is_joined_at_its_ipv4_one() {
        let parse = |text: &str| text.parse::<std::net::SocketAddr>().ok();
        let (v6, v4) = (parse("[::1]:41313"), parse("127.0.0.1:41313"));
        let both: Vec<_> = [v6, v4].into_iter().flatten().collect();
        assert_eq!(super::preferred(&both), v4);
        let only_v6: Vec<_> = v6.into_iter().collect();
        assert_eq!(super::preferred(&only_v6), v6);
        assert_eq!(super::preferred(&[]), None);
    }

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    fn slot_contents(process: &crate::ram::testing::FakeProcess, slot: usize) -> io::Result<u8> {
        let at = len::MAILBOX_PLAYERS_OFFSET + slot * len::PLAYER_SLOT_SIZE;
        Ok(raw(process, at, 4)?.first().copied().unwrap_or_default())
    }

    /// A server of another version: it answers every packet as the real one answers a `Hello`
    /// it cannot read, with a rejection in its own version's packets.
    fn server_of_version(version: u8) -> io::Result<std::net::SocketAddr> {
        let socket = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let address = socket.local_addr()?;
        let mut answer = Packet::Rejected(RejectReason::WrongVersion).encode();
        if let Some(byte) = answer.get_mut(4) {
            *byte = version;
        }
        std::thread::spawn(move || {
            let mut buffer = [0; 2048];
            while let Ok((_, from)) = socket.recv_from(&mut buffer) {
                let _ = socket.send_to(&answer, from);
            }
        });
        Ok(address)
    }

    #[test]
    fn a_server_of_another_version_is_named_not_waited_for() -> io::Result<()> {
        for (version, word) in [
            (PROTOCOL_VERSION + 1, "newer"),
            (PROTOCOL_VERSION - 1, "older"),
        ] {
            let started = std::time::Instant::now();
            let Err(error) = Connection::join(server_of_version(version)?, "", 0) else {
                unreachable!("a server of another version cannot be joined");
            };
            assert!(error.to_string().contains(word), "{error}");
            // Said at the first answer, not after the whole wait for a silent server.
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
        }
        Ok(())
    }

    #[test]
    fn every_other_player_gets_a_distinct_slot_and_nobody_gets_their_own() {
        for me in 0..16 {
            let slots: Vec<_> = (0..16)
                .filter_map(|other| slot_for(id(me), id(other)))
                .map(RemoteSlot::index)
                .collect();
            assert_eq!(slots, (0..15).collect::<Vec<_>>(), "player {me}");
        }
    }

    #[test]
    fn each_slot_leads_back_to_the_player_it_shows() {
        for me in 0..16 {
            for other in (0..16).filter(|&other| other != me) {
                let slot = slot_for(id(me), id(other));
                let back = slot.and_then(|slot| player_for(id(me), slot));
                assert_eq!(back, Some(id(other)), "player {me} seeing {other}");
            }
        }
    }

    #[test]
    fn snapshots_fill_slots_and_departures_are_cleared() -> io::Result<()> {
        let process = game();
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let mut shown = ShownPlayers::default();
        let me = id(2);

        apply_snapshot(
            &mailbox,
            me,
            &[
                (id(0), [10; PLAYER_STATE_SIZE]),
                (id(5), [15; PLAYER_STATE_SIZE]),
            ],
            &mut shown,
        )?;
        assert_eq!(slot_contents(&process, 0)?, 10);
        assert_eq!(slot_contents(&process, 4)?, 15);

        apply_snapshot(
            &mailbox,
            me,
            &[(id(5), [15; PLAYER_STATE_SIZE])],
            &mut shown,
        )?;
        assert_eq!(slot_contents(&process, 0)?, 0);
        assert_eq!(slot_contents(&process, 4)?, 15);

        clear_all(&mailbox, &mut shown)?;
        assert_eq!(slot_contents(&process, 4)?, 0);
        Ok(())
    }

    #[test]
    fn a_player_who_has_not_changed_is_written_again_only_now_and_then() -> io::Result<()> {
        let process = game();
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let mut shown = ShownPlayers::default();
        // How many times slot 0 was written: each write moves its sequence on by two.
        let writes = || -> io::Result<u32> {
            let at = len::MAILBOX_PLAYERS_OFFSET + len::PLAYER_SLOT_SEQUENCE_OFFSET;
            let sequence = raw(&process, at, 4)?;
            Ok(sequence.first_chunk().map_or(0, |b| u32::from_be_bytes(*b)) / 2)
        };
        let draw = |state: u8, shown: &mut ShownPlayers| {
            let players = [(id(0), [state; PLAYER_STATE_SIZE])];
            apply_snapshot(&mailbox, id(1), &players, shown)
        };

        for _ in 0..super::REFRESH_DRAWS {
            draw(10, &mut shown)?;
        }
        assert_eq!(writes()?, 1, "standing still");
        draw(11, &mut shown)?;
        assert_eq!(writes()?, 2, "a change is written at once");
        // Wiped under the bridge, as a lease that ran out wipes the players.
        let players = crate::layout::MAILBOX_ADDR + crate::layout::MAILBOX_PLAYERS_OFFSET;
        N64Ram::new(&process, 0).write(players, &[0; 4])?;
        for _ in 0..=super::REFRESH_DRAWS {
            draw(11, &mut shown)?;
        }
        assert_eq!(slot_contents(&process, 0)?, 11, "back within the refresh");
        Ok(())
    }

    #[test]
    fn a_snapshot_listing_the_local_player_does_not_overwrite_another_slot() -> io::Result<()> {
        let process = game();
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let mut shown = ShownPlayers::default();

        apply_snapshot(
            &mailbox,
            id(0),
            &[(id(0), [9; PLAYER_STATE_SIZE])],
            &mut shown,
        )?;

        for slot in 0..15 {
            assert_eq!(slot_contents(&process, slot)?, 0);
        }
        Ok(())
    }

    #[test]
    fn a_join_that_is_given_up_stops_waiting_on_a_silent_server() -> io::Result<()> {
        // A server that is there and never answers: its socket takes what is sent.
        let silent = std::net::UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let server = silent.local_addr()?;
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        let joined = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                stop.store(true, Ordering::Relaxed);
            });
            Connection::join_unless((server, None), "", 0, Duration::from_secs(30), &stop)
        });
        assert_eq!(
            joined.err().map(|error| error.kind()),
            Some(io::ErrorKind::Interrupted)
        );
        // Within a look or two at the flag, and nowhere near the thirty seconds allowed.
        assert!(started.elapsed() < Duration::from_secs(5));
        Ok(())
    }
}
