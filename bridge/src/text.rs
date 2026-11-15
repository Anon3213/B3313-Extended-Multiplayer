//! Text for the game to draw: names and chat, as plain characters.
//!
//! The game draws them in a font of this project's own (`TrollEngine/Multiplayer/font.txt`),
//! whose glyphs differ in width. Rows of chat are cut here, where it is cheap, by how wide
//! that font draws them, so the game only draws what it is given (`Multiplayer/people.c`).

use crate::layout::{self, byte, len};

/// Ends every text, and pads what is left of its field.
pub const TEXT_END: u8 = byte::TEXT_END;
/// A name's field in the mailbox: up to 16 characters and the end code.
pub const NAME_BYTES: usize = len::NAME_BYTES;
/// A chat row's field in the mailbox: up to `ROW_BYTES - 1` characters and the end code.
pub const ROW_BYTES: usize = len::CHAT_ROW_BYTES;
/// The widest a row of chat may be drawn, in the game's pixels: the chat has the lower left of
/// a screen 320 across, short of the camera's mark in the lower right, and its box while a
/// line is typed is made this wide.
pub const ROW_PIXELS: u32 = layout::CHAT_ROW_PIXELS;
/// The most rows one chat line is cut into: enough for the longest message in letters of the
/// usual width.
pub const ROWS_PER_LINE: usize = 4;

/// The font as the ROM carries it, which begins with how far each glyph moves the next one
/// on: a byte for each character from the space, 0x20, to 0x7F.
const FONT: &[u8] = include_bytes!("../../TrollEngine/Multiplayer/font.bin");
const FIRST_GLYPH: u8 = b' ';
/// What the game draws for a character its font lacks.
const UNKNOWN: u8 = b'?';

/// `character` as the game will draw it: itself if the font has it, or a question mark.
const fn drawn(character: u8) -> u8 {
    if character >= FIRST_GLYPH && character <= b'~' {
        character
    } else {
        UNKNOWN
    }
}

/// How far `character` moves the next one on, in pixels.
fn advance(character: u8) -> u32 {
    let glyph = usize::from(drawn(character) - FIRST_GLYPH);
    FONT.get(glyph).copied().map_or(0, u32::from)
}

/// How wide the game draws `text`, in pixels: its glyphs, and the last one's outline.
#[must_use]
pub fn width(text: &str) -> u32 {
    1 + text.bytes().map(advance).sum::<u32>()
}

/// `text` as a mailbox field of `N` bytes: its first `N - 1` characters, then the end code to
/// the field's end.
#[must_use]
pub fn field<const N: usize>(text: &str) -> [u8; N] {
    let mut bytes = [TEXT_END; N];
    for (slot, character) in bytes.iter_mut().zip(text.bytes()).take(N.saturating_sub(1)) {
        *slot = drawn(character);
    }
    bytes
}

/// How many of `text`'s bytes make a row that fits both its field and the screen.
fn fitting(text: &str) -> usize {
    let mut pixels = 1;
    text.bytes()
        .take(ROW_BYTES - 1)
        .take_while(|&character| {
            pixels += advance(character);
            pixels <= ROW_PIXELS
        })
        .count()
}

/// The end of `text` that fits a row together with `before` and `after`: all of it if it is
/// short, and otherwise its newest characters, which someone still typing it wants to see.
#[must_use]
pub fn ending<'a>(text: &'a str, before: &str, after: &str) -> &'a str {
    let around = before.len() + after.len();
    let bytes = (ROW_BYTES - 1).saturating_sub(around);
    // Each width counts the one pixel of outline that only the row's last glyph adds.
    let pixels = (ROW_PIXELS + 1).saturating_sub(width(before) + width(after));
    let mut taken = 0;
    let kept = text
        .bytes()
        .rev()
        .take(bytes)
        .take_while(|&character| {
            taken += advance(character);
            taken <= pixels
        })
        .count();
    // What is typed is ASCII; text that is not is shown whole, and its field cuts it.
    text.get(text.len() - kept..).unwrap_or(text)
}

