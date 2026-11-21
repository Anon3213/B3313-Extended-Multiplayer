//! Builds the multiplayer ROM on the player's PC, from the two files only they can supply: their
//! Super Mario 64 ROM and a B3313 patch, of any version in `GAMES`. The panel carries only this
//! project's own patches, one for each version.
//!
//! Every stage is checked by SHA-256, so a wrong or damaged file is named, never played. The
//! patch files are read as untrusted data: nothing here indexes past what a file holds.

use sha2::{Digest, Sha256};
use std::fmt;
use std::path::{Path, PathBuf};

/// One version of B3313 this panel can make the multiplayer ROM of. A version is one released
/// ROM: two patches that make different ROMs are two versions, whatever their names say.
pub struct Game {
    pub name: &'static str,
    /// The file the panel builds. Each game has a name of its own, as the emulator names a
    /// save after its ROM, and one game's save is no use to another.
    rom_name: &'static str,
    /// The ROM the player's patch makes of Super Mario 64.
    released_sha256: &'static str,
    /// Whether the release moved the ROM's data, which is undone before this project's patch.
    shifted: bool,
    /// That ROM with any shift undone: what this project's patch is made for.
    base_sha256: &'static str,
    /// This project's patch, made by `scripts/make_patch.py` from the ROM this panel's bridge
    /// was built for, and the SHA-256 of the ROM it makes.
    patch: &'static [u8],
    multiplayer_sha256: &'static str,
}

/// Every version, as `scripts/game_versions.py` decides them: a test there keeps the two equal.
pub const GAMES: [Game; 3] = [
    Game {
        name: "Unabandoned A2 Anniversary Edition",
        // The name from before there were other versions, which its players' saves have.
        rom_name: "B3313 Extended Multiplayer.z64",
        released_sha256: "def0f5dd465b3a3bac6961cdfbccdb2f994b9de423c247802f9ad27695dd2176",
        shifted: true,
        base_sha256: "f8c482f5086349c5dc9054958b97f8ad8bb4d34323f8d2725858d33452898412",
        patch: include_bytes!("../assets/a2ae.bps"),
        multiplayer_sha256: include_str!("../assets/a2ae.bps.sha256"),
    },
    Game {
        name: "1.0.2 hotfix 3",
        rom_name: "B3313 Extended Multiplayer (1.0.2 hotfix 3).z64",
        released_sha256: "2980ddda4248d0276d2a59325e0eceaa2461ad9532f7c92772901cb84ab88f28",
        shifted: false,
        base_sha256: "2980ddda4248d0276d2a59325e0eceaa2461ad9532f7c92772901cb84ab88f28",
        patch: include_bytes!("../assets/v102h3.bps"),
        multiplayer_sha256: include_str!("../assets/v102h3.bps.sha256"),
    },
    Game {
        name: "1.0.2 hotfix 3 with Unabandoned tweaks",
        rom_name: "B3313 Extended Multiplayer (1.0.2 hotfix 3 with Unabandoned tweaks).z64",
        released_sha256: "ced3fb37cdff3e5dc9606412211db3ac9805631bf8abc5511823fddc215a721a",
        shifted: false,
        base_sha256: "ced3fb37cdff3e5dc9606412211db3ac9805631bf8abc5511823fddc215a721a",
        patch: include_bytes!("../assets/v102h3ua.bps"),
        multiplayer_sha256: include_str!("../assets/v102h3ua.bps.sha256"),
    },
];

/// Super Mario 64 (USA), in .z64 byte order.
const VANILLA_SHA256: &str = "17ce077343c6133f8c9f2d6d6d9a4ab62c8cd2aa57c40aea1f490b4c8bb21d91";

/// The release build inserts this marker, then patches `osPiStartDma` to skip it.
const SHIFT_MARKER: &[u8] = b"YAHAAAAAAAAAAAAA";
const SHIFT_MARKER_OFFSET: usize = 0xF_5580;
/// `osPiStartDma` in the ROM file: 0x80324460 to 0x80324570, loaded at 0x80245000.
const OS_PI_START_DMA: std::ops::Range<usize> = 0xD_F460..0xD_F570;

/// No N64 ROM is larger than this; a patch that asks for more is not one of ours.
const LARGEST_ROM: usize = 128 << 20;

