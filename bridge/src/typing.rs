//! The line of chat a player types in the game.
//!
//! The game has no keyboard and little room for code, so the line is kept here: the bridge
//! reads the PC's keys (`keyboard`), this turns them into a line, and the game is given that
//! line as it stands, with a prompt and a cursor, to draw under its chat
//! (`TrollEngine/Multiplayer/people.c`).

use crate::text::{self, ChatSize, ROW_BYTES};
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

/// How many rows of chat the game shows at once as lines arrive, unless the player chooses
/// fewer, and the fewest they may choose. Opening the chat, or resting the mouse on it, shows
/// all of them whatever was chosen.
pub const DEFAULT_CHAT_LINES: u8 = crate::mailbox::CHAT_SHOWN_MOST;
pub const CHAT_LINES_MIN: u8 = 1;

/// The rows a player named for the chat to show at once, if they are a number that may be
/// chosen.
#[must_use]
pub fn chat_lines(named: &str) -> Option<u8> {
    let lines = named.trim().parse().ok()?;
    (CHAT_LINES_MIN..=DEFAULT_CHAT_LINES)
        .contains(&lines)
        .then_some(lines)
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
    /// Page Up and Page Down in the open chat: they scroll it back through earlier lines and
    /// forward again (`People`).
    PageUp,
    PageDown,
    /// The up and down arrows in the open chat: they bring back the lines sent before.
    Up,
    Down,
}

/// How many of the lines a player sent the arrow keys can bring back.
const SENT_KEPT: usize = 20;

/// The line being typed, if one is.
#[derive(Debug, Default)]
pub struct Draft {
    line: Option<String>,
    /// When a key last changed it, which the cursor's blinking starts over from.
    touched: Option<Instant>,
    /// The lines sent, oldest first, and which of them the up arrow last brought back.
    sent: Vec<String>,
    recalled: Option<usize>,
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
    /// and a Backspace with nothing left to remove. The up arrow puts the line sent before the
    /// one shown in its place, and the down arrow the one after, or an empty line after the
    /// last.
    pub fn press(&mut self, key: Key, now: Instant) -> Option<Message> {
        let Some(line) = &mut self.line else {
            match key {
                Key::Chat => self.line = Some(String::new()),
                Key::Command => self.line = Some("/".to_owned()),
                _ => return None,
            }
            self.recalled = None;
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
            Key::Enter => {
                let sent = self.line.take().as_deref().and_then(Message::new)?;
                self.remember(sent.as_str());
                Some(sent)
            }
            Key::Escape => {
                self.line = None;
                None
            }
            Key::Up => {
                let before = match self.recalled {
                    Some(shown) => shown.checked_sub(1),
                    None => self.sent.len().checked_sub(1),
                };
                if let Some(index) = before
                    && let Some(earlier) = self.sent.get(index)
                {
                    line.clone_from(earlier);
                    self.recalled = Some(index);
                }
                None
            }
            Key::Down => {
                if let Some(shown) = self.recalled {
                    let after = shown + 1;
                    if let Some(later) = self.sent.get(after) {
                        line.clone_from(later);
                        self.recalled = Some(after);
                    } else {
                        line.clear();
                        self.recalled = None;
                    }
                }
                None
            }
            // Only pressed while the chat is closed, or for the chat's rows and not the line.
            Key::Chat | Key::Command | Key::PageUp | Key::PageDown => None,
        }
    }

    /// Keeps a line sent for the arrow keys, unless it is the one sent just before.
    fn remember(&mut self, sent: &str) {
        self.recalled = None;
        if self.sent.last().is_some_and(|last| last == sent) {
            return;
        }
        if self.sent.len() == SENT_KEPT {
            self.sent.remove(0);
        }
        self.sent.push(sent.to_owned());
    }

    /// Closes the chat and forgets the line, as when the player turns to another window.
    pub fn close(&mut self) {
        self.line = None;
    }

    /// The row the game draws for the line at `now` in chat text of `size`, if one is being
    /// typed: the prompt, the line, or its end if it is too long for a row, and the cursor,
    /// which blinks.
    #[must_use]
    pub fn row(&self, now: Instant, size: ChatSize) -> Option<[u8; ROW_BYTES]> {
        let line = self.line.as_deref()?;
        // The cursor's room is kept while it is hidden, so a long line does not shift.
        let shown = text::ending(line, PROMPT, CURSOR, size);
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
    use super::{BLINK, Draft, Key, SENT_KEPT, chat_key, chat_lines, chat_seconds};
    use crate::text::{ChatSize, ROW_BYTES, TEXT_END};
    use b3313_protocol::MAX_MESSAGE_LENGTH;
    use std::time::Instant;

    fn typed(draft: &mut Draft, text: &str, now: Instant) {
        for character in text.chars() {
            assert_eq!(draft.press(Key::Character(character), now), None);
        }
    }

    /// The row as text, up to its end code.
    fn shown(draft: &Draft, now: Instant) -> Option<String> {
        let row: [u8; ROW_BYTES] = draft.row(now, ChatSize::Normal)?;
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
    fn the_arrows_bring_back_the_lines_sent_before() {
        let now = Instant::now();
        let mut draft = Draft::default();
        for line in ["first", "second", "second", "third"] {
            draft.press(Key::Chat, now);
            typed(&mut draft, line, now);
            assert!(draft.press(Key::Enter, now).is_some());
        }
        draft.press(Key::Chat, now);
        typed(&mut draft, "half", now);
        // Newest first, the same line sent twice in a row once, and no further than the first.
        draft.press(Key::Up, now);
        assert_eq!(shown(&draft, now).as_deref(), Some("> third_"));
        draft.press(Key::Up, now);
        assert_eq!(shown(&draft, now).as_deref(), Some("> second_"));
        draft.press(Key::Up, now);
        draft.press(Key::Up, now);
        assert_eq!(shown(&draft, now).as_deref(), Some("> first_"));
        draft.press(Key::Down, now);
        assert_eq!(shown(&draft, now).as_deref(), Some("> second_"));
        // Past the newest, an empty line, and a recalled line is sent as typed anew.
        draft.press(Key::Down, now);
        draft.press(Key::Down, now);
        assert_eq!(shown(&draft, now).as_deref(), Some("> _"));
        draft.press(Key::Up, now);
        typed(&mut draft, "!", now);
        let sent = draft.press(Key::Enter, now);
        assert_eq!(
            sent.as_ref().map(b3313_protocol::Message::as_str),
            Some("third!")
        );
        // Only so many are kept: the oldest go first.
        for number in 0..SENT_KEPT {
            draft.press(Key::Chat, now);
            typed(&mut draft, &number.to_string(), now);
            draft.press(Key::Enter, now);
        }
        draft.press(Key::Chat, now);
        for _ in 0..=SENT_KEPT {
            draft.press(Key::Up, now);
        }
        assert_eq!(shown(&draft, now).as_deref(), Some("> 0_"));
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
    fn the_rows_the_chat_shows_at_once_are_one_to_as_many_as_the_game_draws() {
        assert_eq!(chat_lines("1"), Some(1));
        assert_eq!(chat_lines(" 6 "), Some(6));
        assert_eq!(chat_lines("0"), None);
        assert_eq!(chat_lines("7"), None);
        assert_eq!(chat_lines("few"), None);
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
