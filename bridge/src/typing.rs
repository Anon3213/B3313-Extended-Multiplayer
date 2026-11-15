//! The line of chat a player types in the game.
//!
//! The game has no keyboard and little room for code, so the line is kept here: the bridge
//! reads the PC's keys (`keyboard`), this turns them into a line, and the game is given that
//! line as it stands, with a prompt and a cursor, to draw under its chat
//! (`TrollEngine/Multiplayer/people.c`).

use crate::text::{self, ROW_BYTES};
use b3313_protocol::{MAX_MESSAGE_LENGTH, Message};
use std::time::{Duration, Instant};

/// What starts the row the game draws, and the cursor that ends it.
const PROMPT: &str = "> ";
const CURSOR: &str = "_";
/// How long the cursor shows, and then hides, while nothing is typed.
const BLINK: Duration = Duration::from_millis(500);

/// The key that opens the chat unless the player chooses another: where many games have it,
/// and bound to nothing in the keyboard layout Parallel Launcher gives the game.
pub const DEFAULT_CHAT_KEY: char = 't';

/// The key a player named to open the chat with: one letter or digit, which every keyboard
/// types without Shift. Anything else, as "off", is none: the keyboard is then left unread.
#[must_use]
pub fn chat_key(named: &str) -> Option<char> {
    let mut characters = named.trim().chars();
    match (characters.next(), characters.next()) {
        (Some(key), None) if key.is_ascii_alphanumeric() => Some(key.to_ascii_lowercase()),
        _ => None,
    }
}

/// How many seconds the game shows the chat's rows after the newest arrived, unless the
/// player chooses otherwise, and the fewest and the most they may choose.
pub const DEFAULT_CHAT_SECONDS: u8 = 8;
pub const CHAT_SECONDS_MIN: u8 = 3;
pub const CHAT_SECONDS_MAX: u8 = 120;

/// The seconds a player named for the chat to show, if they are a number that may be chosen.
#[must_use]
pub fn chat_seconds(named: &str) -> Option<u8> {
    let seconds = named.trim().parse().ok()?;
    (CHAT_SECONDS_MIN..=CHAT_SECONDS_MAX)
        .contains(&seconds)
        .then_some(seconds)
}

/// A key pressed while the game's window has the keyboard, as the chat takes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// The key that opens the chat, pressed while it is closed.
    Chat,
    /// The slash, pressed while the chat is closed: it opens the chat with a command begun.
    Command,
    /// A character typed into the open chat.
    Character(char),
    Backspace,
    Enter,
    Escape,
}

/// The line being typed, if one is.
#[derive(Debug, Default)]
pub struct Draft {
    line: Option<String>,
    /// When a key last changed it, which the cursor's blinking starts over from.
    touched: Option<Instant>,
}

impl Draft {
    /// Whether a line is being typed, so keys are its characters and not the game's.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.line.is_some()
    }

    /// Takes a key pressed at `now`, and returns the line to send if the key finished one.
    ///
    /// Enter sends the line and closes the chat; an empty line just closes it, as do Escape
    /// and a Backspace with nothing left to remove.
    pub fn press(&mut self, key: Key, now: Instant) -> Option<Message> {
        let Some(line) = &mut self.line else {
            match key {
                Key::Chat => self.line = Some(String::new()),
                Key::Command => self.line = Some("/".to_owned()),
                Key::Character(_) | Key::Backspace | Key::Enter | Key::Escape => return None,
            }
            self.touched = Some(now);
            return None;
        };
        self.touched = Some(now);
        match key {
            // What the font has no glyph for is not typed; nor more than a message holds.
            Key::Character(character) => {
                if (character == ' ' || character.is_ascii_graphic())
                    && line.len() < MAX_MESSAGE_LENGTH
                {
                    line.push(character);
                }
                None
            }
            Key::Backspace => {
                if line.pop().is_none() {
                    self.line = None;
                }
                None
            }
            Key::Enter => self.line.take().as_deref().and_then(Message::new),
            Key::Escape => {
                self.line = None;
                None
            }
            // Only pressed while the chat is closed.
            Key::Chat | Key::Command => None,
        }
    }

    /// Closes the chat and forgets the line, as when the player turns to another window.
    pub fn close(&mut self) {
        self.line = None;
    }

    /// The row the game draws for the line at `now`, if one is being typed: the prompt, the
    /// line, or its end if it is too long for a row, and the cursor, which blinks.
    #[must_use]
    pub fn row(&self, now: Instant) -> Option<[u8; ROW_BYTES]> {
        let line = self.line.as_deref()?;
        // The cursor's room is kept while it is hidden, so a long line does not shift.
        let shown = text::ending(line, PROMPT, CURSOR);
        let since = self
            .touched
            .map_or(Duration::ZERO, |at| now.saturating_duration_since(at));
        let cursor = if (since.as_millis() / BLINK.as_millis()).is_multiple_of(2) {
            CURSOR
        } else {
            ""
        };
        Some(text::field(&format!("{PROMPT}{shown}{cursor}")))
    }
}

#[cfg(test)]
mod tests {
    use super::{BLINK, Draft, Key, chat_key, chat_seconds};
    use crate::text::{ROW_BYTES, TEXT_END};
    use b3313_protocol::MAX_MESSAGE_LENGTH;
    use std::time::Instant;

    fn typed(draft: &mut Draft, text: &str, now: Instant) {
        for character in text.chars() {
            assert_eq!(draft.press(Key::Character(character), now), None);
        }
    }

