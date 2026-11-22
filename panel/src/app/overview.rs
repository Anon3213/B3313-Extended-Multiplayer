//! The first page: the server's state and controls, its numbers, and what happened lately, in
//! panels the user arranges (`arrange.rs` frames and moves them; this says what each shows).

use super::arrange::{PanelContent, accept_panel_drops};
use super::players::player_name;
use super::{
    NOT_SAVED, Page, Panel, RouterPort, Running, ToastTone, display_name, page_frame,
    server_buttons,
};
use crate::dashboard::{Kind, LogEntry, Series, SessionTrends};
use crate::format::{byte_rate, clock, millis, scale_count, scale_millis};
use crate::health::{self, Health};
use crate::icons::{IconName, icon};
use crate::network::{Invite, Reach, place_to_give, server_to_give};
use crate::overview_layout::{PanelKind, PanelState};
use crate::theme::{MONO, Theme, scaled};
use crate::widgets::{
    Button, FocusRing as _, OneLine as _, Stat, Tip, Tone, Trend, badge, banner, card_header, dot,
    stat_tile,
};
use gpui::{
    AnyElement, App, ClipboardItem, Context, FontWeight, SharedString, Window, div, prelude::*,
};
use std::net::SocketAddr;

// What the tiles measure, shown on hover.
const PLAYERS_MEANING: &str = "Players in the session, out of the player limit. Amber when \
                               full: new players are turned away until someone leaves.";
const TICK_MEANING: &str = "How long the server takes, on average, to update everyone.";
const TRAFFIC_MEANING: &str = "What the server sends to all players together each second, by \
                               their latest reports; below it, what it receives from them.";
const TRAFFIC_IN_MEANING: &str = "What the server receives from all players together each \
                                  second, by their latest reports.";
const RATE_LIMITED_MEANING: &str = "Packets the server dropped for coming faster than any game \
                                    sends. Amber at any at all: someone's game is misbehaving \
                                    or was tampered with.";
const UPTIME_MEANING: &str = "How long the server has run since it last started.";
const BANS_MEANING: &str = "IP addresses refused from joining; the Bans page lists them.";
const FLAGS_MEANING: &str = "Times the server flagged a player for unusual movement or \
                             impossible progress, among the log's entries.";

impl Panel {
    pub(super) fn render_overview(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let subtitle = match self.uptime() {
            Some(uptime) => format!("Up for {}", clock(uptime)),
            None => "The server is stopped".to_owned(),
        };
        let actions = self.arrange_actions(cx);
        let body = div()
            .flex()
            .flex_col()
            .gap_5()
            .children(self.render_restart_banner(&theme, cx))
            .children(self.render_arrange_hint(&theme))
            .children(self.render_panels(window, cx))
            .children(self.render_add_panels(&theme, cx));
        let body = accept_panel_drops(body, cx);
        page_frame(&theme, "Overview", subtitle, actions, body)
    }

    /// What `panel` shows, for `arrange.rs` to frame.
    pub(super) fn panel_content(&self, panel: &PanelState, cx: &mut Context<Self>) -> PanelContent {
        let theme = *cx.global::<Theme>();
        match panel.kind {
            PanelKind::Server => self.server_panel(&theme, cx),
            PanelKind::Activity => self.activity_panel(&theme, panel.rows, cx),
            PanelKind::Players => self.players_panel(&theme, cx),
            PanelKind::Log => self.log_panel(&theme, panel, cx),
            PanelKind::Bans => self.bans_panel(&theme, cx),
            PanelKind::Settings => self.settings_panel(&theme, cx),
            PanelKind::Messages => self.messages_panel(&theme, panel.rows),
            PanelKind::Session => self.session_panel(&theme, cx),
            PanelKind::Chat => self.chat_panel(&theme, cx),
            PanelKind::People => self.people_panel(&theme, cx),
            PanelKind::SessionEvents => self.session_events_panel(&theme, cx),
            PanelKind::JoinServer => self.join_server_panel(&theme, cx),
            PanelKind::You => self.you_panel(&theme, cx),
            PanelKind::Game => self.game_panel(&theme, cx),
            tile => PanelContent::Tile(self.tile(&theme, tile, panel.collapsed, cx)),
        }
    }