/// The versions' names in a row, for telling a player which patches will do.
#[must_use]
pub fn game_names() -> String {
    GAMES.map(|game| game.name).join(", ")
}

/// Why the ROM could not be built, in words for the player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SetupError {
    NotARom,
    WrongRom,
    NotAPatch,
    WrongPatch,
    /// This panel's own patch did not give the ROM it was made for: the download is damaged.
    Damaged,
}

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotARom => "The first file is not a Nintendo 64 ROM",
            Self::WrongRom => {
                "The ROM is not Super Mario 64 (USA), the one B3313 is made from; a European, \
                 Japanese or already patched ROM will not do"
            }
            Self::NotAPatch => "The second file is not a BPS patch",
            Self::WrongPatch => {
                return write!(
                    f,
                    "The patch is damaged, or is for a version of B3313 this panel does not \
                     play; it plays {}",
                    game_names()
                );
            }
            Self::Damaged => "This copy of the panel is damaged; download it again",
        })
    }
}

fn sha256_hex(data: &[u8]) -> String {
    use fmt::Write as _;
    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            // Writing to a String cannot fail.
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

fn has_sha256(data: &[u8], expected: &str) -> bool {
    sha256_hex(data) == expected.trim()
}

/// A ROM dump in .z64 byte order, whatever order it was dumped in. None if it is no N64 ROM.
fn to_big_endian(rom: &[u8]) -> Option<Vec<u8>> {
    // .v64 swaps each pair of bytes and .n64 reverses each four: both reverse a word.
    let word = match rom.get(..4)? {
        [0x80, 0x37, 0x12, 0x40] => return Some(rom.to_vec()),
        [0x37, 0x80, 0x40, 0x12] => 2,
        [0x40, 0x12, 0x37, 0x80] => 4,
        _ => return None,
    };
    Some(
        rom.chunks(word)
            .flat_map(|chunk| chunk.iter().rev().copied())
            .collect(),
    )
}

/// Reads a BPS patch's numbers and bytes, never past its end.
struct PatchReader<'a> {
    rest: &'a [u8],
}

impl PatchReader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let (&first, rest) = self.rest.split_first()?;
        self.rest = rest;
        Some(first)
    }

    fn bytes(&mut self, count: usize) -> Option<&[u8]> {
        let (taken, rest) = self.rest.split_at_checked(count)?;
        self.rest = rest;
        Some(taken)
    }

    /// A number in BPS's encoding: seven bits a byte, the last byte marked. None if it runs
    /// off the patch or past what a `usize` holds.
    fn number(&mut self) -> Option<usize> {
        let mut value: usize = 0;
        let mut shift: usize = 1;
        loop {
            let byte = self.byte()?;
            value = value.checked_add(usize::from(byte & 0x7F).checked_mul(shift)?)?;
            if byte & 0x80 != 0 {
                return Some(value);
            }
            shift = shift.checked_mul(128)?;
            value = value.checked_add(shift)?;
        }
    }

    /// A number whose lowest bit is its sign.
    fn offset(&mut self) -> Option<isize> {
        let raw = self.number()?;
        let size = isize::try_from(raw >> 1).ok()?;
        Some(if raw & 1 == 0 { size } else { -size })
    }
}

/// What `patch` makes of `source`, or None if it is not a BPS patch that fits `source`. The
/// patch's own CRCs are not read: callers check the result's SHA-256, which says more.
fn apply_bps(patch: &[u8], source: &[u8]) -> Option<Vec<u8>> {
    // Three CRCs end the patch.
    let body = patch.get(..patch.len().checked_sub(12)?)?;
    let mut reader = PatchReader {
        rest: body.strip_prefix(b"BPS1")?,
    };
    let _source_size = reader.number()?;
    let target_size = reader.number()?;
    if target_size > LARGEST_ROM {
        return None;
    }
    let metadata = reader.number()?;
    reader.bytes(metadata)?;

    let mut target = Vec::with_capacity(target_size);
    let mut source_cursor: usize = 0;
    let mut target_cursor: usize = 0;
    while !reader.rest.is_empty() {
        let command = reader.number()?;
        let length = (command >> 2).checked_add(1)?;
        let end = target.len().checked_add(length)?;
        if end > target_size {
            return None;
        }
        match command & 3 {
            0 => target.extend_from_slice(source.get(target.len()..end)?),
            1 => target.extend_from_slice(reader.bytes(length)?),
            2 => {
                source_cursor = source_cursor.checked_add_signed(reader.offset()?)?;
                let copied_end = source_cursor.checked_add(length)?;
                target.extend_from_slice(source.get(source_cursor..copied_end)?);
                source_cursor = copied_end;
            }
            _ => {
                target_cursor = target_cursor.checked_add_signed(reader.offset()?)?;
                // Byte by byte: a copy may run into what it is itself writing, to repeat it.
                for _ in 0..length {
                    target.push(*target.get(target_cursor)?);
                    target_cursor = target_cursor.checked_add(1)?;
                }
            }
        }
    }
    (target.len() == target_size).then_some(target)
}

