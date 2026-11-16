#!/usr/bin/env python3
"""Draw the programs' icons and pack them for Windows.

Usage: make_icons.py                  write icons/<program>.ico and icons/<program>.res
       make_icons.py --preview FILE   draw every icon into one PNG, to look at

The icon is two pairs of eyes in the dark, a pair to a player: the panel's in two colors, the
bridge's and the server's each in one of them, so the three are told apart in a folder. It is
drawn here, from the shapes below, and not kept as a picture.

A .ico is what Explorer and a shortcut show. A .res is the same pictures as the linker takes
them: each program's build script hands its own to the linker, which is all it takes to give a
program an icon, with no resource compiler and no crate. The panel's window takes its icon
from the program's too. Run this after changing the drawing and commit what it writes.
"""

import math
import struct
import sys
import zlib
from pathlib import Path

ICONS = Path(__file__).resolve().parent.parent / "icons"

# The drawing is on a square this many units a side.
UNITS = 64.0
_CORNER = 14.0
_TILE = (0x0B, 0x0B, 0x12)
TEAL = (0x5D, 0xCA, 0xA5)
MAGENTA = (0xD4, 0x53, 0x7E)
# Each pair of eyes: its two centers, with the half width and half height of an eye.
_EYE = (4.5, 6.0)
_PAIRS = (((17.0, 25.0), (29.0, 25.0)), ((36.0, 42.0), (48.0, 42.0)))

Color = tuple[int, int, int]
# The color of each pair of eyes, by program.
PROGRAMS: dict[str, tuple[Color, Color]] = {
    "panel": (TEAL, MAGENTA),
    "bridge": (TEAL, TEAL),
    "server": (MAGENTA, MAGENTA),
}

# The sizes Windows asks for, from a list's small icon to a folder's largest. The largest is
# kept as a PNG, as Windows expects of it, and the others as plain bitmaps.
SIZES = (16, 20, 24, 32, 40, 48, 64, 256)
_PNG_FROM = 256
# Rows sampled within each pixel's height; across a row the shapes' edges are exact.
_SAMPLES = 8

Span = tuple[float, float]
Image = list[list[tuple[int, int, int, int]]]


def _tile_span(y: float) -> Span:
    """Where the rounded tile begins and ends on the row at `y`, in units."""
    from_edge = min(y, UNITS - y)
    if from_edge >= _CORNER:
        return 0.0, UNITS
    inset = _CORNER - math.sqrt(_CORNER**2 - (_CORNER - from_edge) ** 2)
    return inset, UNITS - inset


def _eye_span(center: tuple[float, float], y: float) -> Span | None:
    """Where an eye begins and ends on the row at `y`, if it reaches that row."""
    half_width, half_height = _EYE
    off = (y - center[1]) / half_height
    if abs(off) >= 1:
        return None
    half = half_width * math.sqrt(1 - off * off)
    return center[0] - half, center[0] + half


def _cover(row: list[float], span: Span, scale: float) -> None:
    """Adds to each pixel of `row` how much of its width `span`, in units, covers."""
    start, end = span[0] * scale, span[1] * scale
    for x in range(max(0, math.floor(start)), min(len(row), math.ceil(end))):
        row[x] += (min(x + 1, end) - max(x, start)) / _SAMPLES


def render(size: int, eyes: tuple[Color, Color]) -> Image:
    """The icon `size` pixels a side, as rows of red, green, blue and opacity."""
    scale = size / UNITS
    image: Image = []
    for y in range(size):
        tile = [0.0] * size
        pairs = ([0.0] * size, [0.0] * size)
        for sample in range(_SAMPLES):
            at = (y + (sample + 0.5) / _SAMPLES) / scale
            _cover(tile, _tile_span(at), scale)
            for pair, covered in zip(_PAIRS, pairs, strict=True):
                for center in pair:
                    span = _eye_span(center, at)
                    if span is not None:
                        _cover(covered, span, scale)
        row: list[tuple[int, int, int, int]] = []
        for x in range(size):
            first, second = pairs[0][x], pairs[1][x]
            dark = 1 - first - second
            mixed = (
                round(_TILE[part] * dark + eyes[0][part] * first + eyes[1][part] * second)
                for part in range(3)
            )
            red, green, blue = mixed
            row.append((red, green, blue, round(255 * min(1.0, tile[x]))))
        image.append(row)
    return image


def _chunk(kind: bytes, data: bytes) -> bytes:
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))


def png(image: Image) -> bytes:
    """`image` as a PNG file."""
    height, width = len(image), len(image[0])
    rows = b"".join(b"\0" + bytes(part for pixel in row for part in pixel) for row in image)
    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _chunk(b"IHDR", header)
        + _chunk(b"IDAT", zlib.compress(rows, 9))
        + _chunk(b"IEND", b"")
    )


