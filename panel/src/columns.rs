//! The widths of a table's columns, which the user changes by dragging the line at a column's
//! right edge, and the preferences keep. One column of each table has no width of its own: it
//! takes the room the others leave, so the table always fills its card.

use crate::dashboard::{LogColumn, PlayerColumn};
use crate::sort::Column;

/// The narrowest a column can be made, in the panel's units at 100%: room for a few
/// characters and the "…" that says there are more.
pub const WIDTH_MIN: u16 = 44;
/// The widest: more than any column's content, and short of hiding the others for good.
pub const WIDTH_MAX: u16 = 640;

/// The Players table's usual widths. A name with the host's or a moderator's mark, "you" and
/// the world's globe, clear of the address beside it.
pub const PLAYER: u16 = 150;
pub const CONNECTED: u16 = 88;
pub const GAP: u16 = 96;
pub const LOSS: u16 = 72;
/// Wide enough for both rates in bits, the longer unit, as `216.8 kbit/s / 29.3 kbit/s`.
pub const TRAFFIC: u16 = 164;
pub const FLAGS: u16 = 56;
/// The Log's. Room for "12:59:59 PM".
pub const TIME: u16 = 96;
pub const KIND: u16 = 92;

/// A column whose width the user can change.
pub trait Resizable: Column {
    /// The width it has until the user changes it, or None for the one column of its table
    /// that takes the room the others leave.
    fn usual_width(self) -> Option<u16>;
}

impl Resizable for PlayerColumn {
    fn usual_width(self) -> Option<u16> {
        match self {
            Self::Player => Some(PLAYER),
            Self::Address => None,
            Self::Connected => Some(CONNECTED),
            Self::Gap => Some(GAP),
            Self::Loss => Some(LOSS),
            Self::Traffic => Some(TRAFFIC),
            Self::Flags => Some(FLAGS),
        }
    }
}

impl Resizable for LogColumn {
    fn usual_width(self) -> Option<u16> {
        match self {
            Self::Time => Some(TIME),
            Self::Kind => Some(KIND),
            Self::Entry => None,
        }
    }
}

/// A width a column may have: `width` rounded and kept within the limits.
#[must_use]
pub fn allowed_width(width: f32) -> u16 {
    // `max` and `min` also turn NaN into the minimum.
    let width = width
        .round()
        .max(f32::from(WIDTH_MIN))
        .min(f32::from(WIDTH_MAX));
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "within WIDTH_MIN..=WIDTH_MAX, so it fits a u16 exactly"
    )]
    let width = width as u16;
    width
}

/// One table's column widths: those the user has changed, the rest being the usual ones.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Widths<C> {
    changed: Vec<(C, u16)>,
}

impl<C> Default for Widths<C> {
    fn default() -> Self {
        Self {
            changed: Vec::new(),
        }
    }
}

impl<C: Resizable> Widths<C> {
    /// How wide `column` is, or None if it takes the room the others leave.
    #[must_use]
    pub fn of(&self, column: C) -> Option<u16> {
        let changed = self.changed.iter().find(|(known, _)| *known == column);
        changed.map(|&(_, width)| width).or(column.usual_width())
    }

    /// Every column's width added up, but for the one that takes the rest.
    #[must_use]
    pub fn total(&self) -> u16 {
        let widths = C::ALL.iter().filter_map(|&column| self.of(column));
        widths.fold(0, u16::saturating_add)
    }

    /// Makes `column` as near `width` wide as a column may be. One that takes the rest has no
    /// width to set, and one set back to its usual width is no longer a change to keep.
    pub fn set(&mut self, column: C, width: f32) {
        let Some(usual) = column.usual_width() else {
            return;
        };
        self.changed.retain(|(known, _)| *known != column);
        let width = allowed_width(width);
        if width != usual {
            self.changed.push((column, width));
            // In the table's order, so that the same widths are equal however they came.
            let place = |column: &C| C::ALL.iter().position(|known| known == column);
            self.changed.sort_by_key(|(column, _)| place(column));
        }
    }

    /// Gives `column` its usual width back.
    pub fn reset(&mut self, column: C) {
        self.changed.retain(|(known, _)| *known != column);
    }

