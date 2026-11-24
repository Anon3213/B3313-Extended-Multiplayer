//! Everyone in the session, with their link's health and the buttons to remove them; and one
//! player's details, their link over time and what they did, opened from their row.

use super::chat::{Chat, ChatBox};
use super::column_resize::TableColumn;
use super::messages::ToastTone;
use super::overview::{activity_row, hint};
use super::play_page::{role_icon, role_meaning};
use super::server::server_buttons;
use super::{Panel, Selected, page_frame};
use crate::columns::{self, Widths};
use crate::dashboard::{Dashboard, Kind, PlayerColumn, PlayerRow};
use crate::format::{byte_rate, clock, millis, scale_millis, scale_percent};
use crate::health::{self, Health};
use crate::icons::{IconName, icon};
use crate::page_layout::ArrangedPage;
use crate::theme::{MONO, Theme, scaled};
use crate::widgets::{
    Button, COLUMN_HANDLE_WIDTH, FocusRing as _, OneLine as _, Stat, Tip, Tone, Trend, badge,
    banner, card, card_header, sort_heading, stat_tile,
};
use b3313_protocol::{PlayerId, Role};
use gpui::{
    AnyElement, App, ClipboardItem, Context, Div, FontWeight, SharedString, Window, div,
    prelude::*, px,
};
use std::rc::Rc;

/// The columns' widths are the user's (`columns`), and the address column takes the rest. The
/// least it has: room for an address cut short, which is whole on hover.
const ADDRESS_MIN: u16 = 104;
/// The row's padding at its left.
const ROW_PADDING: u16 = 16;
/// The chevron that says a row opens.
const OPEN: u16 = 24;
/// The cell beside the rows: Kick and Ban with their words, the chevron, and the cell's padding
/// and gap.
const ACTIONS_CELL: u16 = 140 + OPEN + 32;
/// With the columns' usual widths the table has no room to spare: at 100% it must fit the page
/// at the window's starting width (1240 less the sidebar and margins, 942), so the buttons keep
/// their words there. That is why the world's mark is an icon: as a word it cut the name to
/// two letters.
const _: () = assert!(
    columns::PLAYER
        + columns::CONNECTED
        + columns::GAP
        + columns::LOSS
        + columns::TRAFFIC
        + columns::FLAGS
        + ADDRESS_MIN
        + ROW_PADDING
        + ACTIONS_CELL
        <= 942
);

/// How wide the rows must be, at 100%, for every column as the user has it: narrower than
/// this, they scroll sideways.
fn data_min_width(widths: &Widths<PlayerColumn>) -> f32 {
    f32::from(widths.total() + ADDRESS_MIN + ROW_PADDING)
}

/// A column's width as the user has it, at 100%. The address has none: it takes the rest.
fn width_of(widths: &Widths<PlayerColumn>, column: PlayerColumn) -> f32 {
    widths.of(column).map_or(0., f32::from)
}
const HEADER_HEIGHT: f32 = 38.;
/// The page's own margins, which the table does not get.
const PAGE_MARGINS: f32 = 64.;
/// Entries listed in a player's details, newest first.
const ACTIVITY: usize = 100;

// What the table's columns and the details' tiles mean; both say the same.
const LOSS_MEANING: &str = "Their updates lost on the way to the server, in the last report.";
const GAP_MEANING: &str = "The longest wait between their updates in the last report, which is what makes them \
     stutter for others.";
const FLAGS_MEANING: &str = "Times the server flagged unusual movement or impossible progress since they joined. \
     Possible cheating, worth a look, but not proof.";
const WORLD_MEANING: &str =
    "Their game gave the session its world: the shared state everyone else's games follow.";

/// The newest entries about the player in `dashboard`'s log, newest first, by their places
/// in it. Their link reports are charted above the list, and left out of it as the Overview
/// leaves them out too.
fn pick_activity(dashboard: &Dashboard, selected: Selected) -> Vec<usize> {
    dashboard
        .player_log(selected.id, selected.joined_at)
        .rev()
        .filter(|(_, entry)| entry.kind != Kind::Report)
        .take(ACTIVITY)
        .map(|(place, _)| place)
        .collect()
}

