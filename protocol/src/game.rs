//! The versions of B3313 the mod plays, by the number each game's ROM marks itself with.
//!
//! Players in one session must all play the same one: the levels differ between them. The
//! numbers are decided in `scripts/game_versions.py`, which a test there keeps equal to this
//! table, reading it as text: keep one entry to a line, in the form of those below.

/// Every game's number and name. 0 is no number: it stands for a game that is not known.
pub const GAMES: &[(u8, &str)] = &[
    (1, "Unabandoned A2 Anniversary Edition"),
    (2, "1.0.2 hotfix 3"),
    (3, "1.0.2 hotfix 3 with Unabandoned tweaks"),
];

/// The name of the game numbered `number`, if this program knows it.
#[must_use]
pub fn name(number: u8) -> Option<&'static str> {
    GAMES
        .iter()
        .find(|(known, _)| *known == number)
        .map(|(_, name)| *name)
}

/// The game numbered `number` in words for a player or the server's owner: a newer program may
/// know games this one does not, so a number without a name is still said.
#[must_use]
pub fn describe(number: u8) -> String {
    name(number).map_or_else(
        || format!("a version of B3313 this program does not know (number {number})"),
        |name| format!("B3313 {name}"),
    )
}

#[cfg(test)]
mod tests {
    use super::{GAMES, describe, name};

    #[test]
    fn every_game_has_its_own_number_and_none_is_zero() {
        for (index, (number, _)) in GAMES.iter().enumerate() {
            assert_ne!(*number, 0, "0 means no game is known");
            let earlier = GAMES.iter().take(index);
            assert!(earlier.into_iter().all(|(other, _)| other != number));
        }
    }

    #[test]
    fn a_known_game_is_named_and_an_unknown_one_is_said_by_its_number() {
        for (number, game) in GAMES {
            assert_eq!(name(*number), Some(*game));
            assert_eq!(describe(*number), format!("B3313 {game}"));
        }
        assert_eq!(name(0), None);
        assert_eq!(name(200), None);
        assert!(describe(200).contains("200"), "{}", describe(200));
    }
}
