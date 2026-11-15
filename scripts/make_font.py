#!/usr/bin/env python3
"""Build the font the game draws names and chat in, from its drawing in font.txt.

Usage: make_font.py                          write TrollEngine/Multiplayer/font.bin
       make_font.py --preview FILE [TEXT...]   draw text with the font into a PNG, to look at

font.txt holds each glyph as rows of `#` and `.`. This gives each a dark outline, so text reads
against any background without a box behind it, and packs them all into one texture the game
loads whole (Multiplayer/people.c): 96 cells of 7 by 12 pixels, 16 to a row, in the N64's IA4
format, where a pixel is four bits. font.bin is each cell's advance, a byte apiece, then the
texture; the ROM carries it and the bridge measures text with its first 96 bytes, so the two
cannot disagree. Run this after changing font.txt and commit font.bin with it.
"""

import struct
import sys
import zlib
from pathlib import Path

MULTIPLAYER = Path(__file__).resolve().parent.parent / "TrollEngine" / "Multiplayer"
SOURCE = MULTIPLAYER / "font.txt"
OUTPUT = MULTIPLAYER / "font.bin"

# A glyph's own pixels, and the cell it is kept in with its outline around it.
GLYPH_WIDTH = 5
GLYPH_HEIGHT = 10
CELL_WIDTH = GLYPH_WIDTH + 2
CELL_HEIGHT = GLYPH_HEIGHT + 2
COLUMNS = 16
CELLS = 96
ATLAS_WIDTH = COLUMNS * CELL_WIDTH
ATLAS_HEIGHT = CELLS // COLUMNS * CELL_HEIGHT
# A byte for each cell's advance, then the texture at four bits a pixel: 4,128 bytes, which is
# all but 64 of what the console can hold of one texture at once.
FONT_BYTES = CELLS + ATLAS_WIDTH * ATLAS_HEIGHT // 2
# How far a space moves the next glyph on.
SPACE_ADVANCE = 4

# Cell n is the character with code 0x20 + n. A space is never drawn and 0x7F is no character,
# so their cells hold the player list's two marks instead.
MARKS = {"moderator": 0, "host": CELLS - 1}
_FIRST_CODE = 0x20

# IA4: three bits of brightness and one of opacity. The game tints the bright pixels with the
# color it draws a text in, and the dark ones stay dark.
_CLEAR, _OUTLINE, _FILL = 0x0, 0x1, 0xF


def read_glyphs(text: str) -> dict[str, list[str]]:
    """Each glyph's rows by its name: a character, or one of the marks."""
    glyphs: dict[str, list[str]] = {}
    name = None
    for number, line in enumerate(text.splitlines(), 1):
        if line.startswith("glyph "):
            name = line.removeprefix("glyph ")
            if name in glyphs:
                raise ValueError(f"line {number}: glyph {name} is drawn twice")
            glyphs[name] = []
        elif line and not line.startswith(";"):
            if name is None or set(line) - {"#", "."}:
                raise ValueError(f"line {number}: {line!r} is not a row of a glyph")
            glyphs[name].append(line)
    for name, rows in glyphs.items():
        widths = {len(row) for row in rows}
        if len(widths) != 1 or not 1 <= widths.pop() <= GLYPH_WIDTH:
            raise ValueError(f"glyph {name}: rows must share one width of 1 to {GLYPH_WIDTH}")
        if len(rows) > GLYPH_HEIGHT:
            raise ValueError(f"glyph {name}: more than {GLYPH_HEIGHT} rows")
    wanted = {chr(code) for code in range(_FIRST_CODE + 1, 0x7F)} | set(MARKS)
    if set(glyphs) != wanted:
        odd = sorted(set(glyphs) ^ wanted)
        raise ValueError(f"missing or unknown glyphs: {' '.join(odd)}")
    return glyphs


def cell_of(name: str) -> int:
    return MARKS[name] if name in MARKS else ord(name) - _FIRST_CODE