impl Panel {
    pub(super) fn render_players(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(selected) = self.selected {
            let activity = self.player_activity(selected);
            return self.render_player_detail(selected, &activity, cx);
        }
        let theme = *cx.global::<Theme>();
        let count = self.dashboard.player_count();
        let subtitle = match (&self.running, count) {
            (None, _) => "The server is stopped".to_owned(),
            (Some(_), 0) => "Nobody is playing".to_owned(),
            (Some(_), 1) => "1 player · select a row for details".to_owned(),
            (Some(_), count) => format!("{count} players · select a row for details"),
        };
        // The page's width at 100%, as the Overview works it out for its columns.
        let page_width = window.viewport_size().width / window.rem_size() * 16.
            - self.shown_sidebar_width()
            - PAGE_MARGINS;
        let mut actions = if self.running.is_some() {
            Vec::new()
        } else {
            server_buttons(self, cx)
        };
        actions.push(self.arrange_button(ArrangedPage::Players, cx));
        let panels = vec![
            ("chat", Some(self.render_server_chat(&theme, cx))),
            (
                "table",
                Some(self.render_players_table(&theme, page_width, cx)),
            ),
        ];
        let body = self.arranged(&theme, ArrangedPage::Players, panels, cx);
        page_frame(&theme, "Players", subtitle, actions, body)
    }

