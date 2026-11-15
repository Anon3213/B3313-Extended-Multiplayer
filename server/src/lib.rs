//! Session server: admits up to `MAX_PLAYERS` bridges and relays each one's state to the others.
//!
//! `Session` holds all the rules and does no I/O, so tests can drive it directly; `serve` connects
//! it to a socket.

pub mod bans;
mod coins;
pub mod config;
pub mod console;
mod gate;
mod limits;
pub mod movement;
pub mod objects;
pub mod people;
pub mod progress;

use coins::{CoinAllowance, CollectedCoins};
pub use config::Config;
pub use console::Command;
use gate::Gate;
use limits::LogLimit;
use movement::{Movement, MovementReport, Verdict};
use objects::{Objects, Present, Report};
use people::{ChatCommand, People, Spoken};
use progress::{Allowance, Outcome, SessionProgress};

use b3313_protocol::metrics::{LinkMeter, LinkReport, REPORT_INTERVAL, Timings};
use b3313_protocol::{
    Ack, Coin, Hit, HitKind, Hurt, Inbox, Interaction, MAX_COINS_PER_PACKET, MAX_DATAGRAM_SIZE,
    MAX_HITS_PER_PACKET, Message, Name, Outbox, Outfit, Packet, PlayerId, PlayerState,
    PlayerUpdate, Progress, RejectReason, RemoveReason, Role, SESSION_SIZE, Stream, Whereabouts,
    WorldInputs, is_oversized_datagram,
};
use std::fmt;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

/// Real time, not game ticks: the game's rate varies.
pub use b3313_protocol::TICK_INTERVAL;
/// How often `serve` checks its socket between ticks. Not a socket timeout: on Windows those round
/// up to the 15.6 ms system timer, which made ticks late by up to that much.
const POLL_STEP: Duration = Duration::from_millis(1);
/// A player whose last state is older than this has a game that is not running its objects,
/// and is given none to run. A second: thirty states' worth, so no loss on the way looks
/// like a pause.
const RUNNING_WITHIN: Duration = Duration::from_secs(1);
/// Datagrams taken in one turn of the serve loop before it looks whether a tick is due.
/// Sixteen bridges send far fewer in a turn; only a flood reaches it.
const MOST_RECEIVED_PER_TURN: usize = 256;
/// Players in another level or area than the receiver get an update every this many ticks, once
/// a second: enough to know where they are. A game hides them, so it uses only their place, and
/// one who comes into the receiver's area is sent every tick from then on. At three a second
/// these were half of what a player alone in an area received.
pub const FAR_UPDATE_TICKS: u32 = 30;
/// The world and the session's progress go to a player again every this many ticks, once a
/// second, so a lost packet costs little: `SETTLING_RESENDS` times, and then every
/// `QUIET_RESEND_TICKS`.
pub const WORLD_RESEND_TICKS: u32 = 30;
/// How many times something new to a player is sent again a second apart. One of the six sends
/// arrives unless the connection is all but gone.
pub const SETTLING_RESENDS: u8 = 5;
/// How often a player is sent again what it has had `SETTLING_RESENDS` times and has not changed:
/// every five seconds, which also puts it back in a game that was reset.
pub const QUIET_RESEND_TICKS: u32 = 5 * WORLD_RESEND_TICKS;
/// Object news goes to a player when there is some, and otherwise every this many ticks, so a lost
/// packet about who runs what is made good within a third of a second; `Objects::updates_for`
/// takes fewer of these once who runs what has stayed the same for a while.
pub const OBJECT_REFRESH_TICKS: u32 = 10;
/// How near an object a player must be for their Mario's touch on it to make it theirs, in world
/// units. A copy trails its owner by up to a round trip, so this is looser than
/// `max_hit_distance`.
pub const INTERACTION_REACH: f32 = 1000.0;

/// What a line in the chat from the server's owner is marked as, typed at the console or in
/// the panel. Not "Host": that is the player in the session who holds the host's role, who
/// may be someone else, or nobody.
pub const SERVER_NAME: &str = "Server";

#[derive(Debug)]
struct Player {
    address: SocketAddr,
    /// Order of joining; the lowest present is the session owner.
    joined: u64,
    last_heard: Instant,
    /// The newest state and the sequence it was sent under.
    latest: Option<(u32, PlayerState)>,
    /// When a state last came. A bridge sends one in every send slot while its game runs,
    /// and none while it is paused or at a menu: see `RUNNING_WITHIN`.
    state_at: Instant,
    /// Traffic with this player; its losses are of their states on the way to the server.
    meter: LinkMeter,
    /// Which of this player's hits have been taken, so repeats are dropped.
    hits: Inbox,
    /// When this player last landed an accepted hit on each other player.
    last_hit_on: [Option<Instant>; SESSION_SIZE],
    /// Accepted hits on this player, each sent until their bridge acknowledges it.
    hurts: Outbox<Hurt>,
    /// How fast this player moves, reported with their link.
    movement: Movement,
    /// Which of this player's interactions have been taken, so repeats are dropped.
    interactions: Inbox,
    /// How much new progress this player may still add, and the session's progress version this
    /// player was last sent.
    progress_allowance: Allowance,
    progress_sent: u64,
    /// The send slot of the newest report on objects taken from this player.
    objects_reported: Option<u32>,
    /// Keep this player's refused progress and rejected hits from filling the log.
    progress_refusals: LogLimit,
    hit_rejections: LogLimit,
    /// How many of the `SETTLING_RESENDS` of the world, and of that progress version, this
    /// player has yet to be sent.
    world_resends: u8,
    progress_resends: u8,
    /// Which of this player's coins have been taken, so repeats are dropped; how many coins'
    /// worth it may still collect for everyone; and others' coins to send it.
    coins_taken: Inbox,
    coin_allowance: CoinAllowance,
    coins: Outbox<Coin>,
    /// The place whose earlier coins this player has been sent, so they go once on arriving.
    coins_told_of: Option<objects::Place>,
}

impl Player {
    /// Whether this player's game is running its objects at `now`: a state came lately.
    fn is_running(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.state_at) < RUNNING_WITHIN
    }

    /// What to answer a packet of `stream` from this player with: how far its events have been
    /// taken, so their bridge stops sending those again.
    fn ack(&self, stream: Stream) -> Option<Packet> {
        let inbox = match stream {
            Stream::Attacks => self.hits,
            Stream::Interactions => self.interactions,
            Stream::Coins => self.coins_taken,
            // The chat's numbers are kept with the names; see `People::ack`.
            Stream::Chat => return None,
        };
        inbox.ack(stream).map(|ack| Packet::Acks(vec![ack]))
    }

    /// Takes this player's bridge's word on how far it has taken the events sent to it.
    fn acknowledged(&mut self, ack: Ack) {
        match ack.stream {
            Stream::Attacks => self.hurts.acknowledge(ack.id),
            Stream::Coins => self.coins.acknowledge(ack.id),
            // Nothing of these is sent to a player: a claim is answered by who owns what.
            Stream::Interactions | Stream::Chat => {}
        }
    }
}

/// Why the server refused a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitRejection {
    /// The target is the attacker, or nobody is in that slot.
    NoSuchTarget,
    /// One of the two has sent no state yet.
    NotPlaying,
    /// The target's game is paused, at a menu or between two areas: see `RUNNING_WITHIN`.
    Paused,
    /// They are in different levels or areas, or too far apart.
    OutOfReach,
    /// The attacker hit this target less than `Config::min_hit_interval` ago.
    TooSoon,
}

/// The server's ticks over one reporting period.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    /// How long each took to build and send.
    pub took: Timings,
    /// How long after its scheduled time each started.
    pub late: Timings,
    /// Packets dropped by the rate limits.
    pub rate_limited: u64,
}

impl fmt::Display for TickReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let micros = |timings: &Timings| {
            let mean = timings.mean().unwrap_or_default().as_micros();
            format!("{mean} µs on average, {} at most", timings.max.as_micros())
        };
        write!(
            f,
            "{} ticks; each took {} and started {} late; {} packets over the rate limits",
            self.took.count,
            micros(&self.took),
            micros(&self.late),
            self.rate_limited,
        )
    }
}

/// Something worth logging that the session did.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Joined(PlayerId, SocketAddr),
    Left(PlayerId),
    TimedOut(PlayerId),
    WorldSet(PlayerId),
    /// A player's link over the last `REPORT_INTERVAL`. Boxed: it is far larger than the others.
    Link(PlayerId, Box<LinkReport>),
    Ticks(TickReport),
    Hit {
        attacker: PlayerId,
        target: PlayerId,
        kind: HitKind,
    },
    HitRejected {
        attacker: PlayerId,
        target: PlayerId,
        reason: HitRejection,
    },
    /// A player was removed: by the server's owner, the host or a moderator.
    Kicked(PlayerId),
    /// A player's IP address was banned, by any of those.
    Banned(PlayerId, IpAddr),
    /// The server's owner banned an IP address by itself; anyone playing from it was kicked.
    AddressBanned(IpAddr),
    Unbanned(IpAddr),
    /// Everyone in the session, as `list` asked.
    Players(Vec<(PlayerId, SocketAddr)>),
    /// A console command that could not be carried out, and why.
    CommandFailed(String),
    /// A player's movement over the last `REPORT_INTERVAL`.
    Movement(PlayerId, MovementReport),
    /// A player moved unusually; logged at most every two seconds per player.
    Flagged(PlayerId, Verdict),
    /// A player got this much progress, such as stars, which every game now adds to its save.
    ProgressAdded(PlayerId, u32),
    /// A player reported this much new progress at once, more than anyone could have got; the
    /// session did not take it.
    ProgressRefused(PlayerId, u32),
    /// A player reported collecting coins faster than play allows; those did not count for
    /// everyone. Logged at most every two seconds per player.
    CoinsRefused(PlayerId),
    /// A player has the name they are listed under.
    Named(PlayerId, Name),
    /// A player became the host or a moderator, or an ordinary player again.
    RoleChanged(PlayerId, Role),
    /// Something said in the chat, a command included: by a player, with their name then, or
    /// by the server's owner, as `SERVER_NAME`.
    Chat {
        from: Option<PlayerId>,
        name: String,
        text: Message,
    },
    /// A bridge gave a host password that is not the server's; its player is an ordinary one.
    HostPasswordWrong(PlayerId),
    /// The session was ended, by the host in the chat or the server's owner at the console;
    /// the server stops after this.
    Ended,
}

