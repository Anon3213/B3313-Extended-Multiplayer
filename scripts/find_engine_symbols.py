#!/usr/bin/env python3
"""Find where a B3313 ROM keeps the engine functions and variables the multiplayer code uses.

Usage: find_engine_symbols.py <version> <ROM> <built engine tree>...

Writes TrollEngine/Multiplayer/versions/<version>.asm, which the multiplayer build includes for
that ROM (see Multiplayer/overlay.asm). Run it once for each new version of the game, and commit
what it writes.

Most versions of B3313 were built from engine sources nobody published, so their addresses cannot
come from a build. They are read out of the ROM instead: each function of a built engine tree is
looked for in the ROM with the fields the linker fills in left out, and where one is found, those
fields in the ROM's copy say where everything it refers to is. A tree is Troll Engine at any
commit, after `make objs`; trees from several commits find more, as a function that changed after
one commit may match another. Whatever is not found that way is read from the game's fixed call
sites. The script stops rather than guess: every address is found twice or checked against the
ROM's own code.
"""

import struct
import sys
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

from game_versions import GameVersion, by_key

VERSIONS_DIR = Path(__file__).resolve().parent.parent / "TrollEngine" / "Multiplayer" / "versions"

# Relocation types of 32-bit MIPS ELF objects.
_R_MIPS_32, _R_MIPS_26, _R_MIPS_HI16, _R_MIPS_LO16 = 2, 4, 5, 6
_SECTION_SYMBOL = 3
_SHT_PROGBITS, _SHT_SYMTAB, _SHT_REL = 1, 2, 9

_JAL = 3
_JR_RA = 0x03E00008
_LUI = 0x0F

# The engine's segments, the same in every version found so far: (RAM start, ROM start, size).
# Each ROM's own loader is checked against these before anything else is read.
_MAIN = (0x80245000, 0x0, 0xF5580)
_VANILLA_ENGINE = (0x80378800, 0xF5580, 0x16D00)
_AISEG1 = (0x80367460, 0x10C280, 0x113A0)
_AISEG2 = (0x807E0000, 0x188440, 0x14000)
_GLOBAL_STARS = (0x80560000, 0x3D0DC0, 0x10000)
_SEGMENTS = (_MAIN, _VANILLA_ENGINE, _AISEG1, _AISEG2, _GLOBAL_STARS)
# setup_game_memory's code that loads the two engine segments and then calls troll_setup.
_LOADER = (0x80248A50, 0x34)
_LOADER_WORDS = (
    0x3C048036, 0x34847460, 0x3C050010, 0x34A5C280, 0x3C060011, 0x0C09E141, 0x34C6D61F,
    0x3C04807E, 0x3C050018, 0x34A58440, 0x3C060019, 0x0C09E141, 0x34C6C43F,
)  # fmt: skip
# The filler the engine's build leaves in the unused end of each of its segments.
_FILLER = 0x01

# Calls at addresses no version has moved, and what each one calls.
_SITE_TROLL_SETUP = 0x80248A84
_SITE_POST_OBJECT_LOAD_PASS = 0x8027AF30
_SITE_LEVEL_INIT_INTERCEPT = 0x8037EE68
_SITE_LOAD_STUFF_IN_LEVEL_POOL = 0x8037EF78
_SITE_AREA_GEO_LOAD_INTERCEPT = 0x8037F02C
_SITE_ADD_MORE_OBJECTS = 0x8029D12C
_SITE_MARIO_ANIM_LOAD = 0x802509EC
_RANDOM_U16 = 0x80383BB0

_VOTED_FUNCTIONS = (
    "get_red_star_count",
    "save_file_get_total_star_count",
    "save_file_masked_get_course_star_count",
    "save_file_do_save",
    "blue_star_check",
    # The game's own, which the engine builds again in place, so only its objects name them.
    "dma_read",
    "alloc_display_list",
)
_VOTED_DATA = (
    "personalizationRandSeed",
    "rtcHour",
    "rtcMinute",
    "rtcSecond",
    "nightTimeButReal",
    "betaKeyFlags",
    "gSaveFileModified",
    "mario_patchable_table_TWO",
    # Not used by the game's multiplayer code, but by scripts that look at how a level came out.
    "levelScaleH",
)
# Variables that only functions which differ between versions name. Each is taken to lie as
# far from `rtcHour` as in the built trees, which is then checked against what updateRTC reads.
_BESIDE_RTC_HOUR = ("nightTime", "nightModeSetting")
_FIRST_TRACKER = "TRACKER_speed"
# The trackers that are arrays, which the compiler keeps apart from the rest, and how far each
# lies from the first tracker in every version with a matched function that names it. A version
# with none must at least have code that names the address.
_TRACKER_ARRAYS = {
    "TRACKER_death_reasons": 0xD0,
    "TRACKER_star_preferences_gameplay": 0xE8,
    "TRACKER_star_preferences_level_type": 0x108,
    "TRACKER_star_preferences_cap": 0x128,
}

