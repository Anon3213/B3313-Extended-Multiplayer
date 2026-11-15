//! Colors and fonts. The user picks a color scheme; each comes light or dark to follow the
//! system's choice.

use crate::health::Health;
use gpui::{Global, Hsla, Rems, WindowAppearance, rems, rgb};

/// A length given in pixels at 100% interface size, which grows and shrinks with the size the
/// user picks. Every size in the panel goes through this, so the whole interface zooms together.
#[must_use]
pub fn scaled(pixels: f32) -> Rems {
    rems(pixels / BASE_REM)
}

/// The rem size in pixels at 100%.
pub const BASE_REM: f32 = 16.0;

/// Body text. Segoe UI is Windows' own interface font.
pub const FONT: &str = "Segoe UI";
/// Addresses, numbers that line up, and the log.
pub const MONO: &str = "Consolas";

/// The color schemes the user can pick from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeName {
    #[default]
    Blue,
    Red,
    Green,
    Sepia,
    Violet,
    Teal,
    /// Lower contrast, warm grays and muted colors, for long sessions.
    Comfort,
    /// Black and white with bright colors, for low vision; also used whenever one of Windows'
    /// contrast themes is on.
    HighContrast,
}

/// Whether the panel is light or dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// As Windows is set, changing when it does.
    #[default]
    System,
    Light,
    Dark,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "As Windows",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.key() == key)
    }

    /// Whether to draw dark, given how Windows is set.
    #[must_use]
    pub fn is_dark(self, system: WindowAppearance) -> bool {
        match self {
            Self::System => matches!(
                system,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
            Self::Light => false,
            Self::Dark => true,
        }
    }
}

impl ThemeName {
    pub const ALL: [Self; 8] = [
        Self::Blue,
        Self::Red,
        Self::Green,
        Self::Sepia,
        Self::Violet,
        Self::Teal,
        Self::Comfort,
        Self::HighContrast,
    ];

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Blue => "Blue",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Sepia => "Sepia",
            Self::Violet => "Violet",
            Self::Teal => "Teal",
            Self::Comfort => "Comfort",
            Self::HighContrast => "High contrast",
        }
    }

    /// How the preferences file names it.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Blue => "blue",
            Self::Red => "red",
            Self::Green => "green",
            Self::Sepia => "sepia",
            Self::Violet => "violet",
            Self::Teal => "teal",
            Self::Comfort => "comfort",
            Self::HighContrast => "high-contrast",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|name| name.key() == key)
    }

    /// How the scheme differs from blue, or none for blue itself or a scheme of its own.
    fn swap(self) -> Option<Swap> {
        let swap = |neutral_hue, neutral_saturation, dark, light| {
            Some(Swap {
                neutral_hue,
                neutral_saturation,
                dark,
                light,
            })
        };
        // Accents are dark enough for white labels (4.5:1) yet stand out from dark surfaces
        // (over 3:1), and keep clear of the status colors: red is crimson beside the softer
        // danger red, green a deep leaf green beside the light mint of success, and sepia a
        // copper well away from the warning amber.
        match self {
            Self::Blue | Self::Comfort | Self::HighContrast => None,
            Self::Red => swap(355., 1.0, (0xde_2f49, 0xe8_556b), (0xc2_2a42, 0xab_2238)),
            Self::Green => swap(140., 0.9, (0x2e_8643, 0x36_9c4e), (0x23_7a38, 0x1d_6a30)),
            Self::Sepia => swap(30., 1.8, (0xa0_662e, 0xb0_743a), (0x8a_5524, 0x77_4820)),
            Self::Violet => swap(265., 1.0, (0x7c_4ddb, 0x8b_5fe3), (0x6a_3cc9, 0x5c_31b3)),
            Self::Teal => swap(180., 1.0, (0x12_847a, 0x17_9a8f), (0x0f_766e, 0x0c_6660)),
        }
    }
}