    /// Everyone playing, in a table; or what to do to have someone play.
    fn render_players_table(
        &self,
        theme: &Theme,
        page_width: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *theme;
        let count = self.dashboard.player_count();
        // Where the page is too narrow for every column, Kick and Ban give up their words, to
        // leave the columns what room there is.
        let widths = &self.preferences.player_widths;
        let data_min_width = data_min_width(widths);
        let icons = page_width < data_min_width + f32::from(ACTIONS_CELL);
        if count == 0 {
            let text = if self.running.is_some() {
                "Players appear here as they join. Copy an invite from the Overview \
                 to give them the address and, if one is set, the password."
            } else {
                "Start the server to let players join."
            };
            return card(&theme)
                .p_8()
                .items_center()
                .child(hint(&theme, text))
                .into_any_element();
        }
        let owner = self.dashboard.world_from();
        let you = self.own_player();
        let (rows, actions): (Vec<AnyElement>, Vec<AnyElement>) = self
            .dashboard
            .sorted_players(self.player_sort)
            .into_iter()
            .map(|row| {
                let name = self.shown_text(&player_name(row));
                let shown = (row, name, self.shown_address(row.address));
                let look = RowLook {
                    world_owner: owner == Some(row.id),
                    compact: self.preferences.compact,
                    icons,
                };
                (
                    player_row(&theme, shown, (look, you == Some(row.id)), widths, cx),
                    row_actions(&theme, row, look, cx),
                )
            })
            .unzip();
        // The rows scroll sideways in a window too narrow for them; the buttons are beside
        // that, not in it, so they are never out of reach.
        let mut data = div()
            .id("players-table")
            .flex_1()
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.players_scroll)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(scaled(data_min_width))
                    .child(self.players_header(&theme, cx))
                    .children(rows),
            );
        // The wheel turned up or down is the page's alone: GPUI would have it move these rows
        // sideways as well. Turned sideways (tilted, or with Shift) it moves the rows, and
        // then GPUI would have it move the page as well, so it stops here while the rows have
        // somewhere to go.
        data.style().restrict_scroll_to_axis = Some(true);
        let rows_scroll = self.players_scroll.clone();
        card(&theme)
            .flex_row()
            .on_scroll_wheel(move |event, _, cx| {
                let sideways = event.delta.pixel_delta(px(1.)).x != px(0.);
                if sideways && rows_scroll.max_offset().width >= px(1.) {
                    cx.stop_propagation();
                }
            })
            .child(data)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .child(div().h(scaled(HEADER_HEIGHT)).bg(theme.surface_sunken))
                    .children(actions),
            )
            .into_any_element()
    }

    /// The chat as the server hears it: what players and its owner say, in a box with the
    /// field for the owner to speak to everyone, as the server, or give it a command.
    fn render_server_chat(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let running = self.running.is_some();
        let lines = self
            .server_chat
            .lines()
            .map(|entry| self.chat_line(theme, entry, entry.role))
            .collect();
        let say = Button::new("say-as-server", "Say")
            .icon(IconName::MessageSquare)
            .tone(Tone::Primary)
            .enabled(running)
            .tooltip(if running {
                "Shows in every player's chat, and in their games, as from [Server] (Enter)"
            } else {
                "Start the server to speak to players"
            })
            .on_click(cx.listener(|this, _, _, cx| this.say_as_server(cx)))
            .into_any_element();
        let empty = if running {
            "Nothing has been said yet. What players say, and their commands, show here."
        } else {
            "The server is stopped."
        };
        let chat_box = ChatBox {
            chat: Chat::Server,
            id: "server-chat",
            empty,
        };
        // Said only to one who is both, which is most who host from the panel: the two are
        // easy to take for one, and they speak under different names.
        let about = if self.own_player().is_some() {
            "What everyone says. Here you speak as the server's owner, not as your player, \
             who is the host, with the crown, and speaks from the Play page or the game."
        } else {
            "What everyone says. Here you speak as the server's owner, to everyone."
        };
        card(theme)
            .p_5()
            .gap_3()
            .child(card_header(theme, "Server chat", Some(about)))
            .child(self.chat_box(theme, chat_box, lines, (&self.say_input, say), cx))
            .into_any_element()
    }

    /// What the player whose details are open did, as their details list it, by each
    /// entry's place in the log: found again only when the log or the player has changed.
    fn player_activity(&mut self, selected: Selected) -> Rc<[usize]> {
        let dashboard = &self.dashboard;
        let generation = dashboard.log_generation();
        let picked = &mut self.log_list.activity;
        picked.get(generation, selected, |selected| {
            pick_activity(dashboard, *selected)
        })
    }

    fn render_player_detail(
        &self,
        selected: Selected,
        activity: &[usize],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let id = selected.id;
        let number = id.get();
        // Still the same player only if their number was not given to someone who joined since.
        let row = self
            .dashboard
            .player(id)
            .filter(|row| row.joined_at == selected.joined_at);
        let mut actions = vec![
            Button::new("all-players", "All players")
                .icon(IconName::ArrowLeft)
                .tone(Tone::Quiet)
                .tooltip("Back to the table (Esc)")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.selected = None;
                    cx.notify();
                }))
                .into_any_element(),
        ];
        if let Some(row) = row {
            actions.extend(moderator_button(id, row.role, cx));
            actions.extend(moderation_buttons(id, false, false, cx));
        }
        let subtitle = row.map_or_else(
            || "Left the session · Esc for all players".to_owned(),
            |row| {
                // What the table marks beside the name, in words: there is room for them here.
                let mut who = Vec::new();
                match row.role {
                    Role::Host => who.push("Host"),
                    Role::Moderator => who.push("Moderator"),
                    Role::Player => {}
                }
                if self.own_player() == Some(id) {
                    who.push("you");
                }
                if self.dashboard.world_from() == Some(id) {
                    who.push("their game gave the session its world");
                }
                let who = who.iter().map(|mark| format!("{mark} · "));
                format!(
                    "{}{} · connected {} · Esc for all players",
                    who.collect::<String>(),
                    self.shown_address(row.address),
                    clock(row.joined.elapsed())
                )
            },
        );
        let activity: Vec<AnyElement> = activity
            .iter()
            .filter_map(|&place| self.dashboard.log().get(place))
            .enumerate()
            .map(|(index, entry)| {
                let shown = (self.entry_time(entry), self.entry_text(entry));
                activity_row(&theme, index, entry, shown, self.preferences.compact)
            })
            .collect();
        let empty = activity.is_empty();
        let body = div()
            .flex()
            .flex_col()
            .gap_5()
            .when(row.is_none(), |body| {
                body.child(banner(
                    theme.text_muted,
                    theme.hover,
                    IconName::History,
                    format!("Player {number} has left. What they did while here stays below."),
                ))
            })
            .children(row.map(|row| self.render_player_copy(&theme, row, cx)))
            .children(row.map(|row| link_tiles(&theme, row, cx)))
            .child(
                card(&theme)
                    .p_5()
                    .gap_3()
                    .child(card_header(
                        &theme,
                        "Activity",
                        Some(
                            "Joins, hits, flags and progress, newest first. The Log has the rest.",
                        ),
                    ))
                    .when(empty, |card| card.child(hint(&theme, "Nothing yet.")))
                    .children(activity),
            );
        let title = row.map_or_else(
            || format!("Player {number}"),
            |row| self.shown_text(&player_name(row)),
        );
        page_frame(&theme, &title, subtitle, actions, body)
    }
}

