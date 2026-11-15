//! Shared progress: the stars, keys, cap switches and doors players get in a session.
//!
//! A block is the network part of the game's `struct MpProgress`: a flags word, 58 bytes of star
//! flags, the key flags and B3313's beta keys. Each is a set of bits that only ever grows, so blocks
//! combine with OR, and what a player newly got is their block less what was known.

/// The network part of the game's `struct MpProgress`, through `betaKeys`.
pub const PROGRESS_SIZE: usize = 0x42;

/// The save flags that are progress, as `MP_SHARED_SAVE_FLAGS` in `Multiplayer/progress.c`. Any
/// other bit in a block's flags word is dropped.
const SHARED_SAVE_FLAGS: u32 = 0x1F10_FFFE;

/// One player's or the session's progress, in the game's byte layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress([u8; PROGRESS_SIZE]);

impl Default for Progress {
    fn default() -> Self {
        Self([0; PROGRESS_SIZE])
    }
}

impl Progress {
    /// Keeps only bits that are progress, so a block from the network holds nothing else.
    #[must_use]
    pub fn from_bytes(mut bytes: [u8; PROGRESS_SIZE]) -> Self {
        if let Some(flags) = bytes.first_chunk_mut::<4>() {
            let shared = u32::from_be_bytes(*flags) & SHARED_SAVE_FLAGS;
            *flags = shared.to_be_bytes();
        }
        Self(bytes)
    }

    #[must_use]
    pub const fn bytes(&self) -> &[u8; PROGRESS_SIZE] {
        &self.0
    }

    /// The bits of `self` that `known` lacks.
    #[must_use]
    pub fn without(&self, known: &Self) -> Self {
        let mut bytes = self.0;
        for (byte, &other) in bytes.iter_mut().zip(&known.0) {
            *byte &= !other;
        }
        Self(bytes)
    }

    /// Adds every bit of `other`.
    pub fn add(&mut self, other: &Self) {
        for (byte, &other) in self.0.iter_mut().zip(&other.0) {
            *byte |= other;
        }
    }

    /// The first `most` bits of `self`, or all of it if it has no more.
    #[must_use]
    pub fn first(&self, most: u32) -> Self {
        let mut left = most;
        let mut bytes = self.0;
        for byte in &mut bytes {
            while byte.count_ones() > left {
                // Drops the highest bit set.
                *byte &= !(0x80 >> byte.leading_zeros());
            }
            left -= byte.count_ones();
        }
        Self(bytes)
    }

    /// How many bits are set: a star, key, switch or door each.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.0.iter().map(|byte| byte.count_ones()).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|&byte| byte == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::{PROGRESS_SIZE, Progress};

    #[test]
    fn only_progress_flags_survive_and_bits_combine() {
        let mut bytes = [0; PROGRESS_SIZE];
        // Wing cap and a lost cap's location; and a star in the first course.
        bytes[..4].copy_from_slice(&0x0001_0002_u32.to_be_bytes());
        bytes[4] = 0b1;
        let mine = Progress::from_bytes(bytes);
        assert_eq!(mine.bytes()[..4], 0x0000_0002_u32.to_be_bytes());
        assert_eq!(mine.count(), 2);

        let mut known = Progress::default();
        known.0[4] = 0b11;
        let new = mine.without(&known);
        assert_eq!(new.count(), 1);
        known.add(&mine);
        assert_eq!(known.count(), 3);
        assert!(mine.without(&known).is_empty());
    }

    #[test]
    fn the_first_bits_are_as_many_as_asked_and_all_from_the_whole() {
        let mut whole = Progress::default();
        whole.0[4] = 0b1011_0110;
        whole.0[9] = 0b1000_0001;

        for most in 0..=8 {
            let first = whole.first(most);
            assert_eq!(first.count(), most.min(7), "{most}");
            assert!(first.without(&whole).is_empty(), "{most}");
        }
        assert_eq!(whole.first(5).0[4], whole.0[4], "the earlier byte first");
    }
}