    fn server_panel(&self, theme: &Theme, cx: &mut Context<Self>) -> PanelContent {
        let (state, color, detail) = match &self.running {
            Some(running) => (
                "Running",
                theme.success,
                // Who can join, and what to send them, is the invite's part below.
                format!(
                    "Accepting players on {}.",
                    self.shown_address(running.host.address())
                ),
            ),
            None => (
                "Stopped",
                theme.text_faint,
                "Start the server to let players join.".to_owned(),
            ),
        };
        let header = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div().flex().items_center().gap_2().child(dot(color)).child(
                    div()
                        .text_size(scaled(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(state),
                ),
            )
            .child(
                div()
                    .text_size(scaled(13.))
                    .text_color(theme.text_muted)
                    .child(detail),
            )
            .into_any_element();
        let body = div()
            .flex()
            .flex_col()
            .gap_5()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_6()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .w(scaled(360.))
                            .max_w_full()
                            .child(label(theme, "Listen address"))
                            .child(div().font_family(MONO).child(self.address.clone()))
                            // While running, the invite below says who can join.
                            .when(self.running.is_none(), |column| {
                                column.child(hint(
                                    theme,
                                    "127.0.0.1 admits this PC only; 0.0.0.0 admits other \
                                     machines.",
                                ))
                            }),
                    )
                    .child(self.render_settings_summary(theme, cx)),
            )
            .children(self.render_invite(theme, cx))
            .into_any_element();
        PanelContent::Card {
            header,
            actions: server_buttons(self, cx),
            body: Some(body),
        }
    }

    /// Which settings the server uses, with the full path on hover, and a way to edit them.
    fn render_settings_summary(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let settings = match self.form.path() {
            Some(path) if self.form.is_dirty() => {
                format!("{} (unsaved changes)", display_name(path))
            }
            Some(path) => display_name(path),
            None => NOT_SAVED.to_owned(),
        };
        let settings_path = self.form.path().map_or_else(
            || format!("{NOT_SAVED}: Save on Server settings keeps them"),
            |path| self.shown_path(path),
        );
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .flex_1()
            .min_w(scaled(200.))
            .child(label(theme, "Settings"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(scaled(34.))
                    .child(
                        div()
                            .id("settings-file")
                            .one_line()
                            .tip(settings_path, cx)
                            .child(settings),
                    )
                    .child(
                        Button::new("edit-settings", "Edit")
                            .tone(Tone::Quiet)
                            .small()
                            .tooltip("Open Server settings")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.go_to(Page::Settings, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The name the owner gave for invites, if `running` can be reached by one: a server that
    /// hears this PC alone cannot, and an invite to it by a name would lead nowhere.
    fn invite_name_for(&self, running: &Running) -> &str {
        match running.reach {
            Reach::ThisPc(_) => "",
            Reach::Network(_) | Reach::Unknown { .. } => {
                self.preferences.invite_name.as_deref().unwrap_or_default()
            }
        }
    }

    /// Where an invite says `running` is: at the owner's name for it, or at the address the
    /// panel found for it, if it found one.
    fn invite_place(&self, running: &Running) -> Option<String> {
        let (_, _, address) = where_to_join(running.reach, &running.port);
        // The port players reach it at: the one the router opened, or the one it hears.
        let port = match (running.reach, &running.port) {
            (_, RouterPort::Open(map)) => map.public.port(),
            (Reach::ThisPc(heard) | Reach::Network(heard), _) => heard.port(),
            (Reach::Unknown { port }, _) => port,
        };
        place_to_give(self.invite_name_for(running), port, address)
    }

    /// The field for the name invites give for the server, and what such a name is for.
    fn render_invite_name(&self, theme: &Theme, reach: Reach) -> gpui::Div {
        let what_for = if matches!(reach, Reach::ThisPc(_)) {
            "Not used while the server listens on this PC only: a name leads here from outside."
        } else {
            "If you have a name that follows your address as it changes (dynamic DNS, which \
             many routers can keep up to date), invites give it in place of the address and \
             go on working when the address changes. The server's port is added unless you \
             type one."
        };
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .pt_2()
            .child(label(theme, "Name for invites (optional)"))
            .child(
                div()
                    .w(scaled(360.))
                    .max_w_full()
                    .font_family(MONO)
                    .child(self.invite_name.clone()),
            )
            .child(hint(theme, what_for))
    }

    /// Where players connect, and a button that copies everything they need to.
    fn render_invite(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let running = self.running.as_ref()?;
        let (found, explanation, _) = where_to_join(running.reach, &running.port);
        let place = self.invite_place(running);
        let named = !self.invite_name_for(running).trim().is_empty();
        let password = &running.config.password;
        // The buttons copy the real details; what they show on hover hides them in streamer
        // mode, the password included.
        let shown_password = if self.preferences.streamer_mode && !password.is_empty() {
            "••••••"
        } else {
            password.as_str()
        };
        // With the server's key, so that a player's panel joins this server and no other.
        let server = place
            .as_deref()
            .map(|place| server_to_give(place, &running.host.key()));
        let copy_buttons: Vec<AnyElement> = Invite::ALL
            .into_iter()
            .map(|invite| {
                let preview = server
                    .as_deref()
                    .and_then(|server| invite.text(server, shown_password))
                    .map(|text| self.shown_text(&text));
                invite_button(invite, server.is_some(), preview, cx)
            })
            .collect();
        // What invites give: the owner's name where they gave one, else what was found.
        let explanation = if named {
            where_a_name_leads(running.reach, &running.port)
        } else {
            explanation
        };
        let shown = place.filter(|_| named).unwrap_or(found);
        let (shown, explanation) = (self.shown_text(&shown), self.shown_text(&explanation));
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .pt_4()
                .border_t_1()
                .border_color(theme.border)
                .child(label(theme, "Invite players"))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_3()
                        .child(div().font_family(MONO).child(shown))
                        .child(if password.is_empty() {
                            badge("No password", theme.warning, theme.warning_soft)
                        } else {
                            badge("Password set", theme.text_muted, theme.hover)
                        }),
                )
                .child(hint(theme, &explanation))
                // Why what is copied is longer than the address above it.
                .child(hint(
                    theme,
                    "What you copy also carries this server's key, after a # in the address: \
                     with it, a player's panel joins this server and nobody else who answers \
                     there.",
                ))
                .child(self.render_invite_name(theme, running.reach))
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .pt_2()
                        .child(icon(IconName::Copy, 16., theme.text_faint))
                        .child(
                            div()
                                .text_size(scaled(13.))
                                .text_color(theme.text_muted)
                                .child("Copy"),
                        )
                        .children(copy_buttons),
                )
                .into_any_element(),
        )
    }

    /// Puts `invite` on the clipboard and says what went there, warning when that includes
    /// the password.
    fn copy_invite(&mut self, invite: Invite, cx: &mut Context<Self>) {
        let Some(running) = &self.running else {
            return;
        };
        let Some(place) = self.invite_place(running) else {
            return;
        };
        let password = &running.config.password;
        let server = server_to_give(&place, &running.host.key());
        let Some(text) = invite.text(&server, password) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let copied = match invite {
            Invite::Address => "Address copied",
            Invite::WithPassword => "Address and password copied",
            Invite::Full => "Invite copied",
        };
        let message = if invite.shares_password(password) {
            format!("{copied}: it holds the password, so share it only with your players")
        } else {
            copied.to_owned()
        };
        self.show_toast(ToastTone::Success, message);
        cx.notify();
    }

    pub(super) fn render_restart_banner(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.needs_restart() {
            return None;
        }
        let text = if self.field_errors.is_empty() {
            "The settings changed since the server started. Restart it to use them."
        } else {
            "Some settings are invalid; the server keeps the ones it started with."
        };
        let restart = Button::new("banner-restart", "Restart now")
            .small()
            .enabled(self.field_errors.is_empty())
            .tooltip(if self.field_errors.is_empty() {
                "Stops the server and starts it with the new settings; players rejoin by themselves"
            } else {
                "Fix the settings marked in red on Server settings first"
            })
            .on_click(cx.listener(|this, _, window, cx| this.restart(window, cx)));
        Some(
            banner(
                theme.warning,
                theme.warning_soft,
                IconName::TriangleAlert,
                text.to_owned(),
            )
            .child(restart)
            .into_any_element(),
        )
    }

    /// One of the headline numbers; collapsed, without its chart.
    fn tile(&self, theme: &Theme, kind: PanelKind, collapsed: bool, cx: &App) -> AnyElement {
        // Charts only while running: a stopped server has no history to show.
        let trends = self
            .running
            .as_ref()
            .filter(|_| !collapsed)
            .map(|_| self.dashboard.trends());
        let trend = |pick: fn(&SessionTrends) -> &Series, unit: fn(f64) -> String| {
            trends.map(|trends| Trend::new(pick(trends), unit))
        };
        let ticks = self.dashboard.ticks();
        let (sent, received) = self.dashboard.traffic();
        let stat = match kind {
            PanelKind::PlayersTile => {
                let limit = self.running.as_ref().map_or_else(
                    || self.form.value("max_players").to_owned(),
                    |running| running.config.max_players.to_string(),
                );
                let count = self.dashboard.player_count();
                Stat {
                    icon: IconName::Users,
                    label: "Players",
                    value: format!("{count} / {limit}"),
                    detail: match (self.dashboard.world_from(), count) {
                        (Some(owner), _) => {
                            let named = self.dashboard.player(owner).map(player_name);
                            let owner = named.unwrap_or_else(|| format!("Player {}", owner.get()));
                            self.shown_text(&format!("World from {owner}"))
                        }
                        (None, 0) => "Nobody has joined".to_owned(),
                        // The session keeps its world after whoever gave it has left.
                        (None, _) if self.dashboard.has_world() => "World shared".to_owned(),
                        (None, _) => "No world shared yet".to_owned(),
                    },
                    health: limit
                        .parse()
                        .map_or(Health::Good, |limit| health::capacity(count, limit)),
                    trend: trend(|trends| &trends.players, scale_count),
                    explain: PLAYERS_MEANING.to_owned(),
                }
            }
            PanelKind::TickTile => {
                let mean_tick = ticks.and_then(|report| report.took.mean());
                Stat {
                    icon: IconName::Timer,
                    label: "Tick time",
                    value: mean_tick.map_or_else(|| "—".to_owned(), millis),
                    detail: ticks.map_or_else(
                        || "Reported every few seconds".to_owned(),
                        |report| format!("{} at most", millis(report.took.max)),
                    ),
                    health: mean_tick.map_or(Health::Good, health::tick_time),
                    trend: trend(|trends| &trends.tick_ms, scale_millis),
                    explain: format!("{TICK_MEANING} {}", health::tick_time_scale()),
                }
            }
            PanelKind::TrafficOutTile => Stat {
                icon: IconName::ArrowUpDown,
                label: "Traffic out",
                value: byte_rate(sent),
                detail: format!("{} in", byte_rate(received)),
                health: Health::Good,
                trend: trends.map(|trends| Trend::rate(&trends.traffic_out)),
                explain: TRAFFIC_MEANING.to_owned(),
            },
            PanelKind::TrafficInTile => Stat {
                icon: IconName::ArrowUpDown,
                label: "Traffic in",
                value: byte_rate(received),
                detail: format!("{} out", byte_rate(sent)),
                health: Health::Good,
                trend: trends.map(|trends| Trend::rate(&trends.traffic_in)),
                explain: TRAFFIC_IN_MEANING.to_owned(),
            },
            PanelKind::RateLimitedTile => Stat {
                icon: IconName::ShieldAlert,
                label: "Rate limited",
                value: ticks
                    .map_or_else(|| "—".to_owned(), |report| report.rate_limited.to_string()),
                detail: "Packets dropped in the last report".to_owned(),
                health: ticks.map_or(Health::Good, |report| {
                    health::rate_limited(report.rate_limited)
                }),
                trend: trend(|trends| &trends.rate_limited, scale_count),
                explain: RATE_LIMITED_MEANING.to_owned(),
            },
            other => self.count_stat(other),
        };
        stat_tile(theme, stat, cx)
    }

    /// The tiles that count something without a chart: uptime, bans and flags.
    fn count_stat(&self, kind: PanelKind) -> Stat<'static> {
        match kind {
            PanelKind::UptimeTile => Stat {
                icon: IconName::History,
                label: "Uptime",
                value: self.uptime().map_or_else(|| "—".to_owned(), clock),
                detail: if self.running.is_some() {
                    "Since the server started".to_owned()
                } else {
                    "The server is stopped".to_owned()
                },
                health: Health::Good,
                trend: None,
                explain: UPTIME_MEANING.to_owned(),
            },
            PanelKind::BansTile => {
                let count = self.bans.bans().len();
                Stat {
                    icon: IconName::Ban,
                    label: "Bans",
                    value: count.to_string(),
                    detail: if count == 1 {
                        "Address refused".to_owned()
                    } else {
                        "Addresses refused".to_owned()
                    },
                    health: Health::Good,
                    trend: None,
                    explain: BANS_MEANING.to_owned(),
                }
            }
            _ => {
                let flags = self.dashboard.kind_count(Kind::Flag);
                Stat {
                    icon: IconName::Flag,
                    label: "Flags",
                    value: flags.to_string(),
                    detail: "In the log".to_owned(),
                    health: if flags == 0 {
                        Health::Good
                    } else {
                        Health::Fair
                    },
                    trend: None,
                    explain: FLAGS_MEANING.to_owned(),
                }
            }
        }
    }

    fn activity_panel(&self, theme: &Theme, rows: u8, cx: &mut Context<Self>) -> PanelContent {
        let recent: Vec<AnyElement> = self
            .dashboard
            .log()
            .iter()
            .rev()
            .filter(|entry| entry.kind != Kind::Report)
            .take(usize::from(rows))
            .enumerate()
            .map(|(index, entry)| {
                let shown = (self.entry_time(entry), self.entry_text(entry));
                activity_row(theme, index, entry, shown, self.preferences.compact)
            })
            .collect();
        let body = if recent.is_empty() {
            hint(
                theme,
                "Nothing yet. Joins, kicks, bans and flags show here.",
            )
            .into_any_element()
        } else {
            div().flex().flex_col().children(recent).into_any_element()
        };
        PanelContent::Card {
            header: card_header(theme, "Recent activity", None).into_any_element(),
            actions: vec![view_log_button(cx)],
            body: Some(body),
        }
    }
}

