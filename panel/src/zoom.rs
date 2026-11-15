//! How large the panel draws everything.

use crate::theme::BASE_REM;

/// The sizes offered, in percent, as browsers offer them.
const LEVELS: [u16; 8] = [80, 90, 100, 110, 125, 150, 175, 200];
const DEFAULT: u16 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zoom(u16);

impl Default for Zoom {
    fn default() -> Self {
        Self(DEFAULT)
    }
}

impl Zoom {
    /// The next larger size, or this one if it is the largest.
    #[must_use]
    pub fn larger(self) -> Self {
        LEVELS
            .into_iter()
            .find(|&level| level > self.0)
            .map_or(self, Self)
    }

    /// The next smaller size, or this one if it is the smallest.
    #[must_use]
    pub fn smaller(self) -> Self {
        LEVELS
            .into_iter()
            .rev()
            .find(|&level| level < self.0)
            .map_or(self, Self)
    }

    #[must_use]
    pub fn percent(self) -> u16 {
        self.0
    }

    #[must_use]
    pub fn is_largest(self) -> bool {
        self.larger() == self
    }

    #[must_use]
    pub fn is_smallest(self) -> bool {
        self.smaller() == self
    }

    /// The rem size in pixels, which scales the whole interface.
    #[must_use]
    pub fn rem_size(self) -> f32 {
        BASE_REM * f32::from(self.0) / 100.0
    }

    /// A length in pixels at this size, as for the window's starting size.
    #[must_use]
    pub fn scale(self, length: f32) -> f32 {
        length * f32::from(self.0) / 100.0
    }

    /// The size at `percent`, if it is one of the sizes offered.
    #[must_use]
    pub fn from_percent(percent: u16) -> Option<Self> {
        LEVELS.contains(&percent).then_some(Self(percent))
    }
}

#[cfg(test)]
mod tests {
    use super::Zoom;

    #[test]
    fn sizes_step_through_the_levels_and_stop_at_the_ends() {
        let mut zoom = Zoom::default();
        assert_eq!(zoom.larger().percent(), 110);
        assert_eq!(zoom.smaller().percent(), 90);
        for _ in 0..20 {
            zoom = zoom.larger();
        }
        assert_eq!(zoom.percent(), 200);
        assert!(zoom.is_largest());
        for _ in 0..20 {
            zoom = zoom.smaller();
        }
        assert_eq!(zoom.percent(), 80);
        assert!(zoom.is_smallest());
    }

    #[test]
    fn the_rem_size_follows_the_level() {
        assert!((Zoom::default().rem_size() - 16.0).abs() < f32::EPSILON);
        let large = Zoom::default().larger().larger().larger();
        assert_eq!(large.percent(), 150);
        assert!((large.rem_size() - 24.0).abs() < f32::EPSILON);
        assert!((large.scale(1000.0) - 1500.0).abs() < f32::EPSILON);
    }

    #[test]
    fn only_offered_sizes_are_accepted() {
        assert_eq!(Zoom::from_percent(125).map(Zoom::percent), Some(125));
        assert_eq!(Zoom::from_percent(133), None);
    }
}
