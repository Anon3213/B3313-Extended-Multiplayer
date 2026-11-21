//! The RAM mailbox shared with the game; the layout is `TrollEngine/Multiplayer/mailbox.h`.
//!
//! Every address here is fixed. Data from other players only ever fills a whole `PlayerState`, so
//! it can never choose where a write lands.

use crate::layout::{self, byte, len};
use crate::ram::{N64Ram, ProcessMemory};
use crate::text::{self, NAME_BYTES, ROW_BYTES, TEXT_END};
use b3313_protocol::objects::{OBJECT_SET_SIZE, is_object_id, object_id};
use b3313_protocol::{
    MAX_LIVE_CHILDREN, MAX_OBJECT_ID, OBJECT_STATE_SIZE, ObjectSet, ObjectState, Outfit,
    PLAYER_STATE_SIZE, PROGRESS_SIZE, PlayerState, Progress, Role, WorldInputs,
};
use std::io;

// Every number below that the game's header states is taken from it, through `layout`, which
// `scripts/make_layout.py` writes from the header: none is kept here by hand.
const MAILBOX_ADDRESS: u32 = layout::MAILBOX_ADDR;
const MAGIC: u32 = layout::MAILBOX_MAGIC; // "B3MP"
/// The mailbox version this bridge speaks; a game marked with another is left alone.
pub const VERSION: u32 = layout::MAILBOX_VERSION;
const VERSION_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_VERSION_OFFSET;
/// `game`: which version of B3313 the game is, written by it at boot.
const GAME_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_GAME_OFFSET;
/// `bridgeLease`: the frames the game still takes its bridge to be here for, which it counts
/// down as it runs. At 0 it takes the session out of itself, as `end_session` does.
const BRIDGE_LEASE_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_BRIDGE_LEASE_OFFSET;
/// What the bridge sets the lease to each time it draws: about five seconds of the game.
const BRIDGE_LEASE: u32 = layout::BRIDGE_LEASE;
/// `hitsOut`: hits the local player landed, written by the game.
const HITS_OUT_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_HITS_OUT_OFFSET;
/// `hitsIn`: hits on the local player, written by the bridge.
const HITS_IN_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_HITS_IN_OFFSET;
/// Each ring holds its count, then this many entries: slot, kind and two spare bytes.
const HIT_ENTRIES: u32 = layout::HIT_ENTRIES;
const HIT_ENTRY_SIZE: u32 = layout::HIT_SIZE;
const LOCAL_PLAYER_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_LOCAL_OFFSET;
const REMOTE_PLAYERS_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_PLAYERS_OFFSET;
const PLAYER_SLOT_STRIDE: u32 = layout::PLAYER_SLOT_SIZE;
const SEQUENCE_OFFSET: u32 = layout::PLAYER_SLOT_SEQUENCE_OFFSET;
const LOCAL_WORLD_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_LOCAL_WORLD_OFFSET;
const SESSION_WORLD_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_SESSION_WORLD_OFFSET;
const WORLD_SEQUENCE_OFFSET: u32 = layout::WORLD_SEQUENCE_OFFSET;
/// `objectsOwned`: level, area, the owned and gone sets; guarded by the sequence after them.
const OBJECTS_OWNED_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_OBJECTS_OWNED_OFFSET;
const OBJECTS_OWNED_SIZE: usize = len::OBJECTS_OWNED_SEQUENCE_OFFSET;
const OBJECTS_OWNED_SEQUENCE_OFFSET: u32 = layout::OBJECTS_OWNED_SEQUENCE_OFFSET;
/// `objectsIn`: a ring of states from other owners, written by the bridge.
const OBJECTS_IN_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_OBJECTS_IN_OFFSET;
const OBJECTS_IN_ENTRIES: u32 = layout::OBJECTS_IN_ENTRIES;
/// `objectsOut`: the sequence, level, area, count and gone set, then the game's owned states.
const OBJECTS_OUT_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_OBJECTS_OUT_OFFSET;
const OBJECTS_OUT_HEADER_SIZE: usize = len::OBJECTS_OUT_STATES_OFFSET;
// These records are taken apart by their fields' places, written out where they are read:
// a header that moves one fails here, not in a game.
const _: () = assert!(
    len::OBJECT_STATE_LEVEL_NUM_OFFSET == 0
        && len::OBJECT_STATE_AREA_INDEX_OFFSET == 2
        && len::OBJECT_STATE_ID_OFFSET == 4
        && len::NAME_RING_NAMES_OFFSET == 4
        && len::LIVE_CHILDREN_NAMES_OFFSET == 4
);
const _: () = assert!(
    len::OBJECTS_OUT_SEQUENCE_OFFSET == 0
        && len::OBJECTS_OUT_LEVEL_NUM_OFFSET == 4
        && len::OBJECTS_OUT_AREA_INDEX_OFFSET == 6
        && len::OBJECTS_OUT_COUNT_OFFSET == 8
        && len::OBJECTS_OUT_GONE_OFFSET == 0xC
);
const _: () = assert!(
    len::INTERACTION_LEVEL_NUM_OFFSET == 0
        && len::INTERACTION_AREA_INDEX_OFFSET == 2
        && len::INTERACTION_ID_OFFSET == 4
        && len::INTERACTION_STATUS_OFFSET == 8
        && len::INTERACTION_RING_ENTRIES_OFFSET == 4
);
const _: () = assert!(
    len::COIN_LEVEL_NUM_OFFSET == 0
        && len::COIN_AREA_INDEX_OFFSET == 2
        && len::COIN_VALUE_OFFSET == 3
        && len::COIN_NAME_OFFSET == 4
        && len::COIN_RING_ENTRIES_OFFSET == 4
);
const OBJECTS_OUT_STATES_ADDRESS: u32 = OBJECTS_OUT_ADDRESS + layout::OBJECTS_OUT_STATES_OFFSET;
/// The game's `struct MpObjectState`: level, area and a spare byte, then the network part.
const MAILBOX_OBJECT_SIZE: usize = len::OBJECT_STATE_SIZE;
const MAILBOX_OBJECT_STRIDE: u32 = layout::OBJECT_STATE_SIZE;
const _: () = assert!(MAILBOX_OBJECT_SIZE == len::OBJECT_STATE_ID_OFFSET + OBJECT_STATE_SIZE);
/// `interactionsOut`, written by the game: a count, then entries of level, area, a spare byte,
/// object number, two spare bytes and status.
const INTERACTIONS_OUT_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_INTERACTIONS_OUT_OFFSET;
const INTERACTION_ENTRIES: u32 = layout::INTERACTION_ENTRIES;
const INTERACTION_ENTRY_SIZE: u32 = layout::INTERACTION_SIZE;
/// `childrenGoneOut`, written by the game: a count, then spawned enemies' names, two bytes each.
const CHILDREN_GONE_OUT_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_CHILDREN_GONE_OUT_OFFSET;
const NAME_ENTRIES: u32 = layout::NAME_ENTRIES;
const NAME_RING_SIZE: usize = len::NAME_RING_SIZE;
/// `liveChildren`, written by the bridge: level, area and a count, then that many spawned
/// enemies' names in room for `MAX_LIVE_CHILDREN`; guarded by the sequence after them.
const LIVE_CHILDREN_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_LIVE_CHILDREN_OFFSET;
const LIVE_CHILDREN_SIZE: usize = len::LIVE_CHILDREN_SEQUENCE_OFFSET;
const LIVE_CHILDREN_SEQUENCE_OFFSET: u32 = layout::LIVE_CHILDREN_SEQUENCE_OFFSET;
const _: () = assert!(LIVE_CHILDREN_SIZE == 4 + 2 * MAX_LIVE_CHILDREN);
const _: () = assert!(len::LIVE_CHILDREN == MAX_LIVE_CHILDREN);
/// In `liveChildren`'s count: there is no list, so the game removes no spawned enemy.
const LIVE_CHILDREN_UNKNOWN: u8 = byte::LIVE_UNKNOWN;
/// `progressOut`, written by the game, and `progressIn`, by the bridge: the network part of a
/// progress block, the save file played and a spare byte, then the sequence.
const PROGRESS_OUT_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_PROGRESS_OUT_OFFSET;
const PROGRESS_IN_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_PROGRESS_IN_OFFSET;
const PROGRESS_RECORD_SIZE: usize = len::PROGRESS_SEQUENCE_OFFSET;
const PROGRESS_SEQUENCE_OFFSET: u32 = layout::PROGRESS_SEQUENCE_OFFSET;
const _: () = assert!(PROGRESS_SIZE + 2 == PROGRESS_RECORD_SIZE);
const _: () = assert!(PROGRESS_SIZE == len::PROGRESS_SAVE_FILE_OFFSET);
/// `coinsOut`, written by the game, and `coinsIn`, by the bridge: each a count, then entries of
/// level, area, value, name and two spare bytes.
const COINS_OUT_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_COINS_OUT_OFFSET;
const COINS_IN_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_COINS_IN_OFFSET;
/// How many coins `coinsIn` holds; the bridge writes no more than that beyond what the game
/// has taken.
pub const COIN_ENTRIES: u32 = layout::COIN_ENTRIES;
const COIN_ENTRY_SIZE: u32 = layout::COIN_SIZE;
/// Where a coin ring's entries begin, after its count.
const COIN_RING_ENTRIES: u32 = layout::COIN_RING_ENTRIES_OFFSET;
/// `coinsInTaken`, written by the game: how many entries of `coinsIn` it has taken.
const COINS_IN_TAKEN_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_COINS_IN_TAKEN_OFFSET;
/// `people`, written by the bridge: the local player's role, each remote slot's role, three
/// spare bytes, the local player's name and each remote slot's, then the sequence.
const PEOPLE_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_PEOPLE_OFFSET;
const PEOPLE_RECORD_SIZE: usize = len::PEOPLE_SEQUENCE_OFFSET;
const PEOPLE_SEQUENCE_OFFSET: u32 = layout::PEOPLE_SEQUENCE_OFFSET;
const PEOPLE_LOCAL_NAME_OFFSET: usize = len::PEOPLE_LOCAL_NAME_OFFSET;
/// In a player's byte of `struct MpPeople`, beside the role in its low bits: that the high
/// four bits are the outfit they wear. Clear, as in a slot nobody is in, the game draws its own.
const PERSON_DRESSED: u8 = byte::PERSON_DRESSED;

