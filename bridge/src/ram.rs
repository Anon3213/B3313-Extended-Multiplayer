//! N64 RAM addressing and byte order, and finding RAM inside an emulator's memory.

use std::io;
use std::thread;
use std::time::Duration;

/// Size of the N64 RAM map with the Expansion Pak, which B3313 requires.
pub const RAM_SIZE: u32 = 0x0080_0000;

/// Start of the N64's cached RAM segment, where game code addresses its memory.
const KSEG0_BASE: u32 = 0x8000_0000;

/// The game's own code: ROM offset `0x1000` is loaded here at boot.
const CODE_SIGNATURE_ADDRESS: u32 = 0x8024_6000;
const CODE_SIGNATURE_ROM_RANGE: std::ops::Range<usize> = 0x1000..0x1040;
/// The game's count of the frames it has run, in every version: it stands still only while
/// the game does not run at all.
pub(crate) const GLOBAL_TIMER_ADDRESS: u32 = 0x8032_D5D4;
const WORD_SIZE: usize = 4;
const LIVE_CHECK_DELAY: Duration = Duration::from_millis(500);
/// How much of a region is read at a time while looking for the game's code: whole words, and
/// far more than the signature.
const SCAN_CHUNK: usize = 4 << 20;
const _: () = assert!(
    SCAN_CHUNK.is_multiple_of(WORD_SIZE)
        && (CODE_SIGNATURE_ROM_RANGE.end - CODE_SIGNATURE_ROM_RANGE.start) < SCAN_CHUNK
);

/// Reads and writes another process's memory.
pub trait ProcessMemory {
    /// Fills `buffer` from `address`.
    ///
    /// # Errors
    /// Fails if the whole range cannot be read, as when the process has exited.
    fn read(&self, address: usize, buffer: &mut [u8]) -> io::Result<()>;

    /// Writes `data` to `address`.
    ///
    /// # Errors
    /// Fails if the whole range cannot be written.
    fn write(&self, address: usize, data: &[u8]) -> io::Result<()>;

    /// Returns each committed, readable region.
    fn readable_regions(&self) -> Vec<Region>;

    /// Returns the parts of those regions the process has in use: the memory it has touched
    /// and still holds. All of them, for a process that cannot tell.
    fn regions_in_use(&self) -> Vec<Region> {
        self.readable_regions()
    }
}

/// A committed, readable range of another process's memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub address: usize,
    pub size: usize,
}

/// How much of an emulator's memory is searched for the game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Search {
    /// Only the memory the emulator has in use, which a running game's RAM is. An emulator
    /// sets aside far more than it uses, and Windows gives it real memory for none of that
    /// until it is touched; reading it counts. Searching all of the emulator took 5 seconds
    /// and left it holding 1.5 GB more, for as long as it ran.
    InUse,
    /// All of it, at that price: for a game whose code Windows has taken out of memory, as
    /// it may from a program long idle or a PC short of memory.
    Everything,
}

/// Converts a game address such as `0x8033_B170` into an offset from the start of RAM.
///
/// Returns `None` for addresses outside cached RAM, or when `length` bytes would run past its end.
#[must_use]
pub const fn ram_offset(address: u32, length: u32) -> Option<u32> {
    let Some(offset) = address.checked_sub(KSEG0_BASE) else {
        return None;
    };
    let Some(end) = offset.checked_add(length) else {
        return None;
    };
    if end <= RAM_SIZE { Some(offset) } else { None }
}

/// Swaps each 32-bit word between big-endian N64 order and the host order the core stores it in.
///
/// The core keeps RAM as host-endian words, so byte `n` of a big-endian word lives at `n ^ 3`.
/// Applying this twice restores the input. Trailing bytes that do not fill a word are left as is.
pub fn swap_words_in_place(bytes: &mut [u8]) {
    for word in bytes.as_chunks_mut::<4>().0 {
        word.reverse();
    }
}

fn invalid_input(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Big-endian view of N64 RAM, stored as host-endian words at `base` in another process.
#[derive(Debug)]
pub struct N64Ram<'a, M> {
    memory: &'a M,
    base: usize,
}

impl<'a, M: ProcessMemory> N64Ram<'a, M> {
    #[must_use]
    pub const fn new(memory: &'a M, base: usize) -> Self {
        Self { memory, base }
    }

    /// Only whole, aligned words are allowed, so no access has to merge with its neighbours.
    fn host_address(&self, address: u32, length: usize) -> io::Result<usize> {
        let outside = || invalid_input(format!("{address:#x} + {length:#x} is outside N64 RAM"));
        let length_u32 = u32::try_from(length).map_err(|_| outside())?;
        let offset = ram_offset(address, length_u32).ok_or_else(outside)?;
        if !offset.is_multiple_of(4) || !length.is_multiple_of(WORD_SIZE) {
            return Err(invalid_input(format!(
                "{address:#x} + {length:#x} is not whole words"
            )));
        }
        let offset = usize::try_from(offset).map_err(|_| outside())?;
        self.base.checked_add(offset).ok_or_else(outside)
    }