# Room for Multiplayer/boot.o, which is 0x80 bytes of code.
_BOOT_SIZE = 0x100


def fail(message: str) -> SystemExit:
    return SystemExit(f"find_engine_symbols: {message}")


@dataclass
class Symbol:
    name: str
    value: int
    kind: int
    section: int


@dataclass
class EngineObject:
    """What matching needs of one compiled engine file."""

    symbols: list[Symbol]
    section_names: list[str]
    text_index: int = -1
    text: bytes = b""
    relocations: list[tuple[int, int, int]] = field(default_factory=list[tuple[int, int, int]])

    def functions(self) -> list[tuple[str, int, int]]:
        """(name, start, end) of each function, an end being the next function's start."""
        starts = sorted(
            {
                (symbol.value, symbol.name)
                for symbol in self.symbols
                if symbol.section == self.text_index and symbol.name
                if symbol.kind != _SECTION_SYMBOL
            }
        )
        found: list[tuple[str, int, int]] = []
        for index, (start, name) in enumerate(starts):
            later = (value for value, _ in starts[index + 1 :] if value > start)
            found.append((name, start, next(later, len(self.text))))
        return found

    def offsets_from(self, anchor: str) -> dict[str, int]:
        """How far each variable in `anchor`'s section lies from it."""
        base = next((symbol for symbol in self.symbols if symbol.name == anchor), None)
        if base is None or base.section == 0:
            return {}
        return {
            symbol.name: symbol.value - base.value
            for symbol in self.symbols
            if symbol.section == base.section and symbol.name
            if symbol.kind != _SECTION_SYMBOL
        }


def parse_object(data: bytes) -> EngineObject | None:
    """Read a big-endian 32-bit ELF object; None for anything else."""
    if data[:6] != b"\x7fELF\x01\x02":
        return None
    section_offset = struct.unpack_from(">I", data, 0x20)[0]
    entry_size, count, names_index = struct.unpack_from(">HHH", data, 0x2E)
    headers = [
        struct.unpack_from(">10I", data, section_offset + i * entry_size) for i in range(count)
    ]

    def text_at(table: tuple[int, ...], offset: int) -> str:
        start = table[4] + offset
        return data[start : data.index(b"\0", start)].decode("latin-1")

    names = [text_at(headers[names_index], header[0]) for header in headers]
    symbol_table = next((header for header in headers if header[1] == _SHT_SYMTAB), None)
    if symbol_table is None:
        return None
    strings = headers[symbol_table[6]]
    symbols: list[Symbol] = []
    for offset in range(symbol_table[4], symbol_table[4] + symbol_table[5], 16):
        name, value, _size, info, _other, section = struct.unpack_from(">IIIBBH", data, offset)
        symbols.append(Symbol(text_at(strings, name), value, info & 0xF, section))
    found = EngineObject(symbols, names)
    for index, header in enumerate(headers):
        if header[1] == _SHT_PROGBITS and names[index] == ".text":
            found.text_index = index
            found.text = data[header[4] : header[4] + header[5]]
        elif header[1] == _SHT_REL and names[index] == ".rel.text":
            for offset in range(header[4], header[4] + header[5], 8):
                where, info = struct.unpack_from(">II", data, offset)
                found.relocations.append((where, info >> 8, info & 0xFF))
    return found


def _signed16(value: int) -> int:
    return value - 0x10000 if value & 0x8000 else value


def _longest_plain_run(mask: bytes) -> tuple[int, int]:
    """(start, length) of the longest stretch the linker leaves alone."""
    best = (0, 0)
    start = -1
    for index, value in enumerate(mask + b"\0"):
        if value == 0xFF:
            if start < 0:
                start = index
        elif start >= 0:
            if index - start > best[1]:
                best = (start, index - start)
            start = -1
    return best


