//! The text and selection of a one-line text field, apart from drawing it.
//!
//! Offsets are UTF-8 byte offsets that always fall on character boundaries. The platform's input
//! handler speaks UTF-16, so conversions for it are here too.

use std::ops::Range;

/// The most a field holds, in bytes: more than any path, address or line of chat it is for.
const MOST_TEXT: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditBuffer {
    text: String,
    /// Never reversed: `start <= end`. Empty when only the cursor shows.
    selection: Range<usize>,
    /// Whether the cursor is at the selection's start, as after selecting leftward.
    reversed: bool,
    /// Text an input method is composing, not yet committed.
    marked: Option<Range<usize>>,
}

impl EditBuffer {
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces everything, leaving the cursor at the end.
    pub fn set_text(&mut self, text: &str) {
        text.clone_into(&mut self.text);
        self.selection = self.text.len()..self.text.len();
        self.reversed = false;
        self.marked = None;
    }

    #[must_use]
    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    #[must_use]
    pub fn is_reversed(&self) -> bool {
        self.reversed
    }

    #[must_use]
    pub fn marked(&self) -> Option<Range<usize>> {
        self.marked.clone()
    }

    #[must_use]
    pub fn cursor(&self) -> usize {
        if self.reversed {
            self.selection.start
        } else {
            self.selection.end
        }
    }

    #[must_use]
    pub fn selected_text(&self) -> &str {
        self.text.get(self.selection.clone()).unwrap_or_default()
    }

    pub fn move_to(&mut self, offset: usize) {
        let offset = self.boundary_at_or_before(offset);
        self.selection = offset..offset;
        self.reversed = false;
    }

    /// Moves the cursor to `offset`, keeping the other end of the selection where it is.
    pub fn select_to(&mut self, offset: usize) {
        let offset = self.boundary_at_or_before(offset);
        if self.reversed {
            self.selection.start = offset;
        } else {
            self.selection.end = offset;
        }
        if self.selection.end < self.selection.start {
            self.reversed = !self.reversed;
            self.selection = self.selection.end..self.selection.start;
        }
    }

    pub fn select_all(&mut self) {
        self.selection = 0..self.text.len();
        self.reversed = false;
    }

    /// Left arrow: one character back, or to the start of a selection.
    pub fn left(&mut self) {
        if self.selection.is_empty() {
            self.move_to(self.previous_boundary(self.cursor()));
        } else {
            self.move_to(self.selection.start);
        }
    }

    /// Right arrow: one character on, or to the end of a selection.
    pub fn right(&mut self) {
        if self.selection.is_empty() {
            self.move_to(self.next_boundary(self.cursor()));
        } else {
            self.move_to(self.selection.end);
        }
    }

    pub fn select_left(&mut self) {
        self.select_to(self.previous_boundary(self.cursor()));
    }

    pub fn select_right(&mut self) {
        self.select_to(self.next_boundary(self.cursor()));
    }

    /// Deletes the selection, or the character before the cursor.
    pub fn backspace(&mut self) {
        if self.selection.is_empty() {
            self.select_to(self.previous_boundary(self.cursor()));
        }
        self.replace(None, "");
    }

    /// Deletes the selection, or the character after the cursor.
    pub fn delete(&mut self) {
        if self.selection.is_empty() {
            self.select_to(self.next_boundary(self.cursor()));
        }
        self.replace(None, "");
    }

    /// `new_text` as it goes in place of `range`: on one line, its line breaks made spaces,
    /// and cut short where the field would otherwise pass `MOST_TEXT`. A field is drawn whole
    /// in every frame, so a clipboard of megabytes pasted into one would stall the window.
    fn fitting(&self, range: &Range<usize>, new_text: &str) -> String {
        let kept = self.text.len() - range.len();
        let mut room = MOST_TEXT.saturating_sub(kept).min(new_text.len());
        while !new_text.is_char_boundary(room) {
            room -= 1;
        }
        new_text
            .get(..room)
            .unwrap_or_default()
            .replace(['\r', '\n'], " ")
    }

    /// Replaces `range`, else the text being composed, else the selection, with `new_text`, and
    /// puts the cursor after it. Line breaks become spaces: the field holds one line.
    pub fn replace(&mut self, range: Option<Range<usize>>, new_text: &str) {
        let range = self.target(range);
        let new_text = self.fitting(&range, new_text);
        self.text.replace_range(range.clone(), &new_text);
        let end = range.start + new_text.len();
        self.selection = end..end;
        self.reversed = false;
        self.marked = None;
    }

    /// As `replace`, but marks the new text as being composed, with `selected` (relative to the
    /// new text) as the selection.
    pub fn replace_and_mark(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        selected: Option<Range<usize>>,
    ) {
        let range = self.target(range);
        let new_text = self.fitting(&range, new_text);
        self.text.replace_range(range.clone(), &new_text);
        let end = range.start + new_text.len();
        self.marked = (!new_text.is_empty()).then_some(range.start..end);
        self.selection = match selected {
            Some(selected) => {
                let start = self.boundary_at_or_before(range.start + selected.start);
                start
                    ..self
                        .boundary_at_or_before(range.start + selected.end)
                        .max(start)
            }
            None => end..end,
        };
        self.reversed = false;
    }

    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// The UTF-8 offset of a UTF-16 offset, as the platform counts.
    #[must_use]
    pub fn offset_from_utf16(&self, utf16: usize) -> usize {
        let mut counted = 0;
        for (offset, character) in self.text.char_indices() {
            if counted >= utf16 {
                return offset;
            }
            counted += character.len_utf16();
        }
        self.text.len()
    }