    /// Fills `buffer` with the big-endian bytes at game address `address`.
    ///
    /// # Errors
    /// Fails if the range is not whole words inside RAM, or cannot be read.
    pub fn read(&self, address: u32, buffer: &mut [u8]) -> io::Result<()> {
        let host = self.host_address(address, buffer.len())?;
        self.memory.read(host, buffer)?;
        swap_words_in_place(buffer);
        Ok(())
    }

    /// Reads a big-endian word.
    ///
    /// # Errors
    /// Fails if the address is not an aligned word inside RAM, or cannot be read.
    pub fn read_u32(&self, address: u32) -> io::Result<u32> {
        let mut word = [0; WORD_SIZE];
        self.read(address, &mut word)?;
        Ok(u32::from_be_bytes(word))
    }

    /// Writes big-endian `data` to game address `address`.
    ///
    /// # Errors
    /// Fails if the range is not whole words inside RAM, or cannot be written.
    pub fn write(&self, address: u32, data: &[u8]) -> io::Result<()> {
        let host = self.host_address(address, data.len())?;
        let mut swapped = data.to_vec();
        swap_words_in_place(&mut swapped);
        self.memory.write(host, &swapped)
    }
}

/// Returns every RAM base whose code matches `rom`, searching `regions`.
fn find_signature_bases<M: ProcessMemory>(
    memory: &M,
    rom: &[u8],
    regions: &[Region],
) -> io::Result<Vec<usize>> {
    let mut needle = rom
        .get(CODE_SIGNATURE_ROM_RANGE)
        .ok_or_else(|| invalid_input("the ROM is too small".to_owned()))?
        .to_vec();
    swap_words_in_place(&mut needle);
    let first_word = *needle
        .first_chunk::<WORD_SIZE>()
        .ok_or_else(|| invalid_input("the code signature is empty".to_owned()))?;

    let code_offset = ram_offset(CODE_SIGNATURE_ADDRESS, 0)
        .and_then(|offset| usize::try_from(offset).ok())
        .ok_or_else(|| invalid_input("the code signature is outside RAM".to_owned()))?;
    let mut bases = Vec::new();
    let mut data = Vec::new();
    // Each chunk begins this far after the last: one word less than would leave a signature
    // that ends a chunk to be found again at the start of the next.
    let step = SCAN_CHUNK - needle.len() + WORD_SIZE;
    for region in regions {
        // A chunk at a time: a region may be gigabytes, and reading it whole took as much
        // memory again here.
        for offset in (0..region.size).step_by(step) {
            data.resize(SCAN_CHUNK.min(region.size - offset), 0);
            // Regions can vanish between listing and reading, which only means nothing to find
            // there.
            if memory.read(region.address + offset, &mut data).is_err() {
                continue;
            }
            // RAM is an array of words, so only word offsets need checking, and a cheap
            // comparison of the first word rules out almost all of them.
            for (index, word) in data.as_chunks::<WORD_SIZE>().0.iter().enumerate() {
                let start = index * WORD_SIZE;
                if *word == first_word && data.get(start..start + needle.len()) == Some(&needle) {
                    bases.extend((region.address + offset + start).checked_sub(code_offset));
                }
            }
        }
    }
    Ok(bases)
}

/// Picks the candidate whose game timer advances: stale copies of the code never tick.
fn choose_live_base<M: ProcessMemory>(
    memory: &M,
    bases: &[usize],
    delay: Duration,
) -> io::Result<usize> {
    let timer = |base| N64Ram::new(memory, base).read_u32(GLOBAL_TIMER_ADDRESS);
    let before: Vec<_> = bases
        .iter()
        .filter_map(|&base| timer(base).ok().map(|value| (base, value)))
        .collect();
    // With no candidate there is no timer to wait on.
    if !before.is_empty() {
        thread::sleep(delay);
    }
    before
        .into_iter()
        .find(|&(base, value)| timer(base).is_ok_and(|now| now != value))
        .map(|(base, _)| base)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("none of {} candidates has a running game", bases.len()),
            )
        })
}

/// Locates the live N64 RAM inside an emulator running `rom`, looking through as much of its
/// memory as `search` says.
///
/// # Errors
/// Fails if the ROM is too small, or no copy of its code belongs to a running game.
pub fn attach<'a, M: ProcessMemory>(
    memory: &'a M,
    rom: &[u8],
    search: Search,
) -> io::Result<N64Ram<'a, M>> {
    let regions = match search {
        Search::InUse => memory.regions_in_use(),
        Search::Everything => memory.readable_regions(),
    };
    let bases = find_signature_bases(memory, rom, &regions)?;
    let base = choose_live_base(memory, &bases, LIVE_CHECK_DELAY)?;
    Ok(N64Ram::new(memory, base))
}