def outlined(rows: list[str]) -> list[list[int]]:
    """A glyph's cell: its pixels one in from the cell's edge, and every clear pixel that
    touches one of them, corners included, made the outline."""
    cell = [[_CLEAR] * CELL_WIDTH for _ in range(CELL_HEIGHT)]
    filled = {(x + 1, y + 1) for y, row in enumerate(rows) for x, c in enumerate(row) if c == "#"}
    for x, y in filled:
        for near_y in (y - 1, y, y + 1):
            for near_x in (x - 1, x, x + 1):
                cell[near_y][near_x] = _OUTLINE
    for x, y in filled:
        cell[y][x] = _FILL
    return cell


def build(glyphs: dict[str, list[str]]) -> bytes:
    """font.bin: each cell's advance, then the texture."""
    advances = bytearray(CELLS)
    advances[0] = SPACE_ADVANCE
    atlas = [[_CLEAR] * ATLAS_WIDTH for _ in range(ATLAS_HEIGHT)]
    for name, rows in glyphs.items():
        index = cell_of(name)
        if name not in MARKS:
            advances[index] = len(rows[0]) + 1
        left, top = index % COLUMNS * CELL_WIDTH, index // COLUMNS * CELL_HEIGHT
        for y, row in enumerate(outlined(rows)):
            atlas[top + y][left : left + CELL_WIDTH] = row
    texture = bytearray()
    for row in atlas:
        texture += bytes(row[x] << 4 | row[x + 1] for x in range(0, ATLAS_WIDTH, 2))
    return bytes(advances + texture)


def _write_png(path: Path, width: int, height: int, pixels: bytearray) -> None:
    def chunk(kind: bytes, data: bytes) -> bytes:
        checksum = zlib.crc32(kind + data)
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", checksum)

    rows = b"".join(b"\0" + pixels[y * width * 3 : (y + 1) * width * 3] for y in range(height))
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
    path.write_bytes(png + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


def preview(font: bytes, lines: list[str], path: Path, scale: int = 6) -> None:
    """Draw `lines` as the game would, each over a different color of scenery, enlarged."""
    backdrops = [(40, 90, 230), (60, 170, 40), (200, 180, 110), (150, 150, 150), (250, 250, 250)]
    advances, texture = font[:CELLS], font[CELLS:]
    widest = max(sum(advances[ord(c) - _FIRST_CODE] for c in line) for line in lines)
    width, row_height = widest + 2 * 4 + 1, CELL_HEIGHT + 2
    height = row_height * len(lines)
    screen = [
        [backdrops[y // row_height % len(backdrops)] for _ in range(width)] for y in range(height)
    ]
    for number, line in enumerate(lines):
        x, top = 4, number * row_height + 1
        for character in line:
            index = ord(character) - _FIRST_CODE
            if index != 0:
                left, up = index % COLUMNS * CELL_WIDTH, index // COLUMNS * CELL_HEIGHT
                for y in range(CELL_HEIGHT):
                    for dx in range(CELL_WIDTH):
                        at = (up + y) * ATLAS_WIDTH + left + dx
                        pixel = texture[at // 2] >> (0 if at % 2 else 4) & 0xF
                        if pixel != _CLEAR and x + dx < width:
                            shade = 255 if pixel == _FILL else 0
                            screen[top + y][x + dx] = (shade, shade, shade)
            x += advances[index]
    pixels = bytearray()
    for row in screen:
        for _ in range(scale):
            for color in row:
                pixels += bytes(color) * scale
    _write_png(path, width * scale, height * scale, pixels)


def main() -> None:
    font = build(read_glyphs(SOURCE.read_text(encoding="utf-8")))
    if len(sys.argv) >= 3 and sys.argv[1] == "--preview":
        sample = [
            "The quick brown fox jumps over the lazy dog.",
            "PACK MY BOX WITH FIVE DOZEN LIQUOR JUGS!",
            "0123456789 +-*/=<>% $#@&_ ()[]{} ,.;:'\"?!^~`|\\",
            "Luigi: meet me by the castle door when you",
            "xX_Wario_Xx  Princess_Peach  Toad #7  E. Gadd",
        ]
        preview(font, sys.argv[3:] or sample, Path(sys.argv[2]))
    elif len(sys.argv) == 1:
        OUTPUT.write_bytes(font)
        print(f"Wrote {OUTPUT.name}: {len(font)} bytes")
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