/// A player as the game is told of them.
#[derive(Clone, Copy, Debug)]
pub struct Person<'a> {
    pub role: Role,
    pub outfit: Outfit,
    pub name: &'a str,
}
const PEOPLE_NAMES_OFFSET: usize = len::PEOPLE_NAMES_OFFSET;
/// The game has a slot more than there are remote players; its role and name stay empty.
const PEOPLE_SLOTS: usize = len::PLAYER_SLOTS;
const _: () = assert!(PEOPLE_NAMES_OFFSET + PEOPLE_SLOTS * NAME_BYTES == PEOPLE_RECORD_SIZE);
/// `chat`, written by the bridge: a count, how many of the newest rows to show, how many more
/// frames a line is being typed for, two spare bytes, a ring of rows, then the line being typed.
const CHAT_ADDRESS: u32 = MAILBOX_ADDRESS + layout::MAILBOX_CHAT_OFFSET;
/// Where `shown` is in it; `typing` is the byte after, and the two are written as one word.
const CHAT_SHOWN_OFFSET: u32 = layout::CHAT_SHOWN_OFFSET;
const _: () = assert!(layout::CHAT_TYPING_OFFSET == CHAT_SHOWN_OFFSET + 1);
const CHAT_ROWS_ADDRESS: u32 = CHAT_ADDRESS + layout::CHAT_ROWS_OFFSET;
const CHAT_ROWS: u32 = layout::CHAT_ROWS;
const CHAT_DRAFT_ADDRESS: u32 = CHAT_ADDRESS + layout::CHAT_DRAFT_OFFSET;
/// The most rows the game shows at once.
pub const CHAT_SHOWN_MOST: u8 = byte::CHAT_SHOWN_MOST;
/// How many of its frames the game takes a line to be typed for, after being told one is: it
/// counts them down, and at 0 gives the controller back to Mario.
const TYPING_LEASE: u8 = byte::TYPING_LEASE;
/// Slots for everyone in a session except the local player.
pub const REMOTE_SLOTS: u8 = byte::PUPPETS;
/// `REMOTE_SLOTS` as a length.
pub const REMOTE_SLOT_COUNT: usize = len::PUPPETS;
// What the game is given must be what the protocol and the font say it is.
const _: () = assert!(len::OBJECT_MASK_BYTES == OBJECT_SET_SIZE);
/// A torn read retries this many times before giving up until the next update.
const READ_ATTEMPTS: usize = 4;

const EMPTY_STATE: PlayerState = [0; PLAYER_STATE_SIZE];

/// A level and area, as the game numbers them.
pub type Place = (i16, u8);

/// A level no game is in: the game takes a record about a place only while it is there, so one
/// about this level tells it nothing.
const NO_LEVEL: i16 = layout::NO_LEVEL;

/// What the game published about its synced objects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectsOut {
    pub place: Place,
    /// Synced objects of the area that are gone in this game.
    pub gone: ObjectSet,
    /// The states of the objects this game owns.
    pub states: Vec<ObjectState>,
}

/// The first ring entry to read, given the `seen` entries already read and the writer's `count`,
/// in a ring of `capacity`. If more than the ring holds were written since, the oldest are lost.
fn first_unread(seen: u32, count: u32, capacity: u32) -> u32 {
    let behind = count.wrapping_sub(seen);
    if behind <= capacity {
        seen
    } else if behind < u32::MAX / 2 {
        // Older entries have been overwritten.
        count.wrapping_sub(capacity)
    } else {
        // The count went backwards, as when the game restarts: start again from here.
        count
    }
}

/// An interaction as the game's rings hold it: the place, the object's number and its status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RingInteraction {
    pub place: Place,
    pub object: u16,
    pub status: u32,
}

/// A coin as the game's rings hold it: where it was collected, its name or 0, and its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RingCoin {
    pub place: Place,
    pub name: u16,
    pub value: u8,
}

/// The game's `struct MpObjectState` for `state`, in `place`.
fn mailbox_object(place: Place, state: &ObjectState) -> [u8; MAILBOX_OBJECT_SIZE] {
    let mut entry = [0; MAILBOX_OBJECT_SIZE];
    let (level, area) = place;
    entry[..2].copy_from_slice(&level.to_be_bytes());
    entry[2] = area;
    entry[4..].copy_from_slice(state);
    entry
}

/// An index into the mailbox's remote player slots, always in range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteSlot(u8);

impl RemoteSlot {
    #[must_use]
    pub const fn new(index: u8) -> Option<Self> {
        if index < REMOTE_SLOTS {
            Some(Self(index))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn index(self) -> u8 {
        self.0
    }

    fn address(self) -> u32 {
        REMOTE_PLAYERS_ADDRESS + u32::from(self.0) * PLAYER_SLOT_STRIDE
    }
}

/// A game's mailbox, reached through its RAM.
#[derive(Debug)]
pub struct Mailbox<'a, M> {
    ram: N64Ram<'a, M>,
}

impl<'a, M: ProcessMemory> Mailbox<'a, M> {
    #[must_use]
    pub const fn new(ram: N64Ram<'a, M>) -> Self {
        Self { ram }
    }

    /// Reports whether a multiplayer build of the expected version is running; it marks itself at boot.
    ///
    /// Without this check, a ROM built without multiplayer would get writes into RAM it may use.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read, as when it has exited.
    pub fn is_ready(&self) -> io::Result<bool> {
        // The mark and the version after it, in one read: this is asked on every turn.
        const { assert!(VERSION_ADDRESS == MAILBOX_ADDRESS + 4) };
        let mut marked = [0; 8];
        self.ram.read(MAILBOX_ADDRESS, &mut marked)?;
        let (mark, version) = marked.split_at(4);
        Ok(mark == MAGIC.to_be_bytes() && version == VERSION.to_be_bytes())
    }

    /// The mailbox version the game marked itself with, or `None` for a game that has marked
    /// none: another ROM, or this one before it has started. For saying why a game is not
    /// ready, which `is_ready` only answers yes or no to.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read, as when it has exited.
    pub fn mailbox_version(&self) -> io::Result<Option<u32>> {
        if self.ram.read_u32(MAILBOX_ADDRESS)? != MAGIC {
            return Ok(None);
        }
        self.ram.read_u32(VERSION_ADDRESS).map(Some)
    }

    /// The version of B3313 the game is, as it marked itself at boot and `b3313_protocol::game`
    /// numbers them; 0, for not known, if no game is ready or it wrote no such number.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read, as when it has exited.
    pub fn game(&self) -> io::Result<u8> {
        // Only a ready game's mailbox has the field: an older layout has something else there.
        if !self.is_ready()? {
            return Ok(0);
        }
        Ok(u8::try_from(self.ram.read_u32(GAME_ADDRESS)?).unwrap_or(0))
    }