/// What the server's owner reads: one line, or one line a player for `Players`.
impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Joined(player, from) => write!(f, "Player {} joined from {from}", player.get()),
            Self::Left(player) => write!(f, "Player {} left", player.get()),
            Self::TimedOut(player) => write!(f, "Player {} timed out", player.get()),
            Self::WorldSet(player) => {
                write!(f, "The session's world comes from player {}", player.get())
            }
            Self::Link(player, report) => write!(f, "Player {}: {report}", player.get()),
            Self::Ticks(report) => write!(f, "Server: {report}"),
            Self::Hit {
                attacker,
                target,
                kind,
            } => write!(
                f,
                "Player {} hit player {} ({kind:?})",
                attacker.get(),
                target.get()
            ),
            Self::HitRejected {
                attacker,
                target,
                reason,
            } => write!(
                f,
                "Rejected a hit by player {} on player {}: {reason:?}",
                attacker.get(),
                target.get()
            ),
            Self::Kicked(player) => write!(f, "Player {} was removed", player.get()),
            Self::Banned(player, ip) => write!(f, "Player {} ({ip}) was banned", player.get()),
            Self::AddressBanned(ip) => write!(f, "{ip} was banned"),
            Self::Unbanned(ip) => write!(f, "{ip} may join again"),
            Self::Players(players) if players.is_empty() => write!(f, "Nobody is playing"),
            Self::Players(players) => {
                let lines: Vec<String> = players
                    .iter()
                    .map(|(player, address)| format!("Player {} at {address}", player.get()))
                    .collect();
                write!(f, "{}", lines.join("\n"))
            }
            Self::CommandFailed(message) => write!(f, "{message}"),
            Self::Movement(player, report) => write!(f, "Player {} {report}", player.get()),
            Self::Flagged(player, Verdict::Unusual { speed }) => write!(
                f,
                "FLAG: player {} moved at {speed:.0} units/s; `ban {}` removes them",
                player.get(),
                player.get()
            ),
            Self::Flagged(player, Verdict::Refused { distance }) => write!(
                f,
                "FLAG: player {} jumped {distance:.0} units, over their allowance, and was held \
                 back",
                player.get()
            ),
            // The session flags only unusual movement; this is here for completeness.
            Self::Flagged(player, Verdict::Normal) => {
                write!(f, "Player {} moved normally", player.get())
            }
            Self::ProgressAdded(player, count) => write!(
                f,
                "Player {} got {count} new progress (stars, keys, switches or doors); everyone \
                 shares it",
                player.get()
            ),
            Self::CoinsRefused(player) => write!(
                f,
                "FLAG: player {} collected coins faster than play allows; those did not count. \
                 `ban {}` removes them",
                player.get(),
                player.get()
            ),
            Self::ProgressRefused(player, count) => write!(
                f,
                "FLAG: player {} reported {count} new progress at once, more than play allows; \
                 refused. `ban {}` removes them",
                player.get(),
                player.get()
            ),
            Self::Named(player, name) => {
                write!(f, "Player {} is {}", player.get(), name.as_str())
            }
            Self::RoleChanged(player, Role::Host) => {
                write!(f, "Player {} is the host", player.get())
            }
            Self::RoleChanged(player, Role::Moderator) => {
                write!(f, "Player {} is now a moderator", player.get())
            }
            Self::RoleChanged(player, Role::Player) => {
                write!(f, "Player {} is no longer a moderator", player.get())
            }
            Self::Chat { name, text, .. } => write!(f, "<{name}> {}", text.as_str()),
            Self::HostPasswordWrong(player) => write!(
                f,
                "FLAG: player {} gave a wrong host password, and is an ordinary player",
                player.get()
            ),
            Self::Ended => write!(f, "The session was ended"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Session {
    players: [Option<Player>; SESSION_SIZE],
    joins: u64,
    /// Fixed for the session once set, even if its owner leaves.
    world: Option<Box<WorldInputs>>,
    tick: u32,
    events: Vec<Event>,
    ticks: TickReport,
    ticks_since: Option<Instant>,
    config: Config,
    objects: Objects,
    /// The named coins collected where players are, for games that come there later.
    collected: CollectedCoins,
    progress: SessionProgress,
    people: People,
    /// Removals the host or a moderator asked for in the chat, for `serve` to carry out: it
    /// holds the socket and the bans.
    orders: Vec<Command>,
}

impl Session {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether the player at `address` is in the session.
    #[must_use]
    pub fn is_player(&self, address: SocketAddr) -> bool {
        self.find(address).is_some()
    }

    /// Counts packets the gate dropped by its rate limits, for the tick report.
    pub fn record_rate_limited(&mut self, count: u64) {
        self.ticks.rate_limited += count;
    }

    #[must_use]
    pub fn with_config(config: Config) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// Returns what happened since the last call.
    pub fn take_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn find(&self, address: SocketAddr) -> Option<usize> {
        self.players.iter().position(|slot| {
            slot.as_ref()
                .is_some_and(|player| player.address == address)
        })
    }

    fn id(index: usize) -> Option<PlayerId> {
        PlayerId::new(u8::try_from(index).ok()?)
    }

    /// The longest-present player, whose world the session adopts.
    fn owner(&self) -> Option<usize> {
        self.players
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| Some((index, slot.as_ref()?.joined)))
            .min_by_key(|&(_, joined)| joined)
            .map(|(index, _)| index)
    }

    /// Handles one packet, already opened and let through by the gate, and returns the reply, if
    /// any. A `Hello` from a stranger admits it: the gate passes one on only after the handshake
    /// checks out.
    pub fn receive(&mut self, from: SocketAddr, packet: Packet, now: Instant) -> Option<Packet> {
        let known = self.find(from);
        if let Some(player) = known.and_then(|index| self.players.get_mut(index)?.as_mut()) {
            player.last_heard = now;
        }
        match (packet, known) {
            // A repeat means the Welcome was lost, so it is sent again.
            (Packet::Hello { .. }, Some(index)) => {
                Self::id(index).map(|player| Packet::Welcome { player })
            }
            (Packet::Hello { .. }, None) => Some(self.admit(from, now)),
            (Packet::State { sequence, state }, Some(index)) => {
                self.take_state(index, sequence, state, now);
                None
            }
            // It keeps the last state shown, and counts toward the player's loss report.
            (Packet::Alive { sequence }, Some(index)) => {
                let player = self.players.get_mut(index)?.as_mut()?;
                player.meter.arrived(sequence, now);
                // Out of the level: its coins are back in this player's game when they return.
                player.coins_told_of = None;
                None
            }
            // Answered at once, so the time measured is the network's, not the tick's.
            (Packet::Ping { id }, Some(_)) => Some(Packet::Pong { id }),
            (
                Packet::Introduce {
                    name,
                    outfit,
                    host_password,
                },
                Some(index),
            ) => {
                self.introduce(Self::id(index)?, name.as_ref(), outfit, &host_password);
                None
            }
            // Sent again until the roster shows it, so most of these change nothing.
            (Packet::Wear(outfit), Some(index)) => {
                self.people.wear(Self::id(index)?, outfit);
                None
            }
            (
                packet @ (Packet::Say { .. }
                | Packet::Hits(_)
                | Packet::Coins(_)
                | Packet::Interactions(_)
                | Packet::Acks(_)),
                Some(index),
            ) => self.receive_events(index, packet, now),
            (Packet::Progress(report), Some(index)) => {
                self.take_progress(index, &report, now);
                None
            }
            (
                Packet::Objects {
                    level,
                    area,
                    sequence,
                    gone,
                    gone_children,
                    states,
                },
                Some(index),
            ) => {
                // Only about where the player is now, as their latest state says.
                let player = self.players.get_mut(index)?.as_mut()?;
                let (_, state) = player.latest?;
                if place_of(&state) == (level, area) {
                    // A report the network delivered after a later one: what it says is gone
                    // is still gone, but its states are older than those already taken, and
                    // would bring back an enemy removed since.
                    let is_newest = player.objects_reported.is_none_or(|last| {
                        (1..u32::MAX / 2).contains(&sequence.wrapping_sub(last))
                    });
                    if is_newest {
                        player.objects_reported = Some(sequence);
                    }
                    let report = Report {
                        sequence,
                        gone: &gone,
                        gone_children: &gone_children,
                        states: if is_newest { &states } else { &[] },
                    };
                    let from = Self::id(index)?;
                    self.objects.receive(from, (level, area), &report, now);
                }
                None
            }
            (Packet::Bye, Some(index)) => {
                self.remove(index, Event::Left, "left");
                None
            }
            (Packet::OfferWorld(world), Some(index)) => {
                if self.world.is_none() && self.owner() == Some(index) {
                    self.world = Some(world);
                    if let Some(owner) = Self::id(index) {
                        self.events.push(Event::WorldSet(owner));
                    }
                }
                None
            }
            // Server-to-client packets, or state from a stranger.
            _ => None,
        }
    }

    /// Takes a packet of one of the event streams from the player at `index`, and answers with
    /// how far that stream's events have been taken, so their bridge stops sending those again;
    /// or takes their bridge's word on the same for the events sent to it.
    fn receive_events(&mut self, index: usize, packet: Packet, now: Instant) -> Option<Packet> {
        let id = Self::id(index)?;
        let stream = match packet {
            Packet::Say { id: number, text } => {
                self.hear(id, number, &text, now);
                return self.people.ack(id).map(|ack| Packet::Acks(vec![ack]));
            }
            Packet::Hits(hits) => {
                for hit in hits {
                    self.take_hit(index, hit, now);
                }
                Stream::Attacks
            }
            Packet::Coins(coins) => {
                self.take_coins(index, coins, now);
                Stream::Coins
            }
            Packet::Interactions(list) => {
                for interaction in list {
                    self.take_interaction(index, interaction, now);
                }
                Stream::Interactions
            }
            Packet::Acks(acks) => {
                let player = self.players.get_mut(index)?.as_mut()?;
                for ack in acks {
                    player.acknowledged(ack);
                    if ack.stream == Stream::Chat {
                        self.people.acknowledged(id, ack.id);
                    }
                }
                return None;
            }
            _ => return None,
        };
        self.players.get(index)?.as_ref()?.ack(stream)
    }

    fn admit(&mut self, address: SocketAddr, now: Instant) -> Packet {
        let limit = usize::from(self.config.max_players);
        let free = self.players.iter().take(limit).position(Option::is_none);
        let Some((index, player)) = free.and_then(|index| Some((index, Self::id(index)?))) else {
            return Packet::Rejected(RejectReason::Full);
        };
        if let Some(slot) = self.players.get_mut(index) {
            *slot = Some(Player {
                address,
                joined: self.joins,
                last_heard: now,
                latest: None,
                state_at: now,
                meter: LinkMeter::new(now),
                hits: Inbox::default(),
                last_hit_on: [None; SESSION_SIZE],
                hurts: Outbox::default(),
                movement: Movement::default(),
                interactions: Inbox::default(),
                progress_allowance: Allowance::new(now),
                objects_reported: None,
                progress_refusals: LogLimit::default(),
                hit_rejections: LogLimit::default(),
                progress_sent: 0,
                world_resends: SETTLING_RESENDS,
                progress_resends: 0,
                coins_taken: Inbox::default(),
                coin_allowance: CoinAllowance::new(now),
                coins: Outbox::default(),
                coins_told_of: None,
            });
            self.joins += 1;
        }
        self.people.join(player, now);
        self.events.push(Event::Joined(player, address));
        Packet::Welcome { player }
    }

    /// Takes a player's newest state, unless it is late or moves them further than play allows.
    fn take_state(&mut self, index: usize, sequence: u32, state: PlayerState, now: Instant) {
        let Some(player) = self.players.get_mut(index).and_then(Option::as_mut) else {
            return;
        };
        if !player.meter.arrived(sequence, now) {
            return;
        }
        let here = Whereabouts::of(&state);
        let verdict = player.movement.check(sequence, here, now, &self.config);
        // Whatever is made of where it says the player is, the state says their game runs.
        player.state_at = now;
        if !matches!(verdict, Verdict::Refused { .. }) {
            player.latest = Some((sequence, state));
            // On coming to another place, the coins collected there before: worth nothing, so
            // the player's game takes each away uncounted.
            let (level, area) = (here.level, here.area);
            if player.coins_told_of != Some((level, area)) {
                player.coins_told_of = Some((level, area));
                for &name in self.collected.in_area((level, area)) {
                    player.coins.push(|id| Coin {
                        level,
                        area,
                        name,
                        value: 0,
                        id,
                    });
                }
            }
        }
        if verdict != Verdict::Normal
            && player.movement.may_flag(now)
            && let Some(id) = Self::id(index)
        {
            self.events.push(Event::Flagged(id, verdict));
        }
    }

    /// Takes a bridge's word on its player's name, and makes the player the host if the bridge
    /// knows the host password.
    fn introduce(
        &mut self,
        player: PlayerId,
        name: Option<&Name>,
        outfit: Option<Outfit>,
        host_password: &str,
    ) {
        let is_host =
            !self.config.host_password.is_empty() && host_password == self.config.host_password;
        let Some(name) = self.people.introduce(player, name, is_host) else {
            return;
        };
        // Only with a first introduction, like the name: later ones are repeats.
        if let Some(outfit) = outfit {
            self.people.wear(player, outfit);
        }
        self.events.push(Event::Named(player, name));
        if is_host {
            self.events.push(Event::RoleChanged(player, Role::Host));
        } else if !host_password.is_empty() {
            self.events.push(Event::HostPasswordWrong(player));
        }
    }

    /// Takes something a player typed: chat, which goes to everyone, or a command.
    fn hear(&mut self, player: PlayerId, id: u16, said: &Message, now: Instant) {
        let spoken = self.people.say(player, id, said, now);
        if spoken == Spoken::Nothing {
            return;
        }
        self.events.push(Event::Chat {
            from: Some(player),
            name: self.people.name_of(player),
            text: said.clone(),
        });
        if let Spoken::Command(command) = spoken
            && let Err(why) = self.run_chat_command(player, &command)
        {
            self.people.tell(player, &why);
        }
    }

    /// Carries out a command `actor` gave in the chat, if their role allows it.
    fn run_chat_command(&mut self, actor: PlayerId, command: &ChatCommand) -> Result<(), String> {
        let role = self.people.role_of(actor);
        match command {
            ChatCommand::Help => {
                for line in self.people.help_for(actor) {
                    self.people.tell(actor, line);
                }
            }
            ChatCommand::List => {
                for line in self.people.listing() {
                    self.people.tell(actor, &line);
                }
            }
            ChatCommand::Kick(target) | ChatCommand::Ban(target) => {
                if !role.moderates() {
                    return Err("Only the host and moderators can remove a player.".to_owned());
                }
                let target = self.people.find(target)?;
                if target == actor {
                    return Err("You cannot remove yourself.".to_owned());
                }
                if role != Role::Host && self.people.role_of(target).moderates() {
                    return Err("Only the host can remove a moderator.".to_owned());
                }
                self.orders.push(match command {
                    ChatCommand::Ban(_) => Command::Ban(target),
                    _ => Command::Kick(target),
                });
            }
            ChatCommand::Mod(target) | ChatCommand::Unmod(target) => {
                if role != Role::Host {
                    return Err("Only the host can appoint or remove moderators.".to_owned());
                }
                let target = self.people.find(target)?;
                self.set_moderator(target, matches!(command, ChatCommand::Mod(_)))?;
            }
            ChatCommand::Color(wanted) => self.people.choose_outfit(actor, wanted)?,
            ChatCommand::End => {
                if role != Role::Host {
                    return Err("Only the host can end the session.".to_owned());
                }
                self.orders.push(Command::End);
            }
        }
        Ok(())
    }

    /// Makes a player a moderator, or an ordinary player again, and tells everyone.
    ///
    /// # Errors
    /// Fails, with why, if the player is not here, is the host, or already is what was asked.
    pub fn set_moderator(&mut self, player: PlayerId, moderator: bool) -> Result<(), String> {
        self.people.set_moderator(player, moderator)?;
        self.events
            .push(Event::RoleChanged(player, self.people.role_of(player)));
        Ok(())
    }

    /// Says something to everyone from the server's owner, at its console or in the panel.
    pub fn say_as_server(&mut self, text: &Message) {
        self.people
            .announce(&format!("[{SERVER_NAME}] {}", text.as_str()));
        self.events.push(Event::Chat {
            from: None,
            name: SERVER_NAME.to_owned(),
            text: text.clone(),
        });
    }

    /// The removals asked for in the chat since the last call, for `serve` to carry out.
    pub fn take_orders(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.orders)
    }

    /// Everyone listed, with their roles and names.
    #[must_use]
    pub fn roster(&self) -> Vec<b3313_protocol::RosterEntry> {
        self.people.roster()
    }

    /// Everyone in the session.
    #[must_use]
    pub fn players(&self) -> Vec<(PlayerId, SocketAddr)> {
        (0..self.players.len())
            .filter_map(|index| {
                let player = self.players.get(index)?.as_ref()?;
                Some((Self::id(index)?, player.address))
            })
            .collect()
    }

    /// Removes a player, as someone who may asked, returning the address they played from.
    pub fn kick(&mut self, player: PlayerId, reason: RemoveReason) -> Option<SocketAddr> {
        let index = usize::from(player.get());
        let address = self.players.get(index)?.as_ref()?.address;
        let how = match reason {
            // A session that ends or starts again removes nobody by name; whoever is, was.
            RemoveReason::Kicked | RemoveReason::Ended | RemoveReason::Restarting => "was removed",
            RemoveReason::Banned => "was banned",
        };
        self.remove(index, Event::Kicked, how);
        Some(address)
    }

    /// Removes a player, and tells the others `how` they went, as "left".
    fn remove(&mut self, index: usize, event: fn(PlayerId) -> Event, how: &str) {
        if let (Some(slot), Some(id)) = (self.players.get_mut(index), Self::id(index))
            && slot.take().is_some()
        {
            self.objects.forget(id);
            self.people.leave(id, how);
            self.events.push(event(id));
        }
    }

    /// Counts a datagram received from `from`, as it came over the wire, for its player's report.
    pub fn record_received(&mut self, from: SocketAddr, bytes: usize) {
        if let Some(player) = self
            .find(from)
            .and_then(|index| self.players.get_mut(index)?.as_mut())
        {
            player.meter.received(bytes);
        }
    }

    /// Counts a datagram sent to `to`, as it went over the wire, for its player's report.
    pub fn record_sent(&mut self, to: SocketAddr, bytes: usize) {
        if let Some(player) = self
            .find(to)
            .and_then(|index| self.players.get_mut(index)?.as_mut())
        {
            player.meter.sent(bytes);
        }
    }

    /// Counts how late one tick started and how long it took to build and send.
    pub fn record_tick_time(&mut self, late: Duration, took: Duration) {
        self.ticks.late.record(late);
        self.ticks.took.record(took);
    }

    /// Queues the reports that are due as events.
    fn report(&mut self, now: Instant) {
        let since = *self.ticks_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= REPORT_INTERVAL {
            self.events
                .push(Event::Ticks(std::mem::take(&mut self.ticks)));
            self.ticks_since = Some(now);
        }
        for (index, slot) in self.players.iter_mut().enumerate() {
            if let (Some(player), Some(id)) = (slot.as_mut(), Self::id(index))
                && let Some(report) = player.meter.report_if_due(now)
            {
                self.events.push(Event::Link(id, Box::new(report)));
                let movement = player.movement.take_report();
                self.events.push(Event::Movement(id, movement));
            }
        }
    }

    /// Drops silent players and returns a snapshot of the others for each remaining player.
    pub fn tick(&mut self, now: Instant) -> Vec<(SocketAddr, Packet)> {
        let silent: Vec<usize> = (0..self.players.len())
            .filter(|&index| {
                self.players
                    .get(index)
                    .and_then(Option::as_ref)
                    .is_some_and(|player| {
                        now.saturating_duration_since(player.last_heard)
                            > self.config.player_timeout
                    })
            })
            .collect();
        for index in silent {
            self.remove(index, Event::TimedOut, "lost their connection");
        }
        for (player, name) in self.people.name_the_silent(now) {
            self.events.push(Event::Named(player, name));
        }
        self.report(now);

        self.tick = self.tick.wrapping_add(1);
        let states: Vec<PlayerUpdate> = self
            .players
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let (sequence, state) = slot.as_ref()?.latest?;
                Some(PlayerUpdate {
                    player: Self::id(index)?,
                    sequence,
                    state,
                })
            })
            .collect();
        let present: u16 = states_present(&self.players);
        let tick = self.tick;
        let mut outgoing: Vec<(SocketAddr, Packet)> = self
            .players
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let me = Self::id(index)?;
                let player = slot.as_ref()?;
                let here = player.latest.map(|(_, state)| place_of(&state));
                let players = states
                    .iter()
                    .filter(|update| update.player != me)
                    .filter(|update| {
                        here == Some(place_of(&update.state)) || is_far_update_due(update, tick)
                    })
                    .copied()
                    .collect();
                let snapshot = Packet::Snapshot {
                    tick,
                    present: present & !me.bit(),
                    players,
                };
                Some((player.address, snapshot))
            })
            .collect();
        if let Some(world) = &self.world
            && self.tick.is_multiple_of(WORLD_RESEND_TICKS)
        {
            let quiet_resend = self.tick.is_multiple_of(QUIET_RESEND_TICKS);
            for player in self.players.iter_mut().flatten() {
                if take_resend(&mut player.world_resends) || quiet_resend {
                    outgoing.push((player.address, Packet::World(world.clone())));
                }
            }
        }
        self.forget_coins_of_empty_levels();
        for player in self.players.iter_mut().flatten() {
            if let Some(hurts) = Self::next_hurts(player) {
                outgoing.push((player.address, hurts));
            }
            if let Some(coins) = Self::next_coins(player) {
                outgoing.push((player.address, coins));
            }
        }
        outgoing.extend(self.object_updates(now));
        outgoing.extend(self.progress_updates());
        let resend = self.tick.is_multiple_of(WORLD_RESEND_TICKS);
        let quiet_resend = self.tick.is_multiple_of(QUIET_RESEND_TICKS);
        for (index, slot) in self.players.iter().enumerate() {
            if let (Some(player), Some(id)) = (slot, Self::id(index)) {
                let packets = self.people.packets_for(id, resend, quiet_resend);
                outgoing.extend(packets.into_iter().map(|packet| (player.address, packet)));
            }
        }
        outgoing
    }

    fn take_coins(&mut self, from: usize, coins: Vec<Coin>, now: Instant) {
        for coin in coins {
            self.take_coin(from, coin, now);
        }
    }

    /// Takes a coin the player at `from` collected, unless it repeats one already taken, and
    /// passes it to every other player in that level, within the collector's allowance.
    fn take_coin(&mut self, from: usize, coin: Coin, now: Instant) {
        let Some(collector) = self.players.get_mut(from).and_then(Option::as_mut) else {
            return;
        };
        if !collector.coins_taken.take(coin.id) {
            return;
        }
        let Some((_, state)) = collector.latest else {
            return;
        };
        if place_of(&state).0 != coin.level {
            return;
        }
        if !collector.coin_allowance.take(coin.value, now) {
            if collector.coin_allowance.may_log_refusal(now)
                && let Some(id) = Self::id(from)
            {
                self.events.push(Event::CoinsRefused(id));
            }
            return;
        }
        // Kept for whoever comes to the area later, who still has it.
        if coin.name != 0 {
            self.collected.collect((coin.level, coin.area), coin.name);
        }
        for (index, slot) in self.players.iter_mut().enumerate() {
            let Some(player) = slot.as_mut() else {
                continue;
            };
            let in_level = player
                .latest
                .is_some_and(|(_, state)| place_of(&state).0 == coin.level);
            if index != from && in_level {
                player.coins.push(|id| Coin { id, ..coin });
            }
        }
    }

    /// Forgets the coins collected in levels nobody is in: they are back in the game of
    /// whoever goes there next.
    fn forget_coins_of_empty_levels(&mut self) {
        let levels: Vec<i16> = self
            .players
            .iter()
            .filter_map(|slot| Some(place_of(&slot.as_ref()?.latest?.1).0))
            .collect();
        self.collected
            .forget_empty_levels(|level| levels.contains(&level));
    }

    /// The coins to send `player` this tick: those their bridge has not acknowledged.
    fn next_coins(player: &Player) -> Option<Packet> {
        let batch = player.coins.batch(MAX_COINS_PER_PACKET);
        (!batch.is_empty()).then_some(Packet::CoinsCollected(batch))
    }

    /// Takes the player at `index`'s report of what their game got during the session, within
    /// their allowance, and logs what came of it.
    fn take_progress(&mut self, index: usize, report: &Progress, now: Instant) {
        let (Some(player), Some(id)) = (
            self.players.get_mut(index).and_then(Option::as_mut),
            Self::id(index),
        ) else {
            return;
        };
        match self
            .progress
            .receive(report, &mut player.progress_allowance, now)
        {
            Outcome::Added(count) => self.events.push(Event::ProgressAdded(id, count)),
            // Reported again every second until it is taken; said only now and then.
            Outcome::Refused(count) if player.progress_refusals.allow(now) => {
                self.events.push(Event::ProgressRefused(id, count));
            }
            Outcome::Known | Outcome::Refused(_) => {}
        }
    }

    /// The session's progress, to each player who has not had its newest, and to everyone now and
    /// then, as UDP may have lost it. Nothing until there is some.
    fn progress_updates(&mut self) -> Vec<(SocketAddr, Packet)> {
        let version = self.progress.version();
        if version == 0 {
            return Vec::new();
        }
        let resend = self.tick.is_multiple_of(WORLD_RESEND_TICKS);
        let quiet_resend = self.tick.is_multiple_of(QUIET_RESEND_TICKS);
        let progress = *self.progress.progress();
        self.players
            .iter_mut()
            .flatten()
            .filter_map(|player| {
                if player.progress_sent < version {
                    player.progress_sent = version;
                    player.progress_resends = SETTLING_RESENDS;
                } else if !(resend && (take_resend(&mut player.progress_resends) || quiet_resend)) {
                    return None;
                }
                Some((player.address, Packet::SessionProgress(Box::new(progress))))
            })
            .collect()
    }

    /// Decides who runs each synced object, and tells every player in a level what it runs and
    /// what other owners have sent it has not had yet.
    fn object_updates(&mut self, now: Instant) -> Vec<(SocketAddr, Packet)> {
        let present: Vec<(SocketAddr, Present)> = self
            .players
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let player = slot.as_ref()?;
                let (_, state) = player.latest?;
                let present = Present {
                    player: Self::id(index)?,
                    whereabouts: Whereabouts::of(&state),
                    joined: player.joined,
                    running: player.is_running(now),
                };
                Some((player.address, present))
            })
            .collect();
        let players: Vec<Present> = present.iter().map(|&(_, present)| present).collect();
        self.objects.assign(&players, now);
        let tick = self.tick;
        present
            .iter()
            .filter_map(|(address, present)| {
                let place = (present.whereabouts.level, present.whereabouts.area);
                // Staggered by player, like far updates, so refreshes spread over the ticks.
                let refresh = tick
                    .wrapping_add(u32::from(present.player.get()))
                    .is_multiple_of(OBJECT_REFRESH_TICKS);
                let updates = self.objects.updates_for(present.player, place, refresh)?;
                Some((*address, updates))
            })
            .collect()
    }

    /// Takes one hit from the player at `attacker`, unless it repeats one already taken. If it
    /// passes the checks, it is queued for its target.
    fn take_hit(&mut self, attacker: usize, hit: Hit, now: Instant) {
        let Some(player) = self.players.get_mut(attacker).and_then(Option::as_mut) else {
            return;
        };
        if !player.hits.take(hit.id) {
            return;
        }
        let Some(attacker_id) = Self::id(attacker) else {
            return;
        };
        let target = usize::from(hit.target.get());
        let outcome = self.judge_hit(attacker, target, now);
        if let Err(reason) = outcome {
            let may_log = self
                .players
                .get_mut(attacker)
                .and_then(Option::as_mut)
                .is_some_and(|player| player.hit_rejections.allow(now));
            if may_log {
                self.events.push(Event::HitRejected {
                    attacker: attacker_id,
                    target: hit.target,
                    reason,
                });
            }
            return;
        }
        if let Some(player) = self.players.get_mut(attacker).and_then(Option::as_mut)
            && let Some(last) = player.last_hit_on.get_mut(target)
        {
            *last = Some(now);
        }
        if let Some(victim) = self.players.get_mut(target).and_then(Option::as_mut) {
            victim.hurts.push(|id| Hurt {
                attacker: attacker_id,
                kind: hit.kind,
                id,
            });
        }
        self.events.push(Event::Hit {
            attacker: attacker_id,
            target: hit.target,
            kind: hit.kind,
        });
    }

    /// Checks a hit against what the server knows: both players present and playing, in the same
    /// area and near each other, and not hitting faster than any attack can.
    fn judge_hit(&self, attacker: usize, target: usize, now: Instant) -> Result<(), HitRejection> {
        let player = |index: usize| self.players.get(index).and_then(Option::as_ref);
        let (Some(from), Some(to)) = (player(attacker), player(target)) else {
            return Err(HitRejection::NoSuchTarget);
        };
        if attacker == target {
            return Err(HitRejection::NoSuchTarget);
        }
        let (Some((_, from_state)), Some((_, to_state))) = (from.latest, to.latest) else {
            return Err(HitRejection::NotPlaying);
        };
        // A paused game takes no hit until it goes on, when one made long before would land
        // on a player who never saw it coming. Their Mario still stands there to be touched.
        if !to.is_running(now) {
            return Err(HitRejection::Paused);
        }
        let distance = Whereabouts::of(&from_state).distance_to(&Whereabouts::of(&to_state));
        // NaN, from a malformed position, fails this too.
        if !distance.is_some_and(|distance| distance <= self.config.max_hit_distance) {
            return Err(HitRejection::OutOfReach);
        }
        let last = from.last_hit_on.get(target).copied().flatten();
        let interval = self.config.min_hit_interval;
        if last.is_some_and(|last| now.saturating_duration_since(last) < interval) {
            return Err(HitRejection::TooSoon);
        }
        Ok(())
    }

    /// Takes one interaction from the player at `from`, unless it repeats one already taken: their
    /// Mario attacked or picked up a copy of an object another game runs, so their game has
    /// claimed it. The claim is granted if the sender is in that area and near the object as its
    /// owner last reported it; a tampered client can still take only objects near its player.
    fn take_interaction(&mut self, from: usize, interaction: Interaction, now: Instant) {
        let Some(sender) = self.players.get_mut(from).and_then(Option::as_mut) else {
            return;
        };
        if !sender.interactions.take(interaction.id) {
            return;
        }
        let Some((_, state)) = sender.latest else {
            return;
        };
        let here = Whereabouts::of(&state);
        let place = (interaction.level, interaction.area);
        if (here.level, here.area) != place {
            return;
        }
        let Some((owner, position)) = self.objects.owner_and_position(place, interaction.object)
        else {
            return;
        };
        let object = Whereabouts { position, ..here };
        let near = here
            .distance_to(&object)
            .is_some_and(|distance| distance <= INTERACTION_REACH);
        if let (true, Some(claimer)) = (near, Self::id(from))
            && claimer != owner
        {
            self.objects.claim(place, interaction.object, claimer, now);
        }
    }

    /// The hurts to send `player` this tick: those their bridge has not acknowledged.
    fn next_hurts(player: &Player) -> Option<Packet> {
        let batch = player.hurts.batch(MAX_HITS_PER_PACKET);
        (!batch.is_empty()).then_some(Packet::Hurts(batch))
    }
}

