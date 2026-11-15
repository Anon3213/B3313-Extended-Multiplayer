//! Names, roles and chat: the text players send each other.
//!
//! All text is printable ASCII, which is what the game's font can draw, and is checked on the way
//! in from the network as well as when a player types it.

use crate::PlayerId;

/// The most characters in a player's name.
pub const MAX_NAME_LENGTH: usize = 16;
/// The most characters in a chat message.
pub const MAX_MESSAGE_LENGTH: usize = 100;
/// The most characters of a host password a bridge sends.
pub const MAX_HOST_PASSWORD_LENGTH: usize = 64;
/// The most chat lines one packet carries; more wait for the next.
pub const MAX_LINES_PER_PACKET: usize = 4;

/// Whether `text` is printable ASCII throughout, with no space at either end.
fn is_clean(text: &str) -> bool {
    text.bytes().all(|byte| (0x20..=0x7E).contains(&byte)) && text.trim() == text
}

/// What is left of `text` once everything but printable ASCII is dropped, the ends are trimmed
/// and it is cut to `limit` characters.
fn cleaned(text: &str, limit: usize) -> String {
    let printable: String = text
        .chars()
        .filter(|character| (' '..='~').contains(character))
        .collect();
    let cut: String = printable.trim().chars().take(limit).collect();
    cut.trim_end().to_owned()
}

/// A player's name: 1 to `MAX_NAME_LENGTH` printable ASCII characters.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Name(String);

impl Name {
    /// The name in what a player typed, if anything is left of it once cleaned.
    #[must_use]
    pub fn new(typed: &str) -> Option<Self> {
        let name = cleaned(typed, MAX_NAME_LENGTH);
        (!name.is_empty()).then_some(Self(name))
    }

    /// A name from the network, which must already be clean.
    pub(crate) fn from_wire(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let fits = (1..=MAX_NAME_LENGTH).contains(&text.len());
        (fits && is_clean(text)).then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A chat message: 1 to `MAX_MESSAGE_LENGTH` printable ASCII characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Message(String);

impl Message {
    /// The message in what a player typed, if anything is left of it once cleaned.
    #[must_use]
    pub fn new(typed: &str) -> Option<Self> {
        let message = cleaned(typed, MAX_MESSAGE_LENGTH);
        (!message.is_empty()).then_some(Self(message))
    }

    /// A message from the network, which must already be clean.
    pub(crate) fn from_wire(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        let fits = (1..=MAX_MESSAGE_LENGTH).contains(&text.len());
        (fits && is_clean(text)).then(|| Self(text.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What a player may do in the session beyond playing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Role {
    #[default]
    Player,
    /// May kick and ban ordinary players.
    Moderator,
    /// May also appoint and remove moderators, and end the session.
    Host,
}

impl Role {
    /// The role's code on the wire and in the game's mailbox.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Player => 0,
            Self::Moderator => 1,
            Self::Host => 2,
        }
    }

    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Player),
            1 => Some(Self::Moderator),
            2 => Some(Self::Host),
            _ => None,
        }
    }

    /// Whether this role may kick and ban.
    #[must_use]
    pub const fn moderates(self) -> bool {
        matches!(self, Self::Moderator | Self::Host)
    }
}

/// The colors a player's Mario wears: one of sixteen, each a cap and shirt with overalls to go
/// with them. Two players may wear the same.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outfit(u8);

/// Each outfit's name, the color of its cap and shirt, and that of its overalls. The game has
/// the same colors in the same order, `sPlayerColors` in `TrollEngine/Multiplayer/puppet.c`,
/// which a test here keeps in step.
const OUTFITS: [(&str, [u8; 3], [u8; 3]); 16] = [
    ("red", [0xFF, 0x00, 0x00], [0x00, 0x00, 0xFF]),
    ("green", [0x00, 0xC0, 0x20], [0x20, 0x20, 0xA0]),
    ("yellow", [0xFF, 0xD0, 0x00], [0x70, 0x00, 0xA0]),
    ("purple", [0x80, 0x00, 0xC0], [0x20, 0x20, 0x20]),
    ("orange", [0xFF, 0x70, 0x00], [0x00, 0x60, 0x60]),
    ("cyan", [0x00, 0xC0, 0xE0], [0xC0, 0x30, 0x00]),
    ("pink", [0xFF, 0x60, 0xB0], [0x60, 0x20, 0x80]),
    ("white", [0xF0, 0xF0, 0xF0], [0xC0, 0x00, 0x00]),
    ("black", [0x30, 0x30, 0x30], [0xD0, 0xD0, 0xD0]),
    ("brown", [0x80, 0x48, 0x10], [0x00, 0x70, 0x20]),
    ("lime", [0xA0, 0xFF, 0x00], [0x00, 0x40, 0x90]),
    ("blue", [0x00, 0x30, 0xC0], [0xE0, 0xC0, 0x00]),
    ("magenta", [0xE0, 0x00, 0xA0], [0x00, 0x90, 0x90]),
    ("teal", [0x00, 0x90, 0x80], [0x70, 0x30, 0x00]),
    ("gold", [0xC0, 0x98, 0x00], [0x50, 0x50, 0x50]),
    ("gray", [0x80, 0x80, 0x90], [0x90, 0x00, 0x40]),
];

impl Outfit {
    /// Every outfit, in the order the game numbers them.
    pub fn all() -> impl Iterator<Item = Self> {
        (0..16).map(Self)
    }