/// Undoes the release build's shift: takes the marker out, keeping the ROM's size, and puts
/// back Nintendo's `osPiStartDma` from the player's own ROM. None if `released` has no marker.
fn unshift(released: &[u8], vanilla: &[u8]) -> Option<Vec<u8>> {
    let marker_end = SHIFT_MARKER_OFFSET.checked_add(SHIFT_MARKER.len())?;
    if released.get(SHIFT_MARKER_OFFSET..marker_end)? != SHIFT_MARKER {
        return None;
    }
    let mut rom = Vec::with_capacity(released.len());
    rom.extend_from_slice(released.get(..SHIFT_MARKER_OFFSET)?);
    rom.extend_from_slice(released.get(marker_end..)?);
    rom.resize(released.len(), 0);
    rom.get_mut(OS_PI_START_DMA)?
        .copy_from_slice(vanilla.get(OS_PI_START_DMA)?);
    Some(rom)
}

/// The multiplayer ROM, from the player's Super Mario 64 ROM and a B3313 patch, and which
/// version of the game the patch turned out to be.
///
/// # Errors
/// Says which file is not the one needed.
pub fn build_rom(
    vanilla: &[u8],
    b3313_patch: &[u8],
) -> Result<(&'static Game, Vec<u8>), SetupError> {
    let vanilla = to_big_endian(vanilla).ok_or(SetupError::NotARom)?;
    if !has_sha256(&vanilla, VANILLA_SHA256) {
        return Err(SetupError::WrongRom);
    }
    if !b3313_patch.starts_with(b"BPS1") {
        return Err(SetupError::NotAPatch);
    }
    // A patch's name says little: the ROM it makes is what tells the versions apart.
    let released = apply_bps(b3313_patch, &vanilla).ok_or(SetupError::WrongPatch)?;
    let released_sha256 = sha256_hex(&released);
    let game = GAMES
        .iter()
        .find(|game| game.released_sha256 == released_sha256)
        .ok_or(SetupError::WrongPatch)?;
    // Both inputs are now known to the byte, so anything that fails from here is this panel's.
    let base = if game.shifted {
        unshift(&released, &vanilla).ok_or(SetupError::Damaged)?
    } else {
        released
    };
    if !has_sha256(&base, game.base_sha256) {
        return Err(SetupError::Damaged);
    }
    apply_bps(game.patch, &base)
        .filter(|rom| game.is_current(rom))
        .map(|rom| (game, rom))
        .ok_or(SetupError::Damaged)
}

impl Game {
    /// Where this game's built ROM is kept: beside the panel's preferences. None where there
    /// is no such folder.
    #[must_use]
    pub fn rom_path(&self) -> Option<PathBuf> {
        let folder = crate::preferences::path()?.parent()?.join("game");
        Some(folder.join(self.rom_name))
    }

    /// Whether `rom` is the ROM of this game that this panel's bridge was built for. One
    /// built by an older panel is not, and must be built again.
    #[must_use]
    pub fn is_current(&self, rom: &[u8]) -> bool {
        has_sha256(rom, self.multiplayer_sha256)
    }
}

/// The game whose ROM the panel keeps at `rom`, if it is one the panel built.
#[must_use]
pub fn built_at(rom: &Path) -> Option<&'static Game> {
    GAMES
        .iter()
        .find(|game| game.rom_path().is_some_and(|built| built == rom))
}

/// A ROM the panel built: where it is, and which version of the game.
pub struct Built {
    pub rom: PathBuf,
    pub game: &'static Game,
}

/// Where copies of the game's save are kept, one made at each join. None where there is no
/// such folder.
#[must_use]
pub fn backups_path() -> Option<PathBuf> {
    Some(crate::preferences::path()?.parent()?.join("save-backups"))
}

