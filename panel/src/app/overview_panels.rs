//! Overview panels that bring part of another page onto the Overview: its players, the log,
//! bans, the settings and recent messages, each with a way to the page that has the rest.

use super::arrange::PanelContent;
use super::overview::{activity_row, hint, view_log_button};
use super::play_page::{GAME_MEANING, role_icon};
use super::players::{moderation_buttons, player_name};
use super::{NOT_SAVED, Page, Panel, SaveConfig, Selected, display_name};
use crate::dashboard::PlayerRow;
use crate::format::{byte_rate, millis};
use crate::icons::IconName;
use crate::overview_layout::PanelState;
use crate::theme::{MONO, Theme, scaled};
use crate::widgets::{Button, FocusRing as _, OneLine as _, Tip, Tone, badge, card_header};
use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*};

/// Banned addresses listed before the rest are left to the Bans page.
const BANS_SHOWN: usize = 8;

impl Panel {
    /// Who is playing, their link in brief, and Kick and Ban; a row opens their details.
    pub(super) fn players_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        let compact = self.preferences.compact;
        let rows: Vec<AnyElement> = self
            .dashboard
            .sorted_players(self.player_sort)
            .into_iter()
            .map(|row| {
                let (id, joined_at) = (row.id, row.joined_at);
                let number = id.get();
                // By name, as the Players page has them, with the same marks.
                let name = self.shown_text(&player_name(row));
                let link = link_summary(row);
                let accent = theme.accent;
                div()
                    .id(SharedString::from(format!("overview-player-{number}")))
                    .tip(format!("{name}'s details · player {number}"), cx)
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(compact, Styled::py_1)
                    .when(!compact, Styled::py_2)
                    .px_2()
                    .mx(scaled(-9.))
                    .rounded_md()
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .tab_index(0)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.surface_sunken))
                    .focus_ring(move |style| style.border_color(accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(Selected { id, joined_at });
                        this.go_to(Page::Players, cx);
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1p5()
                            .w(scaled(132.))
                            .font_weight(FontWeight::MEDIUM)
                            .children(role_icon(theme, row.role))
                            .child(div().min_w_0().one_line().child(name)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .one_line()
                                    .font_family(MONO)
                                    .text_size(scaled(13.))
                                    .child(self.shown_address(row.address)),
                            )
                            .child(
                                div()
                                    .one_line()
                                    .text_size(scaled(12.))
                                    .text_color(theme.text_muted)
                                    .child(link),
                            ),
                    )
                    .children(moderation_buttons(id, true, false, cx))
                    .into_any_element()
            })
            .collect();
        let count = rows.len();
        let body = if rows.is_empty() {
            let text = if self.running.is_some() {
                "Nobody is playing yet."
            } else {
                "The server is stopped."
            };
            hint(theme, text).into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap_1()
                .children(rows)
                .into_any_element()
        };
        PanelContent::Card {
            header: titled(theme, "Players", (count > 0).then(|| count.to_string())),
            actions: vec![open_page_button(
                "overview-open-players",
                "All players",
                "The Players page: every column, sorting and details",
                Page::Players,
                cx,
            )],
            body: Some(body),
        }
    }

    /// The log's newest entries of the kinds chosen for this panel.
    pub(super) fn log_panel(
        &self,
        theme: &Theme,
        panel: &PanelState,
        cx: &mut Context<Self>,
    ) -> PanelContent {
        let entries: Vec<AnyElement> = self
            .dashboard
            .log()
            .iter()
            .rev()
            .filter(|entry| panel.kinds.contains(&entry.kind))
            .take(usize::from(panel.rows))
            .enumerate()
            .map(|(index, entry)| {
                let shown = (self.entry_time(entry), self.entry_text(entry));
                activity_row(theme, index, entry, shown, self.preferences.compact)
            })
            .collect();
        let body = if entries.is_empty() {
            let text = if panel.kinds.is_empty() {
                "No kinds of entry are chosen; Customize chooses them."
            } else {
                "No entries of the chosen kinds yet."
            };
            hint(theme, text).into_any_element()
        } else {
            div().flex().flex_col().children(entries).into_any_element()
        };
        // The kinds shown, said briefly when they are all or the usual set.
        let subtitle = if panel.kinds.len() == super::Kind::ALL.len() {
            "Every kind of entry".to_owned()
        } else if panel.kinds == super::Kind::shown_by_default() {
            "Everything but reports".to_owned()
        } else {
            let kinds: Vec<&str> = panel.kinds.iter().map(|kind| kind.label()).collect();
            kinds.join(", ")
        };
        PanelContent::Card {
            header: card_header(theme, "Log", Some(&subtitle)).into_any_element(),
            actions: vec![view_log_button(cx)],
            body: Some(body),
        }
    }

    /// Banned addresses, with Unban, and the field that bans another.
    pub(super) fn bans_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        let compact = self.preferences.compact;
        let bans = self.bans.bans();
        let rows: Vec<AnyElement> = bans
            .iter()
            .take(BANS_SHOWN)
            .map(|ip| {
                let ip = *ip;
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when(compact, Styled::py_0p5)
                    .when(!compact, Styled::py_1p5)
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .one_line()
                            .font_family(MONO)
                            .text_size(scaled(13.))
                            .child(self.shown_ip(ip)),
                    )
                    .child(
                        Button::new(SharedString::from(format!("overview-unban-{ip}")), "Unban")
                            .small()
                            .tone(Tone::Quiet)
                            .tooltip("Lets this address join again")
                            .on_click(cx.listener(move |this, _, _, cx| this.unban(ip, cx))),
                    )
                    .into_any_element()
            })
            .collect();
        let more = bans.len().saturating_sub(BANS_SHOWN);
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_family(MONO)
                            .child(self.ban_input.clone()),
                    )
                    .child(
                        Button::new("overview-ban", "Ban")
                            .icon(IconName::Ban)
                            .tone(Tone::Danger)
                            .tooltip("Refuses this IP address from now on (Enter in the field)")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.ban_address(window, cx)),
                            ),
                    ),
            )
            .child(if rows.is_empty() {
                hint(theme, "Nobody is banned.").into_any_element()
            } else {
                div().flex().flex_col().children(rows).into_any_element()
            })
            .when(more > 0, |body| {
                body.child(hint(theme, &format!("{more} more on the Bans page.")))
            })
            .into_any_element();
        let count = bans.len();
        PanelContent::Card {
            header: titled(theme, "Bans", (count > 0).then(|| count.to_string())),
            actions: vec![open_page_button(
                "overview-open-bans",
                "All bans",
                "The Bans page",
                Page::Bans,
                cx,
            )],
            body: Some(body),
        }
    }

    /// The settings file, whether changes wait to be saved or a restart, and the settings
    /// players notice most.
    pub(super) fn settings_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        let file = self
            .form
            .path()
            .map_or_else(|| NOT_SAVED.to_owned(), display_name);
        let path_tip = self.form.path().map_or_else(
            || format!("{NOT_SAVED}: Save keeps them"),
            |path| self.shown_path(path),
        );
        let state = if !self.field_errors.is_empty() {
            Some(badge("Needs fixing", theme.danger, theme.danger_soft))
        } else if self.form.is_dirty() {
            Some(badge("Unsaved changes", theme.accent, theme.accent_soft))
        } else if self.needs_restart() {
            Some(badge("Restart to apply", theme.warning, theme.warning_soft))
        } else {
            None
        };
        let password = if self.form.value("password").is_empty() {
            "None"
        } else {
            "Set"
        };
        let ban_file = self.form.value("ban_file");
        let ban_file = if ban_file.is_empty() {
            "None".to_owned()
        } else {
            self.shown_text(ban_file)
        };
        let facts = [
            ("Player limit", self.form.value("max_players").to_owned()),
            ("Password", password.to_owned()),
            ("Ban file", ban_file),
        ];
        let body = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("overview-settings-file")
                            .tip(path_tip, cx)
                            .one_line()
                            .child(file),
                    )
                    .children(state),
            )
            .children(facts.into_iter().map(|(name, value)| {
                div()
                    .flex()
                    .gap_3()
                    .text_size(scaled(13.))
                    .child(
                        div()
                            .w(scaled(96.))
                            .flex_none()
                            .text_color(theme.text_muted)
                            .child(name),
                    )
                    .child(div().flex_1().min_w_0().one_line().child(value))
            }))
            .into_any_element();
        let save = Button::new("overview-save", "Save")
            .small()
            .enabled(self.form.is_dirty() && self.field_errors.is_empty())
            .tooltip(if self.form.is_dirty() {
                "Saves the server settings (Ctrl+S)"
            } else {
                "Nothing to save"
            })
            .on_click(|_, window, cx| window.dispatch_action(Box::new(SaveConfig), cx))
            .into_any_element();
        PanelContent::Card {
            header: titled(theme, "Settings", None),
            actions: vec![
                save,
                open_page_button(
                    "overview-open-settings",
                    "Edit",
                    "Server settings, every one of them",
                    Page::Settings,
                    cx,
                ),
            ],
            body: Some(body),
        }
    }

    /// The session this PC's game has joined: where it is, and its link's last report.
    pub(super) fn session_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        let body = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.play_state(theme, cx))
            .children(self.render_link(theme))
            .into_any_element();
        PanelContent::Card {
            header: titled(theme, "Your session", None),
            actions: vec![
                self.play_button(true, cx),
                open_play_button("overview-open-session", "Play", cx),
            ],
            body: Some(body),
        }
    }

    /// The server to join, with the Play page's own fields: what is typed here is typed there.
    pub(super) fn join_server_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        PanelContent::Card {
            header: titled(theme, "Join a server", None),
            actions: vec![
                Self::paste_invite_button(true, cx),
                open_play_button("overview-open-join", "Play", cx),
            ],
            body: Some(self.server_fields(theme, cx).into_any_element()),
        }
    }

    /// The player's name and colors, with the Play page's own fields.
    pub(super) fn you_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        PanelContent::Card {
            header: titled(theme, "You", None),
            actions: vec![open_play_button("overview-open-you", "Play", cx)],
            body: Some(self.you_fields(theme, cx).into_any_element()),
        }
    }

    /// The game's files and its emulator, with the Play page's own buttons.
    pub(super) fn game_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        let body = div()
            .flex()
            .flex_col()
            .gap_4()
            .child(hint(theme, GAME_MEANING))
            .child(self.game_fields(theme, cx))
            .into_any_element();
        PanelContent::Card {
            header: titled(theme, "Game", None),
            actions: vec![open_play_button("overview-open-game", "Play", cx)],
            body: Some(body),
        }
    }

    /// That session's chat, in its box, speaking as this PC's player.
    pub(super) fn chat_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        PanelContent::Card {
            header: titled(theme, "Chat", None),
            actions: vec![open_play_button("overview-open-chat", "Play", cx)],
            body: Some(self.play_chat_box(theme, "overview-chat", cx)),
        }
    }

    /// Who is in that session.
    pub(super) fn people_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        let body = if self.play_has_people() {
            self.play_people(theme, cx).into_any_element()
        } else {
            hint(theme, "Not in a session. Join one on the Play page.").into_any_element()
        };
        PanelContent::Card {
            header: titled(theme, "Who is here", None),
            actions: vec![open_play_button("overview-open-people", "Play", cx)],
            body: Some(body),
        }
    }

    /// What happened in that session lately, newest first.
    pub(super) fn session_events_panel(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> PanelContent {
        let rows = self.play_event_rows(theme);
        let body = if rows.is_empty() {
            hint(
                theme,
                "Nothing yet. Joining, hits and why a session ended show here.",
            )
            .into_any_element()
        } else {
            div().flex().flex_col().children(rows).into_any_element()
        };
        PanelContent::Card {
            header: titled(theme, "Session events", None),
            actions: vec![open_play_button("overview-open-events", "Play", cx)],
            body: Some(body),
        }
    }

    /// The messages shown at the corner, newest first.
    pub(super) fn messages_panel(&self, theme: &Theme, rows: u8) -> PanelContent {
        let messages = self.history_rows(theme, usize::from(rows));
        let body = if messages.is_empty() {
            hint(
                theme,
                "No messages yet. Confirmations and problems show here.",
            )
            .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .mx(scaled(-16.))
                .children(messages)
                .into_any_element()
        };
        PanelContent::Card {
            header: titled(theme, "Recent messages", None),
            actions: Vec::new(),
            body: Some(body),
        }
    }
}