class Rom:
    def __init__(self, data: bytes) -> None:
        self.data = data

    def offset(self, address: int) -> int:
        for ram, rom, size in _SEGMENTS:
            if ram <= address < ram + size:
                return address - ram + rom
        raise fail(f"{address:08X} is in none of the engine's segments")

    def word(self, address: int) -> int:
        return struct.unpack_from(">I", self.data, self.offset(address))[0]

    def call_target(self, address: int) -> int:
        word = self.word(address)
        if word >> 26 != _JAL:
            raise fail(f"{address:08X} holds {word:08X}, which is not a call")
        return 0x80000000 | ((word & 0x03FFFFFF) << 2)

    def function_end(self, address: int) -> int:
        """The address after the function's return and its delay slot."""
        while self.word(address) != _JR_RA:
            address += 4
        return address + 8

    def calls(self, function: int) -> list[tuple[int, int]]:
        """(address, target) of each call in the function."""
        return [
            (address, self.call_target(address))
            for address in range(function, self.function_end(function), 4)
            if self.word(address) >> 26 == _JAL
        ]

    def addresses_named(self, start: int, end: int) -> set[int]:
        """Every address the code builds from a LUI and a 16-bit offset on the same register."""
        upper: dict[int, int] = {}
        named: set[int] = set()
        for address in range(start, end, 4):
            word = self.word(address)
            opcode, base, target = word >> 26, (word >> 21) & 31, (word >> 16) & 31
            if opcode == _LUI:
                upper[target] = (word & 0xFFFF) << 16
            elif opcode >= 8 and base in upper:
                named.add((upper[base] + _signed16(word & 0xFFFF)) & 0xFFFFFFFF)
        return named


class Votes:
    """Where the ROM's copies of matched functions say each symbol is."""

    def __init__(self) -> None:
        self.addresses: dict[str, Counter[int]] = {}
        self.matched = 0

    def add(self, name: str, address: int) -> None:
        self.addresses.setdefault(name, Counter())[address & 0xFFFFFFFF] += 1

    def get(self, name: str) -> int | None:
        counted = self.addresses.get(name)
        if not counted:
            return None
        if len(counted) > 1:
            listed = ", ".join(f"{address:08X} x{count}" for address, count in counted.items())
            raise fail(f"{name} is at more than one address: {listed}")
        return next(iter(counted))

    def need(self, name: str) -> int:
        address = self.get(name)
        if address is None:
            raise fail(f"no matched function names {name}; try a tree from another commit")
        return address


def _match_function(rom: bytes, body: bytes, mask: bytes) -> int | None:
    """The one ROM offset where `body` is, comparing only the bits set in `mask`."""
    start, length = _longest_plain_run(mask)
    if length < 12:
        return None
    anchor = body[start : start + length]
    wanted = bytes(a & b for a, b in zip(body, mask, strict=True))
    hits: list[int] = []
    position = rom.find(anchor)
    while position != -1:
        base = position - start
        candidate = rom[base : base + len(body)] if base >= 0 and base % 4 == 0 else b""
        if len(candidate) == len(body) and wanted == bytes(
            a & b for a, b in zip(candidate, mask, strict=True)
        ):
            hits.append(base)
            if len(hits) > 1:
                return None
        position = rom.find(anchor, position + 1)
    return hits[0] if hits else None