/// Opens the Log, from a panel that shows some of it.
pub(super) fn view_log_button(cx: &mut Context<Panel>) -> AnyElement {
    Button::new("view-log", "View log")
        .tone(Tone::Quiet)
        .small()
        .tooltip(format!(
            "Everything the server reported, with filters and search ({})",
            Page::Log.shortcut()
        ))
        .on_click(cx.listener(|this, _, _, cx| this.go_to(Page::Log, cx)))
        .into_any_element()
}

/// The address to show the server's owner, what to tell them about it, and the address an invite
/// carries, if one is known: the router's public one once it has opened the port.
fn where_to_join(reach: Reach, port: &RouterPort) -> (String, String, Option<SocketAddr>) {
    match (reach, port) {
        (Reach::ThisPc(address), _) => (
            address.to_string(),
            "Only games on this PC can join at 127.0.0.1. To let others in, stop the server \
             and listen on 0.0.0.0."
                .to_owned(),
            Some(address),
        ),
        (Reach::Network(address), RouterPort::Open(map)) => (
            map.public.to_string(),
            format!(
                "Your router opened port {} for this server, so players anywhere join at this \
                 address. A player on your own network joins at {address}.",
                address.port()
            ),
            Some(map.public),
        ),
        (Reach::Network(address), port) => {
            let forward = format!(
                "forward UDP port {} to this PC on your router and give them your public IP \
                 address",
                address.port()
            );
            let internet = match port {
                RouterPort::Asking => {
                    "Asking your router to let players over the internet in…".to_owned()
                }
                RouterPort::Refused(reason) => {
                    format!("{reason} To let players over the internet in, {forward}.")
                }
                _ => format!("For players over the internet, {forward}."),
            };
            (
                address.to_string(),
                format!("Players on your network join at this address. {internet}"),
                Some(address),
            )
        }
        (Reach::Unknown { port }, _) => (
            format!("This PC's IP address, port {port}"),
            "This PC's network address could not be found. Give players its IP address \
             (ipconfig shows it) with this port."
                .to_owned(),
            None,
        ),
    }
}

