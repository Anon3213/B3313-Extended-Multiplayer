//! The server's settings, grouped, each with what it does and why a value is refused.

use super::{
    NOT_SAVED, NewConfig, OpenConfig, Panel, SaveConfig, SaveConfigAs, display_name, page_frame,
};
use crate::icons::{IconName, icon};
use crate::page_layout::ArrangedPage;
use crate::settings::{Field, FieldKind, GROUPS, help};
use crate::theme::{MONO, Theme, scaled};
use crate::widgets::{Button, FocusRing as _, Tip, Tone, card, card_header};
use gpui::{AnyElement, Context, FontWeight, SharedString, Window, div, prelude::*};

impl Panel {
    pub(super) fn render_settings(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let file = self
            .form
            .path()
            .map_or_else(|| NOT_SAVED.to_owned(), display_name);
        let state = match (self.field_errors.len(), self.form.is_dirty()) {
            (0, true) => " · unsaved changes".to_owned(),
            (0, false) => String::new(),
            (1, _) => " · 1 setting needs fixing".to_owned(),
            (count, _) => format!(" · {count} settings need fixing"),
        };
        let dirty = self.form.is_dirty();
        let can_save = self.field_errors.is_empty() && (dirty || self.form.path().is_none());
        let save_tip = if !self.field_errors.is_empty() {
            "Fix the settings marked in red first"
        } else if can_save {
            "Save the settings (Ctrl+S)"
        } else {
            "No changes to save"
        };
        let actions = vec![
            self.arrange_button(ArrangedPage::Settings, cx),
            Button::new("new-config", "New")
                .tone(Tone::Quiet)
                .tooltip("Start from the default settings (Ctrl+N)")
                .on_click(
                    cx.listener(|this, _, window, cx| this.new_config(&NewConfig, window, cx)),
                )
                .into_any_element(),
            Button::new("open-config", "Open…")
                .tooltip("Open a settings file (Ctrl+O)")
                .on_click(
                    cx.listener(|this, _, window, cx| this.open_config(&OpenConfig, window, cx)),
                )
                .into_any_element(),
            Button::new("revert-config", "Revert")
                .enabled(dirty)
                .tooltip(if dirty {
                    "Undo the changes since the file was opened or saved"
                } else {
                    "No changes to undo"
                })
                .on_click(cx.listener(|this, _, _, cx| this.revert_config(cx)))
                .into_any_element(),
            Button::new("save-config-as", "Save as…")
                .tooltip("Save the settings to a new file (Ctrl+Shift+S)")
                .on_click(cx.listener(|this, _, window, cx| {
                    this.save_config_as(&SaveConfigAs, window, cx);
                }))
                .into_any_element(),
            Button::new("save-config", "Save")
                .tone(Tone::Primary)
                .enabled(can_save)
                .tooltip(save_tip)
                .on_click(
                    cx.listener(|this, _, window, cx| this.save_config(&SaveConfig, window, cx)),
                )
                .into_any_element(),
        ];
        // Each group of settings is a panel, by the key `ArrangedPage::Settings` gives it.
        let groups = ArrangedPage::Settings.panels().iter().filter_map(|panel| {
            let group = GROUPS.iter().find(|group| group.title == panel.title)?;
            let fields: Vec<AnyElement> = group
                .fields
                .iter()
                .map(|field| self.render_field(&theme, field, cx))
                .collect();
            let card = card(&theme)
                .p_5()
                .gap_1()
                .child(card_header(&theme, group.title, group.detail))
                .children(fields)
                .into_any_element();
            Some((panel.key, Some(card)))
        });
        let mut panels: Vec<_> = groups.collect();
        panels.push(("recent", self.render_recent_files(&theme, cx)));
        // What needs a restart stays first, whatever the order below it.
        let body = div()
            .flex()
            .flex_col()
            .gap_5()
            .children(self.render_restart_banner(&theme, cx))
            .child(self.arranged(&theme, ArrangedPage::Settings, panels, cx));
        let subtitle = format!("{file}{state}");
        page_frame(&theme, "Server settings", subtitle, actions, body)
    }