/// Cuts a chat line into rows that fit the chat's width, at spaces where it can, and returns
/// the first `ROWS_PER_LINE` of them, oldest first.
#[must_use]
pub fn rows(line: &str) -> Vec<[u8; ROW_BYTES]> {
    let mut rows = Vec::new();
    let mut rest = line.trim();
    while !rest.is_empty() && rows.len() < ROWS_PER_LINE {
        let fits = fitting(rest);
        let cut = if fits == rest.len() {
            fits
        } else {
            // The last space that leaves a row no longer than fits, counting one that falls
            // just after it, or the middle of a word too long to fit anywhere.
            let within = rest.get(..=fits).unwrap_or(rest);
            within.rfind(' ').filter(|&at| at > 0).unwrap_or(fits)
        };
        // A line is ASCII by the time it is here; one that is not is cut where it can be.
        let (row, after) = rest.split_at_checked(cut.max(1)).unwrap_or((rest, ""));
        rows.push(field(row));
        rest = after.trim_start();
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{
        NAME_BYTES, ROW_BYTES, ROW_PIXELS, ROWS_PER_LINE, TEXT_END, ending, field, rows, width,
    };

    /// A row's text, up to its end code.
    fn text(row: &[u8; ROW_BYTES]) -> &str {
        let end = row.iter().position(|&byte| byte == TEXT_END);
        std::str::from_utf8(row.get(..end.unwrap_or(ROW_BYTES)).unwrap_or_default())
            .unwrap_or_default()
    }

    #[test]
    fn text_is_written_as_it_is_typed_and_ends() {
        // Every mark people type has a glyph; what is no character at all is a question mark.
        let name: [u8; NAME_BYTES] = field("Az_09 #!\u{7f}\t");
        assert_eq!(name.get(..10), Some(&b"Az_09 #!??"[..]));
        assert!(name.iter().skip(10).all(|&byte| byte == TEXT_END));
    }

    #[test]
    fn a_name_too_long_for_its_field_is_cut_and_still_ends() {
        let name: [u8; NAME_BYTES] = field(&"x".repeat(40));
        assert_eq!(name.last(), Some(&TEXT_END));
        let letters = name.iter().filter(|&&byte| byte != TEXT_END).count();
        assert_eq!(letters, NAME_BYTES - 1);
    }

    #[test]
    fn glyphs_differ_in_width_and_text_is_as_wide_as_its_glyphs() {
        // An "i" is one pixel wide and an "m" five; each is followed by a pixel of outline,
        // and the last by one more.
        assert_eq!(width("i"), 3);
        assert_eq!(width("m"), 7);
        assert_eq!(width("im"), 9);
        assert!(width("iiii") < width("mm"));
        assert_eq!(width(""), 1);
    }

    #[test]
    fn a_long_line_is_cut_at_spaces_into_rows_that_fit() {
        let line = "Luigi: meet me by the castle door when you have the key, and bring coins";
        let cut = rows(line);
        assert!(cut.len() > 1);
        assert!(cut.iter().all(|row| width(text(row)) <= ROW_PIXELS));
        // No word is split: the first row ends where a space was, and nothing is lost but
        // that space.
        let joined: Vec<&str> = cut.iter().map(text).collect();
        assert_eq!(joined.join(" "), line);
    }

    #[test]
    fn narrow_letters_fill_a_row_by_its_field_and_wide_ones_by_the_screen() {
        // Fifty-one of the narrowest fit the screen with room to spare, so the field ends
        // the row; of the widest, the screen does first.
        // More of each than the rows of one line hold, so every row is a full one.
        let plenty = ROW_BYTES * (ROWS_PER_LINE + 1);
        let narrow = rows(&"i".repeat(plenty));
        assert_eq!(narrow.len(), ROWS_PER_LINE);
        assert!(narrow.iter().all(|row| text(row).len() == ROW_BYTES - 1));
        let wide = rows(&"W".repeat(plenty));
        assert_eq!(wide.len(), ROWS_PER_LINE);
        for row in &wide {
            let drawn = width(text(row));
            assert!(drawn <= ROW_PIXELS && drawn + width("W") > ROW_PIXELS);
            assert!(text(row).len() < ROW_BYTES - 1);
        }
        assert_eq!(rows("   "), Vec::<[u8; ROW_BYTES]>::new());
    }

    #[test]
    fn a_line_being_typed_shows_its_end_once_it_is_too_long_for_a_row() {
        assert_eq!(ending("hello", "> ", "_"), "hello");

        // Wide letters: as many of the last as the row's width has room for, and no more.
        let wide = "m".repeat(100);
        let shown = ending(&wide, "> ", "_");
        assert!(width(&format!("> {shown}_")) <= ROW_PIXELS);
        assert!(width(&format!("> m{shown}_")) > ROW_PIXELS);

        // Narrow ones: as many as the row's field has room for.
        let narrow = "i".repeat(100);
        assert_eq!(
            format!("> {}_", ending(&narrow, "> ", "_")).len(),
            ROW_BYTES - 1
        );

        // The newest, not the oldest.
        let counted = "0123456789".repeat(10);
        let shown = ending(&counted, "> ", "_");
        assert!(shown.len() < counted.len() && counted.ends_with(shown));
    }
}