/// A player's link in a line, by their latest report.
fn link_summary(row: &PlayerRow) -> String {
    row.link.as_ref().map_or_else(
        || "No report yet".to_owned(),
        |link| {
            let gap = if link.gaps.count > 0 {
                millis(link.gaps.max)
            } else {
                "—".to_owned()
            };
            format!(
                "{:.1}% loss · {gap} gap · {} in",
                link.losses.rate() * 100.0,
                byte_rate(link.byte_rates().1)
            )
        },
    )
}

/// A panel's title, with a count beside it when it has one.
fn titled(theme: &Theme, title: &str, count: Option<String>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(card_header(theme, title, None))
        .children(count.map(|count| badge(count, theme.text_muted, theme.hover)))
        .into_any_element()
}

/// A quiet button to the Play page, which the panels about this PC's own session show part of.
fn open_play_button(id: &'static str, label: &'static str, cx: &mut Context<Panel>) -> AnyElement {
    let tooltip = "The Play page: joining and leaving, your name, and the game";
    open_page_button(id, label, tooltip, Page::Play, cx)
}

/// A quiet button to the page a panel shows part of.
fn open_page_button(
    id: &'static str,
    label: &'static str,
    tooltip: &'static str,
    page: Page,
    cx: &mut Context<Panel>,
) -> AnyElement {
    Button::new(id, label)
        .tone(Tone::Quiet)
        .small()
        .tooltip(format!("{tooltip} ({})", page.shortcut()))
        .on_click(cx.listener(move |this, _, _, cx| this.go_to(page, cx)))
        .into_any_element()
}