impl Panel {
    /// Buttons that copy a player's address, IP or details, as for a firewall or a report. As
    /// the invite's do, they copy the real details, and show them on hover as the screen may.
    fn render_player_copy(
        &self,
        theme: &Theme,
        row: &PlayerRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let number = row.id.get();
        let copies = [
            (
                "copy-player-address",
                "Address",
                row.address.to_string(),
                "Address copied".to_owned(),
            ),
            (
                "copy-player-ip",
                "IP",
                row.address.ip().to_string(),
                "IP address copied".to_owned(),
            ),
            (
                "copy-player-details",
                "Details",
                player_details(row, self.dashboard.world_from() == Some(row.id)),
                format!("Player {number}'s details copied"),
            ),
        ];
        let buttons = copies.into_iter().map(|(id, label, text, copied)| {
            Button::new(id, label)
                .small()
                .tooltip(self.shown_text(&text))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    this.show_toast(ToastTone::Success, copied.clone());
                    cx.notify();
                }))
                .into_any_element()
        });
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(icon(IconName::Copy, 16., theme.text_faint))
            .child(
                div()
                    .text_size(scaled(13.))
                    .text_color(theme.text_muted)
                    .child("Copy"),
            )
            .children(buttons)
            .into_any_element()
    }
}

/// A player's details as text, to paste into a report: who, from where, for how long, and
/// their link by the latest report.
fn player_details(row: &PlayerRow, world: bool) -> String {
    let link = row.link.as_ref();
    let loss = link.map_or_else(
        || "—".to_owned(),
        |link| format!("{:.1}%", link.losses.rate() * 100.0),
    );
    let gap = link
        .filter(|link| link.gaps.count > 0)
        .map_or_else(|| "—".to_owned(), |link| millis(link.gaps.max));
    let received = link.map_or_else(|| "—".to_owned(), |link| byte_rate(link.byte_rates().1));
    let world = if world {
        "\nTheir game shares its world"
    } else {
        ""
    };
    format!(
        "Player {}\nAddress: {}\nConnected for {}\nPacket loss: {loss} · Longest gap: {gap} · \
         Traffic in: {received}\nFlags: {}{world}",
        row.id.get(),
        row.address,
        clock(row.joined.elapsed()),
        row.flags,
    )
}