    /// How the panel's preferences save it, as `player=180,loss=60`, in the table's order.
    #[must_use]
    pub fn format(&self) -> String {
        let changed = self.changed.iter();
        let written = changed.map(|(column, width)| format!("{}={width}", column.key()));
        written.collect::<Vec<_>>().join(",")
    }

    /// Reads what `format` writes. A column not known, one that has no width, and a width
    /// that is no number are left out; one outside the limits is brought within them.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut widths = Self::default();
        for part in text.split(',') {
            let Some((key, width)) = part.split_once('=') else {
                continue;
            };
            let column = C::ALL.iter().find(|column| column.key() == key.trim());
            if let (Some(&column), Ok(width)) = (column, width.trim().parse::<u16>()) {
                widths.set(column, f32::from(width));
            }
        }
        widths
    }
}

#[cfg(test)]
mod tests {
    use super::{PLAYER, Resizable, WIDTH_MAX, WIDTH_MIN, Widths, allowed_width};
    use crate::dashboard::{LogColumn, PlayerColumn};
    use crate::sort::Column;

    #[test]
    fn a_column_keeps_its_usual_width_until_it_is_changed_and_has_it_back_on_reset() {
        let mut widths: Widths<PlayerColumn> = Widths::default();
        assert_eq!(widths.of(PlayerColumn::Player), Some(PLAYER));
        widths.set(PlayerColumn::Player, 201.4);
        assert_eq!(widths.of(PlayerColumn::Player), Some(201));
        assert_eq!(widths.format(), "player=201");
        widths.reset(PlayerColumn::Player);
        assert_eq!(widths, Widths::default());
        // Dragged back to where it was, it is no change to save either.
        widths.set(PlayerColumn::Loss, 10.);
        widths.set(PlayerColumn::Loss, 72.);
        assert_eq!(widths.format(), "");
    }

    #[test]
    fn a_width_stays_within_the_limits() {
        assert_eq!(allowed_width(-5.), WIDTH_MIN);
        assert_eq!(allowed_width(9000.), WIDTH_MAX);
        assert_eq!(allowed_width(f32::NAN), WIDTH_MIN);
        let mut widths: Widths<LogColumn> = Widths::default();
        widths.set(LogColumn::Time, 3.);
        assert_eq!(widths.of(LogColumn::Time), Some(WIDTH_MIN));
    }

    #[test]
    fn the_column_that_takes_the_rest_has_no_width_to_set() {
        let mut widths: Widths<PlayerColumn> = Widths::default();
        widths.set(PlayerColumn::Address, 300.);
        assert_eq!(widths.of(PlayerColumn::Address), None);
        assert_eq!(widths, Widths::default());
        // Each table has exactly one such column, or it would not fill its card.
        let rest = |columns: &[Option<u16>]| columns.iter().filter(|w| w.is_none()).count();
        let players: Vec<_> = PlayerColumn::ALL.iter().map(|c| c.usual_width()).collect();
        let log: Vec<_> = LogColumn::ALL.iter().map(|c| c.usual_width()).collect();
        assert_eq!((rest(&players), rest(&log)), (1, 1));
    }

    #[test]
    fn widths_read_back_and_anything_odd_is_left_out() {
        let mut widths: Widths<PlayerColumn> = Widths::default();
        widths.set(PlayerColumn::Flags, 80.);
        widths.set(PlayerColumn::Player, 220.);
        // In the table's order, whatever order they were changed in.
        assert_eq!(widths.format(), "player=220,flags=80");
        assert_eq!(Widths::parse(&widths.format()), widths);
        let odd: Widths<PlayerColumn> =
            Widths::parse("plaid=90, address=300, loss=wide, gap, traffic = 9999 ,player=220");
        assert_eq!(odd.format(), "player=220,traffic=640");
        assert_eq!(Widths::<LogColumn>::parse(""), Widths::default());
    }

    #[test]
    fn the_total_counts_every_column_with_a_width() {
        let mut widths: Widths<LogColumn> = Widths::default();
        assert_eq!(widths.total(), 96 + 92);
        widths.set(LogColumn::Kind, 100.);
        assert_eq!(widths.total(), 96 + 100);
    }
}
