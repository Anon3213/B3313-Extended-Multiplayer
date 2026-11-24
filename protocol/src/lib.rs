//! Wire format between bridges and the session server.
//!
//! Every packet starts with `MAGIC`, the protocol version and a kind byte. Decoding checks every
//! length exactly, because packets come from the network.

use std::fmt;
use std::time::Duration;

pub mod chat;
pub mod game;
pub mod metrics;
pub mod objects;
pub mod progress;
pub mod reliable;
pub mod secure;

pub use chat::{
    ChatLine, MAX_HOST_PASSWORD_LENGTH, MAX_LINES_PER_PACKET, MAX_MESSAGE_LENGTH, MAX_NAME_LENGTH,
    Message, Name, Outfit, Role, RosterEntry,
};
pub use progress::{PROGRESS_SIZE, Progress};

pub use objects::{MAX_OBJECT_ID, OBJECT_STATE_SIZE, ObjectSet, ObjectState};
use objects::{OBJECT_SET_SIZE, is_child_name, is_object_id, object_id};
pub use reliable::{Ack, Inbox, OUTBOX_MOST, Outbox, Stream};

/// How often the server sends every player a snapshot, each with the next tick number. Bridges place
/// snapshots in time by their tick, so both ends must agree on this.
pub const TICK_INTERVAL: Duration = Duration::from_millis(33);

pub const PROTOCOL_VERSION: u8 = 28;
const MAGIC: [u8; 4] = *b"B3NP";
const HEADER_SIZE: usize = MAGIC.len() + 2;

/// Players in one session, the size of the game's mailbox table.
pub const MAX_PLAYERS: u8 = 16;
/// `MAX_PLAYERS` as a length; constants cannot convert types without `as`.
pub const SESSION_SIZE: usize = 16;

/// The used part of the game's `struct MpPlayerSlot`, through `platformOffset`, rounded to a whole
/// word.
pub const PLAYER_STATE_SIZE: usize = 0x24;

/// One player's visible state, in the game's byte layout. The game validates it, not the network.
pub type PlayerState = [u8; PLAYER_STATE_SIZE];

/// The used part of the game's `struct MpWorld`, through its trackers.
pub const WORLD_INPUTS_SIZE: usize = 0x124;

/// Everything the game reads to generate a level, in its byte layout. The game validates it.
pub type WorldInputs = [u8; WORLD_INPUTS_SIZE];

/// The most object states one packet carries; more wait for the next.
pub const MAX_OBJECTS_PER_PACKET: usize = 8;
/// The most spawned enemies one object packet reports gone; more wait for the next.
pub const MAX_GONE_CHILDREN: usize = 8;
/// The most spawned enemies an area's list of those alive holds, as the game's
/// `MP_LIVE_CHILDREN`; with more, the server sends no list, and games remove none.
pub const MAX_LIVE_CHILDREN: usize = 48;
/// In place of a list's count, on the wire and in the game's mailbox: there is no list.
const NO_LIVE_CHILDREN: u8 = u8::MAX;

/// A snapshot holding every other player.
const FULL_SNAPSHOT_SIZE: usize =
    HEADER_SIZE + 4 + 2 + 1 + (SESSION_SIZE - 1) * (1 + 4 + PLAYER_STATE_SIZE);
/// An `ObjectUpdates` with every name and state it can carry.
const FULL_OBJECT_UPDATES_SIZE: usize = HEADER_SIZE
    + 3
    + 2 * OBJECT_SET_SIZE
    + 1
    + MAX_LIVE_CHILDREN * 2
    + 1
    + MAX_OBJECTS_PER_PACKET * (1 + 4 + OBJECT_STATE_SIZE);
/// The largest packet: the larger of those two.
pub const MAX_PACKET_SIZE: usize = if FULL_SNAPSHOT_SIZE > FULL_OBJECT_UPDATES_SIZE {
    FULL_SNAPSHOT_SIZE
} else {
    FULL_OBJECT_UPDATES_SIZE
};
/// The largest datagram: the largest packet, sealed. Receive buffers are this size.
pub const MAX_DATAGRAM_SIZE: usize = HEADER_SIZE + 8 + MAX_PACKET_SIZE + secure::TAG_SIZE;
// Under the typical internet MTU less IP and UDP headers, so no datagram is ever fragmented.
const _: () = assert!(MAX_DATAGRAM_SIZE <= 1500 - 48);

/// Whether a receive failed only because the datagram was longer than the buffer. Windows
/// says so and throws the datagram away, where other systems hand over its beginning. No
/// packet is that long, so whoever receives drops it and goes on.
#[must_use]
pub fn is_oversized_datagram(error: &std::io::Error) -> bool {
    /// `WSAEMSGSIZE`.
    const MESSAGE_TOO_LONG: i32 = 10040;
    cfg!(windows) && error.raw_os_error() == Some(MESSAGE_TOO_LONG)
}

const KIND_HELLO: u8 = 1;
const KIND_WELCOME: u8 = 2;
const KIND_REJECTED: u8 = 3;
const KIND_STATE: u8 = 4;
const KIND_SNAPSHOT: u8 = 5;
const KIND_BYE: u8 = 6;
const KIND_ALIVE: u8 = 7;
const KIND_OFFER_WORLD: u8 = 8;
const KIND_WORLD: u8 = 9;
const KIND_PING: u8 = 10;
const KIND_PONG: u8 = 11;
const KIND_HITS: u8 = 12;
const KIND_HURTS: u8 = 13;
const KIND_CHALLENGE: u8 = 14;
const KIND_ACCEPT: u8 = 15;
const KIND_SEALED: u8 = 16;
const KIND_REMOVED: u8 = 17;
const KIND_OBJECTS: u8 = 18;
const KIND_OBJECT_UPDATES: u8 = 19;
const KIND_INTERACTIONS: u8 = 20;
const KIND_PROGRESS: u8 = 22;
const KIND_SESSION_PROGRESS: u8 = 23;
const KIND_COINS: u8 = 24;
const KIND_COINS_COLLECTED: u8 = 25;
const KIND_INTRODUCE: u8 = 26;
const KIND_ROSTER: u8 = 27;
const KIND_SAY: u8 = 28;
const KIND_SAID: u8 = 29;
const KIND_ACKS: u8 = 30;
const KIND_WEAR: u8 = 31;
/// In an `Introduce`, the outfit number that stands for none chosen.
const NO_OUTFIT: u8 = u8::MAX;
/// In a `Said` line, the player number that stands for the server itself.
const FROM_SERVER: u8 = u8::MAX;
// The largest chat packets must fit the receive buffers: a full `Said` and a full `Roster`.
const _: () = assert!(
    HEADER_SIZE + 1 + MAX_LINES_PER_PACKET * (4 + MAX_MESSAGE_LENGTH) <= MAX_PACKET_SIZE
        && HEADER_SIZE + 1 + SESSION_SIZE * (4 + MAX_NAME_LENGTH) <= MAX_PACKET_SIZE
);

/// A `Hello` is padded to this size, that of a `Challenge`, so the server's answer to a stranger
/// is never larger than what the stranger sent.
const HELLO_MIN_SIZE: usize = HEADER_SIZE + 8 + secure::SALT_SIZE;

/// The most hits or hurts one packet carries; more wait for the next.
pub const MAX_HITS_PER_PACKET: usize = 8;
/// A spawned enemy's going is sent in this many consecutive packets, so one lost packet does not
/// lose it. Hits, touches, coins and chat are sent until acknowledged instead; see `reliable`.
pub const GONE_CHILD_SENDS: u8 = 3;

/// The kind of attack that landed. The victim's game decides what each does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    Punch,
    Kick,
    /// A sweep or slide that trips.
    Trip,
}

impl HitKind {
    /// The game's code for this kind, as in `TrollEngine/Multiplayer/mailbox.h`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Punch => 1,
            Self::Kick => 2,
            Self::Trip => 3,
        }
    }

    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Punch),
            2 => Some(Self::Kick),
            3 => Some(Self::Trip),
            _ => None,
        }
    }
}

/// An attack by the sender on `target`, as the sender's game saw it land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hit {
    pub target: PlayerId,
    pub kind: HitKind,
    /// Raised by one per hit, so the server can drop repeats.
    pub id: u16,
}

/// An attack on the receiver by `attacker`, accepted by the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hurt {
    pub attacker: PlayerId,
    pub kind: HitKind,
    /// Raised by one per hurt sent to this receiver, so it can drop repeats.
    pub id: u16,
}