/// What to tell the server's owner about the name their invites give, in place of what
/// `where_to_join` says of the address: what the name must lead to for a player to arrive.
fn where_a_name_leads(reach: Reach, port: &RouterPort) -> String {
    match (reach, port) {
        (Reach::Network(address), RouterPort::Open(map)) => format!(
            "Invites give this name, which must lead to your public address, {}. Your router \
             opened port {} for this server. A player on your own network joins at {address}.",
            map.public.ip(),
            address.port()
        ),
        (Reach::Network(address), _) => format!(
            "Invites give this name, which must lead to your public IP address, with UDP \
             port {} forwarded to this PC on your router. A player on your own network joins \
             at {address}.",
            address.port()
        ),
        (Reach::Unknown { port }, _) => format!(
            "Invites give this name, which must lead to your public IP address, with UDP \
             port {port} forwarded to this PC on your router."
        ),
        // A name is not used for a server that hears this PC alone.
        (Reach::ThisPc(_), _) => String::new(),
    }
}

/// One of the invite's copy buttons, from least to most information. Each shows on hover
/// exactly what it copies, or why it cannot; the full invite, which works for anyone, stands
/// out.
/// `preview` is what it copies as the screen may show it, none if it has nothing to copy;
/// `placed` says the invite has somewhere to say the server is.
fn invite_button(
    invite: Invite,
    placed: bool,
    preview: Option<String>,
    cx: &mut Context<Panel>,
) -> AnyElement {
    let (id, label) = match invite {
        Invite::Address => ("copy-address", "Address"),
        Invite::WithPassword => ("copy-address-password", "Address + password"),
        Invite::Full => ("copy-invite", "Full invite"),
    };
    let enabled = preview.is_some();
    let tooltip = match (preview, placed) {
        (Some(text), _) => text.trim_end().to_owned(),
        (None, false) => "This PC's address is unknown, so there is nothing to copy. A name \
                          for invites, below, would do in its place."
            .to_owned(),
        (None, true) => "No password is set, so this would copy only the address.".to_owned(),
    };
    Button::new(id, label)
        .small()
        .tone(if invite == Invite::Full {
            Tone::Primary
        } else {
            Tone::Secondary
        })
        .enabled(enabled)
        .tooltip(tooltip)
        .on_click(cx.listener(move |this, _, _, cx| this.copy_invite(invite, cx)))
        .into_any_element()
}

