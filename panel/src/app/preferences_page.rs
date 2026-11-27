//! The panel's own preferences: its look, its alerts, what it does on opening, and its keys.

use super::overview::hint;
use super::{Panel, SHORTCUTS, page_frame};
use crate::format::{self, RateUnit};
use crate::icons::{IconName, icon};
use crate::page_layout::ArrangedPage;
use crate::preferences::{
    self, SIDEBAR_WIDTH_DEFAULT, TOOLTIP_SECONDS_DEFAULT, TOOLTIP_SECONDS_MAX,
};
use crate::theme::{MONO, Mode, Theme, ThemeName, scaled};
use crate::widgets::{
    Button, FocusRing as _, Tone, TooltipDuration, banner, card, card_header, switch,
};
use crate::zoom::Zoom;
use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*};
use std::time::Duration;

impl Panel {
    pub(super) fn render_preferences(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let panels = vec![
            ("appearance", Some(self.render_appearance(&theme, cx))),
            ("privacy", Some(self.render_privacy(&theme, cx))),
            ("behavior", Some(self.render_behavior(&theme, cx))),
            ("chat", Some(self.render_game_chat(&theme, cx))),
            ("bots", Some(self.render_bots(&theme, cx))),
            ("alerts", Some(self.render_alerts(&theme, cx))),
            ("startup", Some(self.render_startup(&theme, cx))),
            ("shortcuts", Some(render_shortcuts(&theme))),
            ("about", Some(Self::render_about(&theme, cx))),
            ("reset", Some(self.render_reset(&theme, cx))),
        ];
        let body = self.arranged(&theme, ArrangedPage::Preferences, panels, cx);
        let actions = vec![self.arrange_button(ArrangedPage::Preferences, cx)];
        let subtitle = "How this panel looks and behaves for you. Saved apart from the server's \
                        settings, in %APPDATA%\\B3313 Extended Multiplayer\\panel.conf"
            .to_owned();
        page_frame(&theme, "Preferences", subtitle, actions, body)
    }

