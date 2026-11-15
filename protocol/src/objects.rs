//! Synced objects: how an object's state and a set of objects travel.
//!
//! Objects are named by their spawn-order number in their area, which every game gives the same
//! object (`TrollEngine/Multiplayer/objects.c`). An enemy that a numbered object spawned is named
//! for it: the high bit, the spawner's number, then seven bits of the enemy's behavior and
//! parameters. The server reads only an object's name and position; the rest passes through to the
//! games, which validate it.

/// The highest object number: the game numbers at most one object per pool slot.
pub const MAX_OBJECT_ID: u16 = 240;

/// The network part of the game's `struct MpObjectState`, from `id` on.
pub const OBJECT_STATE_SIZE: usize = 0x34;

/// One synced object's state, in the game's byte layout: its number first, then its position.
pub type ObjectState = [u8; OBJECT_STATE_SIZE];

/// The name of the object a state is about. Decoding ensures it is one; see `is_object_id`.
#[must_use]
pub fn object_id(state: &ObjectState) -> u16 {
    u16::from_be_bytes([state[0], state[1]])
}

/// Where the object is, at `pos` in `struct MpObjectState`, 4 bytes after `id`. It may be NaN.
#[must_use]
pub fn object_position(state: &ObjectState) -> [f32; 3] {
    let float = |at: usize| {
        let bytes = state.get(at..at + 4).and_then(|b| b.try_into().ok());
        bytes.map_or(f32::NAN, f32::from_be_bytes)
    };
    [float(4), float(8), float(12)]
}

/// The bit that marks a spawned enemy's name, and where its spawner's number sits in it.
const CHILD_NAME_BIT: u16 = 0x8000;
const CHILD_KEY_BITS: u16 = 7;

/// Whether `id` is the number of an object an area placed.
#[must_use]
pub const fn is_numbered(id: u16) -> bool {
    id >= 1 && id <= MAX_OBJECT_ID
}

/// Whether `id` names a spawned enemy: the spawner it names must be a numbered object.
#[must_use]
pub const fn is_child_name(id: u16) -> bool {
    id & CHILD_NAME_BIT != 0 && is_numbered(spawner_of(id))
}

/// The number in a spawned enemy's name: its spawner's.
const fn spawner_of(id: u16) -> u16 {
    (id >> CHILD_KEY_BITS) & 0xFF
}

/// Whether `id` names an object at all: a numbered one or a spawned enemy.
#[must_use]
pub const fn is_object_id(id: u16) -> bool {
    is_numbered(id) || is_child_name(id)
}

/// The numbered object whose owner runs the object `id`: itself, or a spawned enemy's spawner.
#[must_use]
pub const fn owning_number(id: u16) -> u16 {
    if is_child_name(id) {
        spawner_of(id)
    } else {
        id
    }
}

/// The size of an `ObjectSet` on the wire, enough for one bit per object number.
pub const OBJECT_SET_SIZE: usize = 32;
// Every number's byte, `id / 8`, is inside the set: 240 / 8 is under OBJECT_SET_SIZE.
const _: () = assert!(MAX_OBJECT_ID / 8 < 32);

/// A set of object numbers, one bit each: bit `id % 8` of byte `id / 8`, as the game's masks are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ObjectSet([u8; OBJECT_SET_SIZE]);

impl ObjectSet {
    /// Keeps only bits that name objects, so a set read from the network holds nothing else.
    #[must_use]
    pub fn from_bytes(bytes: [u8; OBJECT_SET_SIZE]) -> Self {
        let mut set = Self::default();
        for id in 1..=MAX_OBJECT_ID {
            if bit(&bytes, id) {
                set.insert(id);
            }
        }
        set
    }

    #[must_use]
    pub const fn bytes(&self) -> &[u8; OBJECT_SET_SIZE] {
        &self.0
    }

    #[must_use]
    pub fn contains(&self, id: u16) -> bool {
        is_numbered(id) && bit(&self.0, id)
    }

    /// Adds `id`, if it is an object's number.
    pub fn insert(&mut self, id: u16) {
        if let Some(byte) = self.0.get_mut(usize::from(id / 8))
            && is_numbered(id)
        {
            *byte |= 1 << (id % 8);
        }
    }

    pub fn remove(&mut self, id: u16) {
        if let Some(byte) = self.0.get_mut(usize::from(id / 8)) {
            *byte &= !(1 << (id % 8));
        }
    }

    pub fn ids(&self) -> impl Iterator<Item = u16> + '_ {
        (1..=MAX_OBJECT_ID).filter(|&id| self.contains(id))
    }
}

fn bit(bytes: &[u8; OBJECT_SET_SIZE], id: u16) -> bool {
    bytes
        .get(usize::from(id / 8))
        .is_some_and(|byte| byte >> (id % 8) & 1 == 1)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_OBJECT_ID, OBJECT_STATE_SIZE, ObjectSet, is_child_name, is_object_id, object_id,
        object_position, owning_number,
    };

    #[test]
    fn spawned_enemies_are_named_for_a_numbered_spawner() {
        // Spawner 5, key 0x23, as the game's name_children builds it.
        let name = 0x8000 | (5 << 7) | 0x23;
        assert!(is_child_name(name) && is_object_id(name));
        assert_eq!(owning_number(name), 5);
        assert_eq!(owning_number(17), 17);
        // A spawner number of 0, or above 240, names nothing.
        assert!(!is_object_id(0x8000 | 0x23));
        assert!(!is_object_id(0x8000 | (241 << 7)));
        assert!(!is_object_id(241));
        // Sets hold numbers only.
        let mut set = ObjectSet::default();
        set.insert(name);
        assert_eq!(set.ids().count(), 0);
    }

    #[test]
    fn sets_use_the_games_bit_order_and_hold_only_object_numbers() {
        let mut set = ObjectSet::default();
        set.insert(1);
        set.insert(9);
        set.insert(MAX_OBJECT_ID);
        set.insert(0);
        set.insert(MAX_OBJECT_ID + 1);
        assert_eq!(set.bytes()[0], 0b10);
        assert_eq!(set.bytes()[1], 0b10);
        assert_eq!(set.ids().collect::<Vec<_>>(), [1, 9, MAX_OBJECT_ID]);
        set.remove(9);
        assert!(!set.contains(9));

        let noisy = ObjectSet::from_bytes([0xFF; 32]);
        assert_eq!(noisy.ids().count(), usize::from(MAX_OBJECT_ID));
        assert!(!noisy.contains(0));
    }

    #[test]
    fn a_state_names_its_object_then_its_position() {
        let mut state = [0; OBJECT_STATE_SIZE];
        state[..2].copy_from_slice(&7_u16.to_be_bytes());
        state[4..8].copy_from_slice(&1.5_f32.to_be_bytes());
        state[12..16].copy_from_slice(&(-2.0_f32).to_be_bytes());
        assert_eq!(object_id(&state), 7);
        assert_eq!(object_position(&state), [1.5, 0.0, -2.0]);
    }
}
