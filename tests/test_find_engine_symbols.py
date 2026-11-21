import struct

import pytest
from find_engine_symbols import Rom, Votes, parse_object, vote_from_object

# One function as a compiler leaves it: it calls `helper` and reads `counter`, both elsewhere,
# so the call's target and the two halves of the variable's address are left for the linker.
_FUNCTION = (
    0x27BDFFE8,  # addiu sp, sp, -0x18
    0xAFBF0014,  # sw ra, 0x14(sp)
    0x0C000000,  # jal helper
    0x00000000,  # nop
    0x3C020000,  # lui v0, %hi(counter)
    0x8C420000,  # lw v0, %lo(counter)(v0)
    0x8FBF0014,  # lw ra, 0x14(sp)
    0x03E00008,  # jr ra
    0x27BD0018,  # addiu sp, sp, 0x18
)
_HELPER = 0x80371234
_COUNTER = 0x807EC6B0
# The same function as it is in a ROM, where the linker has filled those three in.
_LINKED = (
    *_FUNCTION[:2],
    0x0C000000 | ((_HELPER & 0x0FFFFFFF) >> 2),
    _FUNCTION[3],
    0x3C020000 | ((_COUNTER + 0x8000) >> 16),
    0x8C420000 | (_COUNTER & 0xFFFF),
    *_FUNCTION[6:],
)


def _words(words: tuple[int, ...]) -> bytes:
    return struct.pack(f">{len(words)}I", *words)


def _object() -> bytes:
    """A big-endian 32-bit ELF object holding `_FUNCTION` and its three relocations."""
    names = b"\0.text\0.rel.text\0.symtab\0.strtab\0.shstrtab\0"
    strings = b"\0update\0helper\0counter\0"
    text = _words(_FUNCTION)
    symbols = b"".join(
        struct.pack(">IIIBBH", name, value, 0, info, 0, section)
        for name, value, info, section in (
            (0, 0, 0, 0),
            (strings.index(b"update"), 0, 0x12, 1),
            (strings.index(b"helper"), 0, 0x10, 0),
            (strings.index(b"counter"), 0, 0x10, 0),
        )
    )
    relocations = b"".join(
        struct.pack(">II", offset, (symbol << 8) | kind)
        for offset, symbol, kind in ((8, 2, 4), (16, 3, 5), (20, 3, 6))
    )
    bodies = [b"", text, relocations, symbols, strings, names]
    kinds = [(0, 0, 0), (1, 1, 0), (7, 9, 3), (17, 2, 4), (25, 3, 0), (33, 3, 0)]
    offset = 0x34
    headers = b""
    for body, (name, kind, link) in zip(bodies, kinds, strict=True):
        headers += struct.pack(">10I", name, kind, 0, 0, offset, len(body), link, 0, 4, 0)
        offset += len(body)
    header = b"\x7fELF\x01\x02\x01" + bytes(9) + struct.pack(">HHI", 1, 8, 1)
    header += struct.pack(">IIIIHHHHHH", 0, 0, offset, 0, 0x34, 0, 0, 40, len(bodies), 5)
    return header + b"".join(bodies) + headers


def test_a_function_found_in_the_rom_tells_where_what_it_names_is() -> None:
    engine_object = parse_object(_object())
    assert engine_object is not None
    rom = bytes(0x40) + _words(_LINKED) + bytes(0x40)
    votes = Votes()
    vote_from_object(rom, engine_object, votes)
    assert votes.matched == 1
    assert votes.need("helper") == _HELPER
    assert votes.need("counter") == _COUNTER


def test_a_function_the_rom_has_twice_or_changed_tells_nothing() -> None:
    engine_object = parse_object(_object())
    assert engine_object is not None
    twice, changed = Votes(), Votes()
    vote_from_object(_words(_LINKED) * 2, engine_object, twice)
    # One instruction the linker does not touch is another: a newer version of the function.
    newer = (*_LINKED[:6], 0x8FBF0010, *_LINKED[7:])
    vote_from_object(_words(newer), engine_object, changed)
    assert twice.matched == changed.matched == 0
    assert twice.get("helper") is None
    with pytest.raises(SystemExit, match="helper"):
        changed.need("helper")


def test_a_name_found_at_two_addresses_is_refused() -> None:
    votes = Votes()
    votes.add("counter", _COUNTER)
    votes.add("counter", _COUNTER + 4)
    with pytest.raises(SystemExit, match="more than one address"):
        votes.get("counter")


def test_anything_that_is_not_such_an_object_is_passed_over() -> None:
    assert parse_object(b"\x7fELF\x02\x01 a 64-bit little-endian file") is None
    assert parse_object(b"") is None


def test_a_roms_code_is_read_by_its_address() -> None:
    # The game's main segment loads at 0x80245000 from the ROM's start.
    rom = Rom(_words(_LINKED) + bytes(0x100))
    start = 0x80245000
    assert rom.function_end(start) == start + 4 * len(_LINKED)
    assert rom.calls(start) == [(start + 8, _HELPER)]
    assert _COUNTER in rom.addresses_named(start, rom.function_end(start))
    with pytest.raises(SystemExit, match="not a call"):
        rom.call_target(start)
    with pytest.raises(SystemExit, match="none of the engine's segments"):
        rom.word(0x80000000)