    /// The outfit a player wears until they choose one: by their number, so that players who
    /// never choose still differ, and the first has the game's own red.
    #[must_use]
    pub const fn of_player(player: PlayerId) -> Self {
        Self(player.get() % 16)
    }

    /// The outfit called `name`, whatever the case of its letters.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::all().find(|outfit| outfit.name().eq_ignore_ascii_case(name.trim()))
    }

    /// The outfit's number on the wire and in the game's mailbox.
    #[must_use]
    pub const fn code(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        if code < 16 { Some(Self(code)) } else { None }
    }

    fn entry(self) -> (&'static str, [u8; 3], [u8; 3]) {
        OUTFITS
            .get(usize::from(self.0))
            .copied()
            .unwrap_or(("red", [0xFF, 0, 0], [0, 0, 0xFF]))
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        self.entry().0
    }

    /// The color of the cap and shirt, as red, green and blue.
    #[must_use]
    pub fn cap(self) -> [u8; 3] {
        self.entry().1
    }

    /// The color of the overalls, as red, green and blue.
    #[must_use]
    pub fn overalls(self) -> [u8; 3] {
        self.entry().2
    }
}

/// One player in a `Roster`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RosterEntry {
    pub player: PlayerId,
    pub role: Role,
    pub outfit: Outfit,
    pub name: Name,
}

/// A line of chat as a player receives it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatLine {
    /// Raised by one per line sent to this receiver, so repeats are dropped.
    pub id: u16,
    /// Who said it, or nobody for a line from the server itself.
    pub from: Option<PlayerId>,
    pub text: Message,
}

#[cfg(test)]
mod tests {
    use super::{MAX_MESSAGE_LENGTH, MAX_NAME_LENGTH, Message, Name, OUTFITS, Outfit, Role};

    #[test]
    fn an_outfit_is_found_by_its_name_and_by_no_other() {
        let green = Outfit::named(" Green ");
        assert_eq!(green.map(Outfit::name), Some("green"));
        assert_eq!(
            green.and_then(|outfit| Outfit::from_code(outfit.code())),
            green
        );
        assert_eq!(Outfit::named("plaid"), None);
        assert_eq!(Outfit::from_code(16), None);
        // Each name picks out one outfit, or a player could not choose every one.
        for outfit in Outfit::all() {
            assert_eq!(Outfit::named(outfit.name()), Some(outfit));
        }
    }

    /// The panel shows these colors and the game draws its own copy of them.
    #[test]
    fn the_games_colors_are_these_colors() {
        let game = include_str!("../../TrollEngine/Multiplayer/puppet.c");
        let table = game
            .split_once("sPlayerColors[MP_PLAYER_SLOTS][6] = {")
            .and_then(|(_, rest)| rest.split_once("};"))
            .map(|(table, _)| table);
        let drawn: Vec<u8> = table
            .unwrap_or_default()
            .split(|character: char| !character.is_ascii_alphanumeric())
            .filter_map(|word| u8::from_str_radix(word.strip_prefix("0x")?, 16).ok())
            .collect();
        let chosen: Vec<u8> = OUTFITS
            .iter()
            .flat_map(|(_, cap, overalls)| cap.iter().chain(overalls).copied())
            .collect();
        assert_eq!(drawn, chosen);
    }

    #[test]
    fn typed_names_are_cleaned_and_cut_and_empty_ones_refused() {
        let name = |typed: &str| Name::new(typed).map(|name| name.as_str().to_owned());
        assert_eq!(name("  Luigi  "), Some("Luigi".to_owned()));
        assert_eq!(name("Lu\u{7}i\ngi\u{e9}"), Some("Luigi".to_owned()));
        assert_eq!(name("   "), None);
        assert_eq!(name("\u{1f600}"), None);
        let long = name(&"x".repeat(MAX_NAME_LENGTH + 5));
        assert_eq!(long.map(|name| name.len()), Some(MAX_NAME_LENGTH));
        // Cutting must not leave a space at the end.
        let cut_at_a_space = name("fifteen letters! and more");
        assert_eq!(cut_at_a_space, Some("fifteen letters!".to_owned()));
    }

    #[test]
    fn names_from_the_network_must_already_be_clean() {
        assert!(Name::from_wire(b"Luigi").is_some());
        for bad in [&b""[..], b" Luigi", b"Luigi ", b"Lu\nigi", b"Lu\xffigi"] {
            assert_eq!(Name::from_wire(bad), None);
        }
        assert_eq!(Name::from_wire(&[b'x'; MAX_NAME_LENGTH + 1]), None);
    }

    #[test]
    fn messages_keep_their_inner_spaces_and_are_cut_to_length() {
        let message = Message::new("  hello   there  ");
        assert_eq!(message.as_ref().map(Message::as_str), Some("hello   there"));
        let long = Message::new(&"y".repeat(MAX_MESSAGE_LENGTH * 2));
        assert_eq!(long.map(|m| m.as_str().len()), Some(MAX_MESSAGE_LENGTH));
        assert_eq!(Message::new("\t\n"), None);
        assert_eq!(Message::from_wire(b"tab\there"), None);
    }

    #[test]
    fn role_codes_round_trip_and_only_staff_moderate() {
        for role in [Role::Player, Role::Moderator, Role::Host] {
            assert_eq!(Role::from_code(role.code()), Some(role));
        }
        assert_eq!(Role::from_code(3), None);
        assert!(!Role::Player.moderates());
        assert!(Role::Moderator.moderates() && Role::Host.moderates());
    }
}
