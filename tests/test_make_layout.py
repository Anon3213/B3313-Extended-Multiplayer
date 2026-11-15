import pytest
from make_layout import HEADER, PYTHON, RUST, read_layout, render_python, render_rust

_HEADER = """\
#define MP_MAILBOX_ADDR 0x807C0200
#define MP_ROWS 8 // A note.
#define MP_NO_LEVEL (-1)
#define MP_FONT_ADDR MP_MAILBOX_ADDR

struct MpChat {
    /*0x000*/ u32 count;
    /*0x004*/ u8 shown;
    /*0x005*/ u8 pad[3];
    /*0x008*/ u8 rows[MP_ROWS][52];
    /*0x1A8*/ u8 localName[20];
    /*0x1BC*/ u8 reserved2[4];
};

typedef char MpChatSizeCheck[sizeof(struct MpChat) == 0x1C0 ? 1 : -1];
"""


def test_the_numbers_the_header_states_are_read_and_nothing_else() -> None:
    # Plain numbers only: not one made of another name, nor the room kept between parts.
    assert read_layout(_HEADER) == {
        "MAILBOX_ADDR": 0x807C0200,
        "ROWS": 8,
        "NO_LEVEL": -1,
        "CHAT_COUNT_OFFSET": 0,
        "CHAT_SHOWN_OFFSET": 4,
        "CHAT_ROWS_OFFSET": 8,
        "CHAT_LOCAL_NAME_OFFSET": 0x1A8,
        "CHAT_SIZE": 0x1C0,
    }


def test_a_name_the_header_gives_twice_is_refused() -> None:
    twice = "#define MP_CHAT_SIZE 4\n" + _HEADER
    with pytest.raises(ValueError, match="CHAT_SIZE twice"):
        read_layout(twice)


def test_each_number_is_written_for_the_bridge_in_the_types_it_uses() -> None:
    layout = {"MAILBOX_ADDR": 0x807C0200, "ROWS": 8, "CHAT_SIZE": 0x1C0, "NO_LEVEL": -1}
    rust = render_rust(layout)

    assert "pub const MAILBOX_ADDR: u32 = 0x807C_0200;\n" in rust
    assert "    pub const CHAT_SIZE: usize = 0x1C0;\n" in rust
    # Only what fits a byte is offered as one.
    assert "    pub const ROWS: u8 = 8;\n" in rust
    assert "CHAT_SIZE: u8" not in rust
    assert "MAILBOX_ADDR: u8" not in rust
    # A level's stand-in has a level's type, and is no length or byte.
    assert rust.count("NO_LEVEL") == 1
    assert "pub const NO_LEVEL: i16 = -1;\n" in rust
    assert "NO_LEVEL = -1\n" in render_python(layout)


def test_the_layout_files_are_the_ones_the_header_gives() -> None:
    # A header changed without running make_layout.py has the bridge read and write the
    # game's RAM by the old layout.
    layout = read_layout(HEADER.read_text(encoding="utf-8"))

    assert RUST.read_text(encoding="utf-8") == render_rust(layout), "run scripts/make_layout.py"
    # The Python file is for tools outside this repository, and is checked where it is.
    if PYTHON.exists():
        assert PYTHON.read_text(encoding="utf-8") == render_python(layout), "run make_layout.py"
    # And it found the layout: a header it could not read would agree with two empty files.
    assert layout["MAILBOX_VERSION"] > 0
    assert layout["MAILBOX_CHAT_OFFSET"] + layout["CHAT_SIZE"] == layout["MAILBOX_SIZE"]