/// One entry in a short list of what happened, as on the overview and a player's details; a
/// long one goes on to further lines, as these lists need no rows of one height. `index`
/// tells the list's rows apart.
pub(super) fn activity_row(
    theme: &Theme,
    index: usize,
    entry: &LogEntry,
    (time, text): (String, String),
    compact: bool,
) -> AnyElement {
    div()
        .id(("activity", index))
        .flex()
        .items_start()
        .gap_3()
        .when(compact, Styled::py_1)
        .when(!compact, Styled::py_2)
        .border_t_1()
        .border_color(theme.border)
        .child(kind_icon(theme, entry.kind))
        .child(
            div()
                .w(scaled(96.))
                .flex_none()
                .font_family(MONO)
                .text_size(scaled(12.))
                .text_color(theme.text_faint)
                .child(time),
        )
        .child(div().flex_1().min_w_0().text_size(scaled(13.)).child(text))
        .into_any_element()
}

pub(super) fn kind_color(theme: &Theme, kind: Kind) -> gpui::Hsla {
    match kind {
        Kind::Session => theme.success,
        Kind::Moderation => theme.accent,
        Kind::Flag => theme.warning,
        Kind::Gameplay | Kind::Chat | Kind::Report => theme.text_faint,
        Kind::Problem => theme.danger,
    }
}

