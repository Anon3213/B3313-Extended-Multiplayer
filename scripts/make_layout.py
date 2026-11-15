#!/usr/bin/env python3
"""Write the mailbox's layout for the bridge from the header the game is built with.

Usage: make_layout.py

`TrollEngine/Multiplayer/mailbox.h` is the one place the layout is decided. This reads its
numbers and writes them as `bridge/src/layout.rs`, which the bridge takes every address and
size from, so it cannot differ from the game. Run it after changing the header and commit the
file; a test fails while it is out of date.

Where there is a `scripts/n64layout.py`, the same numbers for tools written in Python that
read a running game's memory, it is written anew as well. Those tools are not part of this
repository, so a checkout of it has no such file and none is made.

It takes three things from the header:

- each `#define MP_NAME` that is a plain number, as `NAME`;
- each struct member's offset, from the note before it, as `STRUCT_MEMBER_OFFSET`;
- each struct's size, from the check the compiler makes of it, as `STRUCT_SIZE`.

The compiler does not read the notes of offsets, but it does check each struct's size against
them, and a member that moved changes one.
"""

import re
from pathlib import Path

_ROOT = Path(__file__).parent.parent
HEADER = _ROOT / "TrollEngine" / "Multiplayer" / "mailbox.h"
RUST = _ROOT / "bridge" / "src" / "layout.rs"
PYTHON = _ROOT / "scripts" / "n64layout.py"

_NUMBER = r"0x[0-9A-Fa-f]+|\d+"
_DEFINE = re.compile(rf"^#define MP_(\w+) ({_NUMBER})\s*(?://.*)?$", re.MULTILINE)
# A number below nothing, which C wants in brackets: one that stands for no level at all.
_NEGATIVE = re.compile(r"^#define MP_(\w+) \((-\d+)\)\s*(?://.*)?$", re.MULTILINE)
_STRUCT = re.compile(r"^struct Mp(\w+) \{\n(.*?)\n\};", re.MULTILINE | re.DOTALL)
_MEMBER = re.compile(rf"/\*({_NUMBER})\*/[^;\n]*?\b(\w+)(?:\[[^;]*)?;")
_SIZE = re.compile(rf"sizeof\(struct Mp(\w+)\) == ({_NUMBER})")
# Room kept between a struct's parts, which nothing reads or writes by name.
_SPARE = re.compile(r"(pad|reserved)\d*")
_BYTE_MOST = 0xFF


def _shouting(name: str) -> str:
    """`localName` or `PlayerSlot` as `LOCAL_NAME` or `PLAYER_SLOT`."""
    return re.sub(r"(?<=[a-z0-9])(?=[A-Z])", "_", name).upper()


def read_layout(header: str) -> dict[str, int]:
    """The layout's numbers by name, in the header's order."""
    layout: dict[str, int] = {}

    def add(name: str, value: str) -> None:
        if name in layout:
            raise ValueError(f"mailbox.h gives {name} twice")
        layout[name] = int(value, 0)

    for name, value in _DEFINE.findall(header) + _NEGATIVE.findall(header):
        add(name, value)
    for struct, body in _STRUCT.findall(header):
        for offset, member in _MEMBER.findall(body):
            if not _SPARE.fullmatch(member):
                add(f"{_shouting(struct)}_{_shouting(member)}_OFFSET", offset)
    for struct, size in _SIZE.findall(header):
        add(f"{_shouting(struct)}_SIZE", size)
    return layout


def _rust_number(value: int) -> str:
    """`value` as Rust's lints want a long number: in hexadecimal, in groups of four."""
    if value < 0x10000:
        return str(value) if value < 10 else f"{value:#X}".replace("0X", "0x")
    digits = f"{value:X}"
    digits = digits.zfill(-(-len(digits) // 4) * 4)
    return "0x" + "_".join(digits[i : i + 4] for i in range(0, len(digits), 4))


def render_rust(layout: dict[str, int]) -> str:
    """The layout as Rust: each number as a `u32`, for addresses; again in `len` as a `usize`,
    for lengths; and, where it fits, in `byte` as a `u8`. Rust's constants cannot change type
    without `as`, which the workspace's lints refuse. A number below nothing is an `i16`, as
    the level it stands in for is, and is neither a length nor a byte."""
    signed = {name: value for name, value in layout.items() if value < 0}
    layout = {name: value for name, value in layout.items() if value >= 0}
    lines = [
        "//! The mailbox's layout, as the game is compiled with it.",
        "//!",
        "//! Written by `scripts/make_layout.py` from `TrollEngine/Multiplayer/mailbox.h`: do not",
        "//! edit it, change the header and run the script.",
        "",
    ]
    lines += [f"pub const {name}: u32 = {_rust_number(value)};" for name, value in layout.items()]
    lines += [f"pub const {name}: i16 = {value};" for name, value in signed.items()]
    lines += ["", "/// The same numbers as lengths.", "pub mod len {"]
    lines += [
        f"    pub const {name}: usize = {_rust_number(value)};" for name, value in layout.items()
    ]
    lines += [
        "}",
        "",
        "/// Those of the numbers that are a byte's worth, as bytes.",
        "pub mod byte {",
    ]
    lines += [
        f"    pub const {name}: u8 = {_rust_number(value)};"
        for name, value in layout.items()
        if value <= _BYTE_MOST
    ]
    lines += ["}", ""]
    return "\n".join(lines)


def render_python(layout: dict[str, int]) -> str:
    """The layout as Python."""
    lines = [
        '"""The mailbox\'s layout, as the game is compiled with it.',
        "",
        "Written by scripts/make_layout.py from TrollEngine/Multiplayer/mailbox.h: do not edit it,",
        "change the header and run the script.",
        '"""',
        "",
    ]
    lines += [
        f"{name} = {value if value < 10 else f'{value:#X}'.replace('0X', '0x')}"
        for name, value in layout.items()
    ]
    return "\n".join(lines) + "\n"


def main() -> None:
    layout = read_layout(HEADER.read_text(encoding="utf-8"))
    # LF, whatever the platform: git keeps these as they are written.
    RUST.write_text(render_rust(layout), encoding="utf-8", newline="\n")
    written = [RUST.name]
    if PYTHON.exists():
        PYTHON.write_text(render_python(layout), encoding="utf-8", newline="\n")
        written.append(PYTHON.name)
    print(f"Wrote {len(layout)} numbers to {' and '.join(written)}.")


if __name__ == "__main__":
    main()