/// What a player's Mario did to a copy of an object another player's game runs, as the game's
/// `oInteractStatus` for it: an attack, or nothing more when he picked it up. His game runs the
/// object itself from then on, and the server makes it the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Interaction {
    pub level: i16,
    pub area: u8,
    /// The object, by its number in that area.
    pub object: u16,
    pub status: u32,
    /// Raised by one per interaction by each sender, and by the server for each receiver, so
    /// repeats are dropped.
    pub id: u16,
}

/// The most interactions one packet carries; more wait for the next.
pub const MAX_INTERACTIONS_PER_PACKET: usize = 8;

/// A coin a player's Mario collected, which every player in the level shares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Coin {
    pub level: i16,
    pub area: u8,
    /// The coin's name, as objects are named, or 0 if it has none; its value counts either way.
    pub name: u16,
    /// What it is worth to everyone in the level. From the server it may be nothing: a named
    /// coin collected before the receiver came to the area, which its game only takes away.
    pub value: u8,
    /// Raised by one per coin by each sender, and by the server for each receiver, so repeats are
    /// dropped.
    pub id: u16,
}

/// The most a coin is worth: a blue coin's 5, with room to spare.
pub const MAX_COIN_VALUE: u8 = 10;
/// The most coins one packet carries; more wait for the next.
pub const MAX_COINS_PER_PACKET: usize = 8;

/// Reports whether id `id` comes after `last`, allowing for wrapping.
#[must_use]
pub fn is_newer_id(id: u16, last: Option<u16>) -> bool {
    last.is_none_or(|last| (1..u16::MAX / 2).contains(&id.wrapping_sub(last)))
}

/// A player's number in the session, always below `MAX_PLAYERS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PlayerId(u8);

impl PlayerId {
    #[must_use]
    pub const fn new(id: u8) -> Option<Self> {
        if id < MAX_PLAYERS {
            Some(Self(id))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    /// This player's bit in a `Snapshot`'s `present` mask.
    #[must_use]
    pub const fn bit(self) -> u16 {
        1 << self.0
    }
}

/// One player's state in a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerUpdate {
    pub player: PlayerId,
    /// The sequence number the player's bridge sent it under. Bridges number every send slot, one
    /// per `TICK_INTERVAL`, so this places the state in time as the sender saw it.
    pub sequence: u32,
    pub state: PlayerState,
}

/// One object's state as the server passes it on: with whose game runs the object, and the
/// send slot that game read it in, which places it in time as a player's state is placed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectUpdate {
    pub owner: PlayerId,
    pub sequence: u32,
    pub state: ObjectState,
}

/// Where a player is, as their state says: the only part of a state the server reads, for its hit
/// checks. Everything else passes through to the games, which validate it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Whereabouts {
    pub level: i16,
    pub area: u8,
    pub position: [f32; 3],
}

impl Whereabouts {
    /// Reads the game's `struct MpPlayerSlot` layout: `areaIndex` at 1, `levelNum` at 2, `pos` at 4.
    #[must_use]
    pub fn of(state: &PlayerState) -> Self {
        let float = |at: usize| {
            let bytes = state.get(at..at + 4).and_then(|b| b.try_into().ok());
            bytes.map_or(f32::NAN, f32::from_be_bytes)
        };
        Self {
            level: i16::from_be_bytes([state[2], state[3]]),
            area: state[1],
            position: [float(4), float(8), float(12)],
        }
    }

    /// The distance to `other`, if both are in the same level and area.
    #[must_use]
    pub fn distance_to(&self, other: &Self) -> Option<f32> {
        if (self.level, self.area) != (other.level, other.area) {
            return None;
        }
        let squares: f32 = self
            .position
            .iter()
            .zip(other.position)
            .map(|(a, b)| (a - b) * (a - b))
            .sum();
        Some(squares.sqrt())
    }
}

/// Why the server removed a player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoveReason {
    Kicked,
    /// Kicked, and kept out from now on.
    Banned,
    /// Nobody in particular: the session was ended, and everyone in it is told so.
    Ended,
    /// Nobody in particular, and not for long: the server is being started again, and a
    /// bridge that is told so joins it again by itself.
    Restarting,
}

impl RemoveReason {
    /// The reason's code on the wire; `Packet::decode` reads it back.
    const fn code(self) -> u8 {
        match self {
            Self::Kicked => 1,
            Self::Banned => 2,
            Self::Ended => 3,
            Self::Restarting => 4,
        }
    }

    /// The reason a code on the wire stands for, if it is one.
    const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Kicked),
            2 => Some(Self::Banned),
            3 => Some(Self::Ended),
            4 => Some(Self::Restarting),
            _ => None,
        }
    }
}

/// Why the server refused to let a client join.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectReason {
    Full,
    WrongVersion,
    /// The client's handshake was made with another password.
    WrongPassword,
    /// The client's game is another version of B3313 than the session plays, which is the one
    /// numbered here; see `game`.
    WrongGame(u8),
}

/// On the wire, the code of `RejectReason::WrongGame`, which alone has a byte after it.
const REJECT_WRONG_GAME: u8 = 4;

impl RejectReason {
    /// Writes the reason as it is on the wire: its code, and the session's game after the one
    /// reason that has it. `Reader::reject_reason` reads it back.
    fn encode(self, out: &mut Vec<u8>) {
        match self {
            Self::Full => out.push(1),
            Self::WrongVersion => out.push(2),
            Self::WrongPassword => out.push(3),
            Self::WrongGame(game) => out.extend_from_slice(&[REJECT_WRONG_GAME, game]),
        }
    }
}