/// The player's link as the overview shows the session's: the latest numbers, colored when
/// they need attention, over the last half hour.
fn link_tiles(theme: &Theme, row: &PlayerRow, cx: &App) -> AnyElement {
    let link = row.link.as_ref();
    let loss = link.map(|link| link.losses.rate());
    let gap = link
        .filter(|link| link.gaps.count > 0)
        .map(|link| link.gaps.max);
    let received = link.map(|link| link.byte_rates().1);
    let flags_health = if row.flags == 0 {
        Health::Good
    } else {
        Health::Fair
    };
    let trends = &row.trends;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_4()
                .child(stat_tile(
                    theme,
                    Stat {
                        icon: IconName::Activity,
                        label: "Packet loss",
                        value: loss
                            .map_or_else(|| "—".to_owned(), |rate| format!("{:.1}%", rate * 100.0)),
                        detail: "Of their updates, last report".to_owned(),
                        health: loss.map_or(Health::Good, health::packet_loss),
                        trend: Some(Trend::new(&trends.loss_percent, scale_percent)),
                        explain: format!("{LOSS_MEANING} {}", health::packet_loss_scale()),
                    },
                    cx,
                ))
                .child(stat_tile(
                    theme,
                    Stat {
                        icon: IconName::Timer,
                        label: "Longest gap",
                        value: gap.map_or_else(|| "—".to_owned(), millis),
                        detail: "Between their updates".to_owned(),
                        health: gap.map_or(Health::Good, health::update_gap),
                        trend: Some(Trend::new(&trends.gap_ms, scale_millis)),
                        explain: format!("{GAP_MEANING} {}", health::update_gap_scale()),
                    },
                    cx,
                ))
                .child(stat_tile(
                    theme,
                    Stat {
                        icon: IconName::ArrowUpDown,
                        label: "Traffic in",
                        value: received.map_or_else(|| "—".to_owned(), byte_rate),
                        detail: "From their game".to_owned(),
                        health: Health::Good,
                        trend: Some(Trend::rate(&trends.traffic_in)),
                        explain: "What the server receives from their game each second, by \
                                  their latest report."
                            .to_owned(),
                    },
                    cx,
                ))
                .child(stat_tile(
                    theme,
                    Stat {
                        icon: IconName::Flag,
                        label: "Flags",
                        value: row.flags.to_string(),
                        detail: "Unusual movement or progress".to_owned(),
                        health: flags_health,
                        trend: None,
                        explain: FLAGS_MEANING.to_owned(),
                    },
                    cx,
                )),
        )
        .child(hint(
            theme,
            "Charts show up to their last 30 minutes, a point every 10 seconds; each runs from \
             zero to the value at its top right.",
        ))
        .into_any_element()
}

/// Makes a player a moderator, or an ordinary player again; nothing for the host, who is neither.
fn moderator_button(id: PlayerId, role: Role, cx: &mut Context<Panel>) -> Option<AnyElement> {
    let (label, tip, moderator) = match role {
        Role::Host => return None,
        Role::Player => (
            "Make moderator",
            "Lets them kick and ban ordinary players from the chat, until they leave the session",
            true,
        ),
        Role::Moderator => (
            "Remove moderator",
            "Makes them an ordinary player again",
            false,
        ),
    };
    Some(
        Button::new("moderator", label)
            .icon(IconName::Shield)
            .tooltip(tip)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_moderator(id, moderator);
                cx.notify();
            }))
            .into_any_element(),
    )
}

/// Kick and Ban for one player: small in a table row, and as their icons alone where the row
/// has no room for the words.
pub(super) fn moderation_buttons(
    id: PlayerId,
    small: bool,
    icons: bool,
    cx: &mut Context<Panel>,
) -> Vec<AnyElement> {
    let number = id.get();
    let size = |button: Button| {
        let button = button.icon_only(icons);
        if small { button.small() } else { button }
    };
    vec![
        size(
            Button::new(SharedString::from(format!("kick-{number}")), "Kick").icon(IconName::UserX),
        )
        .tooltip("Removes them now; they can join again")
        .on_click(cx.listener(move |this, _, _, cx| {
            this.kick(id);
            cx.notify();
        }))
        .into_any_element(),
        size(
            Button::new(SharedString::from(format!("ban-{number}")), "Ban")
                .icon(IconName::Ban)
                .tone(Tone::Danger),
        )
        .tooltip(
            "Refuses their IP address from now on, including anyone else playing from it; asks \
             first",
        )
        .on_click(cx.listener(move |this, _, window, cx| {
            this.ban_player(id, window, cx);
        }))
        .into_any_element(),
    ]
}

/// What a player is called: the name they are listed under, or their number until they are.
pub(super) fn player_name(row: &PlayerRow) -> String {
    row.name
        .clone()
        .unwrap_or_else(|| format!("Player {}", row.id.get()))
}