def vote_from_object(rom: bytes, engine_object: EngineObject, votes: Votes) -> None:
    relocations = sorted(engine_object.relocations)
    for _name, start, end in engine_object.functions():
        body = bytearray(engine_object.text[start:end])
        # A function is padded with nops to its neighbour, which may differ in the ROM.
        while len(body) > 8 and not any(body[-4:]):
            del body[-4:]
        mask = bytearray(b"\xff" * len(body))
        inside = [(at - start, symbol, kind) for at, symbol, kind in relocations if start <= at]
        inside = [entry for entry in inside if entry[0] < len(body)]
        for at, _symbol, kind in inside:
            if kind == _R_MIPS_26:
                mask[at : at + 4] = b"\xfc\x00\x00\x00"
            elif kind in (_R_MIPS_HI16, _R_MIPS_LO16):
                mask[at + 2 : at + 4] = b"\x00\x00"
            elif kind == _R_MIPS_32:
                mask[at : at + 4] = b"\x00\x00\x00\x00"
        base = _match_function(rom, bytes(body), bytes(mask))
        if base is None:
            continue
        votes.matched += 1
        upper: tuple[int, int, int] | None = None
        for at, symbol_index, kind in inside:
            symbol = engine_object.symbols[symbol_index]
            in_rom = struct.unpack_from(">I", rom, base + at)[0]
            in_object = struct.unpack_from(">I", body, at)[0]
            named = bool(symbol.name) and symbol.kind != _SECTION_SYMBOL
            if kind == _R_MIPS_26 and named:
                target = 0x80000000 | ((in_rom & 0x03FFFFFF) << 2)
                votes.add(symbol.name, target - ((in_object & 0x03FFFFFF) << 2))
            elif kind == _R_MIPS_HI16:
                upper = (in_rom & 0xFFFF, in_object & 0xFFFF, symbol_index)
            elif kind == _R_MIPS_LO16 and upper is not None and upper[2] == symbol_index and named:
                value = (upper[0] << 16) + _signed16(in_rom & 0xFFFF)
                addend = (upper[1] << 16) + _signed16(in_object & 0xFFFF)
                votes.add(symbol.name, value - addend)


def _engine_objects(trees: list[Path]) -> list[EngineObject]:
    objects: list[EngineObject] = []
    for tree in trees:
        for path in sorted(tree.rglob("*.o")):
            # The tools are for the PC, and the multiplayer code is not in any ROM read here.
            if {"tools", "Multiplayer"} & set(path.relative_to(tree).parts):
                continue
            parsed = parse_object(path.read_bytes())
            if parsed is not None and parsed.text:
                objects.append(parsed)
    if not objects:
        raise fail("no engine objects found; run `make objs` in each tree first")
    return objects


def _check_segments(rom: Rom) -> None:
    address, _size = _LOADER
    words = tuple(rom.word(address + 4 * index) for index in range(len(_LOADER_WORDS)))
    if words != _LOADER_WORDS:
        raise fail("this ROM loads the engine's segments differently; _SEGMENTS does not fit it")


def _free_end(rom: Rom, segment: tuple[int, int, int]) -> tuple[int, int]:
    """(address, size) of the filler that ends a segment, from a 16-byte boundary."""
    ram, start, size = segment
    used = len(rom.data[start : start + size].rstrip(bytes([_FILLER])))
    # A last byte of data may itself be 0x01, so one more boundary is left alone.
    used = (used + 0x1F) & ~0xF
    return ram + used, size - used


def _offsets_from(objects: list[EngineObject], anchor: str) -> dict[str, int]:
    """How far each variable beside `anchor` lies from it, in the first tree that has it."""
    for engine_object in objects:
        offsets = engine_object.offsets_from(anchor)
        if offsets:
            return offsets
    return {}


def _trackers(rom: Rom, objects: list[EngineObject], votes: Votes) -> dict[str, int]:
    base = votes.need(_FIRST_TRACKER)
    offsets = _offsets_from(objects, _FIRST_TRACKER)
    trackers = {
        name: base + offset for name, offset in offsets.items() if name.startswith("TRACKER_")
    }
    ram, _rom, size = _AISEG2
    named = rom.addresses_named(ram, ram + size)
    for name, offset in _TRACKER_ARRAYS.items():
        trackers[name] = base + offset
        if votes.get(name) is None and base + offset not in named:
            raise fail(f"no code names {base + offset:08X}, where {name} should be")
    for name, address in trackers.items():
        voted = votes.get(name)
        if voted is not None and voted != address:
            raise fail(
                f"{name} is at {voted:08X}, not {address:08X}: the trackers are laid out anew"
            )
    return trackers