/// Writes a `Hello` after the packet's header.
fn encode_hello(out: &mut Vec<u8>, cookie: u64, game: u8, handshake: &[u8]) {
    out.push(KIND_HELLO);
    out.extend_from_slice(&cookie.to_be_bytes());
    out.push(game);
    // A handshake message is far under 255 bytes; a longer one is cut, and fails.
    let handshake = handshake.get(..usize::from(u8::MAX)).unwrap_or(handshake);
    out.push(u8::try_from(handshake.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(handshake);
    out.resize(out.len().max(HELLO_MIN_SIZE), 0);
}

/// A packet's kind, as the outer layer sees it: only these travel unencrypted.
#[must_use]
pub const fn is_unsealed_kind(packet: &Packet) -> bool {
    matches!(
        packet,
        Packet::Hello { .. }
            | Packet::Challenge { .. }
            | Packet::Accept { .. }
            | Packet::Rejected(_)
            | Packet::Sealed { .. }
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Packet {
    /// Client asks to join; sent again until answered. The first carries cookie 0 and no
    /// handshake, and the server answers with a `Challenge`; the next carries that cookie, proving
    /// the client receives at its address, and the first handshake message. Padded to the size of
    /// a `Challenge`, so the server never answers a stranger with more than it got.
    Hello {
        cookie: u64,
        /// The version of B3313 the client's game is, as `game` numbers them, or 0 if it is
        /// not known. In the clear, as the cookie is: it is no secret.
        game: u8,
        handshake: Vec<u8>,
    },
    /// The cookie for the client's address, to send back, and the salt for the password's key.
    Challenge {
        cookie: u64,
        salt: secure::Salt,
    },
    /// The server's handshake reply, whose payload is the encoded `Welcome` or `Rejected`.
    Accept {
        handshake: Vec<u8>,
    },
    /// Every packet after the handshake, encrypted and authenticated under `nonce`.
    Sealed {
        nonce: u64,
        sealed: Vec<u8>,
    },
    /// Inside an `Accept`: the client's number in the session.
    Welcome {
        player: PlayerId,
    },
    Rejected(RejectReason),
    /// Client's latest state. A higher `sequence` replaces a lower one, so late packets are ignored.
    State {
        sequence: u32,
        state: PlayerState,
    },
    /// Who else is in the session, and the latest state of those the receiver needs now: players
    /// in its level and area every tick, others now and then. A player not in `present` has left.
    Snapshot {
        tick: u32,
        /// One bit per player, `1 << id`.
        present: u16,
        players: Vec<PlayerUpdate>,
    },
    Bye,
    /// Client has no state to send in this send slot, but is still in the session. It uses up
    /// a `sequence` number like a `State`, so the numbers keep counting send slots.
    Alive {
        sequence: u32,
        /// Whether the game has no Mario to show at all: it is at a menu or the title
        /// screen, or has stopped running, as one that crashed has. The others then stop
        /// showing this player. Not so for a game that is only paused, whose Mario stands
        /// where he was.
        away: bool,
    },
    /// Client's world, offered until the session has one; only the session owner's is taken.
    OfferWorld(Box<WorldInputs>),
    /// The session's world, which every player generates levels from. Resent while players stay.
    World(Box<WorldInputs>),
    /// Client asks for a `Pong` with the same `id`, to time the round trip.
    Ping {
        id: u32,
    },
    Pong {
        id: u32,
    },
    /// Client's attacks that landed, oldest first; each is sent until acknowledged.
    Hits(Vec<Hit>),
    /// Attacks on the client accepted by the server, oldest first; each is sent until
    /// acknowledged.
    Hurts(Vec<Hurt>),
    /// The receiver is out of the session, and why. Sent sealed, so only the server can send it.
    Removed(RemoveReason),
    /// Client's synced objects in its level and area: states of those the server gave it to run,
    /// at most `MAX_OBJECTS_PER_PACKET`, which of the area's numbered objects are gone in its game,
    /// and the spawned enemies it ran that have just gone, at most `MAX_GONE_CHILDREN`.
    /// `sequence` is the send slot the states were read in, the one its `State` went under, so
    /// other bridges can place them in time beside its player.
    Objects {
        level: i16,
        area: u8,
        sequence: u32,
        gone: ObjectSet,
        gone_children: Vec<u16>,
        states: Vec<ObjectState>,
    },
    /// For the receiver's level and area: the numbered objects it runs for everyone, those their
    /// owners report gone, and the newest states from other owners that it has not been sent yet.
    ///
    /// `children` is every spawned enemy alive there, as the games that run them last said: the
    /// whole list, so a lost packet is made good by the next. The receiver's game removes its
    /// copy of another game's enemy that is not in it. It is sent when it changes and with each
    /// refresh, and not at all while there are more than `MAX_LIVE_CHILDREN`.
    ObjectUpdates {
        level: i16,
        area: u8,
        owned: ObjectSet,
        gone: ObjectSet,
        children: Option<Vec<u16>>,
        states: Vec<ObjectUpdate>,
    },
    /// Objects other players' games run that the client's Mario attacked or picked up, oldest
    /// first: its game runs each itself from then on, and asks to be its owner.
    Interactions(Vec<Interaction>),
    /// Everything client's game got during the session: stars, keys, cap switches and doors. Sent
    /// whenever it grows, and now and then in case one was lost.
    Progress(Box<Progress>),
    /// Everything every player got during the session, for the receiver's game to add to its save.
    /// Sent whenever it grows, and now and then.
    SessionProgress(Box<Progress>),
    /// Coins client's Mario collected, oldest first.
    Coins(Vec<Coin>),
    /// Coins other players in the receiver's level collected, accepted by the server.
    CoinsCollected(Vec<Coin>),
    /// Client's name and outfit, if its player chose them, and the host password, if it has
    /// one; sent until a `Roster` lists the client.
    Introduce {
        name: Option<Name>,
        outfit: Option<Outfit>,
        host_password: String,
    },
    /// Everyone in the session, with their roles, outfits and names.
    Roster(Vec<RosterEntry>),
    /// The outfit the client's player now chooses to wear; sent until a `Roster` shows it.
    Wear(Outfit),
    /// Something the client's player typed; sent under one `id` until acknowledged.
    Say {
        id: u16,
        text: Message,
    },
    /// Chat lines for the receiver, oldest first; each is sent until acknowledged.
    Said(Vec<ChatLine>),
    /// How far the sender has taken the events of each stream named, so the receiver stops
    /// sending those again. Sent in answer to every packet of a stream.
    Acks(Vec<Ack>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodeError {
    NotOurs,
    WrongVersion(u8),
    Malformed,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOurs => f.write_str("not a B3313 packet"),
            Self::WrongVersion(version) => write!(f, "protocol version {version}"),
            Self::Malformed => f.write_str("malformed packet"),
        }
    }
}

impl std::error::Error for DecodeError {}

impl Packet {
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(MAX_PACKET_SIZE);
        out.extend_from_slice(&MAGIC);
        out.push(PROTOCOL_VERSION);
        match self {
            Self::Hello {
                cookie,
                game,
                handshake,
            } => encode_hello(&mut out, *cookie, *game, handshake),
            Self::Challenge { cookie, salt } => {
                out.push(KIND_CHALLENGE);
                out.extend_from_slice(&cookie.to_be_bytes());
                out.extend_from_slice(salt);
            }
            Self::Accept { handshake } => {
                out.push(KIND_ACCEPT);
                out.extend_from_slice(handshake);
            }
            Self::Sealed { nonce, sealed } => {
                out.push(KIND_SEALED);
                out.extend_from_slice(&nonce.to_be_bytes());
                out.extend_from_slice(sealed);
            }
            Self::Welcome { player } => out.extend_from_slice(&[KIND_WELCOME, player.0]),
            Self::Rejected(reason) => {
                out.push(KIND_REJECTED);
                reason.encode(&mut out);
            }
            Self::State { sequence, state } => {
                out.push(KIND_STATE);
                out.extend_from_slice(&sequence.to_be_bytes());
                out.extend_from_slice(state);
            }
            Self::Snapshot {
                tick,
                present,
                players,
            } => encode_snapshot(&mut out, *tick, *present, players),
            Self::Bye => out.push(KIND_BYE),
            Self::Alive { sequence, away } => {
                out.push(KIND_ALIVE);
                out.extend_from_slice(&sequence.to_be_bytes());
                out.push(u8::from(*away));
            }
            Self::OfferWorld(world) => {
                out.push(KIND_OFFER_WORLD);
                out.extend_from_slice(world.as_slice());
            }
            Self::World(world) => {
                out.push(KIND_WORLD);
                out.extend_from_slice(world.as_slice());
            }
            Self::Ping { id } => {
                out.push(KIND_PING);
                out.extend_from_slice(&id.to_be_bytes());
            }
            Self::Pong { id } => {
                out.push(KIND_PONG);
                out.extend_from_slice(&id.to_be_bytes());
            }
            Self::Hits(hits) => {
                out.push(KIND_HITS);
                let records = hits.iter().map(|hit| (hit.target, hit.kind, hit.id));
                encode_attacks(&mut out, records);
            }
            Self::Hurts(hurts) => {
                out.push(KIND_HURTS);
                let records = hurts.iter().map(|hurt| (hurt.attacker, hurt.kind, hurt.id));
                encode_attacks(&mut out, records);
            }
            Self::Removed(reason) => out.extend_from_slice(&[KIND_REMOVED, reason.code()]),
            Self::Objects { .. } | Self::ObjectUpdates { .. } => self.encode_objects(&mut out),
            Self::Interactions(list) => encode_interactions(&mut out, list),
            Self::Progress(progress) => {
                out.push(KIND_PROGRESS);
                out.extend_from_slice(progress.bytes());
            }
            Self::SessionProgress(progress) => {
                out.push(KIND_SESSION_PROGRESS);
                out.extend_from_slice(progress.bytes());
            }
            Self::Coins(coins) => encode_coins(&mut out, KIND_COINS, coins),
            Self::CoinsCollected(coins) => encode_coins(&mut out, KIND_COINS_COLLECTED, coins),
            Self::Introduce { .. } | Self::Roster(_) | Self::Say { .. } | Self::Said(_) => {
                self.encode_people(&mut out);
            }
            Self::Wear(outfit) => out.extend_from_slice(&[KIND_WEAR, outfit.code()]),
            Self::Acks(acks) => {
                let acks = acks.get(..Stream::ALL.len()).unwrap_or(acks);
                out.push(KIND_ACKS);
                out.push(u8::try_from(acks.len()).unwrap_or(0));
                for ack in acks {
                    out.push(ack.stream.code());
                    out.extend_from_slice(&ack.id.to_be_bytes());
                }
            }
        }
        out
    }

    /// Writes an `Introduce`, `Roster`, `Say` or `Said`: each text as a length byte and its
    /// characters, and a `Roster`'s entries and a `Said`'s lines after a count.
    fn encode_people(&self, out: &mut Vec<u8>) {
        match self {
            Self::Introduce {
                name,
                outfit,
                host_password,
            } => {
                out.push(KIND_INTRODUCE);
                encode_text(out, name.as_ref().map_or("", Name::as_str));
                out.push(outfit.map_or(NO_OUTFIT, Outfit::code));
                let password = host_password.get(..MAX_HOST_PASSWORD_LENGTH);
                encode_text(out, password.unwrap_or(host_password));
            }
            Self::Roster(entries) => {
                let entries = entries.get(..SESSION_SIZE).unwrap_or(entries);
                out.push(KIND_ROSTER);
                out.push(u8::try_from(entries.len()).unwrap_or(0));
                for entry in entries {
                    out.extend_from_slice(&[
                        entry.player.0,
                        entry.role.code(),
                        entry.outfit.code(),
                    ]);
                    encode_text(out, entry.name.as_str());
                }
            }
            Self::Say { id, text } => {
                out.push(KIND_SAY);
                out.extend_from_slice(&id.to_be_bytes());
                encode_text(out, text.as_str());
            }
            Self::Said(lines) => {
                let lines = lines.get(..MAX_LINES_PER_PACKET).unwrap_or(lines);
                out.push(KIND_SAID);
                out.push(u8::try_from(lines.len()).unwrap_or(0));
                for line in lines {
                    out.extend_from_slice(&line.id.to_be_bytes());
                    out.push(line.from.map_or(FROM_SERVER, |player| player.0));
                    encode_text(out, line.text.as_str());
                }
            }
            _ => {}
        }
    }

    /// Parses a packet, accepting it only if every byte is accounted for.
    ///
    /// # Errors
    /// Fails for another program's data, another protocol version, or any malformed packet.
    pub fn decode(data: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader(data);
        if reader.array::<4>().ok() != Some(MAGIC) {
            return Err(DecodeError::NotOurs);
        }
        let version = reader.byte()?;
        if version != PROTOCOL_VERSION {
            return Err(DecodeError::WrongVersion(version));
        }
        let packet = match reader.byte()? {
            KIND_HELLO => reader.hello()?,
            KIND_CHALLENGE => Self::Challenge {
                cookie: u64::from_be_bytes(reader.array()?),
                salt: reader.array()?,
            },
            KIND_ACCEPT => Self::Accept {
                handshake: reader.rest().to_vec(),
            },
            KIND_SEALED => reader.sealed()?,
            KIND_WELCOME => Self::Welcome {
                player: reader.player()?,
            },
            KIND_REJECTED => Self::Rejected(reader.reject_reason()?),
            KIND_STATE => Self::State {
                sequence: u32::from_be_bytes(reader.array()?),
                state: reader.array()?,
            },
            KIND_SNAPSHOT => reader.snapshot()?,
            KIND_BYE => Self::Bye,
            KIND_ALIVE => Self::Alive {
                sequence: u32::from_be_bytes(reader.array()?),
                away: match reader.byte()? {
                    0 => false,
                    1 => true,
                    _ => return Err(DecodeError::Malformed),
                },
            },
            KIND_OFFER_WORLD => Self::OfferWorld(Box::new(reader.array()?)),
            KIND_WORLD => Self::World(Box::new(reader.array()?)),
            KIND_PING => Self::Ping {
                id: u32::from_be_bytes(reader.array()?),
            },
            KIND_PONG => Self::Pong {
                id: u32::from_be_bytes(reader.array()?),
            },
            KIND_HITS => Self::Hits(reader.attacks(|target, kind, id| Hit { target, kind, id })?),
            KIND_HURTS => {
                Self::Hurts(reader.attacks(|attacker, kind, id| Hurt { attacker, kind, id })?)
            }
            KIND_REMOVED => Self::Removed(
                RemoveReason::from_code(reader.byte()?).ok_or(DecodeError::Malformed)?,
            ),
            KIND_OBJECTS => Self::Objects {
                level: i16::from_be_bytes(reader.array()?),
                area: reader.byte()?,
                sequence: u32::from_be_bytes(reader.array()?),
                gone: ObjectSet::from_bytes(reader.array()?),
                gone_children: reader.child_names()?,
                states: reader.object_states(Reader::object_state)?,
            },
            KIND_OBJECT_UPDATES => Self::ObjectUpdates {
                level: i16::from_be_bytes(reader.array()?),
                area: reader.byte()?,
                owned: ObjectSet::from_bytes(reader.array()?),
                gone: ObjectSet::from_bytes(reader.array()?),
                children: reader.live_children()?,
                states: reader.object_states(|reader| {
                    Ok(ObjectUpdate {
                        owner: reader.player()?,
                        sequence: u32::from_be_bytes(reader.array()?),
                        state: reader.object_state()?,
                    })
                })?,
            },
            KIND_INTERACTIONS => Self::Interactions(reader.interactions()?),
            KIND_PROGRESS => Self::Progress(Box::new(Progress::from_bytes(reader.array()?))),
            KIND_SESSION_PROGRESS => {
                Self::SessionProgress(Box::new(Progress::from_bytes(reader.array()?)))
            }
            KIND_COINS => Self::Coins(reader.coins(1)?),
            KIND_COINS_COLLECTED => Self::CoinsCollected(reader.coins(0)?),
            KIND_INTRODUCE => reader.introduce()?,
            KIND_ROSTER => Self::Roster(reader.roster()?),
            KIND_WEAR => {
                Self::Wear(Outfit::from_code(reader.byte()?).ok_or(DecodeError::Malformed)?)
            }
            KIND_SAY => Self::Say {
                id: u16::from_be_bytes(reader.array()?),
                text: reader.message()?,
            },
            KIND_SAID => Self::Said(reader.said()?),
            KIND_ACKS => Self::Acks(reader.acks()?),
            _ => return Err(DecodeError::Malformed),
        };
        if reader.0.is_empty() {
            Ok(packet)
        } else {
            Err(DecodeError::Malformed)
        }
    }
}

/// Writes a count and up to `MAX_HITS_PER_PACKET` (player, kind, id) records. Callers send any
/// more in a later packet; dropping them here keeps every packet decodable.
fn encode_attacks(out: &mut Vec<u8>, records: impl Iterator<Item = (PlayerId, HitKind, u16)>) {
    let records: Vec<_> = records.take(MAX_HITS_PER_PACKET).collect();
    out.push(u8::try_from(records.len()).unwrap_or(0));
    for (player, kind, id) in records {
        out.extend_from_slice(&[player.0, kind.code()]);
        out.extend_from_slice(&id.to_be_bytes());
    }
}

fn encode_snapshot(out: &mut Vec<u8>, tick: u32, present: u16, players: &[PlayerUpdate]) {
    out.push(KIND_SNAPSHOT);
    out.extend_from_slice(&tick.to_be_bytes());
    out.extend_from_slice(&present.to_be_bytes());
    // A snapshot never holds more players than a session can.
    out.push(u8::try_from(players.len()).unwrap_or(u8::MAX));
    for update in players {
        out.push(update.player.0);
        out.extend_from_slice(&update.sequence.to_be_bytes());
        out.extend_from_slice(&update.state);
    }
}

impl Packet {
    /// Writes an `Objects` or `ObjectUpdates`: its kind, level and area, an `Objects`' sequence,
    /// its sets, a count and up to `MAX_GONE_CHILDREN` names, then a count and up to
    /// `MAX_OBJECTS_PER_PACKET` states, each of an `ObjectUpdates` after its owner and sequence.
    /// Callers send any more later.
    fn encode_objects(&self, out: &mut Vec<u8>) {
        match self {
            Self::Objects {
                level,
                area,
                sequence,
                gone,
                gone_children,
                states,
            } => {
                out.push(KIND_OBJECTS);
                out.extend_from_slice(&level.to_be_bytes());
                out.push(*area);
                out.extend_from_slice(&sequence.to_be_bytes());
                out.extend_from_slice(gone.bytes());
                encode_child_names(out, gone_children);
                let states = states.get(..MAX_OBJECTS_PER_PACKET).unwrap_or(states);
                out.push(u8::try_from(states.len()).unwrap_or(0));
                for state in states {
                    out.extend_from_slice(state);
                }
            }
            Self::ObjectUpdates {
                level,
                area,
                owned,
                gone,
                children,
                states,
            } => {
                out.push(KIND_OBJECT_UPDATES);
                out.extend_from_slice(&level.to_be_bytes());
                out.push(*area);
                out.extend_from_slice(owned.bytes());
                out.extend_from_slice(gone.bytes());
                // A list too long to send is sent as none, which removes nothing.
                match children
                    .as_deref()
                    .filter(|names| names.len() <= MAX_LIVE_CHILDREN)
                {
                    Some(names) => {
                        out.push(u8::try_from(names.len()).unwrap_or(0));
                        for name in names {
                            out.extend_from_slice(&name.to_be_bytes());
                        }
                    }
                    None => out.push(NO_LIVE_CHILDREN),
                }
                let states = states.get(..MAX_OBJECTS_PER_PACKET).unwrap_or(states);
                out.push(u8::try_from(states.len()).unwrap_or(0));
                for update in states {
                    out.push(update.owner.0);
                    out.extend_from_slice(&update.sequence.to_be_bytes());
                    out.extend_from_slice(&update.state);
                }
            }
            _ => {}
        }
    }
}

/// Writes a count and up to `MAX_GONE_CHILDREN` spawned enemies' names.
fn encode_child_names(out: &mut Vec<u8>, names: &[u16]) {
    let names = names.get(..MAX_GONE_CHILDREN).unwrap_or(names);
    out.push(u8::try_from(names.len()).unwrap_or(0));
    for name in names {
        out.extend_from_slice(&name.to_be_bytes());
    }
}

/// Writes a count and up to `MAX_INTERACTIONS_PER_PACKET` interactions; callers send any more
/// later.
fn encode_interactions(out: &mut Vec<u8>, list: &[Interaction]) {
    let list = list.get(..MAX_INTERACTIONS_PER_PACKET).unwrap_or(list);
    out.push(KIND_INTERACTIONS);
    out.push(u8::try_from(list.len()).unwrap_or(0));
    for interaction in list {
        out.extend_from_slice(&interaction.level.to_be_bytes());
        out.push(interaction.area);
        out.extend_from_slice(&interaction.object.to_be_bytes());
        out.extend_from_slice(&interaction.status.to_be_bytes());
        out.extend_from_slice(&interaction.id.to_be_bytes());
    }
}

/// Writes a count and up to `MAX_COINS_PER_PACKET` coins; callers send any more later.
fn encode_coins(out: &mut Vec<u8>, kind: u8, coins: &[Coin]) {
    let coins = coins.get(..MAX_COINS_PER_PACKET).unwrap_or(coins);
    out.push(kind);
    out.push(u8::try_from(coins.len()).unwrap_or(0));
    for coin in coins {
        out.extend_from_slice(&coin.level.to_be_bytes());
        out.push(coin.area);
        out.extend_from_slice(&coin.name.to_be_bytes());
        out.push(coin.value);
        out.extend_from_slice(&coin.id.to_be_bytes());
    }
}

/// Writes a length byte and the text. Every caller keeps its text under 256 bytes; a longer one
/// is written as empty, which the receiver refuses, rather than cut into something else.
fn encode_text(out: &mut Vec<u8>, text: &str) {
    let text = if u8::try_from(text.len()).is_ok() {
        text
    } else {
        ""
    };
    out.push(u8::try_from(text.len()).unwrap_or(0));
    out.extend_from_slice(text.as_bytes());
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn bytes(&mut self, count: usize) -> Result<&[u8], DecodeError> {
        if count > self.0.len() {
            return Err(DecodeError::Malformed);
        }
        let (bytes, rest) = self.0.split_at(count);
        self.0 = rest;
        Ok(bytes)
    }

    /// Takes everything left.
    fn rest(&mut self) -> &[u8] {
        std::mem::take(&mut self.0)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let (bytes, rest) = self
            .0
            .split_first_chunk::<N>()
            .ok_or(DecodeError::Malformed)?;
        self.0 = rest;
        Ok(*bytes)
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        let [byte] = self.array::<1>()?;
        Ok(byte)
    }

    fn player(&mut self) -> Result<PlayerId, DecodeError> {
        PlayerId::new(self.byte()?).ok_or(DecodeError::Malformed)
    }

    /// A length byte and that many bytes.
    fn text(&mut self) -> Result<&[u8], DecodeError> {
        let length = usize::from(self.byte()?);
        self.bytes(length)
    }

    fn message(&mut self) -> Result<Message, DecodeError> {
        Message::from_wire(self.text()?).ok_or(DecodeError::Malformed)
    }

    fn introduce(&mut self) -> Result<Packet, DecodeError> {
        let name = match self.text()? {
            [] => None,
            bytes => Some(Name::from_wire(bytes).ok_or(DecodeError::Malformed)?),
        };
        let outfit = match self.byte()? {
            NO_OUTFIT => None,
            code => Some(Outfit::from_code(code).ok_or(DecodeError::Malformed)?),
        };
        let password = self.text()?;
        let is_printable = password.iter().all(|byte| (0x20..=0x7E).contains(byte));
        if password.len() > MAX_HOST_PASSWORD_LENGTH || !is_printable {
            return Err(DecodeError::Malformed);
        }
        Ok(Packet::Introduce {
            name,
            outfit,
            host_password: String::from_utf8_lossy(password).into_owned(),
        })
    }

    fn roster(&mut self) -> Result<Vec<RosterEntry>, DecodeError> {
        let count = usize::from(self.byte()?);
        if count > SESSION_SIZE {
            return Err(DecodeError::Malformed);
        }
        (0..count)
            .map(|_| {
                Ok(RosterEntry {
                    player: self.player()?,
                    role: Role::from_code(self.byte()?).ok_or(DecodeError::Malformed)?,
                    outfit: Outfit::from_code(self.byte()?).ok_or(DecodeError::Malformed)?,
                    name: Name::from_wire(self.text()?).ok_or(DecodeError::Malformed)?,
                })
            })
            .collect()
    }

    fn said(&mut self) -> Result<Vec<ChatLine>, DecodeError> {
        let count = usize::from(self.byte()?);
        if count > MAX_LINES_PER_PACKET {
            return Err(DecodeError::Malformed);
        }
        (0..count)
            .map(|_| {
                let id = u16::from_be_bytes(self.array()?);
                let from = match self.byte()? {
                    FROM_SERVER => None,
                    player => Some(PlayerId::new(player).ok_or(DecodeError::Malformed)?),
                };
                Ok(ChatLine {
                    id,
                    from,
                    text: self.message()?,
                })
            })
            .collect()
    }

    fn reject_reason(&mut self) -> Result<RejectReason, DecodeError> {
        match self.byte()? {
            1 => Ok(RejectReason::Full),
            2 => Ok(RejectReason::WrongVersion),
            3 => Ok(RejectReason::WrongPassword),
            REJECT_WRONG_GAME => Ok(RejectReason::WrongGame(self.byte()?)),
            _ => Err(DecodeError::Malformed),
        }
    }

    fn hello(&mut self) -> Result<Packet, DecodeError> {
        let cookie = u64::from_be_bytes(self.array()?);
        let game = self.byte()?;
        let length = usize::from(self.byte()?);
        let handshake = self.bytes(length)?.to_vec();
        // Only zeros may pad it out.
        if self.rest().iter().any(|&byte| byte != 0) {
            return Err(DecodeError::Malformed);
        }
        Ok(Packet::Hello {
            cookie,
            game,
            handshake,
        })
    }

    fn sealed(&mut self) -> Result<Packet, DecodeError> {
        let nonce = u64::from_be_bytes(self.array()?);
        let sealed = self.rest();
        let sizes = secure::TAG_SIZE..=MAX_PACKET_SIZE + secure::TAG_SIZE;
        if !sizes.contains(&sealed.len()) {
            return Err(DecodeError::Malformed);
        }
        Ok(Packet::Sealed {
            nonce,
            sealed: sealed.to_vec(),
        })
    }

    fn snapshot(&mut self) -> Result<Packet, DecodeError> {
        let tick = u32::from_be_bytes(self.array()?);
        let present = u16::from_be_bytes(self.array()?);
        let count = self.byte()?;
        if count >= MAX_PLAYERS {
            return Err(DecodeError::Malformed);
        }
        let players: Vec<PlayerUpdate> = (0..count)
            .map(|_| {
                Ok(PlayerUpdate {
                    player: self.player()?,
                    sequence: u32::from_be_bytes(self.array()?),
                    state: self.array()?,
                })
            })
            .collect::<Result<_, DecodeError>>()?;
        // A state for a player not present contradicts the snapshot itself.
        let is_absent = |update: &PlayerUpdate| present & update.player.bit() == 0;
        if players.iter().any(is_absent) {
            return Err(DecodeError::Malformed);
        }
        Ok(Packet::Snapshot {
            tick,
            present,
            players,
        })
    }

    /// Reads a count and that many coins, each worth at least `least` and named, if at all, as
    /// an object. One worth nothing must have a name: it is only there to be taken away.
    fn coins(&mut self, least: u8) -> Result<Vec<Coin>, DecodeError> {
        let count = usize::from(self.byte()?);
        if count > MAX_COINS_PER_PACKET {
            return Err(DecodeError::Malformed);
        }
        (0..count)
            .map(|_| {
                let coin = Coin {
                    level: i16::from_be_bytes(self.array()?),
                    area: self.byte()?,
                    name: u16::from_be_bytes(self.array()?),
                    value: self.byte()?,
                    id: u16::from_be_bytes(self.array()?),
                };
                let named = is_object_id(coin.name);
                let is_worth_something = coin.value > 0;
                let well_named = named || (coin.name == 0 && is_worth_something);
                if well_named && (least..=MAX_COIN_VALUE).contains(&coin.value) {
                    Ok(coin)
                } else {
                    Err(DecodeError::Malformed)
                }
            })
            .collect()
    }

    /// Reads a count and that many acknowledgements, at most one a stream.
    fn acks(&mut self) -> Result<Vec<Ack>, DecodeError> {
        let count = usize::from(self.byte()?);
        if count > Stream::ALL.len() {
            return Err(DecodeError::Malformed);
        }
        (0..count)
            .map(|_| {
                Ok(Ack {
                    stream: Stream::from_code(self.byte()?).ok_or(DecodeError::Malformed)?,
                    id: u16::from_be_bytes(self.array()?),
                })
            })
            .collect()
    }

    /// Reads a count and that many spawned enemies' names, `most` at most.
    fn names_of_children(&mut self, count: usize, most: usize) -> Result<Vec<u16>, DecodeError> {
        if count > most {
            return Err(DecodeError::Malformed);
        }
        (0..count)
            .map(|_| {
                let name = u16::from_be_bytes(self.array()?);
                if is_child_name(name) {
                    Ok(name)
                } else {
                    Err(DecodeError::Malformed)
                }
            })
            .collect()
    }

    /// Reads the spawned enemies a client reports gone.
    fn child_names(&mut self) -> Result<Vec<u16>, DecodeError> {
        let count = usize::from(self.byte()?);
        self.names_of_children(count, MAX_GONE_CHILDREN)
    }

    /// Reads an area's list of spawned enemies alive, if the packet has one.
    fn live_children(&mut self) -> Result<Option<Vec<u16>>, DecodeError> {
        match self.byte()? {
            NO_LIVE_CHILDREN => Ok(None),
            count => self
                .names_of_children(usize::from(count), MAX_LIVE_CHILDREN)
                .map(Some),
        }
    }

    /// Reads a count and that many interactions, each naming an object.
    fn interactions(&mut self) -> Result<Vec<Interaction>, DecodeError> {
        let count = usize::from(self.byte()?);
        if count > MAX_INTERACTIONS_PER_PACKET {
            return Err(DecodeError::Malformed);
        }
        (0..count)
            .map(|_| {
                let interaction = Interaction {
                    level: i16::from_be_bytes(self.array()?),
                    area: self.byte()?,
                    object: u16::from_be_bytes(self.array()?),
                    status: u32::from_be_bytes(self.array()?),
                    id: u16::from_be_bytes(self.array()?),
                };
                if is_object_id(interaction.object) {
                    Ok(interaction)
                } else {
                    Err(DecodeError::Malformed)
                }
            })
            .collect()
    }

    /// Reads a count and that many object states, each as `read` reads one.
    fn object_states<T>(
        &mut self,
        read: impl Fn(&mut Self) -> Result<T, DecodeError>,
    ) -> Result<Vec<T>, DecodeError> {
        let count = usize::from(self.byte()?);
        if count > MAX_OBJECTS_PER_PACKET {
            return Err(DecodeError::Malformed);
        }
        (0..count).map(|_| read(self)).collect()
    }

    /// Reads an object's state, which must name an object.
    fn object_state(&mut self) -> Result<ObjectState, DecodeError> {
        let state: ObjectState = self.array()?;
        if is_object_id(object_id(&state)) {
            Ok(state)
        } else {
            Err(DecodeError::Malformed)
        }
    }

    /// Reads a count and that many (player, kind, id) records, built by `make`.
    fn attacks<T>(
        &mut self,
        make: impl Fn(PlayerId, HitKind, u16) -> T,
    ) -> Result<Vec<T>, DecodeError> {
        let count = usize::from(self.byte()?);
        if count > MAX_HITS_PER_PACKET {
            return Err(DecodeError::Malformed);
        }
        (0..count)
            .map(|_| {
                let player = self.player()?;
                let kind = HitKind::from_code(self.byte()?).ok_or(DecodeError::Malformed)?;
                Ok(make(player, kind, u16::from_be_bytes(self.array()?)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Ack, ChatLine, Coin, DecodeError, Hit, HitKind, Hurt, Interaction, MAX_COIN_VALUE,
        MAX_OBJECTS_PER_PACKET, MAX_PACKET_SIZE, Message, Name, OBJECT_STATE_SIZE, ObjectSet,
        ObjectState, ObjectUpdate, Outfit, PLAYER_STATE_SIZE, PROGRESS_SIZE, Packet, PlayerId,
        PlayerState, PlayerUpdate, Progress, RejectReason, RemoveReason, Role, RosterEntry, Stream,
        WORLD_INPUTS_SIZE, Whereabouts, WorldInputs, is_newer_id,
    };

    fn message(text: &str) -> Message {
        Message::new(text).unwrap_or_else(|| unreachable!("the test's message is not empty"))
    }

    fn roster_entry(player: u8, role: Role, name: &str) -> RosterEntry {
        RosterEntry {
            player: id(player),
            role,
            outfit: Outfit::of_player(id(player)),
            name: Name::new(name).unwrap_or_else(|| unreachable!("the test's name is not empty")),
        }
    }

    #[test]
    fn chat_text_that_is_not_clean_is_refused_on_the_way_in() {
        let say = Packet::Say {
            id: 1,
            text: message("hi"),
        };
        let mut bytes = say.encode();
        assert_eq!(Packet::decode(&bytes), Ok(say));
        // A control character where the "i" was.
        if let Some(last) = bytes.last_mut() {
            *last = 0x07;
        }
        assert_eq!(Packet::decode(&bytes), Err(DecodeError::Malformed));
    }

    #[test]
    fn hit_ids_count_as_newer_across_the_wrap_but_not_backwards() {
        assert!(is_newer_id(0, None));
        assert!(is_newer_id(6, Some(5)));
        assert!(is_newer_id(0, Some(u16::MAX)));
        assert!(!is_newer_id(5, Some(5)));
        assert!(!is_newer_id(4, Some(5)));
    }

    #[test]
    fn kinds_match_the_games_codes() {
        for kind in [HitKind::Punch, HitKind::Kick, HitKind::Trip] {
            assert_eq!(HitKind::from_code(kind.code()), Some(kind));
        }
        assert_eq!(HitKind::from_code(0), None);
        assert_eq!(HitKind::from_code(4), None);
    }

    #[test]
    fn whereabouts_come_from_the_slot_layout_and_compare_only_within_an_area() {
        let mut state = [0; PLAYER_STATE_SIZE];
        // active, areaIndex 2, levelNum 0x0110, then pos = (3, 4, 0).
        state[..4].copy_from_slice(&[1, 2, 1, 0x10]);
        state[4..8].copy_from_slice(&3.0_f32.to_be_bytes());
        state[8..12].copy_from_slice(&4.0_f32.to_be_bytes());
        let here = Whereabouts::of(&state);
        assert_eq!((here.level, here.area), (0x0110, 2));
        assert_eq!(here.position, [3.0, 4.0, 0.0]);

        let origin = Whereabouts {
            position: [0.0; 3],
            ..here
        };
        assert_eq!(here.distance_to(&origin), Some(5.0));
        let elsewhere = Whereabouts { area: 3, ..origin };
        assert_eq!(here.distance_to(&elsewhere), None);
    }

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or(PlayerId(0))
    }

    fn state(marker: u8) -> PlayerState {
        [marker; PLAYER_STATE_SIZE]
    }

    fn world(marker: u8) -> WorldInputs {
        [marker; WORLD_INPUTS_SIZE]
    }

    fn object(id: u16) -> ObjectState {
        let mut state = [u8::try_from(id % 256).unwrap_or(0); OBJECT_STATE_SIZE];
        state[..2].copy_from_slice(&id.to_be_bytes());
        state
    }

    fn set_of(ids: &[u16]) -> ObjectSet {
        let mut set = ObjectSet::default();
        for &id in ids {
            set.insert(id);
        }
        set
    }

    fn full_object_updates() -> Packet {
        Packet::ObjectUpdates {
            level: -1,
            area: 2,
            owned: set_of(&[1, 240]),
            gone: set_of(&[3]),
            // Spawner 240's children, the highest names there are.
            children: Some((0..48).map(|key| 0x8000 | (240 << 7) | key).collect()),
            states: (1..=8)
                .map(|number| ObjectUpdate {
                    owner: id(15),
                    sequence: u32::MAX,
                    state: object(number),
                })
                .collect(),
        }
    }

    fn full_snapshot() -> Packet {
        Packet::Snapshot {
            tick: 7,
            present: 0xFFFE,
            players: (1..16)
                .map(|player| PlayerUpdate {
                    player: id(player),
                    sequence: u32::from(player) << 24,
                    state: state(player),
                })
                .collect(),
        }
    }

    /// One of each kind of packet, some at their largest.
    fn every_packet() -> Vec<Packet> {
        vec![
            Packet::Hello {
                cookie: 0,
                game: 0,
                handshake: vec![],
            },
            Packet::Hello {
                cookie: u64::MAX,
                game: u8::MAX,
                handshake: vec![9; 48],
            },
            Packet::Challenge {
                cookie: 7,
                salt: [3; 16],
            },
            Packet::Accept {
                handshake: vec![4; 60],
            },
            Packet::Sealed {
                nonce: 5,
                sealed: vec![6; 40],
            },
            Packet::Welcome { player: id(15) },
            Packet::Rejected(RejectReason::Full),
            Packet::Rejected(RejectReason::WrongVersion),
            Packet::Rejected(RejectReason::WrongPassword),
            Packet::Rejected(RejectReason::WrongGame(3)),
            Packet::Removed(RemoveReason::Kicked),
            Packet::Removed(RemoveReason::Banned),
            Packet::Removed(RemoveReason::Ended),
            Packet::Removed(RemoveReason::Restarting),
            Packet::State {
                sequence: 0xDEAD_BEEF,
                state: state(3),
            },
            full_snapshot(),
            Packet::Snapshot {
                tick: 0,
                present: 0b1010,
                players: vec![],
            },
            Packet::Bye,
            Packet::Alive {
                sequence: 9,
                away: false,
            },
            Packet::Alive {
                sequence: u32::MAX,
                away: true,
            },
            Packet::OfferWorld(Box::new(world(4))),
            Packet::World(Box::new(world(5))),
            Packet::Ping { id: 0x0102_0304 },
            Packet::Pong { id: u32::MAX },
            Packet::Hits(vec![]),
            Packet::Hits(vec![
                Hit {
                    target: id(3),
                    kind: HitKind::Punch,
                    id: 1,
                },
                Hit {
                    target: id(15),
                    kind: HitKind::Trip,
                    id: u16::MAX,
                },
            ]),
            Packet::Hurts(vec![Hurt {
                attacker: id(0),
                kind: HitKind::Kick,
                id: 7,
            }]),
            Packet::Objects {
                level: 16,
                area: 4,
                sequence: 0x0102_0304,
                gone: set_of(&[2, 200]),
                gone_children: vec![0x8000 | (2 << 7) | 5],
                states: vec![object(1), object(240), object(0x8000 | (3 << 7))],
            },
            full_object_updates(),
            Packet::Progress(Box::new(Progress::from_bytes([1; PROGRESS_SIZE]))),
            Packet::SessionProgress(Box::default()),
            Packet::Coins(vec![coin(0, 1), coin(240, 5)]),
            Packet::CoinsCollected(vec![coin(0x8000 | (3 << 7) | 9, 2), coin(5, 0)]),
            Packet::Acks(vec![]),
            Packet::Acks(
                Stream::ALL
                    .into_iter()
                    .map(|stream| Ack {
                        stream,
                        id: u16::MAX,
                    })
                    .collect(),
            ),
        ]
    }

    #[test]
    fn every_packet_survives_a_round_trip() {
        for packet in every_packet() {
            assert_eq!(Packet::decode(&packet.encode()), Ok(packet));
        }
    }

    /// Bytes that differ from run to run of the generator, never from run to run of the test.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> usize {
            // xorshift64: enough to scatter changes over a packet.
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            usize::try_from(self.0 >> 16).unwrap_or_default()
        }
    }

    /// What a server or a bridge may be sent by anyone: packets changed at random. Each must
    /// be refused or read as a packet that is itself sound, and none may stop the program.
    #[test]
    fn damaged_packets_are_refused_or_read_as_sound_ones() {
        let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
        for packet in every_packet() {
            let encoded = packet.encode();
            for _ in 0..3000 {
                let mut damaged = encoded.clone();
                for _ in 0..=noise.next() % 4 {
                    let at = noise.next() % damaged.len();
                    if let Some(byte) = damaged.get_mut(at) {
                        *byte = u8::try_from(noise.next() % 256).unwrap_or_default();
                    }
                }
                // Now and then shorter or longer, as well as changed.
                match noise.next() % 8 {
                    0 => damaged.truncate(noise.next() % damaged.len()),
                    1 => damaged.extend(std::iter::repeat_n(0xA5, noise.next() % 64)),
                    _ => {}
                }
                if let Ok(read) = Packet::decode(&damaged) {
                    assert_eq!(Packet::decode(&read.encode()), Ok(read));
                }
            }
        }
    }

    #[test]
    fn names_rosters_and_chat_survive_a_round_trip() {
        let packets = [
            Packet::Introduce {
                name: Name::new("Luigi"),
                outfit: Outfit::named("green"),
                host_password: "open sesame".to_owned(),
            },
            Packet::Introduce {
                name: None,
                outfit: None,
                host_password: String::new(),
            },
            Packet::Roster(vec![
                roster_entry(0, Role::Host, "Mario"),
                roster_entry(15, Role::Player, "Player 15"),
            ]),
            Packet::Wear(Outfit::of_player(id(9))),
            Packet::Say {
                id: 7,
                text: message("/kick Luigi"),
            },
            Packet::Said(vec![
                ChatLine {
                    id: 1,
                    from: Some(id(3)),
                    text: message("hello there"),
                },
                ChatLine {
                    id: 2,
                    from: None,
                    text: message("Luigi joined"),
                },
            ]),
        ];
        for packet in packets {
            assert_eq!(Packet::decode(&packet.encode()), Ok(packet));
        }
    }

    #[test]
    fn session_size_matches_the_player_limit() {
        assert_eq!(usize::from(super::MAX_PLAYERS), super::SESSION_SIZE);
    }

    #[test]
    fn the_largest_packets_fit_one_datagram() {
        // MAX_PACKET_SIZE sizes receive buffers: the larger of the two packets that can be
        // full, and no other packet may be larger.
        let snapshot = full_snapshot().encode().len();
        let objects = full_object_updates().encode().len();
        assert_eq!(snapshot.max(objects), MAX_PACKET_SIZE);
        assert_eq!(snapshot, super::FULL_SNAPSHOT_SIZE);
        assert_eq!(objects, super::FULL_OBJECT_UPDATES_SIZE);
        assert!(Packet::World(Box::new(world(0))).encode().len() <= MAX_PACKET_SIZE);
        // Under the typical internet MTU minus IP and UDP headers, so it is never fragmented.
        assert!(snapshot.max(objects) <= 1500 - 48);
    }

    #[test]
    fn a_challenge_is_no_larger_than_the_hello_it_answers() {
        let empty = Packet::Hello {
            cookie: 0,
            game: 0,
            handshake: vec![],
        };
        let hello = empty.encode().len();
        let challenge = Packet::Challenge {
            cookie: 1,
            salt: [0; 16],
        };
        assert!(challenge.encode().len() <= hello);
        // The server's other answers to a stranger are no larger either.
        let rejected = Packet::Rejected(RejectReason::WrongPassword);
        assert!(rejected.encode().len() <= hello);
    }

    #[test]
    fn hello_padding_must_be_zeros_and_seals_must_hold_a_tag() {
        let mut hello = Packet::Hello {
            cookie: 1,
            game: 1,
            handshake: vec![2; 3],
        }
        .encode();
        if let Some(last) = hello.last_mut() {
            *last = 1;
        }
        assert_eq!(Packet::decode(&hello), Err(DecodeError::Malformed));

        let tagless = Packet::Sealed {
            nonce: 0,
            sealed: vec![0; 15],
        };
        let decoded = Packet::decode(&tagless.encode());
        assert_eq!(decoded, Err(DecodeError::Malformed));
    }

    #[test]
    fn a_hello_names_its_game_after_the_cookie_and_cut_short_is_rejected() {
        let hello = Packet::Hello {
            cookie: 0x0102_0304_0506_0708,
            game: 3,
            handshake: vec![9; 48],
        };
        let encoded = hello.encode();
        assert_eq!(
            encoded.get(..16),
            Some(&b"B3NP\x1C\x01\x01\x02\x03\x04\x05\x06\x07\x08\x03\x30"[..])
        );
        // Longer than the padding reaches, so every byte of it is needed.
        for length in 5..encoded.len() {
            let truncated = encoded.get(..length).unwrap_or_default();
            assert!(Packet::decode(truncated).is_err(), "length {length}");
        }
        assert_eq!(Packet::decode(&encoded), Ok(hello));
    }

    #[test]
    fn the_largest_packet_sealed_fits_a_datagram_buffer() {
        let sealed = Packet::Sealed {
            nonce: u64::MAX,
            sealed: vec![0; MAX_PACKET_SIZE + super::secure::TAG_SIZE],
        };
        assert_eq!(sealed.encode().len(), super::MAX_DATAGRAM_SIZE);
    }

    #[test]
    fn state_layout_matches_the_documented_format() {
        let encoded = Packet::State {
            sequence: 0x0102_0304,
            state: state(9),
        }
        .encode();
        assert_eq!(
            encoded.get(..10),
            Some(&b"B3NP\x1C\x04\x01\x02\x03\x04"[..])
        );
        assert_eq!(encoded.len(), 10 + PLAYER_STATE_SIZE);
    }

    #[test]
    fn foreign_and_future_packets_are_told_apart() {
        assert_eq!(Packet::decode(b"GET / HTTP/1.1"), Err(DecodeError::NotOurs));
        assert_eq!(Packet::decode(b""), Err(DecodeError::NotOurs));
        assert_eq!(
            Packet::decode(b"B3NP\x63\x01"),
            Err(DecodeError::WrongVersion(99))
        );
    }

    #[test]
    fn every_truncation_and_extension_is_rejected() {
        for packet in [
            Packet::Welcome { player: id(1) },
            Packet::State {
                sequence: 1,
                state: state(1),
            },
            full_snapshot(),
            Packet::World(Box::new(world(1))),
            Packet::Pong { id: 1 },
            Packet::Alive {
                sequence: 1,
                away: false,
            },
            Packet::Hurts(vec![Hurt {
                attacker: id(2),
                kind: HitKind::Punch,
                id: 1,
            }]),
            full_object_updates(),
            // One reason has a byte after it, and the others none.
            Packet::Rejected(RejectReason::Full),
            Packet::Rejected(RejectReason::WrongGame(2)),
        ] {
            let encoded = packet.encode();
            for length in 5..encoded.len() {
                let truncated = encoded.get(..length).unwrap_or_default();
                assert!(Packet::decode(truncated).is_err(), "length {length}");
            }
            let mut extended = encoded.clone();
            extended.push(0);
            assert_eq!(Packet::decode(&extended), Err(DecodeError::Malformed));
        }
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        assert_eq!(
            Packet::decode(b"B3NP\x1C\x02\x10"),
            Err(DecodeError::Malformed)
        );
        assert_eq!(
            Packet::decode(b"B3NP\x1C\x03\x05"),
            Err(DecodeError::Malformed)
        );
        assert_eq!(Packet::decode(b"B3NP\x1C\x63"), Err(DecodeError::Malformed));
        // Hits beyond the per-packet limit, an unknown kind, and a target outside the session.
        let mut nine_hits = b"B3NP\x1C\x0C\x09".to_vec();
        nine_hits.extend(std::iter::repeat_n([1, 1, 0, 1], 9).flatten());
        assert_eq!(Packet::decode(&nine_hits), Err(DecodeError::Malformed));
        assert_eq!(
            Packet::decode(b"B3NP\x1C\x0C\x01\x01\x04\0\x01"),
            Err(DecodeError::Malformed)
        );
        assert_eq!(
            Packet::decode(b"B3NP\x1C\x0D\x01\x10\x01\0\x01"),
            Err(DecodeError::Malformed)
        );
        // A snapshot can never list sixteen others.
        let mut too_many = b"B3NP\x1C\x05\0\0\0\0\xFF\xFF\x10".to_vec();
        too_many.extend(std::iter::repeat_n(0, 16 * (1 + 4 + PLAYER_STATE_SIZE)));
        assert_eq!(Packet::decode(&too_many), Err(DecodeError::Malformed));
        // A state for player 1, who the present mask says is not there.
        let mut absent = b"B3NP\x1C\x05\0\0\0\0\0\x01\x01\x01".to_vec();
        absent.extend(std::iter::repeat_n(0, 4 + PLAYER_STATE_SIZE));
        assert_eq!(Packet::decode(&absent), Err(DecodeError::Malformed));
    }

    #[test]
    fn object_states_must_name_objects_and_fit_the_limit() {
        let objects = |gone_children: &[u16], states: Vec<ObjectState>| {
            // Objects, level 16, area 1, send slot 5, no numbered object gone.
            let mut encoded = b"B3NP\x1C\x12\0\x10\x01\0\0\0\x05".to_vec();
            encoded.extend([0; 32]);
            encoded.push(u8::try_from(gone_children.len()).unwrap_or(0));
            for name in gone_children {
                encoded.extend(name.to_be_bytes());
            }
            encoded.push(u8::try_from(states.len()).unwrap_or(0));
            for state in &states {
                encoded.extend(state);
            }
            Packet::decode(&encoded)
        };
        assert!(objects(&[], vec![object(240)]).is_ok());
        assert_eq!(objects(&[], vec![object(0)]), Err(DecodeError::Malformed));
        assert_eq!(objects(&[], vec![object(241)]), Err(DecodeError::Malformed));
        let too_many = (1..=9).map(object).collect();
        assert_eq!(objects(&[], too_many), Err(DecodeError::Malformed));
        assert_eq!(MAX_OBJECTS_PER_PACKET, 8);
        // A gone child must name one, for a numbered spawner.
        assert!(objects(&[0x8000 | (7 << 7)], vec![]).is_ok());
        assert_eq!(objects(&[7], vec![]), Err(DecodeError::Malformed));
        assert_eq!(objects(&[0x8000], vec![]), Err(DecodeError::Malformed));
    }

    fn coin(name: u16, value: u8) -> Coin {
        Coin {
            level: 9,
            area: 1,
            name,
            value,
            id: 4,
        }
    }

    #[test]
    fn coins_must_be_worth_something_and_name_an_object_if_any() {
        let decode = |coin: Coin| Packet::decode(&Packet::Coins(vec![coin]).encode());
        assert!(decode(coin(0, MAX_COIN_VALUE)).is_ok());
        assert_eq!(decode(coin(0, 0)), Err(DecodeError::Malformed));
        let too_much = coin(0, MAX_COIN_VALUE + 1);
        assert_eq!(decode(too_much), Err(DecodeError::Malformed));
        assert_eq!(decode(coin(241, 1)), Err(DecodeError::Malformed));
        // A player's coin is always worth something. The server's may be worth nothing, as one
        // collected before the receiver came is, but then it must say which coin it is.
        assert_eq!(decode(coin(7, 0)), Err(DecodeError::Malformed));
        let told = |coin: Coin| Packet::decode(&Packet::CoinsCollected(vec![coin]).encode());
        assert!(told(coin(7, 0)).is_ok());
        assert_eq!(told(coin(0, 0)), Err(DecodeError::Malformed));
        let too_many = Packet::Coins(vec![coin(1, 1); 9]).encode();
        // The encoder keeps a packet decodable by sending eight.
        assert!(matches!(Packet::decode(&too_many), Ok(Packet::Coins(coins)) if coins.len() == 8));
    }

    #[test]
    fn interactions_must_name_objects_and_fit_the_limit() {
        let one = |object: u16| Interaction {
            level: 9,
            area: 1,
            object,
            status: 0x0000_C001,
            id: 3,
        };
        let packet = Packet::Interactions(vec![one(1), one(240)]);
        assert_eq!(Packet::decode(&packet.encode()), Ok(packet));
        let mut wrong = Packet::Interactions(vec![one(1)]).encode();
        // The object number is after the count, level and area.
        if let Some(object) = wrong.get_mut(10..12) {
            object.copy_from_slice(&241_u16.to_be_bytes());
        }
        assert_eq!(Packet::decode(&wrong), Err(DecodeError::Malformed));
        let mut too_many = b"B3NP\x1C\x14\x09".to_vec();
        too_many.extend(std::iter::repeat_n([0, 9, 1, 0, 1, 0, 0, 0xC0, 1, 0, 0], 9).flatten());
        assert_eq!(Packet::decode(&too_many), Err(DecodeError::Malformed));
    }
}
