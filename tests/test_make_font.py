import pytest
from make_font import (
    ATLAS_WIDTH,
    CELL_HEIGHT,
    CELL_WIDTH,
    CELLS,
    COLUMNS,
    FONT_BYTES,
    OUTPUT,
    SOURCE,
    build,
    outlined,
    read_glyphs,
)

_CLEAR, _OUTLINE, _FILL = 0x0, 0x1, 0xF


def _drawing() -> dict[str, list[str]]:
    return read_glyphs(SOURCE.read_text(encoding="utf-8"))


def _pixel(font: bytes, cell: int, x: int, y: int) -> int:
    """A pixel of a cell of the font's texture, which follows its advances."""
    at = (cell // COLUMNS * CELL_HEIGHT + y) * ATLAS_WIDTH + cell % COLUMNS * CELL_WIDTH + x
    return font[CELLS + at // 2] >> (0 if at % 2 else 4) & 0xF


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