/// A kind's glyph in its color: the shape tells kinds apart where color alone would not, as for
/// those who do not see red and green apart.
pub(super) fn kind_icon(theme: &Theme, kind: Kind) -> gpui::Svg {
    let name = match kind {
        Kind::Session => IconName::ArrowRightLeft,
        Kind::Moderation => IconName::Gavel,
        Kind::Flag => IconName::Flag,
        Kind::Gameplay => IconName::Gamepad,
        Kind::Chat => IconName::MessageSquare,
        Kind::Report => IconName::Activity,
        Kind::Problem => IconName::CircleAlert,
    };
    icon(name, 14., kind_color(theme, kind))
}

/// Shows or hides one kind of entry, as the Log's filters and the Log panel's choices do: in
/// the kind's color while shown, faint while hidden, with how many there are if given.
pub(super) fn kind_chip(
    theme: &Theme,
    id: SharedString,
    kind: Kind,
    (on, count): (bool, Option<usize>),
    cx: &App,
    select: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let action = if on {
        "select to hide"
    } else {
        "select to show"
    };
    let accent = theme.accent;
    div()
        .id(id)
        .tip(format!("{}; {action}", kind.description()), cx)
        .flex()
        .items_center()
        .gap_2()
        .h(scaled(30.))
        .px_3()
        .rounded_full()
        .border_1()
        .tab_index(0)
        .cursor_pointer()
        .text_size(scaled(13.))
        .border_color(if on {
            kind_color(theme, kind)
        } else {
            theme.border
        })
        .text_color(if on { theme.text } else { theme.text_faint })
        .when(on, |chip| chip.bg(theme.surface))
        .hover(|style| style.bg(theme.hover))
        .focus_ring(move |style| style.border_color(accent))
        .on_click(select)
        .child(kind_icon(theme, kind))
        .child(kind.label())
        .children(count.map(|count| div().text_color(theme.text_faint).child(count.to_string())))
        .into_any_element()
}

pub(super) fn label(theme: &Theme, text: &str) -> gpui::Div {
    div()
        .text_size(scaled(13.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text)
        .child(text.to_owned())
}

pub(super) fn hint(theme: &Theme, text: &str) -> gpui::Div {
    div()
        .text_size(scaled(12.))
        .text_color(theme.text_muted)
        .child(text.to_owned())
}
