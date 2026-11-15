//! Lucide icons (ISC licensed, `assets/icons/LICENSE`), built into the program so it needs no
//! files beside it. Lucide is also gpui-component's icon set, so the panel's glyphs would stay the
//! same if it moved to that library.

use crate::theme::scaled;
use gpui::{AssetSource, Hsla, SharedString, Svg, prelude::*, svg};
use std::borrow::Cow;

macro_rules! icons {
    ($($name:ident => $file:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum IconName {
            $($name,)*
        }

        impl IconName {
            pub const ALL: &[Self] = &[$(Self::$name,)*];

            /// Where GPUI asks the asset source for it.
            #[must_use]
            pub fn path(self) -> &'static str {
                match self {
                    $(Self::$name => concat!("icons/", $file),)*
                }
            }

            fn svg(self) -> &'static [u8] {
                match self {
                    $(Self::$name => include_bytes!(concat!("../assets/icons/", $file)),)*
                }
            }
        }
    };
}

icons! {
    Activity => "activity.svg",
    ArrowDown => "arrow-down.svg",
    ArrowLeft => "arrow-left.svg",
    ArrowRightLeft => "arrow-right-left.svg",
    ArrowUp => "arrow-up.svg",
    ArrowUpDown => "arrow-up-down.svg",
    Ban => "ban.svg",
    Bell => "bell.svg",
    ChevronDown => "chevron-down.svg",
    ChevronRight => "chevron-right.svg",
    CircleAlert => "circle-alert.svg",
    CircleCheck => "circle-check.svg",
    Copy => "copy.svg",
    Crown => "crown.svg",
    Download => "download.svg",
    EyeOff => "eye-off.svg",
    FileText => "file-text.svg",
    Flag => "flag.svg",
    Gamepad => "gamepad-2.svg",
    Gavel => "gavel.svg",
    Globe => "globe.svg",
    Grip => "grip-vertical.svg",
    History => "history.svg",
    Keyboard => "keyboard.svg",
    LayoutDashboard => "layout-dashboard.svg",
    MessageSquare => "message-square.svg",
    Minus => "minus.svg",
    PanelLeftClose => "panel-left-close.svg",
    PanelLeftOpen => "panel-left-open.svg",
    Pencil => "pencil.svg",
    Play => "play.svg",
    Plus => "plus.svg",
    RotateCcw => "rotate-ccw.svg",
    RotateCw => "rotate-cw.svg",
    ScrollText => "scroll-text.svg",
    Search => "search.svg",
    Server => "server.svg",
    Settings => "settings.svg",
    Shield => "shield.svg",
    ShieldAlert => "shield-alert.svg",
    SlidersHorizontal => "sliders-horizontal.svg",
    Square => "square.svg",
    Timer => "timer.svg",
    TriangleAlert => "triangle-alert.svg",
    Undo => "undo-2.svg",
    UserX => "user-x.svg",
    Users => "users.svg",
    X => "x.svg",
}

/// An icon `size` pixels square at 100%, drawn in `color`. GPUI colors an SVG with the element's
/// own text color, which it does not inherit, so the color must be given here.
pub fn icon(name: IconName, size: f32, color: Hsla) -> Svg {
    svg()
        .path(name.path())
        .flex_none()
        .size(scaled(size))
        .text_color(color)
}

/// Serves the built-in icons to GPUI.
#[derive(Debug)]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(IconName::ALL
            .iter()
            .find(|name| name.path() == path)
            .map(|name| Cow::Borrowed(name.svg())))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(IconName::ALL
            .iter()
            .map(|name| name.path())
            .filter(|asset| asset.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::{Assets, IconName};
    use gpui::AssetSource;

    #[test]
    fn every_icon_is_served_as_an_svg() -> gpui::Result<()> {
        for name in IconName::ALL {
            let bytes = Assets.load(name.path())?.unwrap_or_default();
            let text = String::from_utf8_lossy(&bytes);
            assert!(text.contains("<svg"), "{} is not an SVG", name.path());
        }
        assert!(Assets.load("icons/missing.svg")?.is_none());
        assert_eq!(Assets.list("icons/")?.len(), IconName::ALL.len());
        Ok(())
    }
}