#[cfg(test)]
pub mod testing {
    use super::{ProcessMemory, Region};
    use std::cell::RefCell;
    use std::io;

    /// A process with one region of memory, starting at `start`.
    #[derive(Debug)]
    pub struct FakeProcess {
        pub start: usize,
        pub bytes: RefCell<Vec<u8>>,
        /// Where each write went, in the order they were made: the game reads the same
        /// memory at any moment, so the order of a record's writes is part of what is right.
        pub written: RefCell<Vec<usize>>,
        /// The parts of the region the process has in use, if it is not all of it.
        pub in_use: RefCell<Option<Vec<Region>>>,
    }

    impl FakeProcess {
        #[must_use]
        pub fn new(start: usize, size: usize) -> Self {
            Self {
                start,
                bytes: RefCell::new(vec![0; size]),
                written: RefCell::new(Vec::new()),
                in_use: RefCell::new(None),
            }
        }
    }

    impl ProcessMemory for FakeProcess {
        fn read(&self, address: usize, buffer: &mut [u8]) -> io::Result<()> {
            let offset = address.wrapping_sub(self.start);
            let bytes = self.bytes.borrow();
            let source = bytes
                .get(offset..offset.wrapping_add(buffer.len()))
                .ok_or(io::ErrorKind::PermissionDenied)?;
            buffer.copy_from_slice(source);
            Ok(())
        }

        fn write(&self, address: usize, data: &[u8]) -> io::Result<()> {
            let offset = address.wrapping_sub(self.start);
            let mut bytes = self.bytes.borrow_mut();
            let target = bytes
                .get_mut(offset..offset.wrapping_add(data.len()))
                .ok_or(io::ErrorKind::PermissionDenied)?;
            target.copy_from_slice(data);
            self.written.borrow_mut().push(offset);
            Ok(())
        }

        fn readable_regions(&self) -> Vec<Region> {
            vec![Region {
                address: self.start,
                size: self.bytes.borrow().len(),
            }]
        }