    /// Reads a record the game writes, guarded by the sequence number at `sequence_address`.
    ///
    /// Returns `None` if the game was writing it on every attempt; the next update will succeed.
    fn read_guarded<const N: usize>(
        &self,
        address: u32,
        sequence_address: u32,
    ) -> io::Result<Option<[u8; N]>> {
        for _ in 0..READ_ATTEMPTS {
            let before = self.ram.read_u32(sequence_address)?;
            let mut record = [0; N];
            self.ram.read(address, &mut record)?;
            if before % 2 == 0 && self.ram.read_u32(sequence_address)? == before {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    /// Writes a record the game reads, making its sequence number odd for the duration.
    fn write_guarded(&self, address: u32, sequence_address: u32, record: &[u8]) -> io::Result<()> {
        // Only the bridge writes these records, so the stored sequence is always even here.
        let writing = self.ram.read_u32(sequence_address)? | 1;
        self.ram.write(sequence_address, &writing.to_be_bytes())?;
        self.ram.write(address, record)?;
        self.ram
            .write(sequence_address, &writing.wrapping_add(1).to_be_bytes())
    }

    /// Reads the state this game publishes about its own Mario.
    ///
    /// Returns `None` until the game has published one, as before its first level, or if the
    /// game was writing it on every attempt; the next update will succeed.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn read_local_player(&self) -> io::Result<Option<PlayerState>> {
        let state: Option<PlayerState> =
            self.read_guarded(LOCAL_PLAYER_ADDRESS, LOCAL_PLAYER_ADDRESS + SEQUENCE_OFFSET)?;
        // The first byte is `active`: zero until the game has a Mario to tell of.
        Ok(state.filter(|state| state.first().is_some_and(|&active| active != 0)))
    }

    /// How many times the game has published its Mario, doubled: it moves on in every frame
    /// in which the game runs its objects, and stands still while it is paused, between two
    /// areas, or at a menu after play.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn local_player_sequence(&self) -> io::Result<u32> {
        self.ram.read_u32(LOCAL_PLAYER_ADDRESS + SEQUENCE_OFFSET)
    }

    /// Shows another player in this game.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_remote_player(&self, slot: RemoteSlot, state: &PlayerState) -> io::Result<()> {
        self.write_guarded(slot.address(), slot.address() + SEQUENCE_OFFSET, state)
    }

    /// Reads the world this game generated with before it had a session world, if it has one.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn read_local_world(&self) -> io::Result<Option<WorldInputs>> {
        let world: Option<WorldInputs> = self.read_guarded(
            LOCAL_WORLD_ADDRESS,
            LOCAL_WORLD_ADDRESS + WORLD_SEQUENCE_OFFSET,
        )?;
        // The first byte is `valid`: zero until the game has generated a level.
        Ok(world.filter(|world| world.first().is_some_and(|&valid| valid != 0)))
    }