    /// The row as text, up to its end code.
    fn shown(draft: &Draft, now: Instant) -> Option<String> {
        let row: [u8; ROW_BYTES] = draft.row(now)?;
        let end = row.iter().position(|&byte| byte == TEXT_END)?;
        Some(String::from_utf8_lossy(row.get(..end)?).into_owned())
    }

    #[test]
    fn a_line_is_opened_typed_and_sent_with_enter() {
        let now = Instant::now();
        let mut draft = Draft::default();
        // Closed, keys are the game's: nothing is typed, and there is nothing to draw.
        typed(&mut draft, "w", now);
        assert_eq!(draft.press(Key::Enter, now), None);
        assert!(!draft.is_open());
        assert_eq!(shown(&draft, now), None);

        draft.press(Key::Chat, now);
        assert!(draft.is_open());
        assert_eq!(shown(&draft, now).as_deref(), Some("> _"));
        typed(&mut draft, "hello thete", now);
        draft.press(Key::Backspace, now);
        draft.press(Key::Backspace, now);
        typed(&mut draft, "re", now);
        assert_eq!(shown(&draft, now).as_deref(), Some("> hello there_"));

        let sent = draft.press(Key::Enter, now);
        assert_eq!(
            sent.as_ref().map(b3313_protocol::Message::as_str),
            Some("hello there")
        );
        assert!(!draft.is_open());
    }

    #[test]
    fn the_slash_opens_the_chat_with_a_command_begun() {
        let now = Instant::now();
        let mut draft = Draft::default();
        draft.press(Key::Command, now);
        typed(&mut draft, "end", now);
        let sent = draft.press(Key::Enter, now);
        assert_eq!(
            sent.as_ref().map(b3313_protocol::Message::as_str),
            Some("/end")
        );
    }

    #[test]
    fn the_chat_closes_without_sending_in_three_ways() {
        let now = Instant::now();
        let mut draft = Draft::default();

        draft.press(Key::Chat, now);
        typed(&mut draft, "never mind", now);
        assert_eq!(draft.press(Key::Escape, now), None);
        assert!(!draft.is_open());

        // Nothing typed, or nothing but spaces, is nothing to send.
        draft.press(Key::Chat, now);
        typed(&mut draft, "  ", now);
        assert_eq!(draft.press(Key::Enter, now), None);
        assert!(!draft.is_open());

        // Backspace removes what was typed, and then, with nothing left, closes.
        draft.press(Key::Chat, now);
        typed(&mut draft, "a", now);
        draft.press(Key::Backspace, now);
        assert!(draft.is_open());
        draft.press(Key::Backspace, now);
        assert!(!draft.is_open());

        // What was typed before is gone when the chat opens again.
        draft.press(Key::Chat, now);
        assert_eq!(shown(&draft, now).as_deref(), Some("> _"));
    }

    #[test]
    fn only_what_the_game_can_draw_is_typed_and_no_more_than_a_message_holds() {
        let now = Instant::now();
        let mut draft = Draft::default();
        draft.press(Key::Chat, now);
        typed(&mut draft, "a\u{e9}\tb\u{7f}", now);
        assert_eq!(shown(&draft, now).as_deref(), Some("> ab_"));

        typed(&mut draft, &"x".repeat(MAX_MESSAGE_LENGTH * 2), now);
        let sent = draft.press(Key::Enter, now);
        assert_eq!(
            sent.map(|line| line.as_str().len()),
            Some(MAX_MESSAGE_LENGTH)
        );
    }

    #[test]
    fn the_cursor_blinks_and_shows_again_at_each_key() {
        let start = Instant::now();
        let mut draft = Draft::default();
        draft.press(Key::Chat, start);
        assert_eq!(shown(&draft, start).as_deref(), Some("> _"));
        assert_eq!(shown(&draft, start + BLINK).as_deref(), Some("> "));
        assert_eq!(shown(&draft, start + BLINK * 2).as_deref(), Some("> _"));

        // Typed while it is hidden: it shows at once, after the new character.
        draft.press(Key::Character('a'), start + BLINK);
        assert_eq!(shown(&draft, start + BLINK).as_deref(), Some("> a_"));
    }

    #[test]
    fn the_key_that_opens_the_chat_is_one_letter_or_digit_or_none() {
        assert_eq!(chat_key("t"), Some('t'));
        assert_eq!(chat_key(" Y "), Some('y'));
        assert_eq!(chat_key("5"), Some('5'));
        // The slash opens the chat for a command whatever key is chosen, and marks sit on
        // different keys from one keyboard to the next.
        assert_eq!(chat_key("/"), None);
        assert_eq!(chat_key("off"), None);
        assert_eq!(chat_key(""), None);
    }

    #[test]
    fn the_time_the_chat_shows_is_a_number_of_seconds_within_bounds() {
        assert_eq!(chat_seconds("8"), Some(8));
        assert_eq!(chat_seconds(" 120 "), Some(120));
        // Too short to read, or as good as always.
        assert_eq!(chat_seconds("2"), None);
        assert_eq!(chat_seconds("121"), None);
        assert_eq!(chat_seconds("long"), None);
    }

    #[test]
    fn a_line_longer_than_a_row_shows_its_end() {
        let now = Instant::now();
        let mut draft = Draft::default();
        draft.press(Key::Chat, now);
        typed(&mut draft, &"m".repeat(60), now);
        typed(&mut draft, "END", now);
        let row = shown(&draft, now).unwrap_or_default();
        assert!(row.starts_with("> m") && row.ends_with("END_"), "{row}");
        assert!(row.len() < 60);
    }
}