        fn regions_in_use(&self) -> Vec<Region> {
            let in_use = self.in_use.borrow().clone();
            in_use.unwrap_or_else(|| self.readable_regions())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeProcess;
    use super::{
        CODE_SIGNATURE_ROM_RANGE, N64Ram, ProcessMemory, RAM_SIZE, Region, SCAN_CHUNK,
        choose_live_base, find_signature_bases, ram_offset, swap_words_in_place,
    };
    use std::time::Duration;

    #[test]
    fn ram_offset_maps_cached_addresses() {
        assert_eq!(ram_offset(0x8033_B170, 4), Some(0x0033_B170));
        assert_eq!(ram_offset(0x8000_0000, RAM_SIZE), Some(0));
    }

    #[test]
    fn ram_offset_rejects_addresses_outside_ram() {
        assert_eq!(ram_offset(0x7FFF_FFFF, 1), None);
        assert_eq!(ram_offset(0x8080_0000, 1), None);
        assert_eq!(ram_offset(0x807F_FFFF, 2), None);
        assert_eq!(ram_offset(0xFFFF_FFFF, u32::MAX), None);
    }

    #[test]
    fn swap_matches_observed_rom_code_layout() {
        // Bytes at ROM 0x1000, which RetroArch held in this order when read from a live process.
        let mut bytes = [0x3C, 0x08, 0x80, 0x34, 0x3C, 0x09, 0x00, 0x02];
        swap_words_in_place(&mut bytes);
        assert_eq!(bytes, [0x34, 0x80, 0x08, 0x3C, 0x02, 0x00, 0x09, 0x3C]);
    }

    #[test]
    fn swap_leaves_trailing_partial_word_alone() {
        let mut bytes = [1, 2, 3, 4, 5, 6];
        swap_words_in_place(&mut bytes);
        assert_eq!(bytes, [4, 3, 2, 1, 5, 6]);
    }

    #[test]
    fn writes_store_host_order_and_read_back_big_endian() {
        let process = FakeProcess::new(0x1000, 16);
        let ram = N64Ram::new(&process, 0x1000);

        ram.write(0x8000_0004, &[1, 2, 3, 4]).unwrap_or_default();

        assert_eq!(process.bytes.borrow().get(4..8), Some(&[4, 3, 2, 1][..]));
        assert_eq!(ram.read_u32(0x8000_0004).ok(), Some(0x0102_0304));
    }

    #[test]
    fn partial_words_and_addresses_outside_ram_are_rejected() {
        let process = FakeProcess::new(0, 16);
        let ram = N64Ram::new(&process, 0);

        assert!(ram.write(0x8000_0002, &[0; 4]).is_err());
        assert!(ram.write(0x8000_0000, &[0; 3]).is_err());
        assert!(ram.write(0x7FFF_FFFC, &[0; 4]).is_err());
        assert!(ram.write(0x807F_FFFC, &[0; 8]).is_err());
        assert!(process.bytes.borrow().iter().all(|&byte| byte == 0));
    }

    fn rom_with_signature() -> Vec<u8> {
        let mut rom = vec![0; CODE_SIGNATURE_ROM_RANGE.end];
        for (index, byte) in rom.iter_mut().enumerate() {
            *byte = u8::try_from(index & 0x7F).unwrap_or(0) | 0x80;
        }
        rom
    }

    #[test]
    fn signature_scan_finds_host_order_code_and_maps_it_to_a_ram_base() {
        let rom = rom_with_signature();
        let signature = rom.get(CODE_SIGNATURE_ROM_RANGE).unwrap_or_default();
        let mut host_order = signature.to_vec();
        swap_words_in_place(&mut host_order);

        // The code loads 0x246000 bytes into RAM, so its region starts well before it.
        let process = FakeProcess::new(0x1000_0000, 0x0025_0000);
        process
            .write(0x1000_0000 + 0x10 + 0x24_6000, &host_order)
            .unwrap_or_default();

        let everywhere = process.readable_regions();
        let bases = find_signature_bases(&process, &rom, &everywhere).unwrap_or_default();

        assert_eq!(bases, [0x1000_0010]);
    }

    #[test]
    fn signature_scan_finds_code_wherever_it_falls_among_the_chunks_read() {
        let rom = rom_with_signature();
        let mut host_order = rom
            .get(CODE_SIGNATURE_ROM_RANGE)
            .unwrap_or_default()
            .to_vec();
        swap_words_in_place(&mut host_order);
        // The code is 0x246000 into RAM, so a base is that far before where it is found.
        let code = 0x24_6000;
        // Where the signature is in the region, which is read from its start.
        let places = [
            // Ending the first chunk exactly, and so also beginning the second's overlap.
            SCAN_CHUNK - host_order.len(),
            // Across the end of the first chunk.
            SCAN_CHUNK - 8,
            // Well into the second.
            SCAN_CHUNK + 0x1000,
        ];
        for place in places {
            let process = FakeProcess::new(0x1000_0000, 2 * SCAN_CHUNK);
            process
                .write(0x1000_0000 + place, &host_order)
                .unwrap_or_default();
            let everywhere = process.readable_regions();
            let bases = find_signature_bases(&process, &rom, &everywhere).unwrap_or_default();
            assert_eq!(bases, [0x1000_0000 + place - code], "at {place:#x}");
        }
    }

    #[test]
    fn signature_scan_ignores_big_endian_copies() {
        let rom = rom_with_signature();
        let signature = rom.get(CODE_SIGNATURE_ROM_RANGE).unwrap_or_default();
        let process = FakeProcess::new(0x1000_0000, 0x0025_0000);
        process
            .write(0x1000_0000 + 0x24_6000, signature)
            .unwrap_or_default();

        let everywhere = process.readable_regions();
        assert_eq!(
            find_signature_bases(&process, &rom, &everywhere).ok(),
            Some(vec![])
        );
    }

    #[test]
    fn a_search_of_the_memory_in_use_finds_code_there_and_reads_nothing_else() {
        let rom = rom_with_signature();
        let mut host_order = rom
            .get(CODE_SIGNATURE_ROM_RANGE)
            .unwrap_or_default()
            .to_vec();
        swap_words_in_place(&mut host_order);
        let start = 0x1000_0000;
        let process = FakeProcess::new(start, 0x0050_0000);
        // The code twice: in the page the process has in use, and in memory it only set aside.
        let (used, unused) = (0x24_6000, 0x40_0000);
        for place in [used, unused] {
            process
                .write(start + place, &host_order)
                .unwrap_or_default();
        }
        let page = Region {
            address: start + used,
            size: 0x1000,
        };
        *process.in_use.borrow_mut() = Some(vec![page]);

        let found = |regions: Vec<Region>| find_signature_bases(&process, &rom, &regions).ok();
        assert_eq!(found(process.regions_in_use()), Some(vec![start]));
        let everywhere = found(process.readable_regions());
        assert_eq!(everywhere, Some(vec![start, start + unused - used]));
    }

    #[test]
    fn no_running_game_is_an_error() {
        let process = FakeProcess::new(0, 0x0080_0000);

        assert!(choose_live_base(&process, &[0], Duration::ZERO).is_err());
        assert!(choose_live_base(&process, &[], Duration::ZERO).is_err());
    }
}