/// How a row of the table is drawn.
#[derive(Clone, Copy)]
struct RowLook {
    /// Their game gave the session its world.
    world_owner: bool,
    /// Tighter rows, as the preferences ask.
    compact: bool,
    /// Kick and Ban as icons alone, for want of room.
    icons: bool,
}

/// A player's row. `name` and `address` are the row's as the screen may show them, and
/// `is_you` says the player is this PC's own, joined to the server it hosts.
fn player_row(
    theme: &Theme,
    (row, name, address): (&PlayerRow, String, String),
    (look, is_you): (RowLook, bool),
    widths: &Widths<PlayerColumn>,
    cx: &mut Context<Panel>,
) -> AnyElement {
    let wide = |column| column_of(width_of(widths, column));
    let RowLook {
        world_owner,
        compact,
        ..
    } = look;
    let id = row.id;
    let number = id.get();
    let role = match row.role {
        Role::Player => String::new(),
        role => format!(" · {}", role_meaning(role)),
    };
    let you = is_you.then(|| {
        badge("you", theme.text_muted, theme.hover)
            .id(SharedString::from(format!("you-{number}")))
            .tip(
                "Your own player, joined from this panel. You are also this server's owner, \
                 which is not a role in the session: see the server chat.",
                cx,
            )
    });
    let selected = Selected {
        id,
        joined_at: row.joined_at,
    };
    let (gap, loss, traffic) = link_cells(theme, row, cx);
    let flags = if row.flags == 0 {
        div().text_color(theme.text_faint).child("0")
    } else {
        badge(row.flags.to_string(), theme.warning, theme.warning_soft)
    };
    let flags = div()
        .id(SharedString::from(format!("flags-{number}")))
        .tip(FLAGS_MEANING, cx)
        .child(flags);
    let accent = theme.accent;
    div()
        .id(SharedString::from(format!("player-{number}")))
        .flex()
        .items_center()
        .h(scaled(row_height(compact)))
        .pl_4()
        .border_t_1()
        .border_color(theme.border)
        .cursor_pointer()
        .tab_index(0)
        .hover(|style| style.bg(theme.surface_sunken))
        .focus_ring(move |style| style.border_color(accent))
        .on_click(cx.listener(move |this, _, _, cx| {
            this.selected = Some(selected);
            cx.notify();
        }))
        .child(
            wide(PlayerColumn::Player)
                .flex()
                .items_center()
                .gap_2()
                // Clear of the address beside it.
                .pr_3()
                .children(role_icon(theme, row.role))
                .child(
                    div()
                        .id(SharedString::from(format!("name-{number}")))
                        .tip(format!("{name} · player {number}{role}"), cx)
                        .min_w_0()
                        .one_line()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name),
                )
                .children(you)
                .when(world_owner, |cell| {
                    cell.child(
                        div()
                            .id(SharedString::from(format!("world-{number}")))
                            .tip(WORLD_MEANING, cx)
                            .flex_none()
                            .child(icon(IconName::Globe, 13., theme.accent)),
                    )
                }),
        )
        .child(
            div()
                .id(SharedString::from(format!("address-{number}")))
                .tip(address.clone(), cx)
                .flex_1()
                .min_w_0()
                .pr_3()
                .font_family(MONO)
                .text_size(scaled(13.))
                // Cut short inside the padding, which text would otherwise run through.
                .child(div().one_line().child(address)),
        )
        .child(cell(wide(PlayerColumn::Connected)).child(clock(row.joined.elapsed())))
        .child(cell(wide(PlayerColumn::Gap)).child(gap))
        .child(cell(wide(PlayerColumn::Loss)).child(loss))
        .child(
            cell(wide(PlayerColumn::Traffic))
                .text_size(scaled(13.))
                .child(traffic),
        )
        .child(wide(PlayerColumn::Flags).child(flags))
        .into_any_element()
}

