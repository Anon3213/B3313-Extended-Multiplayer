import struct

from make_icons import (
    GROUP,
    ICONS,
    MAGENTA,
    PROGRAMS,
    SIZES,
    TEAL,
    build,
    ico,
    pictures,
    render,
    res,
)

_PNG = b"\x89PNG\r\n\x1a\n"


def test_the_icons_the_programs_are_built_with_are_the_ones_drawn() -> None:
    # The files are built from the drawing and both are committed: a change to the drawing
    # that was not built would leave the programs with the old icon.
    for path, data in build().items():
        assert path.read_bytes() == data, "run scripts/make_icons.py"
    assert {path.name for path in build()} == {
        f"{program}.{kind}" for program in PROGRAMS for kind in ("ico", "res")
    }
    assert ICONS.is_dir()


def test_each_pair_of_eyes_has_its_programs_color_on_a_tile_with_clear_corners() -> None:
    def at(program: str, x: int, y: int) -> tuple[int, int, int, int]:
        return render(64, PROGRAMS[program])[y][x]

    # The middle of an eye of the upper pair, and of the lower.
    upper, lower = (17, 25), (48, 42)
    assert at("panel", *upper) == (*TEAL, 255)
    assert at("panel", *lower) == (*MAGENTA, 255)
    assert at("bridge", *lower) == (*TEAL, 255)
    assert at("server", *upper) == (*MAGENTA, 255)
    # Dark between the eyes, and nothing at all outside the tile's rounded corner.
    red, green, blue, opacity = at("panel", 32, 10)
    assert max(red, green, blue) < 0x20 and opacity == 255
    assert at("panel", 0, 0)[3] == 0
    assert at("panel", 63, 63)[3] == 0


def test_an_edge_is_smoothed_and_never_brighter_or_more_solid_than_whole() -> None:
    image = render(16, PROGRAMS["panel"])
    opacities = {pixel[3] for row in image for pixel in row}
    # Clear, solid, and the part-covered pixels of the rounded corners between.
    assert {0, 255} < opacities
    assert all(0 <= part <= 255 for row in image for pixel in row for part in pixel)


def test_an_ico_lists_every_size_and_where_its_picture_is() -> None:
    images = pictures(PROGRAMS["panel"])
    file = ico(images)
    reserved, kind, count = struct.unpack_from("<HHH", file)
    assert (reserved, kind, count) == (0, 1, len(SIZES))
    for index, (size, data) in enumerate(images):
        width, height, _, _, planes, bits, length, start = struct.unpack_from(
            "<BBBBHHII", file, 6 + 16 * index
        )
        # A byte cannot say 256, so 0 does.
        assert (width, height) == (size % 256, size % 256)
        assert (planes, bits) == (1, 32)
        assert file[start : start + length] == data
    assert sum(len(data) for _, data in images) + 6 + 16 * len(images) == len(file)


def test_the_largest_picture_is_a_png_and_the_others_bitmaps_with_their_masks() -> None:
    for size, data in pictures(PROGRAMS["server"]):
        if size == max(SIZES):
            assert data.startswith(_PNG)
            assert struct.unpack_from(">II", data, 16) == (size, size)
            continue
        header, width, height, planes, bits = struct.unpack_from("<IiiHH", data)
        # Twice the height, which counts the mask after the pixels.
        assert (header, width, height, planes, bits) == (40, size, size * 2, 1, 32)
        mask_row = (size + 31) // 32 * 4
        assert len(data) == 40 + size * size * 4 + size * mask_row
        # The bottom row's first pixel is in the tile's corner: clear, and so masked out.
        assert data[40 + 3] == 0
        assert data[40 + size * size * 4] & 0x80


def test_a_res_holds_each_picture_under_a_number_and_one_group_that_lists_them() -> None:
    images = pictures(PROGRAMS["bridge"])
    file = res(images)
    resources: list[tuple[int, int, bytes]] = []
    at = 0
    while at < len(file):
        length, header = struct.unpack_from("<II", file, at)
        marks = struct.unpack_from("<HHHH", file, at + 8)
        # Named by numbers, which a mark of 0xFFFF before each says.
        assert header == 32 and marks[0] == marks[2] == 0xFFFF
        resources.append((marks[1], marks[3], file[at + 32 : at + 32 + length]))
        at += 32 + length + -length % 4
    assert at == len(file)

    # Every .res file begins with an empty resource.
    assert resources[0] == (0, 0, b"")
    icons = resources[1:-1]
    assert [(kind, number) for kind, number, _ in icons] == [
        (3, number) for number in range(1, len(SIZES) + 1)
    ]
    assert [data for _, _, data in icons] == [data for _, data in images]

    kind, number, group = resources[-1]
    assert (kind, number) == (14, GROUP)
    assert struct.unpack_from("<HHH", group) == (0, 1, len(SIZES))
    for index, (size, data) in enumerate(images):
        width, _, _, _, _, bits, length, listed = struct.unpack_from(
            "<BBBBHHIH", group, 6 + 14 * index
        )
        assert (width, bits, length, listed) == (size % 256, 32, len(data), index + 1)
