//! Putting the panel's own preferences into effect and keeping them: the interface's size
//! and colors, the clock, the tables' sorting and the Log's filters, and the file they are
//! saved in.

use super::Panel;
use super::messages::Undo;
use crate::format;
use crate::preferences::{self, Preferences};
use crate::theme::{Mode, Theme, ThemeName};
use crate::widgets::TooltipDuration;
use crate::zoom::Zoom;
use gpui::{Context, Window, px};
use std::time::Duration;

impl Panel {
    /// Everything on the Preferences page back to its default, offering to put it back.
    pub(super) fn reset_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let yours = self.preferences.reset();
        let yours = std::mem::replace(&mut self.preferences, yours);
        self.apply_preferences(window, cx);
        self.save_preferences();
        self.show_undoable(
            "Preferences back to their defaults".to_owned(),
            Undo::RestorePreferences(Box::new(yours)),
        );
        cx.notify();
    }

    /// Puts into effect what the preferences say, after they changed all at once.
    pub(super) fn apply_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_rem_size(px(self.preferences.zoom.rem_size()));
        self.apply_theme(window, cx);
        cx.set_global(TooltipDuration(Duration::from_secs(u64::from(
            self.preferences.tooltip_seconds,
        ))));
        format::set_rate_unit(self.preferences.rate_unit);
        self.hour24 = self.preferences.uses_24_hour();
        self.mask_fields(cx);
        self.load_views();
    }

    /// The tables' sorting and the Log's filters as last kept.
    fn load_views(&mut self) {
        self.player_sort = self.preferences.player_sort;
        self.log_sort = self.preferences.log_sort;
        self.ban_order = self.preferences.ban_order;
        self.log_kinds.clone_from(&self.preferences.log_kinds);
    }

    /// Keeps the tables' sorting and the Log's filters for next time, if the user wants them
    /// kept; called whenever one changes.
    pub(super) fn remember_views(&mut self) {
        if !self.preferences.remember_views {
            return;
        }
        self.preferences.player_sort = self.player_sort;
        self.preferences.log_sort = self.log_sort;
        self.preferences.ban_order = self.ban_order;
        self.preferences.log_kinds.clone_from(&self.log_kinds);
        self.save_preferences();
    }

    /// Whether to keep the tables' sorting and the Log's filters. Turned off, the ones kept go
    /// back to the defaults, so the next run starts afresh.
    pub(super) fn set_remember_views(&mut self, on: bool, cx: &mut Context<Self>) {
        self.preferences.remember_views = on;
        let defaults = Preferences::default();
        if on {
            self.remember_views();
        } else {
            self.preferences.player_sort = defaults.player_sort;
            self.preferences.log_sort = defaults.log_sort;
            self.preferences.ban_order = defaults.ban_order;
            self.preferences.log_kinds = defaults.log_kinds;
        }
        self.save_preferences();
        cx.notify();
    }

    /// Writes times of day on the 24-hour clock or the 12-hour one, and remembers it.
    pub(super) fn set_clock(&mut self, hour24: bool, cx: &mut Context<Self>) {
        self.hour24 = hour24;
        self.preferences.clock_24h = Some(hour24);
        self.save_preferences();
        cx.notify();
    }

    /// Draws everything at `zoom` from now on, and remembers it for the next run.
    pub(super) fn set_zoom(&mut self, zoom: Zoom, window: &mut Window, cx: &mut Context<Self>) {
        if zoom == self.preferences.zoom {
            return;
        }
        self.preferences.zoom = zoom;
        window.set_rem_size(px(zoom.rem_size()));
        self.save_preferences();
        cx.notify();
    }

    /// Colors everything in `theme` from now on, and remembers it for the next run.
    pub(super) fn set_theme(
        &mut self,
        theme: ThemeName,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if theme == self.preferences.theme {
            return;
        }
        self.preferences.theme = theme;
        self.apply_theme(window, cx);
        self.save_preferences();
    }

    /// Draws light, dark, or as Windows is set, from now on, and remembers it.
    pub(super) fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if mode == self.preferences.mode {
            return;
        }
        self.preferences.mode = mode;
        self.apply_theme(window, cx);
        self.save_preferences();
    }

    /// Colors the panel as the preferences and Windows' settings now say.
    pub(super) fn apply_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        let preferences = &self.preferences;
        cx.set_global(Theme::for_user(
            preferences.theme,
            preferences.mode,
            window.appearance(),
            self.high_contrast,
        ));
        cx.notify();
    }

    pub(super) fn save_preferences(&mut self) {
        if self.preferences_unread {
            return;
        }
        if let Some(path) = preferences::path()
            && let Err(message) = self.preferences.save(&path)
        {
            self.report_problem(message);
        }
    }
}