/// Kick, Ban and the way in to a player's details, in a cell of their own beside the row: the
/// row scrolls sideways in a narrow window, and these must stay in reach.
fn row_actions(
    theme: &Theme,
    row: &PlayerRow,
    look: RowLook,
    cx: &mut Context<Panel>,
) -> AnyElement {
    let number = row.id.get();
    let selected = Selected {
        id: row.id,
        joined_at: row.joined_at,
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .h(scaled(row_height(look.compact)))
        .pl_2()
        .pr_4()
        .border_t_1()
        .border_color(theme.border)
        .children(moderation_buttons(row.id, true, look.icons, cx))
        .child(
            column_of(f32::from(OPEN))
                .id(SharedString::from(format!("open-{number}")))
                .tip("Their details: link charts and what they did", cx)
                .flex()
                .justify_end()
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected = Some(selected);
                    cx.notify();
                }))
                .child(icon(IconName::ChevronRight, 16., theme.text_faint)),
        )
        .into_any_element()
}

/// A row's height, at 100%: the same for its data and for the cell of buttons beside it.
fn row_height(compact: bool) -> f32 {
    if compact { 40. } else { 52. }
}

/// A row's longest gap, loss and traffic, the first two colored and explained.
fn link_cells(
    theme: &Theme,
    row: &PlayerRow,
    cx: &App,
) -> (impl IntoElement, impl IntoElement, String) {
    let number = row.id.get();
    let link = row.link.as_ref();
    // The server measures no round trips (bridges ping it, not the reverse), but it does see
    // the longest wait between a player's updates, which is what makes them stutter for others.
    let measured_gap = link
        .filter(|link| link.gaps.count > 0)
        .map(|link| link.gaps.max);
    let gap = measured(
        theme,
        (format!("gap-{number}"), health::update_gap_scale()),
        measured_gap.map_or_else(|| "—".to_owned(), millis),
        measured_gap.map_or(Health::Good, health::update_gap),
        cx,
    );
    let loss = measured(
        theme,
        (format!("loss-{number}"), health::packet_loss_scale()),
        link.map_or_else(
            || "—".to_owned(),
            |link| format!("{:.1}%", link.losses.rate() * 100.0),
        ),
        link.map_or(Health::Good, |link| health::packet_loss(link.losses.rate())),
        cx,
    );
    let traffic = link.map_or_else(
        || "—".to_owned(),
        |link| {
            let (sent, received) = link.byte_rates();
            // As the server counts: in from this player, out to them.
            format!("{} / {}", byte_rate(received), byte_rate(sent))
        },
    );
    (gap, loss, traffic)
}

/// A measurement, in the warning or danger color when it is out of its usual range, with the
/// range explained on hover. `tip` is the cell's id and that explanation.
fn measured(
    theme: &Theme,
    tip: (String, String),
    text: String,
    health: Health,
    cx: &App,
) -> impl IntoElement {
    let (id, explain) = tip;
    div()
        .id(SharedString::from(id))
        .tip(explain, cx)
        .when_some(theme.health(health), |cell, color| {
            cell.text_color(color).font_weight(FontWeight::SEMIBOLD)
        })
        .child(text)
}

/// A column `width` wide at 100%, which neither grows nor gives way.
fn column_of(width: f32) -> Div {
    div().flex_none().w(scaled(width))
}

/// A column's cell of text, which a column made too narrow for it cuts short, clear of the
/// column beside it.
fn cell(column: Div) -> Div {
    column.pr_2().one_line()
}

impl Panel {
    /// Column headings, each sorting by its column and explaining it on hover, with a line
    /// between each two that changes a column's width.
    fn players_header(&self, theme: &Theme, cx: &Context<Self>) -> AnyElement {
        let sort = self.player_sort;
        let widths = &self.preferences.player_widths;
        let heading = |sorts: PlayerColumn, text: &str, explain: String| {
            let cell = sort_heading(
                theme,
                SharedString::from(format!("heading-{text}")),
                text,
                (sort.column == sorts).then_some(sort.order),
                &explain,
                cx.listener(move |this, _, _, cx| {
                    this.player_sort = this.player_sort.select(sorts);
                    this.remember_views();
                    cx.notify();
                }),
                cx,
            );
            // As wide as its column in the rows: the address takes the rest there too.
            let column = match widths.of(sorts) {
                Some(width) => column_of(f32::from(width)),
                None => div().flex_1().min_w_0(),
            };
            // The line at its right edge, between it and the next column's heading.
            let line = columns::line_after(sorts).map(|(changed, rightward)| {
                self.column_handle(theme, (TableColumn::Player(changed), rightward), cx)
            });
            column
                .relative()
                .h_full()
                .flex()
                .items_center()
                .overflow_hidden()
                .pr(scaled(COLUMN_HANDLE_WIDTH))
                .child(cell)
                .children(line)
                .into_any_element()
        };
        header_row(theme, heading)
    }
}