    /// Gives the game the session's world, which it generates every later level from.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_session_world(&self, world: &WorldInputs) -> io::Result<()> {
        self.write_guarded(
            SESSION_WORLD_ADDRESS,
            SESSION_WORLD_ADDRESS + WORLD_SEQUENCE_OFFSET,
            world,
        )
    }

    /// Removes another player from this game.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn clear_remote_player(&self, slot: RemoteSlot) -> io::Result<()> {
        self.write_remote_player(slot, &EMPTY_STATE)
    }

    /// How many hits the game has written to `hitsOut` since it booted.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn hits_out_count(&self) -> io::Result<u32> {
        self.ram.read_u32(HITS_OUT_ADDRESS)
    }

    /// Reads the hits the game wrote after the first `seen`, and returns them as (remote slot index,
    /// kind code) with the new count. Unchecked: the caller validates both.
    ///
    /// The game writes an entry before raising the count, so every entry below it is complete. If
    /// more than the ring holds were written since, the oldest are lost.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn read_hits_out(&self, seen: u32) -> io::Result<(Vec<(u8, u8)>, u32)> {
        let count = self.hits_out_count()?;
        let first = first_unread(seen, count, HIT_ENTRIES);
        let mut hits = Vec::new();
        let mut index = first;
        while index != count {
            // Whole words: the emulator keeps N64 RAM byte-swapped in 4-byte units.
            let mut entry = [0; 4];
            let address = Self::hit_entry(HITS_OUT_ADDRESS, index);
            self.ram.read(address, &mut entry)?;
            hits.push((entry[0], entry[1]));
            index = index.wrapping_add(1);
        }
        Ok((hits, count))
    }

    /// How many hits the bridges have written to `hitsIn` since the game booted.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn hits_in_count(&self) -> io::Result<u32> {
        self.ram.read_u32(HITS_IN_ADDRESS)
    }

    /// Adds a hit on the local player by the player shown in `attacker`, after the `count` written
    /// so far, and returns the new count. The entry goes in before the count rises, so the game
    /// never reads a half-written one.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_hit_in(&self, count: u32, attacker: RemoteSlot, kind: u8) -> io::Result<u32> {
        let entry = Self::hit_entry(HITS_IN_ADDRESS, count);
        self.ram.write(entry, &[attacker.index(), kind, 0, 0])?;
        let count = count.wrapping_add(1);
        self.ram.write(HITS_IN_ADDRESS, &count.to_be_bytes())?;
        Ok(count)
    }

    /// The address of entry `index` of the ring at `ring`; the index wraps within the ring.
    fn hit_entry(ring: u32, index: u32) -> u32 {
        ring + layout::HIT_RING_ENTRIES_OFFSET + (index % HIT_ENTRIES) * HIT_ENTRY_SIZE
    }

    /// How many interactions the game has written to `interactionsOut` since it booted.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn interactions_out_count(&self) -> io::Result<u32> {
        self.ram.read_u32(INTERACTIONS_OUT_ADDRESS)
    }

    /// Reads the interactions the game wrote after the first `seen`, with the new count, as for
    /// hits. Unchecked: the server checks them, and the owner's game validates them again.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn read_interactions_out(&self, seen: u32) -> io::Result<(Vec<RingInteraction>, u32)> {
        let count = self.interactions_out_count()?;
        let mut index = first_unread(seen, count, INTERACTION_ENTRIES);
        let mut interactions = Vec::new();
        while index != count {
            let mut entry = [0; len::INTERACTION_SIZE];
            let address = INTERACTIONS_OUT_ADDRESS
                + layout::INTERACTION_RING_ENTRIES_OFFSET
                + (index % INTERACTION_ENTRIES) * INTERACTION_ENTRY_SIZE;
            self.ram.read(address, &mut entry)?;
            let [l0, l1, area, _, o0, o1, _, _, s0, s1, s2, s3] = entry;
            interactions.push(RingInteraction {
                place: (i16::from_be_bytes([l0, l1]), area),
                object: u16::from_be_bytes([o0, o1]),
                status: u32::from_be_bytes([s0, s1, s2, s3]),
            });
            index = index.wrapping_add(1);
        }
        Ok((interactions, count))
    }

    /// How many coins the game has reported collected in `coinsOut` since it booted.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn coins_out_count(&self) -> io::Result<u32> {
        self.ram.read_u32(COINS_OUT_ADDRESS)
    }

    /// Reads the coins the game wrote after the first `seen`, with the new count, as for hits.
    /// Unchecked: the protocol, the server and the other games check them.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn read_coins_out(&self, seen: u32) -> io::Result<(Vec<RingCoin>, u32)> {
        let count = self.coins_out_count()?;
        let mut index = first_unread(seen, count, COIN_ENTRIES);
        let mut coins = Vec::new();
        while index != count {
            let mut entry = [0; len::COIN_SIZE];
            let address =
                COINS_OUT_ADDRESS + COIN_RING_ENTRIES + (index % COIN_ENTRIES) * COIN_ENTRY_SIZE;
            self.ram.read(address, &mut entry)?;
            let [l0, l1, area, value, n0, n1, _, _] = entry;
            coins.push(RingCoin {
                place: (i16::from_be_bytes([l0, l1]), area),
                name: u16::from_be_bytes([n0, n1]),
                value,
            });
            index = index.wrapping_add(1);
        }
        Ok((coins, count))
    }

    /// How many coins the bridges have written to `coinsIn` since the game booted.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn coins_in_count(&self) -> io::Result<u32> {
        self.ram.read_u32(COINS_IN_ADDRESS)
    }

    /// How many entries of `coinsIn` the game has taken, as of the last frame it ran.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn coins_in_taken(&self) -> io::Result<u32> {
        self.ram.read_u32(COINS_IN_TAKEN_ADDRESS)
    }

    /// Adds a coin another player collected, after the `count` written so far, and returns the
    /// new count; the entry goes in before the count rises.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_coin_in(&self, count: u32, coin: &RingCoin) -> io::Result<u32> {
        let address =
            COINS_IN_ADDRESS + COIN_RING_ENTRIES + (count % COIN_ENTRIES) * COIN_ENTRY_SIZE;
        let (level, area) = coin.place;
        let [l0, l1] = level.to_be_bytes();
        let [n0, n1] = coin.name.to_be_bytes();
        self.ram
            .write(address, &[l0, l1, area, coin.value, n0, n1, 0, 0])?;
        let count = count.wrapping_add(1);
        self.ram.write(COINS_IN_ADDRESS, &count.to_be_bytes())?;
        Ok(count)
    }

    /// Reads the progress of the save file the game is playing, with the file's number, 1 to 4.
    ///
    /// Returns `None` outside a level, or if the game was writing it on every attempt.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn read_progress_out(&self) -> io::Result<Option<(u8, Progress)>> {
        let record: Option<[u8; PROGRESS_RECORD_SIZE]> = self.read_guarded(
            PROGRESS_OUT_ADDRESS,
            PROGRESS_OUT_ADDRESS + PROGRESS_SEQUENCE_OFFSET,
        )?;
        Ok(record.and_then(|record| {
            let (block, rest) = record.split_first_chunk::<PROGRESS_SIZE>()?;
            let &save_file = rest.first()?;
            (save_file != 0).then(|| (save_file, Progress::from_bytes(*block)))
        }))
    }

    /// Gives the game the session's progress, to add to its save.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_session_progress(&self, progress: &Progress) -> io::Result<()> {
        let mut record = [0; PROGRESS_RECORD_SIZE];
        if let Some(block) = record.first_chunk_mut::<PROGRESS_SIZE>() {
            *block = *progress.bytes();
        }
        self.write_guarded(
            PROGRESS_IN_ADDRESS,
            PROGRESS_IN_ADDRESS + PROGRESS_SEQUENCE_OFFSET,
            &record,
        )
    }

    /// Tells the game who the players are: the local player, and the player in each remote
    /// slot, for the names it draws, its player list, and the colors each Mario wears.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_people(
        &self,
        local: &Person<'_>,
        others: &[(RemoteSlot, Person<'_>)],
    ) -> io::Result<()> {
        // A slot nobody is in has no name: only the end code, as every byte of a name's field
        // after its characters is.
        let mut record = [TEXT_END; PEOPLE_RECORD_SIZE];
        let mut put = |who_at: usize, name_at: usize, person: &Person<'_>| {
            if let Some(byte) = record.get_mut(who_at) {
                let outfit = person.outfit.code() << byte::PERSON_OUTFIT_SHIFT;
                *byte = person.role.code() | PERSON_DRESSED | outfit;
            }
            if let Some(field) = record.get_mut(name_at..name_at + NAME_BYTES) {
                field.copy_from_slice(&text::field::<NAME_BYTES>(person.name));
            }
        };
        put(0, PEOPLE_LOCAL_NAME_OFFSET, local);
        for (slot, person) in others {
            let index = usize::from(slot.index());
            put(1 + index, PEOPLE_NAMES_OFFSET + index * NAME_BYTES, person);
        }
        self.write_guarded(
            PEOPLE_ADDRESS,
            PEOPLE_ADDRESS + PEOPLE_SEQUENCE_OFFSET,
            &record,
        )
    }

    /// Tells the game its bridge is still here. Call it as often as players are drawn: a game
    /// left without it for `BRIDGE_LEASE` of its frames takes the bridge for gone, as one that
    /// was killed is, and removes the session from itself.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn renew_lease(&self) -> io::Result<()> {
        self.ram
            .write(BRIDGE_LEASE_ADDRESS, &BRIDGE_LEASE.to_be_bytes())
    }

    /// Takes back what a session gave the game to act on by itself, for a game that plays on
    /// without one: the chat goes from the screen, nobody has a name, a role or an outfit, and
    /// no object is another game's to run. With the last word on that left in place, the game
    /// would go on removing the enemies it took for other games', in the area it was about.
    ///
    /// `parting` is rows of chat to leave on the screen, which say why the session ended to a
    /// player who is looking at the game and not at the bridge. With no bridge left to take
    /// them off, the game is left to: it does when the bridge's lease runs out.
    ///
    /// `keys_held` says the player was typing a line as the session ended, or had just sent
    /// one, as the host who types `/end` has: a key may still be down that the game would
    /// take for a button, so the keys are kept from it for one lease more, which the game
    /// counts down by itself.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read or written.
    pub fn end_session(&self, parting: &[[u8; ROW_BYTES]], keys_held: bool) -> io::Result<()> {
        self.write_chat_draft(&[TEXT_END; ROW_BYTES])?;
        if parting.is_empty() {
            // No bridge is here now, so the game has nothing to wait out.
            self.ram.write(BRIDGE_LEASE_ADDRESS, &0_u32.to_be_bytes())?;
            self.show_chat(0, keys_held)?;
        } else {
            let mut count = self.chat_count()?;
            for row in parting {
                count = self.write_chat_row(count, row)?;
            }
            let rows = u8::try_from(parting.len()).unwrap_or(u8::MAX);
            self.show_chat(rows, keys_held)?;
            self.renew_lease()?;
        }
        self.write_guarded(
            PEOPLE_ADDRESS,
            PEOPLE_ADDRESS + PEOPLE_SEQUENCE_OFFSET,
            &[TEXT_END; PEOPLE_RECORD_SIZE],
        )?;
        // The session's progress goes with it: the save played has it already, and another
        // file opened later must not be given a finished session's stars. The game takes a
        // sequence of nothing for no record at all.
        self.ram.write(
            PROGRESS_IN_ADDRESS + PROGRESS_SEQUENCE_OFFSET,
            &0_u32.to_be_bytes(),
        )?;
        self.run_every_object()
    }

    /// Has the game run every object itself, as it does with no session: no object is
    /// another game's, wherever the game is.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn run_every_object(&self) -> io::Result<()> {
        let nothing = ObjectSet::default();
        self.write_objects_owned((NO_LEVEL, 0), &nothing, &nothing)
    }

    /// How many chat rows the bridges have written to `chat` since the game booted.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn chat_count(&self) -> io::Result<u32> {
        self.ram.read_u32(CHAT_ADDRESS)
    }

    /// Adds a row of chat after the `count` written so far, and returns the new count.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_chat_row(&self, count: u32, row: &[u8; ROW_BYTES]) -> io::Result<u32> {
        let stride = u32::try_from(ROW_BYTES).map_err(io::Error::other)?;
        self.ram
            .write(CHAT_ROWS_ADDRESS + (count % CHAT_ROWS) * stride, row)?;
        let count = count.wrapping_add(1);
        // Last, so the game never draws a half-written row.
        self.ram.write(CHAT_ADDRESS, &count.to_be_bytes())?;
        Ok(count)
    }

    /// Writes `row` over the newest row of chat, the last of the `count` written so far: for a
    /// row that counts something down, which would otherwise push the others off as it went.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn rewrite_chat_row(&self, count: u32, row: &[u8; ROW_BYTES]) -> io::Result<()> {
        let stride = u32::try_from(ROW_BYTES).map_err(io::Error::other)?;
        let newest = count.wrapping_sub(1) % CHAT_ROWS;
        self.ram.write(CHAT_ROWS_ADDRESS + newest * stride, row)
    }

    /// Has the game draw the newest `rows` rows of the chat, or none. If `typing`, it also
    /// draws the line being typed under them, all in a box, and shows Mario no key or button,
    /// for the next `TYPING_LEASE` of its frames: so this is called again and again while a
    /// line is typed, and a bridge that stops calling it has stopped typing.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn show_chat(&self, rows: u8, typing: bool) -> io::Result<()> {
        let lease = if typing { TYPING_LEASE } else { 0 };
        // A whole word, as every write is: the two, and the two spare bytes after them.
        self.ram.write(
            CHAT_ADDRESS + CHAT_SHOWN_OFFSET,
            &[rows.min(CHAT_SHOWN_MOST), lease, 0, 0],
        )
    }

    /// Gives the game the line being typed, as the row it draws; an empty one for none, which
    /// has the game draw no box, though it may still be keeping the keys from Mario.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_chat_draft(&self, row: &[u8; ROW_BYTES]) -> io::Result<()> {
        self.ram.write(CHAT_DRAFT_ADDRESS, row)
    }

    /// How many spawned enemies the game has reported gone in `childrenGoneOut` since it booted.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn children_gone_out_count(&self) -> io::Result<u32> {
        self.ram.read_u32(CHILDREN_GONE_OUT_ADDRESS)
    }

    /// Reads the names the game wrote to `childrenGoneOut` after the first `seen`, with the new
    /// count, as for hits. Unchecked: the protocol and the other games check them.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn read_children_gone_out(&self, seen: u32) -> io::Result<(Vec<u16>, u32)> {
        // The whole ring at once: names are half words, and RAM is read in whole ones.
        let mut ring = [0; NAME_RING_SIZE];
        self.ram.read(CHILDREN_GONE_OUT_ADDRESS, &mut ring)?;
        let [c0, c1, c2, c3, names @ ..] = ring;
        let count = u32::from_be_bytes([c0, c1, c2, c3]);
        let mut index = first_unread(seen, count, NAME_ENTRIES);
        let mut gone = Vec::new();
        while index != count {
            let at = usize::try_from(index % NAME_ENTRIES).unwrap_or(0) * 2;
            if let Some(&[high, low]) = names.get(at..at + 2) {
                gone.push(u16::from_be_bytes([high, low]));
            }
            index = index.wrapping_add(1);
        }
        Ok((gone, count))
    }

    /// Tells the game which spawned enemies are alive in `place`, as the games that run them
    /// last said, so it removes its copies of any others; or, with no list, that it is not
    /// known, so it removes none.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_live_children(&self, place: Place, children: Option<&[u16]>) -> io::Result<()> {
        let mut record = [0; LIVE_CHILDREN_SIZE];
        let (level, area) = place;
        record[..2].copy_from_slice(&level.to_be_bytes());
        record[2] = area;
        // A list longer than the game has room for is no list.
        let listed = children.and_then(|names| Some((u8::try_from(names.len()).ok()?, names)));
        match listed.filter(|(_, names)| names.len() <= MAX_LIVE_CHILDREN) {
            Some((count, names)) => {
                record[3] = count;
                for (field, name) in record[4..].as_chunks_mut::<2>().0.iter_mut().zip(names) {
                    *field = name.to_be_bytes();
                }
            }
            None => record[3] = LIVE_CHILDREN_UNKNOWN,
        }
        self.write_guarded(
            LIVE_CHILDREN_ADDRESS,
            LIVE_CHILDREN_ADDRESS + LIVE_CHILDREN_SEQUENCE_OFFSET,
            &record,
        )
    }

    /// Tells the game which objects of `place` it runs for everyone, and which are gone.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_objects_owned(
        &self,
        place: Place,
        owned: &ObjectSet,
        gone: &ObjectSet,
    ) -> io::Result<()> {
        let mut record = [0; OBJECTS_OWNED_SIZE];
        let (level, area) = place;
        record[..2].copy_from_slice(&level.to_be_bytes());
        record[2] = area;
        // The two sets follow the level, area and spare byte.
        let sets = owned.bytes().iter().chain(gone.bytes());
        for (byte, &set_byte) in record.iter_mut().skip(4).zip(sets) {
            *byte = set_byte;
        }
        self.write_guarded(
            OBJECTS_OWNED_ADDRESS,
            OBJECTS_OWNED_ADDRESS + OBJECTS_OWNED_SEQUENCE_OFFSET,
            &record,
        )
    }

    /// Reads what the game published about its synced objects.
    ///
    /// Returns `None` if the game was writing it on every attempt; the next update will succeed.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn read_objects_out(&self) -> io::Result<Option<ObjectsOut>> {
        for _ in 0..READ_ATTEMPTS {
            let mut header = [0; OBJECTS_OUT_HEADER_SIZE];
            self.ram.read(OBJECTS_OUT_ADDRESS, &mut header)?;
            let before = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
            let place = (i16::from_be_bytes([header[4], header[5]]), header[6]);
            // The game never publishes more than one state per object number.
            let count = u16::from_be_bytes([header[8], header[9]]).min(MAX_OBJECT_ID);
            let mut entries = vec![0; usize::from(count) * MAILBOX_OBJECT_SIZE];
            if count > 0 {
                self.ram.read(OBJECTS_OUT_STATES_ADDRESS, &mut entries)?;
            }
            if before % 2 != 0 || self.ram.read_u32(OBJECTS_OUT_ADDRESS)? != before {
                continue;
            }
            let mut gone = [0; OBJECT_SET_SIZE];
            gone.copy_from_slice(&header[0xC..0xC + OBJECT_SET_SIZE]);
            // Each entry ends with the network part, after the level, area and spare byte.
            let states = entries
                .as_chunks::<MAILBOX_OBJECT_SIZE>()
                .0
                .iter()
                .filter_map(|entry| entry.last_chunk::<OBJECT_STATE_SIZE>().copied())
                .filter(|state| is_object_id(object_id(state)))
                .collect();
            return Ok(Some(ObjectsOut {
                place,
                gone: ObjectSet::from_bytes(gone),
                states,
            }));
        }
        Ok(None)
    }

    /// How many states the bridges have written to `objectsIn` since the game booted.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn objects_in_count(&self) -> io::Result<u32> {
        self.ram.read_u32(OBJECTS_IN_ADDRESS)
    }

    /// Adds other owners' states for objects in `place`, after the `count` written so far, and
    /// returns the new count. The entries go in before the count rises, as for hits, and the
    /// count rises once for them all: writing it after each cost a system call a state, sixty
    /// times a second.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn write_objects_in(
        &self,
        mut count: u32,
        place: Place,
        states: &[ObjectState],
    ) -> io::Result<u32> {
        if states.is_empty() {
            return Ok(count);
        }
        for state in states {
            let entry = OBJECTS_IN_ADDRESS
                + layout::OBJECTS_IN_ENTRIES_OFFSET
                + (count % OBJECTS_IN_ENTRIES) * MAILBOX_OBJECT_STRIDE;
            self.ram.write(entry, &mailbox_object(place, state))?;
            count = count.wrapping_add(1);
        }
        self.ram.write(OBJECTS_IN_ADDRESS, &count.to_be_bytes())?;
        Ok(count)
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::{LOCAL_PLAYER_ADDRESS, MAGIC, SEQUENCE_OFFSET, VERSION};
    use crate::layout::{self, len};
    use crate::ram::N64Ram;
    use crate::ram::testing::FakeProcess;
    use b3313_protocol::PlayerState;
    use std::io;

    /// Where the N64's RAM begins, as the game addresses it.
    const RAM_START: usize = 0x8000_0000;
    pub const MAILBOX_OFFSET: usize = len::MAILBOX_ADDR - RAM_START;
    /// Where the mailbox's room ends and the game's code begins.
    pub const MAILBOX_END: usize = len::CODE_ADDR - RAM_START;

    pub fn game() -> FakeProcess {
        FakeProcess::new(0, 0x0080_0000)
    }

    pub fn make_ready(process: &FakeProcess) -> io::Result<()> {
        let ram = N64Ram::new(process, 0);
        ram.write(layout::MAILBOX_ADDR, &MAGIC.to_be_bytes())?;
        let version = layout::MAILBOX_ADDR + layout::MAILBOX_VERSION_OFFSET;
        ram.write(version, &VERSION.to_be_bytes())
    }

    pub fn publish(process: &FakeProcess, state: &PlayerState, sequence: u32) -> io::Result<()> {
        let ram = N64Ram::new(process, 0);
        ram.write(LOCAL_PLAYER_ADDRESS, state)?;
        ram.write(
            LOCAL_PLAYER_ADDRESS + SEQUENCE_OFFSET,
            &sequence.to_be_bytes(),
        )
    }

    /// Where the writes made since the last call went, as offsets from the mailbox, in the
    /// order they were made.
    pub fn written(process: &FakeProcess) -> Vec<usize> {
        let offsets = process.written.take();
        offsets
            .into_iter()
            .map(|offset| offset.wrapping_sub(MAILBOX_OFFSET))
            .collect()
    }

    /// Reads big-endian bytes at an offset from the mailbox, independently of the code under test.
    pub fn raw(process: &FakeProcess, offset: usize, length: usize) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; length];
        let start = u32::try_from(MAILBOX_OFFSET + offset).map_err(io::Error::other)?;
        N64Ram::new(process, 0).read(0x8000_0000 + start, &mut bytes)?;
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{game, make_ready, publish, raw, written};
    use super::{
        Mailbox, ObjectsOut, Outfit, Person, Progress, RemoteSlot, RingCoin, Role, VERSION,
    };
    use crate::ram::N64Ram;
    use crate::ram::testing::FakeProcess;
    use b3313_protocol::{
        OBJECT_STATE_SIZE, ObjectSet, ObjectState, PLAYER_STATE_SIZE, WORLD_INPUTS_SIZE,
    };
    use std::io;

    fn mailbox(process: &FakeProcess) -> Mailbox<'_, FakeProcess> {
        Mailbox::new(N64Ram::new(process, 0))
    }

    /// Bytes that differ from call to call, never from run to run of the test.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> u64 {
            // xorshift64.
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn bytes<const N: usize>(&mut self) -> [u8; N] {
            std::array::from_fn(|_| self.next().to_le_bytes()[3])
        }

        fn below(&mut self, limit: u64) -> usize {
            usize::try_from(self.next() % limit).unwrap_or_default()
        }

        fn slot(&mut self) -> RemoteSlot {
            let index = self.bytes::<1>()[0] % super::REMOTE_SLOTS;
            RemoteSlot::new(index).unwrap_or_else(|| unreachable!("below the slot count"))
        }

        fn role(&mut self) -> Role {
            match self.below(3) {
                0 => Role::Player,
                1 => Role::Moderator,
                _ => Role::Host,
            }
        }

        fn person<'a>(&mut self, name: &'a str) -> Person<'a> {
            let outfit = Outfit::from_code(self.bytes::<1>()[0] % 16)
                .unwrap_or_else(|| unreachable!("below the outfit count"));
            Person {
                role: self.role(),
                outfit,
                name,
            }
        }

        fn place(&mut self) -> super::Place {
            (i16::from_le_bytes(self.bytes()), self.bytes::<1>()[0])
        }

        /// Text of any length up to far past a name's, of any bytes that make a string.
        fn text(&mut self) -> String {
            let length = self.below(80);
            let bytes: Vec<u8> = (0..length).map(|_| self.bytes::<1>()[0]).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    /// The rule the bridge's safety rests on: what the network sends fills mailbox fields and
    /// never chooses where a write goes. Whatever is written, with whatever counts, sizes and
    /// names, the game's memory outside the mailbox stays as it was.
    #[test]
    fn nothing_written_lands_outside_the_mailbox() -> io::Result<()> {
        const UNTOUCHED: u8 = 0xA5;
        let process = game();
        process.bytes.borrow_mut().fill(UNTOUCHED);
        let target = mailbox(&process);
        let mut noise = Noise(0x9E37_79B9_7F4A_7C15);
        for _ in 0..4000 {
            let count = u32::from_le_bytes(noise.bytes());
            let place = noise.place();
            match noise.below(13) {
                0 => target.write_remote_player(noise.slot(), &noise.bytes())?,
                1 => target.clear_remote_player(noise.slot())?,
                2 => target.write_session_world(&noise.bytes())?,
                3 => {
                    target.write_hit_in(count, noise.slot(), noise.bytes::<1>()[0])?;
                }
                4 => {
                    let coin = RingCoin {
                        place,
                        name: u16::from_le_bytes(noise.bytes()),
                        value: noise.bytes::<1>()[0],
                    };
                    target.write_coin_in(count, &coin)?;
                }
                5 => target.write_session_progress(&Progress::from_bytes(noise.bytes()))?,
                6 => {
                    let names: Vec<String> = (0..noise.below(24)).map(|_| noise.text()).collect();
                    let others: Vec<_> = names
                        .iter()
                        .map(|name| (noise.slot(), noise.person(name)))
                        .collect();
                    let mine = noise.text();
                    target.write_people(&noise.person(&mine), &others)?;
                }
                7 => {
                    target.write_chat_row(count, &noise.bytes())?;
                }
                8 => target.show_chat(noise.bytes::<1>()[0], noise.below(2) == 1)?,
                11 => target.write_chat_draft(&noise.bytes())?,
                9 => {
                    let children: Vec<u16> = (0..noise.below(300))
                        .map(|_| u16::from_le_bytes(noise.bytes()))
                        .collect();
                    let listed = (noise.below(4) > 0).then_some(children.as_slice());
                    target.write_live_children(place, listed)?;
                }
                10 => {
                    let owned = ObjectSet::from_bytes(noise.bytes());
                    let gone = ObjectSet::from_bytes(noise.bytes());
                    target.write_objects_owned(place, &owned, &gone)?;
                }
                _ => {
                    target.write_objects_in(count, place, &[noise.bytes()])?;
                }
            }
        }
        let memory = process.bytes.borrow();
        let (before, rest) = memory.split_at(super::testing::MAILBOX_OFFSET);
        let room = super::testing::MAILBOX_END - super::testing::MAILBOX_OFFSET;
        let (inside, after) = rest.split_at(room);
        assert!(before.iter().all(|&byte| byte == UNTOUCHED));
        assert!(after.iter().all(|&byte| byte == UNTOUCHED));
        // The writes did happen: this is not a test of writers that all failed.
        assert!(inside.iter().any(|&byte| byte != UNTOUCHED));
        Ok(())
    }

    #[test]
    fn slot_count_matches_the_slot_limit() {
        assert_eq!(usize::from(super::REMOTE_SLOTS), super::REMOTE_SLOT_COUNT);
    }

    #[test]
    fn slots_past_the_table_do_not_exist() {
        assert!(RemoteSlot::new(14).is_some());
        assert_eq!(RemoteSlot::new(15), None);
    }

    #[test]
    fn remote_slots_match_the_layout_the_game_reads() -> io::Result<()> {
        let process = game();

        mailbox(&process).write_remote_player(RemoteSlot(1), &[7; PLAYER_STATE_SIZE])?;

        // struct MpMailbox puts players at 0x200, and each slot is 0x80 bytes.
        assert_eq!(raw(&process, 0x27C, 4)?, [0; 4]);
        assert_eq!(
            raw(&process, 0x280, PLAYER_STATE_SIZE)?,
            [7; PLAYER_STATE_SIZE]
        );
        assert_eq!(raw(&process, 0x280 + PLAYER_STATE_SIZE, 4)?, [0; 4]);
        Ok(())
    }

    #[test]
    fn each_remote_write_leaves_the_sequence_even_and_advanced() -> io::Result<()> {
        let process = game();
        let target = mailbox(&process);

        written(&process);
        target.write_remote_player(RemoteSlot(0), &[1; PLAYER_STATE_SIZE])?;
        // The sequence first, made odd, then the record, then the sequence again: the game,
        // which may read at any moment between, takes nothing while the sequence is odd.
        assert_eq!(written(&process), [0x27C, 0x200, 0x27C]);
        assert_eq!(raw(&process, 0x27C, 4)?, 2_u32.to_be_bytes());
        target.clear_remote_player(RemoteSlot(0))?;
        assert_eq!(raw(&process, 0x27C, 4)?, 4_u32.to_be_bytes());
        assert_eq!(
            raw(&process, 0x200, PLAYER_STATE_SIZE)?,
            [0; PLAYER_STATE_SIZE]
        );
        Ok(())
    }

    #[test]
    fn the_session_world_lands_where_the_game_reads_it() -> io::Result<()> {
        let process = game();

        mailbox(&process).write_session_world(&[3; WORLD_INPUTS_SIZE])?;

        // struct MpMailbox puts sessionWorld at 0xB40, with its sequence 0x13C into it.
        assert_eq!(raw(&process, 0xB3C, 4)?, [0; 4]);
        assert_eq!(
            raw(&process, 0xB40, WORLD_INPUTS_SIZE)?,
            [3; WORLD_INPUTS_SIZE]
        );
        assert_eq!(raw(&process, 0xB40 + 0x13C, 4)?, 2_u32.to_be_bytes());
        Ok(())
    }

    #[test]
    fn a_local_world_is_read_only_once_the_game_has_one() -> io::Result<()> {
        let process = game();
        let ram = N64Ram::new(&process, 0);
        assert_eq!(mailbox(&process).read_local_world()?, None);

        // `valid` set, sequence even: struct MpMailbox puts localWorld at 0xA00.
        ram.write(0x807C_0C00, &[1; WORLD_INPUTS_SIZE])?;
        assert_eq!(
            mailbox(&process).read_local_world()?,
            Some([1; WORLD_INPUTS_SIZE])
        );

        ram.write(0x807C_0C00 + 0x13C, &1_u32.to_be_bytes())?;
        assert_eq!(mailbox(&process).read_local_world()?, None);
        Ok(())
    }

    #[test]
    fn a_consistent_local_player_is_read() -> io::Result<()> {
        let process = game();
        publish(&process, &[9; PLAYER_STATE_SIZE], 6)?;

        assert_eq!(
            mailbox(&process).read_local_player()?,
            Some([9; PLAYER_STATE_SIZE])
        );
        Ok(())
    }

    /// At the title screen: the bridge must not take the game for one in play, where the
    /// chat can be typed and a state is worth sending.
    #[test]
    fn a_game_that_has_published_no_mario_has_no_local_player() -> io::Result<()> {
        let process = game();

        assert_eq!(mailbox(&process).read_local_player()?, None);
        Ok(())
    }

    #[test]
    fn a_local_player_being_written_is_not_read() -> io::Result<()> {
        let process = game();
        publish(&process, &[9; PLAYER_STATE_SIZE], 7)?;

        assert_eq!(mailbox(&process).read_local_player()?, None);
        Ok(())
    }

    /// Writes hits as the game does: entry `index % 8` of `hitsOut` at 0x14, then the count at 0x10.
    fn game_writes_hits(process: &FakeProcess, hits: std::ops::Range<u32>) -> io::Result<()> {
        let ram = N64Ram::new(process, 0);
        for index in hits.clone() {
            let marker = u8::try_from(index).unwrap_or(0);
            ram.write(0x807C_0214 + (index % 8) * 4, &[marker, 1, 0, 0])?;
        }
        ram.write(0x807C_0210, &hits.end.to_be_bytes())
    }

    fn slots_of(hits: &[(u8, u8)]) -> Vec<u8> {
        hits.iter().map(|&(slot, _)| slot).collect()
    }

    #[test]
    fn new_hits_out_are_read_once_in_order() -> io::Result<()> {
        let process = game();
        game_writes_hits(&process, 0..3)?;
        let (hits, count) = mailbox(&process).read_hits_out(0)?;
        assert_eq!((hits, count), (vec![(0, 1), (1, 1), (2, 1)], 3));
        assert_eq!(mailbox(&process).read_hits_out(3)?, (vec![], 3));
        Ok(())
    }

    #[test]
    fn hits_out_beyond_the_ring_keep_the_newest_and_a_restart_resyncs() -> io::Result<()> {
        let process = game();
        game_writes_hits(&process, 0..11)?;
        let (hits, count) = mailbox(&process).read_hits_out(0)?;
        assert_eq!((slots_of(&hits), count), ((3..11).collect(), 11));

        // The game restarted and has written two since; the bridge had seen five before.
        game_writes_hits(&process, 0..2)?;
        assert_eq!(mailbox(&process).read_hits_out(5)?, (vec![], 2));
        Ok(())
    }

    #[test]
    fn hits_in_go_into_the_ring_before_the_count() -> io::Result<()> {
        let process = game();
        let target = mailbox(&process);
        let mut count = 0;
        for slot in 0..9 {
            written(&process);
            count = target.write_hit_in(count, RemoteSlot(slot), 2)?;
            // The count last: the game takes every entry the count covers.
            assert_eq!(written(&process).last(), Some(&0x40), "hit {slot}");
        }

        // struct MpMailbox puts hitsIn at 0x40: the count, then 4-byte entries.
        assert_eq!(raw(&process, 0x40, 4)?, 9_u32.to_be_bytes());
        // The ninth wrapped round to the first entry.
        assert_eq!(raw(&process, 0x44, 4)?, [8, 2, 0, 0]);
        assert_eq!(raw(&process, 0x48, 4)?, [1, 2, 0, 0]);
        assert_eq!(target.hits_in_count()?, 9);
        Ok(())
    }

    fn object(id: u16, marker: u8) -> ObjectState {
        let mut state = [marker; OBJECT_STATE_SIZE];
        state[..2].copy_from_slice(&id.to_be_bytes());
        state
    }

    #[test]
    fn ownership_lands_where_the_game_reads_it() -> io::Result<()> {
        let process = game();
        let (mut owned, mut gone) = (ObjectSet::default(), ObjectSet::default());
        owned.insert(9);
        gone.insert(16);

        mailbox(&process).write_objects_owned((-2, 3), &owned, &gone)?;

        // struct MpMailbox puts objectsOwned at 0xC80: level, area, pad, owned, gone, and the
        // sequence at 0x4C into it.
        assert_eq!(raw(&process, 0xC80, 4)?, [0xFF, 0xFE, 3, 0]);
        assert_eq!(raw(&process, 0xC84, 4)?, [0, 0b10, 0, 0]);
        assert_eq!(raw(&process, 0xCA4, 4)?, [0, 0, 1, 0]);
        assert_eq!(raw(&process, 0xCCC, 4)?, 2_u32.to_be_bytes());
        Ok(())
    }

    /// Writes `objectsOut` as the game does, at 0x1D80, under `sequence`.
    fn game_publishes_objects(
        process: &FakeProcess,
        sequence: u32,
        states: &[ObjectState],
    ) -> io::Result<()> {
        let ram = N64Ram::new(process, 0);
        let mut header = [0; 0x2C];
        header[..4].copy_from_slice(&sequence.to_be_bytes());
        header[4..8].copy_from_slice(&[0, 16, 1, 0]);
        let count = u16::try_from(states.len()).unwrap_or(0);
        header[8..10].copy_from_slice(&count.to_be_bytes());
        header[0xC] = 0b100; // Object 2 is gone.
        ram.write(0x807C_1F80, &header)?;
        for (index, state) in (0_u32..).zip(states) {
            let mut entry = vec![0, 16, 1, 0];
            entry.extend(state);
            ram.write(0x807C_1FAC + index * 0x38, &entry)?;
        }
        Ok(())
    }

    #[test]
    fn published_objects_are_read_whole_or_not_at_all() -> io::Result<()> {
        let process = game();
        game_publishes_objects(&process, 4, &[object(1, 7), object(5, 8)])?;
        let out = mailbox(&process).read_objects_out()?;
        let expected = ObjectsOut {
            place: (16, 1),
            gone: ObjectSet::from_bytes({
                let mut bytes = [0; 32];
                bytes[0] = 0b100;
                bytes
            }),
            states: vec![object(1, 7), object(5, 8)],
        };
        assert_eq!(out, Some(expected));

        game_publishes_objects(&process, 5, &[object(1, 7)])?;
        assert_eq!(mailbox(&process).read_objects_out()?, None);
        Ok(())
    }

    #[test]
    fn states_in_go_into_the_ring_before_the_count() -> io::Result<()> {
        let process = game();
        let target = mailbox(&process);
        let mut count = 0;
        for id in 1..=65 {
            written(&process);
            count = target.write_objects_in(count, (16, 1), &[object(id, 3)])?;
            // The count last: the game takes every entry the count covers.
            assert_eq!(written(&process).last(), Some(&0xD00), "object {id}");
        }

        // struct MpMailbox puts objectsIn at 0xD00: the count, then 0x38-byte entries, each the
        // level, area and a spare byte before the network part.
        assert_eq!(raw(&process, 0xD00, 4)?, 65_u32.to_be_bytes());
        // The 65th wrapped round to the first entry.
        assert_eq!(raw(&process, 0xD04, 8)?, [0, 16, 1, 0, 0, 65, 3, 3]);
        assert_eq!(raw(&process, 0xD04 + 0x38, 8)?, [0, 16, 1, 0, 0, 2, 3, 3]);
        assert_eq!(target.objects_in_count()?, 65);
        Ok(())
    }

    #[test]
    fn people_and_chat_land_where_the_game_reads_them() -> io::Result<()> {
        let process = game();
        let target = mailbox(&process);
        let third = RemoteSlot::new(2).ok_or_else(|| io::Error::other("slot 2"))?;
        let outfit = |name| Outfit::named(name).ok_or_else(|| io::Error::other("an outfit"));
        let al = Person {
            role: Role::Host,
            outfit: outfit("green")?,
            name: "Al",
        };
        let bo = Person {
            role: Role::Moderator,
            outfit: outfit("gray")?,
            name: "Bo",
        };
        target.write_people(&al, &[(third, bo)])?;

        // struct MpMailbox puts people at 0x5580: the local player's byte, sixteen slots'
        // bytes, three spare bytes, the local name at 0x14, the slots' names from 0x28, 20
        // bytes each, and the sequence at 0x168. A player's byte is their role, 8 to say they
        // are dressed, and their outfit in the high four bits: green is 1 and gray 15. Names
        // are plain characters, and a zero ends each.
        assert_eq!(raw(&process, 0x5580, 4)?, [0x1A, 0, 0, 0xF9]);
        assert_eq!(raw(&process, 0x5580 + 0x14, 4)?, [b'A', b'l', 0, 0]);
        assert_eq!(raw(&process, 0x5580 + 0x28, 4)?, [0; 4]);
        let third_name = raw(&process, 0x5580 + 0x28 + 2 * 20, 4)?;
        assert_eq!(third_name, [b'B', b'o', 0, 0]);
        assert_eq!(raw(&process, 0x5580 + 0x168, 4)?, 2_u32.to_be_bytes());

        // chat at 0x5700: the count, how many rows to show, the frames a line is typed for,
        // two spare bytes, then eight 52-byte rows, the ninth written over the first, and
        // after them, at 0x1A8, the line being typed.
        let mut count = target.chat_count()?;
        for row in 0..9_u8 {
            count = target.write_chat_row(count, &[row; 52])?;
        }
        target.show_chat(9, false)?;
        assert_eq!(raw(&process, 0x5700, 8)?, [0, 0, 0, 9, 6, 0, 0, 0]);
        assert_eq!(raw(&process, 0x5708, 4)?, [8; 4]);
        assert_eq!(raw(&process, 0x5708 + 52, 4)?, [1; 4]);

        target.show_chat(2, true)?;
        target.write_chat_draft(&[b'>'; 52])?;
        assert_eq!(raw(&process, 0x5704, 4)?, [2, 30, 0, 0]);
        assert_eq!(raw(&process, 0x5700 + 0x1A8, 52)?, [b'>'; 52]);
        // The mailbox ends with it.
        assert_eq!(raw(&process, 0x5700 + 0x1A8 + 52, 4)?, [0; 4]);
        Ok(())
    }

    #[test]
    fn coins_use_the_rings_the_game_reads_and_writes() -> io::Result<()> {
        let process = game();
        let ram = N64Ram::new(&process, 0);
        let target = mailbox(&process);
        // The game wrote one coin to coinsOut at 0x5380: level 9, area 1, worth 2, named 0x8185.
        ram.write(0x807C_5584, &[0, 9, 1, 2, 0x81, 0x85, 0, 0])?;
        ram.write(0x807C_5580, &1_u32.to_be_bytes())?;
        let coin = super::RingCoin {
            place: (9, 1),
            name: 0x8185,
            value: 2,
        };
        assert_eq!(target.read_coins_out(0)?, (vec![coin], 1));

        // coinsIn at 0x5480: the count, then the entry.
        let count = target.write_coin_in(0, &coin)?;
        assert_eq!(raw(&process, 0x5480, 4)?, count.to_be_bytes());
        assert_eq!(raw(&process, 0x5484, 8)?, [0, 9, 1, 2, 0x81, 0x85, 0, 0]);
        Ok(())
    }

    #[test]
    fn progress_is_read_and_written_where_the_game_keeps_it() -> io::Result<()> {
        let process = game();
        let ram = N64Ram::new(&process, 0);
        let target = mailbox(&process);
        // progressOut at 0x5280: no file played yet, so nothing to read.
        assert_eq!(target.read_progress_out()?, None);
        // The wing cap and one star, in file 2, sequence even.
        let mut record = [0; 0x48];
        record[..4].copy_from_slice(&2_u32.to_be_bytes());
        record[4] = 1;
        record[0x42] = 2;
        ram.write(0x807C_5480, &record)?;
        let (file, progress) = target.read_progress_out()?.unwrap_or_default();
        assert_eq!((file, progress.count()), (2, 2));

        // progressIn at 0x5300, with its sequence at 0x44 into it.
        target.write_session_progress(&progress)?;
        assert_eq!(raw(&process, 0x5300, 8)?, [0, 0, 0, 2, 1, 0, 0, 0]);
        assert_eq!(raw(&process, 0x5344, 4)?, 2_u32.to_be_bytes());
        Ok(())
    }

    #[test]
    fn spawned_enemies_gone_and_alive_are_where_the_game_writes_and_reads_them() -> io::Result<()> {
        let process = game();
        let ram = N64Ram::new(&process, 0);
        // The game wrote three names to childrenGoneOut at 0x1C80: entries 0 to 2.
        ram.write(0x807C_1E84, &[0x81, 0x91, 0x81, 0x92, 0x81, 0x93, 0, 0])?;
        ram.write(0x807C_1E80, &3_u32.to_be_bytes())?;
        let target = mailbox(&process);
        assert_eq!(target.read_children_gone_out(1)?, (vec![0x8192, 0x8193], 3));

        // liveChildren at 0x1D00: level, area and the count, the names, and the sequence at
        // 0x64 into it.
        target.write_live_children((-2, 3), Some(&[0x8201, 0x8202, 0x8203]))?;
        assert_eq!(raw(&process, 0x1D00, 4)?, [0xFF, 0xFE, 3, 3]);
        assert_eq!(
            raw(&process, 0x1D04, 8)?,
            [0x82, 0x01, 0x82, 0x02, 0x82, 0x03, 0, 0]
        );
        assert_eq!(raw(&process, 0x1D64, 4)?, 2_u32.to_be_bytes());
        // No list, and one longer than the game has room for, are both written as not known.
        target.write_live_children((16, 1), None)?;
        assert_eq!(raw(&process, 0x1D00, 4)?, [0, 16, 1, 0xFF]);
        target.write_live_children((16, 1), Some(&[]))?;
        assert_eq!(raw(&process, 0x1D00, 4)?, [0, 16, 1, 0]);
        target.write_live_children((16, 1), Some(&[0x8201; 49]))?;
        assert_eq!(raw(&process, 0x1D00, 4)?, [0, 16, 1, 0xFF]);
        Ok(())
    }

    #[test]
    fn interactions_are_read_from_the_ring_the_game_writes() -> io::Result<()> {
        let process = game();
        let ram = N64Ram::new(&process, 0);
        // The game wrote two to interactionsOut at 0x1B80: object 7 punched, object 9 kicked.
        ram.write(0x807C_1D84, &[0, 9, 1, 0, 0, 7, 0, 0, 0, 0, 0xC0, 1])?;
        ram.write(0x807C_1D90, &[0, 9, 1, 0, 0, 9, 0, 0, 0, 0, 0xC0, 2])?;
        ram.write(0x807C_1D80, &2_u32.to_be_bytes())?;
        let (read, count) = mailbox(&process).read_interactions_out(0)?;
        let objects: Vec<(u16, u32)> = read.iter().map(|i| (i.object, i.status)).collect();
        assert_eq!((objects, count), (vec![(7, 0xC001), (9, 0xC002)], 2));
        assert_eq!(read.first().map(|i| i.place), Some((9, 1)));
        Ok(())
    }

    #[test]
    fn a_different_mailbox_version_is_not_ready() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        assert!(mailbox(&process).is_ready()?);

        N64Ram::new(&process, 0).write(0x807C_0204, &(VERSION + 1).to_be_bytes())?;
        assert!(!mailbox(&process).is_ready()?);
        Ok(())
    }

    #[test]
    fn the_game_is_the_number_a_ready_game_wrote_and_otherwise_not_known() -> io::Result<()> {
        let process = game();
        let ram = N64Ram::new(&process, 0);
        // The game's number, at 0xC, before the game has marked the mailbox as its own.
        ram.write(0x807C_020C, &3_u32.to_be_bytes())?;
        assert_eq!(mailbox(&process).game()?, 0, "no game is ready");

        make_ready(&process)?;
        assert_eq!(mailbox(&process).game()?, 3);
        // A number no game has, which no byte of a packet could carry.
        ram.write(0x807C_020C, &0x0000_0103_u32.to_be_bytes())?;
        assert_eq!(mailbox(&process).game()?, 0);

        ram.write(0x807C_020C, &2_u32.to_be_bytes())?;
        ram.write(0x807C_0204, &(VERSION + 1).to_be_bytes())?;
        assert_eq!(mailbox(&process).game()?, 0, "another mailbox version");
        Ok(())
    }

    /// Where the bridge's numbers and the game's are the same thing kept twice, by the
    /// protocol and by the game's header, they must agree. Most such pairs are checked as the
    /// bridge is compiled, beside the constants; this one's types do not allow that.
    #[test]
    fn the_game_numbers_as_many_objects_as_the_protocol_names() {
        assert_eq!(
            crate::layout::OBJECT_IDS,
            u32::from(b3313_protocol::MAX_OBJECT_ID)
        );
    }
}
