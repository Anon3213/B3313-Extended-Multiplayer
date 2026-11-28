import struct

import pytest
from make_font import (
    CELL_HEIGHT,
    CELL_WIDTH,
    FONT_BYTES,
    LAYER_CELLS,
    OUTPUT,
    PALETTE_BYTES,
    SOURCE,
    build,
    outlined,
    pixel_of,
    read_glyphs,
)

_CLEAR, _OUTLINE, _FILL = 0, 1, 2


def _drawing() -> dict[str, list[str]]:
    return read_glyphs(SOURCE.read_text(encoding="utf-8"))


def _pixel(font: bytes, cell: int, x: int, y: int) -> int:
    return pixel_of(font, cell, x, y)


def test_the_font_the_rom_carries_is_the_one_drawn() -> None:
    # font.bin is built from font.txt and both are committed: a change to the drawing that
    # was not built would leave the game and the bridge with the old font.
    assert OUTPUT.read_bytes() == build(_drawing()), "run scripts/make_font.py"


def test_the_font_fills_the_room_the_game_gives_it() -> None:
    font = build(_drawing())
    assert len(font) == FONT_BYTES
    # Every character from the space to the tilde moves the next one on.
    assert all(font[code - 0x20] > 0 for code in range(0x20, 0x7F))


def test_a_glyph_is_outlined_on_every_side_and_kept_in_its_own_cell() -> None:
    font = build(_drawing())
    stop = ord(".") - 0x20
    # A full stop is one pixel, low in its cell: bright, with dark all around it, corners too.
    bright = [
        (x, y)
        for y in range(CELL_HEIGHT)
        for x in range(CELL_WIDTH)
        if _pixel(font, stop, x, y) == _FILL
    ]
    assert bright == [(1, 8)]
    for y in range(CELL_HEIGHT):
        for x in range(CELL_WIDTH):
            near = abs(x - 1) <= 1 and abs(y - 8) <= 1
            expected = _FILL if (x, y) == (1, 8) else _OUTLINE if near else _CLEAR
            assert _pixel(font, stop, x, y) == expected
    # It is one pixel wide, and the next glyph starts a pixel after it.
    assert font[stop] == 2


def test_the_two_images_share_the_texture_and_each_palette_sees_one() -> None:
    font = build(_drawing())
    # A full stop is in the first image and a caret in the second, in the same place: low and
    # high in it, each comes out of the texels they share as it was drawn.
    stop, caret = ord(".") - 0x20, ord("^") - 0x20
    assert stop < LAYER_CELLS <= caret
    assert stop % LAYER_CELLS == caret % LAYER_CELLS
    assert (_pixel(font, stop, 1, 8), _pixel(font, caret, 1, 8)) == (_FILL, _CLEAR)
    assert (_pixel(font, stop, 3, 1), _pixel(font, caret, 3, 1)) == (_CLEAR, _FILL)
    shades = struct.unpack(">32H", font[-PALETTE_BYTES:])
    first, second = shades[:16], shades[16:]
    # A texel with the fill in its high bits and the outline in its low.
    texel = _FILL << 2 | _OUTLINE
    assert (first[texel], second[texel]) == (0xFFFF, 0x00FF)
    assert first[_OUTLINE] == 0 and second[_FILL << 2] == 0


def test_the_widest_and_tallest_glyph_still_has_its_outline() -> None:
    cell = outlined(["#####"] * 10)
    assert [row[0] for row in cell] == [_OUTLINE] * CELL_HEIGHT
    assert cell[0] == [_OUTLINE] * CELL_WIDTH and cell[-1] == [_OUTLINE] * CELL_WIDTH
    assert all(pixel == _FILL for row in cell[1:-1] for pixel in row[1:-1])


@pytest.mark.parametrize(
    ("drawn", "changed", "complaint"),
    [
        ("glyph A\n.###.", "glyph A\n.###", "share one width"),
        ("glyph A\n.###.", "glyph A\n.####.", "share one width"),
        ("glyph B\n", "glyph A\n", "drawn twice"),
        ("glyph ~\n", "glyph tilde\n", "missing or unknown"),
        ("glyph A\n.###.", "glyph A\n.#x#.", "not a row"),
        ("glyph |\n", "glyph |\n#\n#\n", "more than"),
    ],
    ids=["ragged", "too wide", "twice", "missing", "stray mark", "too tall"],
)
def test_a_drawing_that_cannot_be_packed_is_refused(
    drawn: str, changed: str, complaint: str
) -> None:
    drawing = SOURCE.read_text(encoding="utf-8")
    assert drawn in drawing
    with pytest.raises(ValueError, match=complaint):
        read_glyphs(drawing.replace(drawn, changed))