/// The bit of every player in the session.
fn states_present(players: &[Option<Player>]) -> u16 {
    players
        .iter()
        .enumerate()
        .filter(|(_, slot)| slot.is_some())
        .filter_map(|(index, _)| Session::id(index))
        .fold(0, |mask, id| mask | id.bit())
}

/// A state's level and area, which decide whether its player is near the receiver.
fn place_of(state: &PlayerState) -> (i16, u8) {
    let whereabouts = Whereabouts::of(state);
    (whereabouts.level, whereabouts.area)
}

/// Takes one of a player's settling resends, if any are left.
const fn take_resend(left: &mut u8) -> bool {
    if *left == 0 {
        return false;
    }
    *left -= 1;
    true
}

/// Whether a player elsewhere is due an update this tick. Each player's turn is staggered by their
/// number, so far updates spread over the ticks instead of all landing on one.
fn is_far_update_due(update: &PlayerUpdate, tick: u32) -> bool {
    tick.wrapping_add(u32::from(update.player.get()))
        .is_multiple_of(FAR_UPDATE_TICKS)
}

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
            match remove_player(socket, session, gate, player, RemoveReason::Banned) {
                Some(address) => {
                    gate.ban(address.ip());
                    Some(Event::Banned(player, address.ip()))
                }
                None => Some(no_player(player)),
            }
        }
        Command::BanAddress(ip) => {
            let players = session.players();
            let playing = players.iter().filter(|(_, address)| address.ip() == ip);
            for &(player, _) in playing {
                remove_player(socket, session, gate, player, RemoveReason::Banned);
            }
            gate.ban(ip);
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
        thread::sleep(
            next_tick
                .saturating_duration_since(Instant::now())
                .min(POLL_STEP),
        );
    }
    for (_, address) in session.players() {
        notify_removed(socket, &mut gate, address, parting);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        Command, Config, Event, FAR_UPDATE_TICKS, HitRejection, INTERACTION_REACH, Session,
        TICK_INTERVAL, Verdict, WORLD_RESEND_TICKS,
    };
    use crate::limits::LOG_INTERVAL;
    use b3313_protocol::metrics::REPORT_INTERVAL;
    use b3313_protocol::{
        Ack, Coin, Hit, HitKind, Interaction, Message, Name, OBJECT_STATE_SIZE, ObjectSet, Outfit,
        PLAYER_STATE_SIZE, PROGRESS_SIZE, Packet, PlayerId, PlayerState, PlayerUpdate, Progress,
        RejectReason, RemoveReason, Role, Stream, WORLD_INPUTS_SIZE,
    };
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::{Duration, Instant};

    fn offer(marker: u8) -> Packet {
        Packet::OfferWorld(Box::new([marker; WORLD_INPUTS_SIZE]))
    }

    /// Ticks until the world goes out, and returns who received which world marker.
    fn worlds_sent(session: &mut Session, now: Instant) -> Vec<(SocketAddr, u8)> {
        (0..WORLD_RESEND_TICKS)
            .flat_map(|_| session.tick(now))
            .filter_map(|(address, packet)| match packet {
                Packet::World(world) => Some((address, world.first().copied().unwrap_or(0))),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_owners_world_reaches_everyone_and_others_are_ignored() {
        let mut session = Session::new();
        let now = Instant::now();
        for port in 0..3 {
            send(&mut session, client(port), &hello(), now);
        }
        send(&mut session, client(1), &offer(1), now);
        assert_eq!(worlds_sent(&mut session, now), vec![]);

        send(&mut session, client(0), &offer(9), now);
        send(&mut session, client(0), &offer(7), now);

        assert_eq!(
            worlds_sent(&mut session, now),
            vec![(client(0), 9), (client(1), 9), (client(2), 9)]
        );
        assert!(session.take_events().contains(&Event::WorldSet(id(0))));
    }

    #[test]
    fn the_world_and_progress_are_resent_each_second_at_first_and_then_every_five() {
        let mut session = Session::new();
        let now = Instant::now();
        send(&mut session, client(0), &hello(), now);
        send(&mut session, client(0), &offer(9), now);
        let mut star = [0; PROGRESS_SIZE];
        star[4] = 1;
        let report = Packet::Progress(Box::new(Progress::from_bytes(star)));
        send(&mut session, client(0), &report, now);

        let twenty_seconds = 20 * WORLD_RESEND_TICKS;
        let sent: Vec<Packet> = (0..twenty_seconds)
            .flat_map(|_| session.tick(now))
            .map(|(_, packet)| packet)
            .collect();
        let worlds = sent
            .iter()
            .filter(|packet| matches!(packet, Packet::World(_)));
        let progress = sent
            .iter()
            .filter(|packet| matches!(packet, Packet::SessionProgress(_)));
        // Seconds 1 to 5, then 10, 15 and 20; the progress also at once, when it was new.
        assert_eq!(worlds.count(), 8);
        assert_eq!(progress.count(), 9);
    }

    #[test]
    fn the_world_outlives_its_owner_and_ownership_passes_on() {
        let mut session = Session::new();
        let now = Instant::now();
        send(&mut session, client(0), &hello(), now);
        send(&mut session, client(1), &hello(), now);
        send(&mut session, client(0), &Packet::Bye, now);

        // Player 1 is now the longest present, so its offer counts.
        send(&mut session, client(1), &offer(4), now);
        send(&mut session, client(1), &Packet::Bye, now);
        send(&mut session, client(2), &hello(), now);

        assert_eq!(worlds_sent(&mut session, now), vec![(client(2), 4)]);
    }

    fn client(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    /// A state in area 1 of level 16, filled with `marker` after that.
    fn state(marker: u8) -> PlayerState {
        let mut state = [marker; PLAYER_STATE_SIZE];
        state[..4].copy_from_slice(&[1, 1, 0, 16]);
        state
    }

    fn update(player: u8, sequence: u32, marker: u8) -> PlayerUpdate {
        PlayerUpdate {
            player: id(player),
            sequence,
            state: state(marker),
        }
    }

    /// Passes a packet to the session, as the gate does once it has let it through.
    fn send(
        session: &mut Session,
        from: SocketAddr,
        packet: &Packet,
        now: Instant,
    ) -> Option<Packet> {
        session.receive(from, packet.clone(), now)
    }

    /// What the gate passes on once a stranger's handshake checks out.
    fn hello() -> Packet {
        Packet::Hello {
            cookie: 0,
            handshake: vec![],
        }
    }

    fn snapshot_for(session: &mut Session, who: SocketAddr, now: Instant) -> Option<Packet> {
        session
            .tick(now)
            .into_iter()
            .find(|(address, _)| *address == who)
            .map(|(_, packet)| packet)
    }

    #[test]
    fn sixteen_players_join_and_the_seventeenth_is_turned_away() {
        let mut session = Session::new();
        let now = Instant::now();
        for port in 0..16 {
            assert_eq!(
                send(&mut session, client(port), &hello(), now),
                Some(Packet::Welcome {
                    player: id(u8::try_from(port).unwrap_or(0))
                })
            );
        }
        assert_eq!(
            send(&mut session, client(99), &hello(), now),
            Some(Packet::Rejected(RejectReason::Full))
        );
    }

    #[test]
    fn a_smaller_player_limit_from_the_config_turns_players_away_sooner() {
        let config = Config {
            max_players: 2,
            ..Config::default()
        };
        let mut session = Session::with_config(config);
        let now = Instant::now();
        send(&mut session, client(0), &hello(), now);
        send(&mut session, client(1), &hello(), now);
        assert_eq!(
            send(&mut session, client(2), &hello(), now),
            Some(Packet::Rejected(RejectReason::Full))
        );
    }

    #[test]
    fn the_tick_report_counts_what_the_gate_dropped() {
        let mut session = Session::new();
        let start = Instant::now();
        session.tick(start);
        session.record_rate_limited(150);
        session.record_rate_limited(2);
        session.tick(start + REPORT_INTERVAL);
        let events = session.take_events();
        let dropped = events.into_iter().find_map(|event| match event {
            Event::Ticks(report) => Some(report.rate_limited),
            _ => None,
        });
        assert_eq!(dropped, Some(152));
    }

    #[test]
    fn a_repeated_hello_keeps_the_same_player() {
        let mut session = Session::new();
        let now = Instant::now();
        let first = send(&mut session, client(1), &hello(), now);
        assert_eq!(send(&mut session, client(1), &hello(), now), first);
        assert_eq!(session.take_events().len(), 1);
    }

    #[test]
    fn each_player_receives_everyone_else_but_not_themself() {
        let mut session = Session::new();
        let now = Instant::now();
        for port in 0..3 {
            send(&mut session, client(port), &hello(), now);
            let marker = u8::try_from(port).unwrap_or(0) + 10;
            send(
                &mut session,
                client(port),
                &Packet::State {
                    sequence: 1,
                    state: state(marker),
                },
                now,
            );
        }

        assert_eq!(
            snapshot_for(&mut session, client(1), now),
            Some(Packet::Snapshot {
                tick: 1,
                present: 0b101,
                players: vec![update(0, 1, 10), update(2, 1, 12)]
            })
        );
    }

    #[test]
    fn players_elsewhere_are_sent_now_and_then_but_always_listed_as_present() {
        let now = Instant::now();
        let mut session = Session::new();
        // Player 0 in level 16, player 1 beside it, player 2 in level 17.
        for (port, level) in [(0, 16), (1, 16), (2, 17)] {
            send(&mut session, client(port), &hello(), now);
            let mut here = state(port.try_into().unwrap_or(0));
            here[3] = level;
            let packet = Packet::State {
                sequence: 1,
                state: here,
            };
            send(&mut session, client(port), &packet, now);
        }

        let mut near = 0;
        let mut far = 0;
        for _ in 0..FAR_UPDATE_TICKS * 3 {
            let snapshot = snapshot_for(&mut session, client(0), now);
            let Some(Packet::Snapshot {
                present, players, ..
            }) = snapshot
            else {
                continue;
            };
            assert_eq!(present, 0b110);
            near += players.iter().filter(|u| u.player == id(1)).count();
            far += players.iter().filter(|u| u.player == id(2)).count();
        }
        // The player beside it every tick, the one elsewhere once in every `FAR_UPDATE_TICKS`.
        let ticks = usize::try_from(FAR_UPDATE_TICKS * 3).unwrap_or(0);
        assert_eq!((near, far), (ticks, 3));
    }

    #[test]
    fn late_states_do_not_replace_newer_ones() {
        let mut session = Session::new();
        let now = Instant::now();
        send(&mut session, client(0), &hello(), now);
        send(&mut session, client(1), &hello(), now);
        let watching = Packet::State {
            sequence: 1,
            state: state(0),
        };
        send(&mut session, client(0), &watching, now);
        send(
            &mut session,
            client(1),
            &Packet::State {
                sequence: 5,
                state: state(5),
            },
            now,
        );
        send(
            &mut session,
            client(1),
            &Packet::State {
                sequence: 4,
                state: state(4),
            },
            now,
        );

        assert_eq!(
            snapshot_for(&mut session, client(0), now),
            Some(Packet::Snapshot {
                tick: 1,
                present: 0b10,
                players: vec![update(1, 5, 5)]
            })
        );
    }

    #[test]
    fn alive_keeps_the_last_state_and_counts_as_sent() {
        let mut session = Session::new();
        let start = Instant::now();
        send(&mut session, client(0), &hello(), start);
        send(&mut session, client(1), &hello(), start);
        // Player 0 is playing too, beside player 1, so it gets player 1's every state.
        let watching = Packet::State {
            sequence: 1,
            state: state(0),
        };
        send(&mut session, client(0), &watching, start);
        let first = Packet::State {
            sequence: 1,
            state: state(1),
        };
        let between = Packet::Alive { sequence: 2 };
        send(&mut session, client(1), &first, start);
        send(&mut session, client(1), &between, start);
        let third = Packet::State {
            sequence: 3,
            state: state(3),
        };
        send(&mut session, client(1), &third, start);

        let snapshot = snapshot_for(&mut session, client(0), start);
        assert_eq!(
            snapshot,
            Some(Packet::Snapshot {
                tick: 1,
                present: 0b10,
                players: vec![update(1, 3, 3)]
            })
        );
        session.take_events();
        let end = start + REPORT_INTERVAL;
        send(&mut session, client(1), &Packet::Alive { sequence: 4 }, end);
        session.tick(end);
        let missing = session.take_events().iter().find_map(|event| match event {
            Event::Link(player, report) if *player == id(1) => Some(report.losses.missing),
            _ => None,
        });
        assert_eq!(missing, Some(0));
    }

    #[test]
    fn strangers_cannot_post_state_or_get_snapshots() {
        let mut session = Session::new();
        let now = Instant::now();
        send(&mut session, client(0), &hello(), now);
        send(
            &mut session,
            client(7),
            &Packet::State {
                sequence: 1,
                state: state(7),
            },
            now,
        );

        assert_eq!(
            session.tick(now),
            vec![(
                client(0),
                Packet::Snapshot {
                    tick: 1,
                    present: 0,
                    players: vec![]
                }
            )]
        );
    }

    #[test]
    fn bye_and_silence_both_remove_a_player_and_free_their_number() {
        let mut session = Session::new();
        let start = Instant::now();
        send(&mut session, client(0), &hello(), start);
        send(&mut session, client(1), &hello(), start);
        send(&mut session, client(0), &Packet::Bye, start);

        // Player 0 left; player 1 stays quiet past the timeout.
        let later = start + Config::default().player_timeout + Duration::from_millis(1);
        assert_eq!(session.tick(later), vec![]);
        assert_eq!(
            session.take_events(),
            vec![
                Event::Joined(id(0), client(0)),
                Event::Joined(id(1), client(1)),
                Event::Left(id(0)),
                Event::TimedOut(id(1)),
            ]
        );
        assert_eq!(
            send(&mut session, client(5), &hello(), later),
            Some(Packet::Welcome { player: id(0) })
        );
    }

    #[test]
    fn players_get_pongs_and_strangers_do_not() {
        let mut session = Session::new();
        let now = Instant::now();
        send(&mut session, client(0), &hello(), now);

        let ping = Packet::Ping { id: 7 };
        assert_eq!(
            send(&mut session, client(0), &ping, now),
            Some(Packet::Pong { id: 7 })
        );
        assert_eq!(send(&mut session, client(1), &ping, now), None);
    }

    #[test]
    fn each_link_and_the_ticks_are_reported_once_per_interval() {
        let mut session = Session::new();
        let start = Instant::now();
        send(&mut session, client(0), &hello(), start);
        for sequence in [1, 2, 4] {
            let packet = Packet::State {
                sequence,
                state: state(1),
            };
            send(&mut session, client(0), &packet, start);
        }
        session.record_sent(client(0), 100);
        session.record_sent(client(9), 100);
        // The gate counts datagrams as they came over the wire.
        for _ in 0..4 {
            session.record_received(client(0), 60);
        }
        session.record_received(client(9), 60);
        session.tick(start);
        session.record_tick_time(Duration::from_millis(1), Duration::from_millis(5));
        session.take_events();

        let end = start + REPORT_INTERVAL;
        send(&mut session, client(0), &Packet::Alive { sequence: 5 }, end);
        session.tick(end);
        let events = session.take_events();

        let link = events.iter().find_map(|event| match event {
            Event::Link(player, report) if *player == id(0) => Some(**report),
            _ => None,
        });
        // Only player 0's datagrams count; a stranger's are nobody's.
        let counts = link.map(|r| (r.received.bytes, r.losses.missing, r.sent.bytes));
        assert_eq!(counts, Some((240, 1, 100)));
        let ticks = events.iter().find_map(|event| match event {
            Event::Ticks(report) => Some(*report),
            _ => None,
        });
        assert_eq!(
            ticks.map(|t| (t.took.count, t.took.max, t.late.max)),
            Some((1, Duration::from_millis(5), Duration::from_millis(1)))
        );
        session.tick(end + TICK_INTERVAL);
        assert_eq!(session.take_events(), []);
    }

    /// A state in area 1 of `level`, at `x` along one axis.
    fn standing(level: u8, x: f32) -> PlayerState {
        let mut state = [0; PLAYER_STATE_SIZE];
        state[..4].copy_from_slice(&[1, 1, 0, level]);
        state[4..8].copy_from_slice(&x.to_be_bytes());
        state
    }

    /// Players 0 and 1, both playing in level 16, `apart` units from each other.
    fn two_players(apart: f32, now: Instant) -> Session {
        let mut session = Session::new();
        for (port, x) in [(0, 0.0), (1, apart)] {
            send(&mut session, client(port), &hello(), now);
            let state = Packet::State {
                sequence: 1,
                state: standing(16, x),
            };
            send(&mut session, client(port), &state, now);
        }
        session
    }

    fn punch(target: u8, id: u16) -> Hit {
        Hit {
            target: self::id(target),
            kind: HitKind::Punch,
            id,
        }
    }

    /// The hurts each tick sends to `who`, over `ticks` ticks.
    fn hurts_for(session: &mut Session, who: SocketAddr, ticks: usize, now: Instant) -> Vec<u16> {
        (0..ticks)
            .flat_map(|_| session.tick(now))
            .filter(|(address, _)| *address == who)
            .filter_map(|(_, packet)| match packet {
                Packet::Hurts(hurts) => Some(hurts),
                _ => None,
            })
            .flatten()
            .map(|hurt| hurt.id)
            .collect()
    }

    /// Has the bridge at `port` acknowledge the events of `stream` up to number `id`.
    fn acknowledge(session: &mut Session, port: u16, stream: Stream, id: u16, now: Instant) {
        let acks = Packet::Acks(vec![Ack { stream, id }]);
        send(session, client(port), &acks, now);
    }

    #[test]
    fn an_accepted_hit_goes_to_its_target_until_acknowledged_and_repeats_count_once() {
        let now = Instant::now();
        let mut session = two_players(100.0, now);
        session.take_events();
        let hits = Packet::Hits(vec![punch(1, 7)]);
        // The attacker's bridge is told the hit arrived, so it stops repeating it.
        let taken = Packet::Acks(vec![Ack {
            stream: Stream::Attacks,
            id: 7,
        }]);
        assert_eq!(
            send(&mut session, client(0), &hits, now),
            Some(taken.clone())
        );
        // The same hit again, as the attacker's bridge repeats it until it hears that.
        assert_eq!(send(&mut session, client(0), &hits, now), Some(taken));

        // Every tick, until the target's bridge says it has it.
        assert_eq!(hurts_for(&mut session, client(1), 5, now), [0; 5]);
        acknowledge(&mut session, 1, Stream::Attacks, 0, now);
        assert_eq!(hurts_for(&mut session, client(1), 5, now), []);
        assert_eq!(hurts_for(&mut session, client(0), 5, now), []);
        assert_eq!(
            session.take_events(),
            [Event::Hit {
                attacker: id(0),
                target: id(1),
                kind: HitKind::Punch
            }]
        );
    }

    fn punch_from_0(session: &mut Session, target: u8, id: u16, now: Instant) {
        let hits = Packet::Hits(vec![punch(target, id)]);
        send(session, client(0), &hits, now);
    }

    fn rejection(session: &mut Session) -> Option<HitRejection> {
        let events = session.take_events();
        events.into_iter().find_map(|event| match event {
            Event::HitRejected { reason, .. } => Some(reason),
            _ => None,
        })
    }

    #[test]
    fn hits_out_of_reach_or_on_nobody_are_rejected() {
        let now = Instant::now();
        let mut far = two_players(Config::default().max_hit_distance + 1.0, now);
        far.take_events();
        punch_from_0(&mut far, 1, 1, now);
        assert_eq!(rejection(&mut far), Some(HitRejection::OutOfReach));

        let mut session = two_players(100.0, now);
        let elsewhere = Packet::State {
            sequence: 2,
            state: standing(17, 100.0),
        };
        send(&mut session, client(1), &elsewhere, now);
        session.take_events();
        punch_from_0(&mut session, 1, 1, now);
        assert_eq!(rejection(&mut session), Some(HitRejection::OutOfReach));

        // Each after the log has room for another line about this player.
        let mut later = now;
        for (target, id) in [(0, 2), (5, 3)] {
            later += LOG_INTERVAL;
            punch_from_0(&mut session, target, id, later);
            assert_eq!(rejection(&mut session), Some(HitRejection::NoSuchTarget));
        }

        let broken = Packet::State {
            sequence: 3,
            state: standing(16, f32::NAN),
        };
        later += LOG_INTERVAL;
        send(&mut session, client(1), &broken, later);
        punch_from_0(&mut session, 1, 4, later);
        assert_eq!(rejection(&mut session), Some(HitRejection::OutOfReach));
        assert_eq!(hurts_for(&mut session, client(1), 5, later), []);
    }

    #[test]
    fn rejected_hits_and_refused_progress_do_not_fill_the_log() {
        let now = Instant::now();
        let mut session = two_players(Config::default().max_hit_distance + 1.0, now);
        session.take_events();

        for id in 1..=20 {
            punch_from_0(&mut session, 1, id, now);
        }
        // More than the allowance holds, reported again and again as a bridge does.
        let everything = Packet::Progress(Box::new(Progress::from_bytes([0xFF; PROGRESS_SIZE])));
        for _ in 0..20 {
            send(&mut session, client(0), &everything, now);
        }

        let events = session.take_events();
        let hits = events
            .iter()
            .filter(|event| matches!(event, Event::HitRejected { .. }));
        let refusals = events
            .iter()
            .filter(|event| matches!(event, Event::ProgressRefused(..)));
        assert_eq!((hits.count(), refusals.count()), (1, 1));
    }

    #[test]
    fn a_player_whose_game_is_paused_cannot_be_hit_until_it_goes_on() {
        let now = Instant::now();
        let mut session = two_players(100.0, now);
        session.take_events();

        // Player 1's bridge sends no state while their game is paused; player 0's goes on.
        let paused = now + super::RUNNING_WITHIN;
        let still_here = Packet::State {
            sequence: 2,
            state: standing(16, 0.0),
        };
        send(&mut session, client(0), &still_here, paused);
        punch_from_0(&mut session, 1, 1, paused);
        assert_eq!(rejection(&mut session), Some(HitRejection::Paused));
        assert_eq!(hurts_for(&mut session, client(1), 5, paused), []);

        // On again, they can be hit as before.
        let resumed = Packet::State {
            sequence: 2,
            state: standing(16, 100.0),
        };
        send(&mut session, client(1), &resumed, paused);
        punch_from_0(&mut session, 1, 2, paused);
        assert_eq!(rejection(&mut session), None);
    }

    #[test]
    fn a_player_who_has_not_played_cannot_hit_or_be_hit() {
        let now = Instant::now();
        let mut session = Session::new();
        send(&mut session, client(0), &hello(), now);
        send(&mut session, client(1), &hello(), now);
        session.take_events();
        punch_from_0(&mut session, 1, 1, now);
        assert_eq!(rejection(&mut session), Some(HitRejection::NotPlaying));
    }

    #[test]
    fn a_jump_over_the_allowance_is_held_back_and_flagged() {
        let now = Instant::now();
        let config = Config {
            jumps_per_minute: 1,
            ..Config::default()
        };
        let mut session = Session::with_config(config);
        for port in [0, 1] {
            send(&mut session, client(port), &hello(), now);
        }
        move_to(&mut session, 0, 1, 0.0, now);
        move_to(&mut session, 1, 1, 0.0, now);
        move_to(&mut session, 1, 2, 5000.0, now); // Allowed: the first jump this minute.
        session.take_events();

        move_to(&mut session, 1, 3, 0.0, now); // Over the allowance.
        let flagged = session.take_events().into_iter().any(|event| {
            matches!(event, Event::Flagged(player, Verdict::Refused { .. }) if player == id(1))
        });
        assert!(flagged, "the refused jump should be flagged");
        // Player 0 still sees player 1 where the last allowed state put them.
        assert_eq!(sequence_seen(&mut session, now), Some(2));

        move_to(&mut session, 1, 4, 5000.0 + 33.0, now); // Running on from there: passed on.
        assert_eq!(sequence_seen(&mut session, now), Some(4));
    }

    fn move_to(session: &mut Session, port: u16, sequence: u32, x: f32, now: Instant) {
        let state = Packet::State {
            sequence,
            state: standing(16, x),
        };
        send(session, client(port), &state, now);
    }

    /// The sequence of player 1's state in player 0's next snapshot.
    fn sequence_seen(session: &mut Session, now: Instant) -> Option<u32> {
        match snapshot_for(session, client(0), now)? {
            Packet::Snapshot { players, .. } => players.first().map(|update| update.sequence),
            _ => None,
        }
    }

    #[test]
    fn hits_faster_than_any_attack_are_rejected() {
        let start = Instant::now();
        let mut session = two_players(100.0, start);
        punch_from_0(&mut session, 1, 1, start);
        session.take_events();

        let interval = Config::default().min_hit_interval;
        let soon = start + interval / 2;
        punch_from_0(&mut session, 1, 2, soon);
        assert_eq!(rejection(&mut session), Some(HitRejection::TooSoon));

        let later = start + interval;
        punch_from_0(&mut session, 1, 3, later);
        assert_eq!(rejection(&mut session), None);
        // Both accepted hits wait for the next tick, and go out together.
        assert_eq!(hurts_for(&mut session, client(1), 1, later), [0, 1]);
    }

    #[test]
    fn object_states_pass_from_their_owner_to_the_others_in_its_area() {
        let mut session = Session::new();
        let now = Instant::now();
        for port in 0..2 {
            send(&mut session, client(port), &hello(), now);
            let playing = Packet::State {
                sequence: 1,
                state: state(0),
            };
            send(&mut session, client(port), &playing, now);
        }
        // Nobody has reported positions yet, so player 0, there first, owns every object.
        session.tick(now);
        let mut object = [0; OBJECT_STATE_SIZE];
        object[1] = 5;
        let report = Packet::Objects {
            level: 16,
            area: 1,
            sequence: 1,
            gone: ObjectSet::default(),
            gone_children: vec![],
            states: vec![object],
        };
        send(&mut session, client(1), &report, now);
        // Who received how many object states in one tick.
        let states_sent = |session: &mut Session| -> Vec<(SocketAddr, usize)> {
            let outgoing = session.tick(now).into_iter();
            outgoing
                .filter_map(|(address, packet)| match packet {
                    Packet::ObjectUpdates { states, .. } if !states.is_empty() => {
                        Some((address, states.len()))
                    }
                    _ => None,
                })
                .collect()
        };
        assert_eq!(states_sent(&mut session), []);

        send(&mut session, client(0), &report, now);
        assert_eq!(states_sent(&mut session), [(client(1), 1)]);

        // An earlier report that the network delivers late is not news.
        let late = Packet::Objects {
            level: 16,
            area: 1,
            sequence: 0,
            gone: ObjectSet::default(),
            gone_children: vec![],
            states: vec![object],
        };
        send(&mut session, client(0), &late, now);
        assert_eq!(states_sent(&mut session), []);
    }

    #[test]
    fn a_touch_makes_a_nearby_player_the_objects_owner() {
        let mut session = Session::new();
        let now = Instant::now();
        for port in 0..2 {
            send(&mut session, client(port), &hello(), now);
            let playing = Packet::State {
                sequence: 1,
                state: state(0),
            };
            send(&mut session, client(port), &playing, now);
        }
        session.tick(now);
        // Player 0 owns object 5 and reports it at the origin, where both players stand.
        let mut object = [0; OBJECT_STATE_SIZE];
        object[1] = 5;
        let report = Packet::Objects {
            level: 16,
            area: 1,
            sequence: 1,
            gone: ObjectSet::default(),
            gone_children: vec![],
            states: vec![object],
        };
        send(&mut session, client(0), &report, now);
        session.tick(now);
        let touch = |object: u16, id: u16| Interaction {
            level: 16,
            area: 1,
            object,
            status: 0xC001,
            id,
        };
        // Which player the server's next word on ownership gives `object` to.
        let owner_of = |session: &mut Session, object: u16| -> Option<SocketAddr> {
            let outgoing = session.tick(now).into_iter();
            let mut owners = outgoing.filter_map(|(address, packet)| match packet {
                Packet::ObjectUpdates { owned, .. } if owned.contains(object) => Some(address),
                _ => None,
            });
            owners.next()
        };
        // A touch on an object nobody has reported changes nothing: player 0 runs it still.
        let unreported = Packet::Interactions(vec![touch(6, 0)]);
        send(&mut session, client(1), &unreported, now);
        // Every packet is acknowledged, so the bridge stops repeating it.
        let taken = |id: u16| {
            Some(Packet::Acks(vec![Ack {
                stream: Stream::Interactions,
                id,
            }]))
        };
        let claim = Packet::Interactions(vec![touch(5, 1)]);
        assert_eq!(send(&mut session, client(1), &claim, now), taken(1));
        // Player 1 attacked object 5, beside it: it is theirs from the next tick.
        assert_eq!(owner_of(&mut session, 5), Some(client(1)));
        assert_eq!(owner_of(&mut session, 6), None);

        // Far from the object, player 0's touch does not take it back.
        let mut far = state(0);
        far[4..8].copy_from_slice(&(2.0 * INTERACTION_REACH).to_be_bytes());
        let away = Packet::State {
            sequence: 2,
            state: far,
        };
        send(&mut session, client(0), &away, now);
        // Player 1's game reports it now, so the server knows where it is.
        send(&mut session, client(1), &report, now);
        let from_afar = Packet::Interactions(vec![touch(5, 0)]);
        send(&mut session, client(0), &from_afar, now);
        assert_eq!(owner_of(&mut session, 5), None);
        let near = Packet::State {
            sequence: 3,
            state: state(0),
        };
        send(&mut session, client(0), &near, now);
        let hit_back = Packet::Interactions(vec![touch(5, 1)]);
        send(&mut session, client(0), &hit_back, now);
        assert_eq!(owner_of(&mut session, 5), Some(client(0)));
    }

    #[test]
    fn progress_one_player_gets_goes_to_everyone_once() {
        let mut session = Session::new();
        let now = Instant::now();
        for port in 0..2 {
            send(&mut session, client(port), &hello(), now);
        }
        let progress_sent = |session: &mut Session| -> Vec<SocketAddr> {
            let outgoing = session.tick(now).into_iter();
            outgoing
                .filter(|(_, packet)| matches!(packet, Packet::SessionProgress(_)))
                .map(|(address, _)| address)
                .collect()
        };
        // Nothing to share yet.
        assert_eq!(progress_sent(&mut session), []);

        let mut star = [0; PROGRESS_SIZE];
        star[4] = 1;
        let report = Packet::Progress(Box::new(Progress::from_bytes(star)));
        send(&mut session, client(1), &report, now);
        let events = session.take_events();
        assert!(events.contains(&Event::ProgressAdded(id(1), 1)));
        assert_eq!(progress_sent(&mut session), [client(0), client(1)]);
        assert_eq!(progress_sent(&mut session), []);
    }

    /// A session with a host password, and players 0, 1 and 2 called Mario, Luigi and Peach;
    /// Mario's bridge knows the password.
    fn named_session(now: Instant) -> Session {
        let mut session = Session::with_config(Config {
            host_password: "my own".to_owned(),
            ..Config::default()
        });
        for (port, name) in [(0, "Mario"), (1, "Luigi"), (2, "Peach")] {
            send(&mut session, client(port), &hello(), now);
            let introduce = Packet::Introduce {
                name: Name::new(name),
                // Peach comes already dressed; the others take what their numbers give.
                outfit: Outfit::named("pink").filter(|_| port == 2),
                host_password: if port == 0 { "my own" } else { "" }.to_owned(),
            };
            send(&mut session, client(port), &introduce, now);
        }
        session
    }

    /// Has the player at `port` say `text`, under a new id each time.
    fn say(session: &mut Session, port: u16, id: u16, text: &str, now: Instant) {
        let text = Message::new(text).unwrap_or_else(|| unreachable!("the test's text"));
        send(session, client(port), &Packet::Say { id, text }, now);
    }

    /// One tick's chat lines for the player at `port`, as text, which their bridge then
    /// acknowledges.
    fn chat_for(session: &mut Session, port: u16, now: Instant) -> Vec<String> {
        let outgoing = session.tick(now).into_iter();
        let lines: Vec<_> = outgoing
            .filter(|&(address, _)| address == client(port))
            .filter_map(|(_, packet)| match packet {
                Packet::Said(lines) => Some(lines),
                _ => None,
            })
            .flatten()
            .collect();
        if let Some(last) = lines.last() {
            acknowledge(session, port, Stream::Chat, last.id, now);
        }
        let texts = lines.iter().map(|line| line.text.as_str().to_owned());
        texts.collect()
    }

    #[test]
    fn a_player_wears_what_they_brought_or_chose_and_everyone_is_told() {
        let now = Instant::now();
        let mut session = named_session(now);
        let worn = |session: &Session| -> Vec<&'static str> {
            let roster = session.roster();
            roster.iter().map(|entry| entry.outfit.name()).collect()
        };
        // By their numbers, but for Peach, who brought hers.
        assert_eq!(worn(&session), ["red", "green", "pink"]);
        chat_for(&mut session, 1, now);

        say(&mut session, 1, 1, "/color", now);
        let listed = chat_for(&mut session, 1, now).join(" ");
        assert!(
            listed.contains("red") && listed.contains("gray"),
            "{listed}"
        );
        say(&mut session, 1, 2, "/color plaid", now);
        assert!(
            chat_for(&mut session, 1, now)
                .join(" ")
                .contains("not a color")
        );
        assert_eq!(worn(&session), ["red", "green", "pink"]);

        // Two may wear the same; the roster that carries it goes to everyone again.
        say(&mut session, 1, 3, "/color Pink", now);
        assert_eq!(worn(&session), ["red", "pink", "pink"]);
        let sent = session.tick(now);
        let told = |port, wanted: fn(&Packet) -> bool| {
            sent.iter()
                .any(|(address, packet)| *address == client(port) && wanted(packet))
        };
        assert!(told(1, |packet| matches!(packet, Packet::Said(_))));
        assert!(told(0, |packet| matches!(packet, Packet::Roster(_))));
    }

    #[test]
    fn the_host_is_whoever_knows_the_host_password_and_a_wrong_one_is_flagged() {
        let now = Instant::now();
        let mut session = named_session(now);
        let roles: Vec<(u8, Role)> = session
            .roster()
            .iter()
            .map(|entry| (entry.player.get(), entry.role))
            .collect();
        assert_eq!(
            roles,
            [(0, Role::Host), (1, Role::Player), (2, Role::Player)]
        );
        session.take_events();

        send(&mut session, client(3), &hello(), now);
        let guess = Packet::Introduce {
            name: Name::new("Wario"),
            outfit: None,
            host_password: "a guess".to_owned(),
        };
        send(&mut session, client(3), &guess, now);
        assert!(
            session
                .take_events()
                .contains(&Event::HostPasswordWrong(id(3)))
        );
        assert_eq!(
            session.roster().last().map(|entry| entry.role),
            Some(Role::Player)
        );
    }

    #[test]
    fn only_the_host_appoints_moderators_who_may_remove_players_but_not_each_other() {
        let now = Instant::now();
        let mut session = named_session(now);
        chat_for(&mut session, 1, now);

        // Luigi is nobody yet.
        say(&mut session, 1, 0, "/kick Peach", now);
        say(&mut session, 1, 1, "/mod Luigi", now);
        assert_eq!(session.take_orders(), []);
        let refused = chat_for(&mut session, 1, now);
        assert!(
            refused
                .iter()
                .any(|line| line.contains("Only the host and moderators"))
        );
        assert!(
            refused
                .iter()
                .any(|line| line.contains("Only the host can appoint"))
        );

        // The host makes him a moderator, by name in any case.
        say(&mut session, 0, 0, "/mod luigi", now);
        assert!(
            session
                .take_events()
                .contains(&Event::RoleChanged(id(1), Role::Moderator))
        );
        say(&mut session, 1, 2, "/kick 2", now);
        say(&mut session, 1, 3, "/ban Mario", now);
        say(&mut session, 1, 4, "/kick Luigi", now);
        assert_eq!(session.take_orders(), [Command::Kick(id(2))]);

        // And the host may remove a moderator.
        say(&mut session, 0, 1, "/ban Luigi", now);
        assert_eq!(session.take_orders(), [Command::Ban(id(1))]);
    }

    #[test]
    fn what_the_servers_owner_says_is_marked_as_the_servers_and_not_the_hosts() {
        let now = Instant::now();
        let mut session = named_session(now);
        chat_for(&mut session, 1, now);
        session.take_events();

        // Mario is the host, a player. This is from outside the session, and says so.
        let Some(text) = Message::new("back in five") else {
            unreachable!("the test's message is not empty");
        };
        session.say_as_server(&text);

        assert_eq!(chat_for(&mut session, 1, now), ["[Server] back in five"]);
        let from_nobody = session
            .take_events()
            .iter()
            .any(|event| matches!(event, Event::Chat { from: None, name, .. } if name == "Server"));
        assert!(from_nobody);
    }

    #[test]
    fn only_the_host_ends_the_session_from_the_chat() {
        let now = Instant::now();
        let mut session = named_session(now);
        chat_for(&mut session, 0, now);
        chat_for(&mut session, 1, now);

        say(&mut session, 1, 0, "/end", now);
        assert_eq!(session.take_orders(), []);
        let refused = chat_for(&mut session, 1, now).join(" ");
        assert!(refused.contains("Only the host can end"), "{refused}");

        // The host's help names it, at the end of more lines than one packet holds, and the
        // host's word is an order to stop.
        say(&mut session, 0, 0, "/help", now);
        let mut help = chat_for(&mut session, 0, now);
        help.extend(chat_for(&mut session, 0, now));
        assert!(help.join(" ").contains("/end"), "{help:?}");
        say(&mut session, 0, 1, "/end", now);
        assert_eq!(session.take_orders(), [Command::End]);
    }

    #[test]
    fn chat_reaches_everyone_and_a_removal_is_announced() {
        let now = Instant::now();
        let mut session = named_session(now);
        for port in 0..3 {
            chat_for(&mut session, port, now);
        }
        say(&mut session, 2, 0, "hello all", now);
        assert_eq!(chat_for(&mut session, 0, now), ["hello all"]);
        let events = session.take_events();
        let chat = events.iter().find_map(|event| match event {
            Event::Chat { from, name, text } => Some((*from, name.as_str(), text.as_str())),
            _ => None,
        });
        assert_eq!(chat, Some((Some(id(2)), "Peach", "hello all")));

        session.kick(id(2), RemoveReason::Banned);
        assert_eq!(chat_for(&mut session, 0, now), ["Peach was banned"]);
    }

    #[test]
    fn a_coin_counts_for_everyone_else_in_the_level_once() {
        let mut session = Session::new();
        let now = Instant::now();
        // Players 0 and 1 in level 16, player 2 in level 9.
        for (port, level) in [(0, 16), (1, 16), (2, 9)] {
            send(&mut session, client(port), &hello(), now);
            let mut here = state(0);
            here[2..4].copy_from_slice(&i16::to_be_bytes(level));
            let playing = Packet::State {
                sequence: 1,
                state: here,
            };
            send(&mut session, client(port), &playing, now);
        }
        let coin = Coin {
            level: 16,
            area: 1,
            name: 7,
            value: 1,
            id: 0,
        };
        let collected = Packet::Coins(vec![coin]);
        send(&mut session, client(0), &collected, now);
        // A repeat of the same coin is dropped.
        send(&mut session, client(0), &collected, now);
        let coins_sent = |session: &mut Session| -> Vec<(SocketAddr, usize)> {
            let outgoing = session.tick(now).into_iter();
            outgoing
                .filter_map(|(address, packet)| match packet {
                    Packet::CoinsCollected(coins) => Some((address, coins.len())),
                    _ => None,
                })
                .collect()
        };
        // Every tick, until player 1's bridge says it has it.
        for _ in 0..5 {
            assert_eq!(coins_sent(&mut session), [(client(1), 1)]);
        }
        acknowledge(&mut session, 1, Stream::Coins, 0, now);
        assert_eq!(coins_sent(&mut session), []);
    }

    #[test]
    fn a_player_who_comes_later_is_told_which_coins_are_gone() {
        let mut session = Session::new();
        let now = Instant::now();
        let enter = |session: &mut Session, port: u16, sequence: u32, level: i16| {
            let mut here = state(0);
            here[2..4].copy_from_slice(&level.to_be_bytes());
            let playing = Packet::State {
                sequence,
                state: here,
            };
            send(session, client(port), &playing, now);
        };
        for port in 0..2 {
            send(&mut session, client(port), &hello(), now);
        }
        enter(&mut session, 0, 1, 16);
        enter(&mut session, 1, 1, 9);
        let coin = |name: u16, id: u16| Coin {
            level: 16,
            area: 1,
            name,
            value: 1,
            id,
        };
        // Player 0 collects coin 7 and one with no name, alone in level 16.
        let collected = Packet::Coins(vec![coin(7, 0), coin(0, 1)]);
        send(&mut session, client(0), &collected, now);
        let coins_for = |session: &mut Session, port: u16| -> Vec<(u16, u8)> {
            let outgoing = session.tick(now).into_iter();
            outgoing
                .filter(|&(address, _)| address == client(port))
                .filter_map(|(_, packet)| match packet {
                    Packet::CoinsCollected(coins) => Some(coins),
                    _ => None,
                })
                .flatten()
                .map(|coin| (coin.name, coin.value))
                .collect()
        };
        assert_eq!(coins_for(&mut session, 1), []);

        // Player 1 arrives: coin 7 is to go from their game, worth nothing to them.
        enter(&mut session, 1, 2, 16);
        assert_eq!(coins_for(&mut session, 1), [(7, 0)]);
        acknowledge(&mut session, 1, Stream::Coins, 0, now);
        assert_eq!(coins_for(&mut session, 1), []);

        // Another coin that shares coin 7's name, as two of one ring may: it counts for
        // player 0 like any other, and is kept as well.
        send(
            &mut session,
            client(1),
            &Packet::Coins(vec![coin(7, 0)]),
            now,
        );
        assert_eq!(coins_for(&mut session, 0), [(7, 1)]);
        acknowledge(&mut session, 0, Stream::Coins, 0, now);

        // Player 0 leaves the area and comes back to it: both are to go from their game again.
        let elsewhere = |session: &mut Session, port: u16, sequence: u32| {
            let mut here = state(0);
            here[1] = 2;
            let playing = Packet::State {
                sequence,
                state: here,
            };
            send(session, client(port), &playing, now);
        };
        elsewhere(&mut session, 0, 2);
        enter(&mut session, 0, 3, 16);
        assert_eq!(coins_for(&mut session, 0), [(7, 0), (7, 0)]);

        // Once the level is empty its coins are back, so nobody is told of them.
        enter(&mut session, 0, 4, 9);
        enter(&mut session, 1, 3, 9);
        session.tick(now);
        enter(&mut session, 1, 4, 16);
        assert_eq!(coins_for(&mut session, 1), []);
    }
}