def resolve(rom: Rom, objects: list[EngineObject], votes: Votes) -> dict[str, int]:
    """Every label the multiplayer build needs for this ROM."""
    _check_segments(rom)
    labels: dict[str, int] = {
        "troll_setup": rom.call_target(_SITE_TROLL_SETUP),
        "postObjectLoadPass": rom.call_target(_SITE_POST_OBJECT_LOAD_PASS),
        "level_init_intercept": rom.call_target(_SITE_LEVEL_INIT_INTERCEPT),
        "load_stuff_in_level_pool": rom.call_target(_SITE_LOAD_STUFF_IN_LEVEL_POOL),
        "area_geo_load_intercept": rom.call_target(_SITE_AREA_GEO_LOAD_INTERCEPT),
        "addMoreObjects": rom.call_target(_SITE_ADD_MORE_OBJECTS),
        "mario_anim_load_patchable_table": rom.call_target(_SITE_MARIO_ANIM_LOAD),
    }
    for name in _VOTED_FUNCTIONS + _VOTED_DATA:
        labels[name] = votes.need(name)

    # troll_geo_layout follows area_geo_load_intercept, which does nothing but call it. Its first
    # two calls count the save's stars, and after its four random numbers it reads the clock.
    geo_layout = rom.function_end(labels["area_geo_load_intercept"])
    calls = rom.calls(geo_layout)
    star_counts = (labels["save_file_get_total_star_count"], labels["get_red_star_count"])
    if tuple(target for _, target in calls[:2]) != star_counts:
        raise fail(f"{geo_layout:08X} does not begin as troll_geo_layout does")
    labels["mp_site_geo_total_stars"] = calls[0][0]
    random_calls = [index for index, (_, target) in enumerate(calls) if target == _RANDOM_U16]
    if len(random_calls) != 4 or random_calls[-1] + 1 >= len(calls):
        raise fail("troll_geo_layout does not roll four random numbers and then read the clock")
    labels["updateRTC"] = calls[random_calls[-1] + 1][1]
    if votes.get("updateRTC") not in (None, labels["updateRTC"]):
        raise fail("updateRTC is not where troll_geo_layout calls it")

    # interceptSegLoad's one call is to interceptBgSegLoad, which no other file can name.
    background_calls = rom.calls(votes.need("interceptSegLoad"))
    if len(background_calls) != 1:
        raise fail("interceptSegLoad does not make exactly one call")
    labels["mp_site_bg_seg_load"], labels["interceptBgSegLoad"] = background_calls[0]

    beside = _offsets_from(objects, "rtcHour")
    clock = labels["updateRTC"]
    clock_reads = rom.addresses_named(clock, rom.function_end(clock))
    for name in _BESIDE_RTC_HOUR:
        address = votes.get(name)
        if address is None:
            if name not in beside:
                raise fail(f"no tree says where {name} lies from rtcHour")
            address = labels["rtcHour"] + beside[name]
        if address not in clock_reads:
            raise fail(f"updateRTC does not use {address:08X}, where {name} should be")
        labels[name] = address

    labels.update(_trackers(rom, objects, votes))
    labels["mp_boot_addr"], room = _free_end(rom, _AISEG2)
    if room < _BOOT_SIZE:
        raise fail(f"the engine's second segment has {room:#x} bytes free, too few for the loader")
    return labels


def format_labels(version: GameVersion, labels: dict[str, int], matched: int) -> str:
    lines = [
        f"; Where B3313 {version.name} keeps what the multiplayer code uses of the engine.",
        f"; Written by scripts/find_engine_symbols.py from {matched} functions found in the ROM;",
        "; do not edit. See Multiplayer/overlay.asm.",
        "",
        "; Not an address: what the game tells its bridge it is (scripts/game_versions.py).",
        f".definelabel mp_game_number, {version.number}",
        "",
    ]
    lines += [f".definelabel {name}, 0x{address:08X}" for name, address in sorted(labels.items())]
    return "\n".join(lines) + "\n"


def main() -> None:
    if len(sys.argv) < 4:
        raise SystemExit(__doc__)
    version = by_key(sys.argv[1])
    rom = Rom(Path(sys.argv[2]).read_bytes())
    objects = _engine_objects([Path(tree) for tree in sys.argv[3:]])
    votes = Votes()
    for engine_object in objects:
        vote_from_object(rom.data, engine_object, votes)
    labels = resolve(rom, objects, votes)
    VERSIONS_DIR.mkdir(exist_ok=True)
    path = VERSIONS_DIR / f"{version.key}.asm"
    path.write_bytes(format_labels(version, labels, votes.matched).encode())
    print(f"Wrote {path}: {len(labels)} labels from {votes.matched} matched functions")


if __name__ == "__main__":
    main()