/// A palette swap of the blue scheme: its tinted grays turned to another hue, and another
/// accent. Status colors stay as they are, so they mean the same in every scheme.
#[derive(Clone, Copy, Debug)]
struct Swap {
    /// In degrees.
    neutral_hue: f32,
    /// Scales the grays' faint tint; above 1 for schemes whose character is in the tint.
    neutral_saturation: f32,
    /// The accent and its hover color, in dark and light.
    dark: (u32, u32),
    light: (u32, u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub background: Hsla,
    pub sidebar: Hsla,
    /// Cards and tables.
    pub surface: Hsla,
    /// Behind disabled fields and table headers.
    pub surface_sunken: Hsla,
    pub hover: Hsla,
    pub border: Hsla,
    pub input: Hsla,
    pub text: Hsla,
    pub text_muted: Hsla,
    /// Placeholders and the least important labels.
    pub text_faint: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    /// Text on an accent background.
    pub on_accent: Hsla,
    /// Behind the current page in the sidebar, and other light accent fills.
    pub accent_soft: Hsla,
    pub selection: Hsla,
    pub success: Hsla,
    pub success_soft: Hsla,
    pub warning: Hsla,
    pub warning_soft: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    /// A danger button's background under the mouse: stronger than `danger_soft`, but still
    /// light enough for `danger` text on it.
    pub danger_hover: Hsla,
}

impl Global for Theme {}

fn color(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// `hex` with `alpha` out of 255.
fn translucent(hex: u32, alpha: u8) -> Hsla {
    let mut color = color(hex);
    color.a = f32::from(alpha) / 255.0;
    color
}

/// How strongly the accent shows behind the current page and behind selected text.
const DARK_ACCENT_SOFT: u8 = 0x2e;
const DARK_SELECTION: u8 = 0x66;
const LIGHT_ACCENT_SOFT: u8 = 0x1f;
const LIGHT_SELECTION: u8 = 0x40;

impl Theme {
    /// The color for a number in this state, or none for one in its usual range.
    #[must_use]
    pub fn health(&self, health: Health) -> Option<Hsla> {
        match health {
            Health::Good => None,
            Health::Fair => Some(self.warning),
            Health::Poor => Some(self.danger),
        }
    }

    /// The colors the panel draws with: the user's scheme, light or dark as they chose, except
    /// that a Windows contrast theme (`high_contrast`) always brings High contrast, as Windows
    /// asks apps to.
    #[must_use]
    pub fn for_user(
        name: ThemeName,
        mode: Mode,
        system: WindowAppearance,
        high_contrast: bool,
    ) -> Self {
        let name = if high_contrast {
            ThemeName::HighContrast
        } else {
            name
        };
        Self::new(name, mode.is_dark(system))
    }

    #[must_use]
    pub fn new(name: ThemeName, dark: bool) -> Self {
        match (name, dark) {
            (ThemeName::Comfort, true) => Self::comfort_dark(),
            (ThemeName::Comfort, false) => Self::comfort_light(),
            (ThemeName::HighContrast, true) => Self::high_contrast_dark(),
            (ThemeName::HighContrast, false) => Self::high_contrast_light(),
            (_, true) => Self::blue_dark().swapped(name.swap(), true),
            (_, false) => Self::blue_light().swapped(name.swap(), false),
        }
    }

    fn swapped(mut self, swap: Option<Swap>, dark: bool) -> Self {
        let Some(swap) = swap else {
            return self;
        };
        let retint = |color: &mut Hsla| {
            color.h = swap.neutral_hue / 360.0;
            color.s = (color.s * swap.neutral_saturation).min(1.0);
        };
        for neutral in [
            &mut self.background,
            &mut self.sidebar,
            &mut self.surface,
            &mut self.surface_sunken,
            &mut self.hover,
            &mut self.border,
            &mut self.input,
            &mut self.text,
            &mut self.text_muted,
            &mut self.text_faint,
        ] {
            retint(neutral);
        }
        let ((accent, hover), soft, selection) = if dark {
            (swap.dark, DARK_ACCENT_SOFT, DARK_SELECTION)
        } else {
            (swap.light, LIGHT_ACCENT_SOFT, LIGHT_SELECTION)
        };
        self.accent = color(accent);
        self.accent_hover = color(hover);
        self.accent_soft = translucent(accent, soft);
        self.selection = translucent(accent, selection);
        self
    }

    fn blue_dark() -> Self {
        Self {
            background: color(0x0f_1115),
            sidebar: color(0x14_171c),
            surface: color(0x1a_1d23),
            surface_sunken: color(0x15_181d),
            hover: color(0x23_272e),
            border: color(0x2a_2f37),
            input: color(0x11_1317),
            text: color(0xe6_e8eb),
            text_muted: color(0x9a_a3ae),
            text_faint: color(0x6b_7480),
            accent: color(0x46_69f5),
            accent_hover: color(0x5f_7ff7),
            on_accent: color(0xff_ffff),
            accent_soft: translucent(0x46_69f5, DARK_ACCENT_SOFT),
            selection: translucent(0x46_69f5, DARK_SELECTION),
            success: color(0x3d_c483),
            success_soft: translucent(0x3d_c483, 0x24),
            warning: color(0xf0_b43c),
            warning_soft: translucent(0xf0_b43c, 0x24),
            danger: color(0xf0_5a5f),
            danger_soft: translucent(0xf0_5a5f, 0x24),
            danger_hover: translucent(0xf0_5a5f, 0x40),
        }
    }

    fn blue_light() -> Self {
        Self {
            background: color(0xf6_f7f9),
            sidebar: color(0xee_f0f3),
            surface: color(0xff_ffff),
            surface_sunken: color(0xf1_f3f5),
            hover: color(0xe7_eaee),
            border: color(0xdd_e1e6),
            input: color(0xff_ffff),
            text: color(0x16_191d),
            text_muted: color(0x5c_6570),
            text_faint: color(0x8a_929c),
            accent: color(0x3b_5bdb),
            accent_hover: color(0x34_51c7),
            on_accent: color(0xff_ffff),
            accent_soft: translucent(0x3b_5bdb, LIGHT_ACCENT_SOFT),
            selection: translucent(0x3b_5bdb, LIGHT_SELECTION),
            success: color(0x1f_8f57),
            success_soft: translucent(0x1f_8f57, 0x1c),
            warning: color(0xa8_6a00),
            warning_soft: translucent(0xd9_9a1c, 0x26),
            danger: color(0xd6_333a),
            danger_soft: translucent(0xd6_333a, 0x1a),
            danger_hover: translucent(0xd6_333a, 0x30),
        }
    }

    /// Comfort, dark: a warm charcoal rather than near black, off-white text at about
    /// 9:1 rather than 16:1 (pure white on black glares and blurs for many), and muted colors.
    /// The accent is light, with dark labels, so it needs no bright white on it.
    fn comfort_dark() -> Self {
        Self {
            background: color(0x1f_1d1b),
            sidebar: color(0x24_2220),
            surface: color(0x2a_2825),
            surface_sunken: color(0x25_2321),
            hover: color(0x34_312d),
            border: color(0x3b_3833),
            input: color(0x1c_1a18),
            text: color(0xd6_d0c4),
            text_muted: color(0xa3_9c90),
            text_faint: color(0x7d_776d),
            accent: color(0x8a_a4bf),
            accent_hover: color(0x9a_b1c9),
            on_accent: color(0x1f_1d1b),
            accent_soft: translucent(0x8a_a4bf, 0x29),
            selection: translucent(0x8a_a4bf, 0x55),
            success: color(0x7d_ba8f),
            success_soft: translucent(0x7d_ba8f, 0x24),
            warning: color(0xd8_b46a),
            warning_soft: translucent(0xd8_b46a, 0x24),
            danger: color(0xd9_8080),
            danger_soft: translucent(0xd9_8080, 0x24),
            danger_hover: translucent(0xd9_8080, 0x40),
        }
    }

    /// Comfort, light: cream paper instead of white, and dark brown-gray text instead
    /// of black, at about 10:1.
    fn comfort_light() -> Self {
        Self {
            background: color(0xf2_ede3),
            sidebar: color(0xeb_e5d9),
            surface: color(0xf8_f4ec),
            surface_sunken: color(0xed_e8de),
            hover: color(0xe3_ddd0),
            border: color(0xd8_d1c3),
            input: color(0xfb_f8f2),
            text: color(0x3b_362f),
            text_muted: color(0x6e_665a),
            text_faint: color(0x93_8b7e),
            accent: color(0x4f_6f8f),
            accent_hover: color(0x45_627f),
            on_accent: color(0xfb_f8f2),
            accent_soft: translucent(0x4f_6f8f, LIGHT_ACCENT_SOFT),
            selection: translucent(0x4f_6f8f, LIGHT_SELECTION),
            success: color(0x3f_7f55),
            success_soft: translucent(0x3f_7f55, 0x1c),
            warning: color(0x92_681e),
            warning_soft: translucent(0xc9_9a3c, 0x26),
            danger: color(0xb0_4a4a),
            danger_soft: translucent(0xb0_4a4a, 0x1a),
            danger_hover: translucent(0xb0_4a4a, 0x30),
        }
    }

    /// High contrast, dark: white on black with bright colors, gray borders around every area,
    /// and a cyan accent with black labels, kept apart from the warning yellow.
    fn high_contrast_dark() -> Self {
        Self {
            background: color(0x00_0000),
            sidebar: color(0x00_0000),
            surface: color(0x00_0000),
            surface_sunken: color(0x14_1414),
            hover: color(0x26_2626),
            border: color(0xc8_c8c8),
            input: color(0x00_0000),
            text: color(0xff_ffff),
            text_muted: color(0xe6_e6e6),
            text_faint: color(0xc8_c8c8),
            accent: color(0x00_e5ff),
            accent_hover: color(0x66_efff),
            on_accent: color(0x00_0000),
            accent_soft: translucent(0x00_e5ff, 0x40),
            selection: translucent(0x00_e5ff, 0x80),
            success: color(0x3d_ff6e),
            success_soft: translucent(0x3d_ff6e, 0x33),
            warning: color(0xff_d400),
            warning_soft: translucent(0xff_d400, 0x33),
            danger: color(0xff_6b6b),
            danger_soft: translucent(0xff_6b6b, 0x33),
            danger_hover: translucent(0xff_6b6b, 0x55),
        }
    }

    /// High contrast, light: black on white, dark borders, and deep colors.
    fn high_contrast_light() -> Self {
        Self {
            background: color(0xff_ffff),
            sidebar: color(0xff_ffff),
            surface: color(0xff_ffff),
            surface_sunken: color(0xf0_f0f0),
            hover: color(0xe0_e0e0),
            border: color(0x3a_3a3a),
            input: color(0xff_ffff),
            text: color(0x00_0000),
            text_muted: color(0x1a_1a1a),
            text_faint: color(0x3d_3d3d),
            accent: color(0x00_00c8),
            accent_hover: color(0x00_009a),
            on_accent: color(0xff_ffff),
            accent_soft: translucent(0x00_00c8, 0x24),
            selection: translucent(0x00_00c8, 0x48),
            success: color(0x00_6b1f),
            success_soft: translucent(0x00_6b1f, 0x24),
            warning: color(0x6b_4a00),
            warning_soft: translucent(0xd9_9a1c, 0x40),
            danger: color(0xb0_0000),
            danger_soft: translucent(0xb0_0000, 0x1f),
            danger_hover: translucent(0xb0_0000, 0x38),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Mode, Theme, ThemeName};
    use gpui::{Hsla, Rgba, WindowAppearance};

    /// WCAG relative luminance.
    fn luminance(color: Hsla) -> f32 {
        let Rgba { r, g, b, .. } = color.into();
        let channel = |c: f32| {
            if c <= 0.039_28 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (light, dark) = {
            let (a, b) = (luminance(a), luminance(b));
            (a.max(b), a.min(b))
        };
        (light + 0.05) / (dark + 0.05)
    }

    #[test]
    fn every_scheme_keeps_text_and_buttons_readable() {
        for name in ThemeName::ALL {
            for dark in [true, false] {
                let theme = Theme::new(name, dark);
                let which = format!("{name:?} dark: {dark}");
                // High contrast holds itself to more: WCAG's enhanced level and beyond.
                let strict = name == ThemeName::HighContrast;
                let (text, muted, label, mark) = if strict {
                    (15.0, 7.0, 7.0, 4.5)
                } else {
                    (7.0, 4.5, 4.5, 3.0)
                };
                let found = contrast(theme.text, theme.surface);
                assert!(found >= text, "{which}: text {found:.1}");
                let found = contrast(theme.text_muted, theme.surface);
                assert!(found >= muted, "{which}: muted {found:.1}");
                let found = contrast(theme.on_accent, theme.accent);
                assert!(found >= label, "{which}: button label {found:.1}");
                // The accent marks the current page and focused fields on the background.
                let found = contrast(theme.accent, theme.surface);
                assert!(found >= mark, "{which}: accent {found:.1}");
                if strict {
                    for (what, color) in [
                        ("success", theme.success),
                        ("warning", theme.warning),
                        ("danger", theme.danger),
                    ] {
                        let found = contrast(color, theme.surface);
                        assert!(found >= 4.5, "{which}: {what} {found:.1}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_mode_decides_light_or_dark() {
        assert!(Mode::System.is_dark(WindowAppearance::Dark));
        assert!(!Mode::System.is_dark(WindowAppearance::Light));
        assert!(!Mode::Light.is_dark(WindowAppearance::Dark));
        assert!(Mode::Dark.is_dark(WindowAppearance::Light));
        for mode in Mode::ALL {
            assert_eq!(Mode::from_key(mode.key()), Some(mode));
        }
    }

    #[test]
    fn swaps_keep_the_status_colors_and_blue_is_unchanged() {
        let blue = Theme::new(ThemeName::Blue, true);
        assert_eq!(blue, Theme::blue_dark());
        let red = Theme::new(ThemeName::Red, true);
        assert_eq!(red.danger, blue.danger);
        assert_eq!(red.success, blue.success);
        assert_eq!(red.warning, blue.warning);
        assert_ne!(red.background, blue.background);
    }

    #[test]
    fn names_read_back_from_their_keys() {
        for name in ThemeName::ALL {
            assert_eq!(ThemeName::from_key(name.key()), Some(name));
        }
        assert_eq!(ThemeName::from_key("plaid"), None);
    }
}