    fn render_appearance(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let current = self.preferences.theme;
        let dark = self.preferences.mode.is_dark(cx.window_appearance());
        let schemes: Vec<AnyElement> = ThemeName::ALL
            .into_iter()
            .map(|name| {
                let selected = name == current;
                let swatch = Theme::new(name, dark).accent;
                let accent = theme.accent;
                div()
                    .id(SharedString::from(format!("theme-{}", name.key())))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(scaled(36.))
                    .px_3()
                    .rounded_md()
                    .border_1()
                    .border_color(if selected { accent } else { theme.border })
                    .when(selected, |chip| chip.bg(theme.accent_soft))
                    .tab_index(0)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.hover))
                    .focus_ring(move |style| style.border_color(accent))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.set_theme(name, window, cx);
                    }))
                    .child(div().size(scaled(14.)).rounded_full().bg(swatch))
                    .child(name.label())
                    .into_any_element()
            })
            .collect();
        card(theme)
            .p_5()
            .gap_4()
            .child(card_header(
                theme,
                "Appearance",
                Some("Every color scheme comes light or dark."),
            ))
            .when(self.high_contrast, |card| {
                card.child(banner(
                    theme.text,
                    theme.surface_sunken,
                    IconName::CircleAlert,
                    "A Windows contrast theme is on, so the panel uses High contrast until it is \
                     turned off; the scheme chosen here comes back then."
                        .to_owned(),
                ))
            })
            .child(div().flex().flex_wrap().gap_2().children(schemes))
            .child(self.render_mode_control(theme, cx))
            .child(self.render_zoom_control(theme, cx))
            .child(self.render_sidebar_control(theme, cx))
            .child(self.render_sidebar_width_control(theme, cx))
            .child(self.render_clock_control(theme, cx))
            .child(self.render_tooltip_control(theme, cx))
            .child(self.render_units_control(theme, cx))
            .child(self.render_density_control(theme, cx))
            .into_any_element()
    }

    /// How long tooltips stay, from off to the longest, in steps of a second, as the interface
    /// size steps; the value itself goes back to the default.
    fn render_tooltip_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let seconds = self.preferences.tooltip_seconds;
        let shown = |seconds: u8| match seconds {
            0 => "Off".to_owned(),
            seconds => format!("{seconds} s"),
        };
        let step = |id: &'static str, label: &'static str, to: u8, enabled: bool| {
            let tooltip = match (id, enabled) {
                ("tooltips-shorter", true) if to == 0 => "Turn tooltips off".to_owned(),
                ("tooltips-shorter", true) => format!("Shorter: {}", shown(to)),
                ("tooltips-shorter", false) => "Tooltips are off".to_owned(),
                (_, true) => format!("Longer: {}", shown(to)),
                (_, false) => "Already the longest".to_owned(),
            };
            Button::new(id, label)
                .small()
                .enabled(enabled)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _, _, cx| this.set_tooltip_seconds(to, cx)))
                .into_any_element()
        };
        let is_default = seconds == TOOLTIP_SECONDS_DEFAULT;
        let controls = vec![
            step(
                "tooltips-shorter",
                "−",
                seconds.saturating_sub(1),
                seconds > 0,
            ),
            stepper_value(
                Button::new("tooltips-default", shown(seconds))
                    .small()
                    .tone(Tone::Quiet)
                    .enabled(!is_default)
                    .tooltip(if is_default {
                        "The usual time".to_owned()
                    } else {
                        format!("Back to {}", shown(TOOLTIP_SECONDS_DEFAULT))
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_tooltip_seconds(TOOLTIP_SECONDS_DEFAULT, cx);
                    })),
            ),
            step(
                "tooltips-longer",
                "+",
                (seconds + 1).min(TOOLTIP_SECONDS_MAX),
                seconds < TOOLTIP_SECONDS_MAX,
            ),
        ];
        setting_row(
            theme,
            "Tooltips",
            "How long hover text stays before it goes away. Off shows none.",
            controls,
        )
    }

    /// Keeps tooltips up for `seconds` from now on, none at zero, and remembers it.
    fn set_tooltip_seconds(&mut self, seconds: u8, cx: &mut Context<Self>) {
        self.preferences.tooltip_seconds = seconds;
        cx.set_global(TooltipDuration(Duration::from_secs(u64::from(seconds))));
        self.save_preferences();
        cx.notify();
    }

    /// Light, dark, or as Windows is set.
    fn render_mode_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let current = self.preferences.mode;
        let options: Vec<AnyElement> = Mode::ALL
            .into_iter()
            .map(|mode| {
                let tooltip = match mode {
                    Mode::System => "Light or dark as Windows is set, changing when it does",
                    Mode::Light => "Always light",
                    Mode::Dark => "Always dark",
                };
                choice(
                    SharedString::from(format!("mode-{}", mode.key())),
                    mode.label(),
                    mode == current,
                    tooltip,
                    cx.listener(move |this, _, window, cx| this.set_mode(mode, window, cx)),
                )
            })
            .collect();
        setting_row(
            theme,
            "Light or dark",
            "As Windows follows the app mode in Windows Settings, Personalization, Colors.",
            options,
        )
    }

    /// The sidebar in full, or only its icons.
    fn render_sidebar_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let collapsed = self.preferences.sidebar_collapsed;
        let options = vec![
            choice(
                "sidebar-full",
                "Full",
                !collapsed,
                "Page names, server status and the settings file in words",
                cx.listener(|this, _, _, cx| this.set_sidebar_collapsed(false, cx)),
            ),
            choice(
                "sidebar-icons",
                "Icons only",
                collapsed,
                "Only icons, with the details on hover, leaving the page more room",
                cx.listener(|this, _, _, cx| this.set_sidebar_collapsed(true, cx)),
            ),
        ];
        setting_row(
            theme,
            "Sidebar",
            "Ctrl+B or the button at the sidebar's foot switches between these too.",
            options,
        )
    }

    /// The full sidebar's width, a step at a time; the value itself goes back to the default.
    fn render_sidebar_width_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        const STEP: f32 = 20.;
        let width = self.preferences.sidebar_width;
        let (narrower, wider) = (
            preferences::sidebar_width(f32::from(width) - STEP),
            preferences::sidebar_width(f32::from(width) + STEP),
        );
        let step = |id: &'static str, label: &'static str, to: u16| {
            let enabled = to != width;
            let tooltip = match (id, enabled) {
                ("sidebar-narrower", true) => "Narrower",
                ("sidebar-narrower", false) => "Already the narrowest",
                (_, true) => "Wider",
                (_, false) => "Already the widest",
            };
            Button::new(id, label)
                .small()
                .enabled(enabled)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _, _, cx| this.set_sidebar_width(to, cx)))
                .into_any_element()
        };
        let is_default = width == SIDEBAR_WIDTH_DEFAULT;
        let controls = vec![
            step("sidebar-narrower", "−", narrower),
            stepper_value(
                Button::new("sidebar-width-default", format!("{width} px"))
                    .small()
                    .tone(Tone::Quiet)
                    .enabled(!is_default)
                    .tooltip(if is_default {
                        "The usual width".to_owned()
                    } else {
                        format!("Back to {SIDEBAR_WIDTH_DEFAULT} px")
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.set_sidebar_width(SIDEBAR_WIDTH_DEFAULT, cx);
                    })),
            ),
            step("sidebar-wider", "+", wider),
        ];
        setting_row(
            theme,
            "Sidebar width",
            "When it shows in full, at 100% size. Dragging the sidebar's right edge changes it \
             too, and dragging it far left shows only icons.",
            controls,
        )
    }

    /// Traffic in bytes or in bits.
    fn render_units_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let current = self.preferences.rate_unit;
        let options = RateUnit::ALL
            .into_iter()
            .map(|unit| {
                let (label, tooltip) = match unit {
                    RateUnit::Bytes => ("Bytes", "Traffic like 4.2 KB/s"),
                    RateUnit::Bits => ("Bits", "Traffic like 33.7 kbit/s, as internet speeds are"),
                };
                choice(
                    SharedString::from(format!("units-{}", unit.key())),
                    label,
                    unit == current,
                    tooltip,
                    cx.listener(move |this, _, _, cx| {
                        this.preferences.rate_unit = unit;
                        format::set_rate_unit(unit);
                        this.save_preferences();
                        cx.notify();
                    }),
                )
            })
            .collect::<Vec<_>>();
        setting_row(
            theme,
            "Traffic",
            "In bytes per second, or in bits as internet plans and speed tests give them.",
            options,
        )
    }

    /// Rows of lists at their usual height, or tighter.
    fn render_density_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let compact = self.preferences.compact;
        let option = |id: &'static str, label: &str, on: bool, tooltip: &str| {
            choice(
                id,
                label,
                compact == on,
                tooltip,
                cx.listener(move |this, _, _, cx| {
                    this.preferences.compact = on;
                    this.save_preferences();
                    cx.notify();
                }),
            )
        };
        let options = vec![
            option(
                "density-comfortable",
                "Comfortable",
                false,
                "Rows with room around them",
            ),
            option(
                "density-compact",
                "Compact",
                true,
                "Tighter rows, for many players or a small screen",
            ),
        ];
        setting_row(
            theme,
            "Lists",
            "The Players table, bans, activity and messages; the Log is always compact.",
            options,
        )
    }

    /// The 12-hour clock or the 24-hour one, for times of day in the log and activity.
    fn render_clock_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let options = vec![
            choice(
                "clock-12",
                "12-hour",
                !self.hour24,
                "Times like 9:05:09 PM",
                cx.listener(|this, _, _, cx| this.set_clock(false, cx)),
            ),
            choice(
                "clock-24",
                "24-hour",
                self.hour24,
                "Times like 21:05:09",
                cx.listener(|this, _, _, cx| this.set_clock(true, cx)),
            ),
        ];
        let detail = if self.preferences.clock_24h.is_none() {
            "For times of day in the log and activity lists; until chosen, as Windows writes them."
        } else {
            "For times of day in the log and activity lists, and in exported logs."
        };
        setting_row(theme, "Times of day", detail, options)
    }

    /// Smaller, the current size (which resets it), and larger.
    fn render_zoom_control(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let zoom = self.preferences.zoom;
        let (smaller, larger) = (zoom.smaller(), zoom.larger());
        let percent = SharedString::from(format!("{}%", zoom.percent()));
        let step = |id: &'static str, label: SharedString, to: Zoom, enabled: bool| {
            let tooltip = match (id, enabled) {
                ("zoom-out", true) => "Smaller (Ctrl+−)",
                ("zoom-out", false) => "Already the smallest size",
                (_, true) => "Larger (Ctrl+=)",
                (_, false) => "Already the largest size",
            };
            Button::new(id, label)
                .small()
                .enabled(enabled)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _, window, cx| this.set_zoom(to, window, cx)))
                .into_any_element()
        };
        let controls = vec![
            step("zoom-out", "−".into(), smaller, !zoom.is_smallest()),
            stepper_value(
                Button::new("zoom-reset", percent)
                    .small()
                    .tone(Tone::Quiet)
                    .enabled(zoom != Zoom::default())
                    .tooltip(if zoom == Zoom::default() {
                        "The normal size"
                    } else {
                        "Back to 100% (Ctrl+0)"
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_zoom(Zoom::default(), window, cx);
                    })),
            ),
            step("zoom-in", "+".into(), larger, !zoom.is_largest()),
        ];
        setting_row(
            theme,
            "Interface size",
            "Ctrl+= and Ctrl+− change it from anywhere, as does Ctrl with the mouse wheel. Select \
             the size to go back to 100%.",
            controls,
        )
    }

    fn render_privacy(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        card(theme)
            .p_5()
            .gap_3()
            .child(card_header(
                theme,
                "Privacy",
                Some("For streaming or sharing your screen."),
            ))
            .child(switch(
                theme,
                "streamer-mode-switch",
                "Streamer mode",
                "Hides IP addresses, the names of servers you join and your Windows user folder \
                 everywhere on screen, and keeps the password hidden; 127.0.0.1 and 0.0.0.0 stay, being the same on every PC. \
                 Copying an invite still copies the real details. Ctrl+Shift+H turns it on or \
                 off from anywhere.",
                self.preferences.streamer_mode,
                cx.listener(|this, _, _, cx| {
                    let on = !this.preferences.streamer_mode;
                    this.set_streamer_mode(on, cx);
                }),
            ))
            .into_any_element()
    }
}