    #[must_use]
    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        self.text
            .char_indices()
            .take_while(|&(index, _)| index < offset)
            .map(|(_, character)| character.len_utf16())
            .sum()
    }

    #[must_use]
    pub fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    #[must_use]
    pub fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    /// The range an edit applies to, kept within the text and on character boundaries.
    fn target(&self, range: Option<Range<usize>>) -> Range<usize> {
        let range = range
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.selection.clone());
        let start = self.boundary_at_or_before(range.start);
        start..self.boundary_at_or_before(range.end).max(start)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.text
            .char_indices()
            .rev()
            .map(|(index, _)| index)
            .find(|&index| index < offset)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.text
            .char_indices()
            .map(|(index, _)| index)
            .find(|&index| index > offset)
            .unwrap_or(self.text.len())
    }

    fn boundary_at_or_before(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }
}

#[cfg(test)]
mod tests {
    use super::EditBuffer;

    fn buffer(text: &str) -> EditBuffer {
        let mut buffer = EditBuffer::default();
        buffer.set_text(text);
        buffer
    }

    #[test]
    fn typing_replaces_the_selection_and_moves_the_cursor_after_it() {
        let mut field = buffer("192.168.0.1");
        field.move_to(8);
        field.select_to(11);
        assert_eq!(field.selected_text(), "0.1");
        field.replace(None, "1.20");
        assert_eq!(field.text(), "192.168.1.20");
        assert_eq!(field.selection(), 12..12);
    }

    #[test]
    fn a_field_takes_only_so_much_text_and_only_whole_characters() {
        let mut field = buffer("ab");
        field.replace(None, &"ñ".repeat(super::MOST_TEXT));
        // Two bytes each, after the two already there: as many as fit, and no half of one.
        assert_eq!(field.text().len(), super::MOST_TEXT);
        assert_eq!(field.text().chars().count(), 2 + (super::MOST_TEXT - 2) / 2);
        field.replace(None, "c");
        assert_eq!(field.text().len(), super::MOST_TEXT, "full");
        // What is selected makes room for what replaces it.
        field.select_left();
        field.replace(None, "de");
        assert!(field.text().ends_with("de"));
    }

    #[test]
    fn deleting_works_on_whole_characters() {
        let mut field = buffer("añb");
        field.left();
        field.backspace();
        assert_eq!(field.text(), "ab");
        assert_eq!(field.cursor(), 1);
        field.delete();
        assert_eq!(field.text(), "a");
        field.backspace();
        field.backspace();
        assert_eq!(field.text(), "", "backspace at the start does nothing");
    }

    #[test]
    fn a_selection_can_cross_its_anchor() {
        let mut field = buffer("abcd");
        field.move_to(2);
        field.select_right();
        field.select_left();
        field.select_left();
        assert_eq!(field.selection(), 1..2);
        assert!(field.is_reversed());
        assert_eq!(field.cursor(), 1);
        field.right();
        assert_eq!(
            field.selection(),
            2..2,
            "right collapses to the selection's end"
        );
    }

    #[test]
    fn pasted_line_breaks_become_spaces() {
        let mut field = buffer("");
        field.replace(None, "one\r\ntwo\n");
        assert_eq!(field.text(), "one  two ");
    }

    #[test]
    fn composition_is_marked_until_committed() {
        let mut field = buffer("x");
        field.replace_and_mark(None, "ni", Some(2..2));
        assert_eq!(field.text(), "xni");
        assert_eq!(field.marked(), Some(1..3));
        assert_eq!(field.selection(), 3..3);
        // Committing replaces what was composed.
        field.replace(None, "你");
        assert_eq!(field.text(), "x你");
        assert_eq!(field.marked(), None);
        assert_eq!(field.cursor(), "x你".len());
    }

    #[test]
    fn utf16_offsets_map_to_character_boundaries() {
        // 'é' is one UTF-16 unit and two bytes; '😀' is two units and four bytes.
        let field = buffer("é😀z");
        assert_eq!(field.offset_from_utf16(1), 2);
        assert_eq!(field.offset_from_utf16(3), 6);
        assert_eq!(field.offset_from_utf16(99), 7);
        assert_eq!(field.offset_to_utf16(6), 3);
        assert_eq!(field.range_to_utf16(&(0..7)), 0..4);
    }

    #[test]
    fn edits_far_outside_the_text_stay_inside_it() {
        let mut field = buffer("ab");
        field.replace(Some(5..9), "c");
        assert_eq!(field.text(), "abc");
        field.move_to(100);
        assert_eq!(field.cursor(), 3);
        let mut accented = buffer("é");
        accented.replace(Some(1..1), "x");
        assert_eq!(
            accented.text(),
            "xé",
            "an offset inside a character moves to its start"
        );
    }
}