def bitmap(image: Image) -> bytes:
    """`image` as an icon's bitmap: its pixels from the bottom row up, then a mask of the
    pixels that are clear, for a program that does not read their opacity."""
    size = len(image)
    # An icon's bitmap gives twice its height, to count the mask.
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = b"".join(
        bytes((blue, green, red, alpha))
        for row in reversed(image)
        for red, green, blue, alpha in row
    )
    mask_row_bytes = (size + 31) // 32 * 4
    mask = b""
    for row in reversed(image):
        bits = sum(1 << (size - 1 - x) for x, pixel in enumerate(row) if pixel[3] == 0)
        mask += (bits << (mask_row_bytes * 8 - size)).to_bytes(mask_row_bytes, "big")
    return header + pixels + mask


def pictures(eyes: tuple[Color, Color]) -> list[tuple[int, bytes]]:
    """The icon at every size, each as the bytes both files hold it in."""
    drawn = ((size, render(size, eyes)) for size in SIZES)
    return [(size, png(image) if size >= _PNG_FROM else bitmap(image)) for size, image in drawn]


def _entry(size: int, data: bytes) -> bytes:
    """The start of a picture's entry in either file's list; 0 stands for 256."""
    return struct.pack("<BBBBHHI", size % 256, size % 256, 0, 0, 1, 32, len(data))


def ico(images: list[tuple[int, bytes]]) -> bytes:
    """A .ico file of `images`: a list of them, then each."""
    start = 6 + 16 * len(images)
    entries = b""
    for size, data in images:
        entries += _entry(size, data) + struct.pack("<I", start)
        start += len(data)
    return struct.pack("<HHH", 0, 1, len(images)) + entries + b"".join(d for _, d in images)


_RT_ICON = 3
_RT_GROUP_ICON = 14
# The one group of each program. GPUI gives a window the icon of this number.
GROUP = 1
_ENGLISH_US = 0x0409
# How Windows may keep a resource in memory, as a resource compiler marks icons and groups.
_ICON_FLAGS = 0x1010
_GROUP_FLAGS = 0x1030


def _resource(kind: int, number: int, data: bytes, flags: int = 0, language: int = 0) -> bytes:
    """One resource of a .res file: a header of 32 bytes naming it by numbers, then its
    data, padded to four bytes."""
    name = struct.pack("<HHHH", 0xFFFF, kind, 0xFFFF, number)
    rest = struct.pack("<IHHII", 0, flags, language, 0, 0)
    return struct.pack("<II", len(data), 32) + name + rest + data + b"\0" * (-len(data) % 4)


def res(images: list[tuple[int, bytes]]) -> bytes:
    """A .res file of `images`: the empty resource that begins one, each picture numbered
    from 1, and the group that lists them."""
    group = struct.pack("<HHH", 0, 1, len(images))
    body = b""
    for number, (size, data) in enumerate(images, 1):
        body += _resource(_RT_ICON, number, data, _ICON_FLAGS, _ENGLISH_US)
        group += _entry(size, data) + struct.pack("<H", number)
    listed = _resource(_RT_GROUP_ICON, GROUP, group, _GROUP_FLAGS, _ENGLISH_US)
    return _resource(0, 0, b"") + body + listed


def build() -> dict[Path, bytes]:
    """Every file this writes, with what goes in it."""
    files: dict[Path, bytes] = {}
    for program, eyes in PROGRAMS.items():
        images = pictures(eyes)
        files[ICONS / f"{program}.ico"] = ico(images)
        files[ICONS / f"{program}.res"] = res(images)
    return files


def preview() -> Image:
    """Every program's icon side by side, large, with three small sizes under each."""
    large, small, gap = 256, (48, 32, 16), 16
    clear = (0, 0, 0, 0)
    sheet: Image = [
        [clear] * ((large + gap) * len(PROGRAMS)) for _ in range(large + gap + small[0])
    ]

    def place(image: Image, left: int, top: int) -> None:
        for y, row in enumerate(image):
            sheet[top + y][left : left + len(row)] = row

    for index, eyes in enumerate(PROGRAMS.values()):
        left = index * (large + gap)
        place(render(large, eyes), left, 0)
        for size in small:
            place(render(size, eyes), left, large + gap)
            left += size + gap
    return sheet


def main() -> None:
    if sys.argv[1:2] == ["--preview"] and len(sys.argv) == 3:
        Path(sys.argv[2]).write_bytes(png(preview()))
        return
    if len(sys.argv) != 1:
        raise SystemExit(__doc__)
    ICONS.mkdir(exist_ok=True)
    for path, data in build().items():
        path.write_bytes(data)
        print(f"wrote {path.name}: {len(data)} bytes")


if __name__ == "__main__":
    main()