/// A setting's name and explanation, with its control at the right.
pub(super) fn setting_row(
    theme: &Theme,
    title: &str,
    detail: &str,
    control: impl IntoIterator<Item = AnyElement>,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_4()
        .child(
            // The text wraps rather than pushing the control past the card's edge.
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(title.to_owned()),
                )
                .child(hint(theme, detail)),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .children(control),
        )
        .into_any_element()
}

/// The value between a stepper's − and +, given one width so steppers on rows above and below
/// line up whatever their values say.
pub(super) fn stepper_value(value: Button) -> AnyElement {
    div()
        .flex()
        .justify_center()
        .w(scaled(64.))
        .child(value)
        .into_any_element()
}

/// One of a few options, the chosen one standing out as the primary button does.
pub(super) fn choice(
    id: impl Into<gpui::ElementId>,
    label: &str,
    chosen: bool,
    tooltip: &str,
    select: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    Button::new(id, label.to_owned())
        .small()
        .tone(if chosen {
            Tone::Primary
        } else {
            Tone::Secondary
        })
        .tooltip(tooltip.to_owned())
        .on_click(select)
        .into_any_element()
}

fn render_shortcuts(theme: &Theme) -> AnyElement {
    let rows = SHORTCUTS.iter().map(|(keys, action)| {
        div()
            .flex()
            .items_center()
            .gap_4()
            .py_2()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_none()
                    .w(scaled(220.))
                    .font_family(MONO)
                    .text_size(scaled(13.))
                    .text_color(theme.text)
                    .child(*keys),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(scaled(13.))
                    .text_color(theme.text_muted)
                    .child(*action),
            )
    });
    card(theme)
        .p_5()
        .gap_3()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(icon(IconName::Keyboard, 16., theme.text_muted))
                .child(card_header(theme, "Keyboard shortcuts", None)),
        )
        .child(div().flex().flex_col().children(rows))
        .into_any_element()
}