/// The table's headings in their row, each made by `heading` from its column, its words and
/// what it means.
fn header_row(
    theme: &Theme,
    heading: impl Fn(PlayerColumn, &str, String) -> AnyElement,
) -> AnyElement {
    let player = heading(
        PlayerColumn::Player,
        "PLAYER",
        "Who they are, with the host's or a moderator's mark; sorts by their number in the \
         session, given as they join"
            .to_owned(),
    );
    let address = heading(
        PlayerColumn::Address,
        "ADDRESS",
        "The IP address and port they connect from; a ban refuses the IP address".to_owned(),
    );
    let connected = heading(
        PlayerColumn::Connected,
        "CONNECTED",
        "How long since they joined".to_owned(),
    );
    let gap = heading(
        PlayerColumn::Gap,
        "LONGEST GAP",
        format!("{GAP_MEANING} {}", health::update_gap_scale()),
    );
    let loss = heading(
        PlayerColumn::Loss,
        "LOSS",
        format!("{LOSS_MEANING} {}", health::packet_loss_scale()),
    );
    let traffic = heading(
        PlayerColumn::Traffic,
        "TRAFFIC IN / OUT",
        "What comes from their game each second / what goes to it, by their latest report; \
         sorts by what comes in"
            .to_owned(),
    );
    let flags = heading(PlayerColumn::Flags, "FLAGS", FLAGS_MEANING.to_owned());
    div()
        .flex()
        .items_center()
        .h(scaled(HEADER_HEIGHT))
        .pl_4()
        .bg(theme.surface_sunken)
        .text_size(scaled(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(player)
        .child(address)
        .child(connected)
        .child(gap)
        .child(loss)
        .child(traffic)
        .child(flags)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{ACTIVITY, Selected, pick_activity};
    use crate::dashboard::{Dashboard, Kind};
    use b3313_protocol::metrics::{LinkReport, Losses, Timings, Traffic};
    use b3313_protocol::{HitKind, PlayerId};
    use b3313_server::Event;
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::{Duration, Instant};

    #[test]
    fn a_players_details_list_what_they_did_newest_first_without_their_link_reports() {
        let (Some(luigi), Some(peach)) = (PlayerId::new(2), PlayerId::new(4)) else {
            unreachable!("the test's player numbers");
        };
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 9000));
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        let link = LinkReport {
            elapsed: Duration::from_secs(10),
            sent: Traffic::default(),
            received: Traffic::default(),
            losses: Losses::default(),
            round_trips: Timings::default(),
            gaps: Timings::default(),
        };
        let hit = Event::Hit {
            attacker: peach,
            target: luigi,
            kind: HitKind::Punch,
        };
        dashboard.apply(&Event::Joined(luigi, address), now);
        dashboard.apply(&Event::Joined(peach, address), now);
        dashboard.apply(&Event::Link(luigi, Box::new(link)), now);
        dashboard.note(Kind::Problem, "about nobody".to_owned(), now);
        dashboard.apply(&hit, now);
        let selected = Selected {
            id: luigi,
            joined_at: Duration::ZERO,
        };
        assert_eq!(pick_activity(&dashboard, selected), [4, 0]);
        // Only the newest, however much there is.
        for _ in 0..=ACTIVITY {
            dashboard.apply(&hit, now);
        }
        let newest = pick_activity(&dashboard, selected);
        assert_eq!(newest.len(), ACTIVITY);
        assert_eq!(newest.first(), Some(&(dashboard.log().len() - 1)));
    }
}