    /// The other settings files opened or saved lately, each a click from being open again.
    fn render_recent_files(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let current = self.form.path();
        let chips: Vec<AnyElement> = self
            .preferences
            .recent_files
            .iter()
            .filter(|path| Some(path.as_path()) != current)
            .enumerate()
            .map(|(index, path)| {
                let accent = theme.accent;
                let open = path.clone();
                div()
                    .id(("recent-file", index))
                    .flex()
                    .items_center()
                    .h(scaled(30.))
                    .px_3()
                    .rounded_full()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .text_size(scaled(13.))
                    .tab_index(0)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.hover))
                    .focus_ring(move |style| style.border_color(accent))
                    .tip(format!("Open {}", self.shown_path(path)), cx)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let open = open.clone();
                        this.unless_unsaved(
                            window,
                            cx,
                            "Open another settings file?",
                            move |this, _, cx| this.open_path(&open, cx),
                        );
                    }))
                    .child(display_name(path))
                    .into_any_element()
            })
            .collect();
        if chips.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(icon(IconName::History, 16., theme.text_faint))
                .child(
                    div()
                        .text_size(scaled(13.))
                        .text_color(theme.text_muted)
                        .child("Recent"),
                )
                .children(chips)
                .into_any_element(),
        )
    }

    /// Show, or Hide: the button beside the password field `key`, which is `shown` as text or
    /// not. Each field has its own, which shows that field alone.
    fn password_toggle(&self, key: &'static str, shown: bool, cx: &mut Context<Self>) -> Button {
        Button::new(
            SharedString::from(format!("show-{key}")),
            if shown { "Hide" } else { "Show" },
        )
        .tone(Tone::Quiet)
        .small()
        .enabled(!self.preferences.streamer_mode)
        .tooltip(match (self.preferences.streamer_mode, shown) {
            (true, _) => "Streamer mode keeps the password hidden",
            (false, true) => "Hide the password as dots",
            (false, false) => "Show the password as text",
        })
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_password(key, cx)))
    }

    fn render_field(&self, theme: &Theme, field: &Field, cx: &mut Context<Self>) -> AnyElement {
        let input = self
            .fields
            .iter()
            .find(|(key, _)| *key == field.key)
            .map(|(_, input)| input.clone());
        let error = self.field_errors.get(field.key).cloned();
        let extra = match field.kind {
            FieldKind::Number { unit } => Some(
                div()
                    .w(scaled(84.))
                    .text_size(scaled(13.))
                    .text_color(theme.text_muted)
                    .child(unit)
                    .into_any_element(),
            ),
            FieldKind::Password => {
                let shown = input
                    .as_ref()
                    .is_some_and(|input| !input.read(cx).is_masked());
                Some(
                    div()
                        .w(scaled(84.))
                        .child(self.password_toggle(field.key, shown, cx))
                        .into_any_element(),
                )
            }
            FieldKind::Path => None,
        };
        let width = if field.kind == FieldKind::Path {
            420.
        } else {
            240.
        };
        // The label and field share a line; the help runs the full width beneath, so it stays
        // readable however large the interface is drawn.
        let control = div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .w(scaled(width))
                            .when(field.kind != FieldKind::Password, |cell| {
                                cell.font_family(MONO)
                            })
                            .children(input),
                    )
                    .children(extra),
            )
            .children(error.map(|error| {
                div()
                    .max_w(scaled(width + 96.))
                    .text_size(scaled(12.))
                    .text_color(theme.danger)
                    .child(error)
            }));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .py_3()
            .border_t_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap_6()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pt(scaled(7.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(field.label),
                    )
                    .child(control),
            )
            .child(
                div()
                    .text_size(scaled(12.))
                    .text_color(theme.text_muted)
                    .child(help(field.key)),
            )
            .into_any_element()
    }
}