/// Builds the ROM from the two files and writes it to its game's `rom_path`.
///
/// # Errors
/// Says, in a sentence for the player, which file could not be read or is not the one needed.
pub fn set_up(vanilla: &Path, b3313_patch: &Path) -> Result<Built, String> {
    let (game, rom) = build_rom(&read_chosen(vanilla)?, &read_chosen(b3313_patch)?)
        .map_err(|error| format!("{error}."))?;
    let path = game
        .rom_path()
        .ok_or("There is no folder to keep the game in.")?;
    // Put in place whole: a panel closed half way through would leave half a ROM, which is
    // then taken for a game an older panel built.
    path.parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| crate::preferences::replace_file_bytes(&path, &rom))
        .map_err(|error| format!("Could not write {}: {error}.", path.display()))?;
    Ok(Built { rom: path, game })
}

/// The bytes of a file the player chose as a ROM or a patch. One larger than any of either
/// is refused unread: all of a file picked by mistake would otherwise be taken into memory
/// before it is found to be neither.
fn read_chosen(path: &Path) -> Result<Vec<u8>, String> {
    let failed = |error| format!("Could not read {}: {error}.", path.display());
    let size = std::fs::metadata(path).map_err(failed)?.len();
    if usize::try_from(size).is_ok_and(|size| size <= LARGEST_ROM) {
        std::fs::read(path).map_err(failed)
    } else {
        let megabytes = LARGEST_ROM >> 20;
        Err(format!(
            "{} is neither a ROM nor a patch: it is larger than {megabytes} MB.",
            path.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        GAMES, OS_PI_START_DMA, SHIFT_MARKER, SHIFT_MARKER_OFFSET, SetupError, apply_bps,
        build_rom, to_big_endian, unshift,
    };

    fn number(mut value: usize) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let low = u8::try_from(value & 0x7F).unwrap_or_default();
            value >>= 7;
            if value == 0 {
                out.push(low | 0x80);
                return out;
            }
            out.push(low);
            value -= 1;
        }
    }

    fn command(action: usize, length: usize) -> Vec<u8> {
        number(((length - 1) << 2) | action)
    }

    /// A patch with `commands`, and CRCs nothing reads.
    fn patch(source: usize, target: usize, commands: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"BPS1".to_vec();
        out.extend(number(source));
        out.extend(number(target));
        out.extend(number(0));
        out.extend(commands.concat());
        out.extend([0; 12]);
        out
    }

    #[test]
    fn a_rom_reads_the_same_in_every_dump_order() {
        let z64 = [0x80, 0x37, 0x12, 0x40, b'A', b'B', b'C', b'D'];
        let v64 = [0x37, 0x80, 0x40, 0x12, b'B', b'A', b'D', b'C'];
        let n64 = [0x40, 0x12, 0x37, 0x80, b'D', b'C', b'B', b'A'];
        for dump in [z64, v64, n64] {
            assert_eq!(to_big_endian(&dump).as_deref(), Some(z64.as_slice()));
        }
        assert_eq!(to_big_endian(b"PK\x03\x04 a zip"), None);
        assert_eq!(to_big_endian(b""), None);
    }

    #[test]
    fn a_patch_copies_writes_and_repeats() {
        let source = b"0123456789";
        let made = apply_bps(
            &patch(
                10,
                15,
                &[
                    command(0, 2),                            // "01", from the same place
                    [command(1, 2), b"xy".to_vec()].concat(), // new bytes
                    [command(2, 3), number(7 << 1)].concat(), // "789", from the source at 7
                    [command(3, 8), number(2 << 1)].concat(), // from the output at 2, into itself
                ],
            ),
            source,
        );
        assert_eq!(made.as_deref(), Some(b"01xy789xy789xy7".as_slice()));
    }

    #[test]
    fn a_patch_that_does_not_fit_is_refused_not_trusted() {
        let source = b"0123456789";
        let whole = patch(10, 4, &[command(0, 4)]);
        assert_eq!(
            apply_bps(&whole, source).as_deref(),
            Some(b"0123".as_slice())
        );
        // Cut anywhere, it makes nothing.
        for length in 0..whole.len() {
            assert_eq!(
                apply_bps(whole.get(..length).unwrap_or_default(), source),
                None
            );
        }
        let refused = [
            // Copies past the source's end, and from before its start.
            patch(10, 12, &[command(0, 12)]),
            patch(10, 2, &[[command(2, 2), number((1 << 1) | 1)].concat()]),
            // Reads the output before any is written.
            patch(10, 2, &[[command(3, 2), number(0)].concat()]),
            // Writes more, and less, than it said it would.
            patch(10, 2, &[command(0, 3)]),
            patch(10, 5, &[command(0, 3)]),
            // Asks for more memory than any ROM needs.
            patch(10, usize::MAX >> 8, &[command(0, 1)]),
        ];
        for bad in refused {
            assert_eq!(apply_bps(&bad, source), None);
        }
    }

    #[test]
    fn unshifting_takes_the_marker_out_and_keeps_the_size() {
        let length = OS_PI_START_DMA.end.max(SHIFT_MARKER_OFFSET) + 64;
        let vanilla = vec![0x11; length];
        let mut released = vec![0x22; length];
        released
            .get_mut(SHIFT_MARKER_OFFSET..SHIFT_MARKER_OFFSET + SHIFT_MARKER.len())
            .unwrap_or_default()
            .copy_from_slice(SHIFT_MARKER);
        let base = unshift(&released, &vanilla).unwrap_or_default();
        assert_eq!(base.len(), length);
        // Nintendo's function is back, the marker is gone, and the end is padded.
        assert!(
            base.get(OS_PI_START_DMA)
                .unwrap_or_default()
                .iter()
                .all(|&b| b == 0x11)
        );
        assert_eq!(base.get(SHIFT_MARKER_OFFSET), Some(&0x22));
        assert_eq!(base.last(), Some(&0));
        assert_eq!(unshift(&vanilla, &vanilla), None);
    }

    #[test]
    fn the_wrong_files_are_named() {
        let not_a_rom = build_rom(b"not a rom", b"BPS1").err();
        assert_eq!(not_a_rom, Some(SetupError::NotARom));
        let other_rom = [0x80, 0x37, 0x12, 0x40, 0, 0, 0, 0];
        let wrong_rom = build_rom(&other_rom, b"BPS1").err();
        assert_eq!(wrong_rom, Some(SetupError::WrongRom));
    }

    #[test]
    fn every_game_has_a_name_and_a_file_of_its_own() {
        for (index, game) in GAMES.iter().enumerate() {
            for other in GAMES.iter().skip(index + 1) {
                assert_ne!(game.name, other.name);
                assert_ne!(game.rom_name, other.rom_name);
                assert_ne!(game.released_sha256, other.released_sha256);
            }
        }
    }

    /// The whole path, with the real files. They are Nintendo's and the B3313 team's, so they
    /// cannot be in the repository: run this by hand, with them in place, after a ROM build.
    #[test]
    #[ignore = "needs the Super Mario 64 ROM and the B3313 patches, which cannot be committed"]
    fn the_real_files_make_each_games_multiplayer_rom() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let Ok(vanilla) = std::fs::read(root.join("ROM/Super Mario 64 (USA).n64")) else {
            unreachable!("the ROM must be in place to run this test");
        };
        let patches = [
            (
                "B3313_ua_a2_anniversary_edition.bps",
                "Unabandoned A2 Anniversary Edition",
            ),
            ("B3313 [1.0.2, hotfix 3].bps", "1.0.2 hotfix 3"),
            (
                "B3313-1.0.2-hotfix3.bps",
                "1.0.2 hotfix 3 with Unabandoned tweaks",
            ),
        ];
        for (file, game) in patches {
            let Ok(patch) = std::fs::read(root.join("patches").join(file)) else {
                unreachable!("the patch {file} must be in place to run this test");
            };
            let built = build_rom(&vanilla, &patch).map(|(built, _)| built.name);
            assert_eq!(built, Ok(game), "{file}");
        }
        assert_eq!(
            build_rom(&vanilla, &vanilla).err(),
            Some(SetupError::NotAPatch)
        );
        // A patch that fits the ROM and makes none of the games: this project's own.
        let Some(own) = GAMES.first() else {
            unreachable!("there is at least one game");
        };
        assert_eq!(
            build_rom(&vanilla, own.patch).err(),
            Some(SetupError::WrongPatch)
        );
    }
}
