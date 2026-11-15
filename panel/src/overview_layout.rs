//! What the Overview shows and in what order: panels the user arranges, collapses, removes and
//! adds, saved with the panel's preferences. Only what the layout is lives here; drawing it is
//! the Overview's.

use crate::dashboard::Kind;
use std::fmt::Write as _;

/// Something the Overview can show. Each appears at most once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PanelKind {
    /// The server's state, controls, listen address and invite. It cannot be removed, so
    /// starting and stopping the server is always on the Overview.
    Server,
    PlayersTile,
    TickTile,
    TrafficOutTile,
    TrafficInTile,
    RateLimitedTile,
    UptimeTile,
    BansTile,
    FlagsTile,
    Activity,
    Players,
    Log,
    Bans,
    Settings,
    Messages,
    /// The session this PC's game has joined, from the Play page: its state and its link.
    Session,
    /// That session's chat, in its box.
    Chat,
    /// Who is in that session.
    People,
    /// What happened in that session lately.
    SessionEvents,
    /// The server to join, as on the Play page: its address and password, from an invite,
    /// typed, or recent.
    JoinServer,
    /// The player's name and colors, as on the Play page.
    You,
    /// The game's files and the emulator, as on the Play page.
    Game,
}

impl PanelKind {
    /// Every panel, in the order the list of panels to add shows them.
    pub const ALL: [Self; 22] = [
        Self::Server,
        Self::PlayersTile,
        Self::TickTile,
        Self::TrafficOutTile,
        Self::TrafficInTile,
        Self::RateLimitedTile,
        Self::UptimeTile,
        Self::BansTile,
        Self::FlagsTile,
        Self::Activity,
        Self::Players,
        Self::Log,
        Self::Bans,
        Self::Settings,
        Self::Messages,
        Self::Session,
        Self::Chat,
        Self::People,
        Self::SessionEvents,
        Self::JoinServer,
        Self::You,
        Self::Game,
    ];

    /// How the preferences save it.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::PlayersTile => "players-tile",
            Self::TickTile => "tick-tile",
            Self::TrafficOutTile => "traffic-out-tile",
            Self::TrafficInTile => "traffic-in-tile",
            Self::RateLimitedTile => "rate-limited-tile",
            Self::UptimeTile => "uptime-tile",
            Self::BansTile => "bans-tile",
            Self::FlagsTile => "flags-tile",
            Self::Activity => "activity",
            Self::Players => "players",
            Self::Log => "log",
            Self::Bans => "bans",
            Self::Settings => "settings",
            Self::Messages => "messages",
            Self::Session => "session",
            Self::Chat => "chat",
            Self::People => "people",
            Self::SessionEvents => "session-events",
            Self::JoinServer => "join-server",
            Self::You => "you",
            Self::Game => "game",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Server => "Server",
            Self::PlayersTile | Self::Players => "Players",
            Self::TickTile => "Tick time",
            Self::TrafficOutTile => "Traffic out",
            Self::TrafficInTile => "Traffic in",
            Self::RateLimitedTile => "Rate limited",
            Self::UptimeTile => "Uptime",
            Self::BansTile | Self::Bans => "Bans",
            Self::FlagsTile => "Flags",
            Self::Activity => "Recent activity",
            Self::Log => "Log",
            Self::Settings => "Settings",
            Self::Messages => "Recent messages",
            Self::Session => "Your session",
            Self::Chat => "Chat",
            Self::People => "Who is here",
            Self::SessionEvents => "Session events",
            Self::JoinServer => "Join a server",
            Self::You => "You",
            Self::Game => "Game",
        }
    }

    /// What it shows, as the list of panels to add says.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Server => "The server's state and controls, its listen address and the invite",
            Self::PlayersTile => "How many are playing, out of the limit, over time",
            Self::TickTile => "How long the server takes to update everyone, over time",
            Self::TrafficOutTile => "What the server sends to all players, over time",
            Self::TrafficInTile => "What the server receives from all players, over time",
            Self::RateLimitedTile => "Packets dropped for coming too fast, over time",
            Self::UptimeTile => "How long the server has been running",
            Self::BansTile => "How many addresses are banned",
            Self::FlagsTile => "How many times players were flagged this session",
            Self::Activity => "Joins, kicks, bans, flags and problems, newest first",
            Self::Players => "Who is playing, their link, and Kick and Ban",
            Self::Log => "The log's newest entries, of the kinds you choose",
            Self::Bans => "Banned addresses, with a field to ban another",
            Self::Settings => "The settings file, what waits to be saved, and key settings",
            Self::Messages => "The messages shown at the corner, with their times",
            Self::Session => "The session your game has joined, its link, and Join or Leave",
            Self::Chat => "That session's chat, with the field to say something",
            Self::People => "Who is in that session, with the host and moderators marked",
            Self::SessionEvents => "What happened in that session lately: joining, hits, its end",
            Self::JoinServer => "The server to join: paste an invite, or pick a recent server",
            Self::You => "Your name and your Mario's colors, as other players see them",
            Self::Game => "The game's files and the emulator that runs it",
        }
    }

    /// A small number with a chart, grouped in a row with the tiles beside it.
    #[must_use]
    pub fn is_tile(self) -> bool {
        matches!(
            self,
            Self::PlayersTile
                | Self::TickTile
                | Self::TrafficOutTile
                | Self::TrafficInTile
                | Self::RateLimitedTile
                | Self::UptimeTile
                | Self::BansTile
                | Self::FlagsTile
        )
    }

    #[must_use]
    pub fn removable(self) -> bool {
        self != Self::Server
    }

    /// Whether it lists entries, of which the user chooses how many.
    #[must_use]
    pub fn has_rows(self) -> bool {
        matches!(self, Self::Activity | Self::Log | Self::Messages)
    }

    fn default_width(self) -> Width {
        match self {
            Self::Bans
            | Self::Settings
            | Self::Messages
            | Self::Session
            | Self::People
            | Self::SessionEvents
            | Self::You
            | Self::Game => Width::Half,
            _ => Width::Full,
        }
    }
}

/// How much of the Overview's width a panel takes; two halves side by side make a row. Tiles
/// share their row with the tiles beside them instead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Full,
    Half,
}

/// How many entries a listing panel may show.
pub const ROW_CHOICES: [u8; 3] = [5, 8, 15];
const DEFAULT_ROWS: u8 = 8;

/// One panel on the Overview and how the user set it.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelState {
    pub kind: PanelKind,
    pub width: Width,
    /// Only its heading shows; a tile shows its number without its chart.
    pub collapsed: bool,
    /// For a listing panel, how many entries it shows.
    pub rows: u8,
    /// For the Log panel, the kinds of entry it shows.
    pub kinds: Vec<Kind>,
}

impl PanelState {
    #[must_use]
    pub fn new(kind: PanelKind) -> Self {
        Self {
            kind,
            width: kind.default_width(),
            collapsed: false,
            rows: DEFAULT_ROWS,
            kinds: Kind::shown_by_default(),
        }
    }

    /// Reads one panel as `format` writes it, as `log:half:collapsed:rows=5:kinds=flag+problem`.
    /// None for an unknown panel; anything else unreadable keeps its default.
    fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(':').map(str::trim);
        let mut panel = Self::new(PanelKind::from_key(parts.next()?)?);
        for part in parts {
            match part.split_once('=') {
                None if part == "full" => panel.width = Width::Full,
                None if part == "half" => panel.width = Width::Half,
                None if part == "collapsed" => panel.collapsed = true,
                Some(("rows", rows)) => {
                    if let Some(rows) = rows.parse().ok().filter(|rows| ROW_CHOICES.contains(rows))
                    {
                        panel.rows = rows;
                    }
                }
                Some(("kinds", kinds)) => panel.kinds = Kind::parse_list(kinds),
                _ => {}
            }
        }
        Some(panel)
    }

    fn format(&self) -> String {
        let mut text = self.kind.key().to_owned();
        if !self.kind.is_tile() {
            text.push_str(match self.width {
                Width::Full => ":full",
                Width::Half => ":half",
            });
        }
        if self.collapsed {
            text.push_str(":collapsed");
        }
        // Writing to a String cannot fail.
        if self.kind.has_rows() && self.rows != DEFAULT_ROWS {
            let _ = write!(text, ":rows={}", self.rows);
        }
        if self.kind == PanelKind::Log && self.kinds != Kind::shown_by_default() {
            let _ = write!(text, ":kinds={}", Kind::format_list(&self.kinds));
        }
        text
    }
}

/// The Overview's panels, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct OverviewLayout {
    panels: Vec<PanelState>,
}

impl Default for OverviewLayout {
    /// The Overview as it was before it could be arranged.
    fn default() -> Self {
        Self {
            panels: [
                PanelKind::Server,
                PanelKind::PlayersTile,
                PanelKind::TickTile,
                PanelKind::TrafficOutTile,
                PanelKind::RateLimitedTile,
                PanelKind::Activity,
            ]
            .into_iter()
            .map(PanelState::new)
            .collect(),
        }
    }
}

impl OverviewLayout {
    /// Reads a layout as `format` writes it: panels apart by commas. Unknown or repeated panels
    /// are left out, and the Server panel is put first if it is missing.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut panels: Vec<PanelState> = Vec::new();
        for panel in text.split(',').filter_map(PanelState::parse) {
            if !panels.iter().any(|known| known.kind == panel.kind) {
                panels.push(panel);
            }
        }
        if !panels.iter().any(|panel| panel.kind == PanelKind::Server) {
            panels.insert(0, PanelState::new(PanelKind::Server));
        }
        Self { panels }
    }

    #[must_use]
    pub fn format(&self) -> String {
        let panels: Vec<String> = self.panels.iter().map(PanelState::format).collect();
        panels.join(", ")
    }

    #[must_use]
    pub fn panels(&self) -> &[PanelState] {
        &self.panels
    }

    #[must_use]
    pub fn position(&self, kind: PanelKind) -> Option<usize> {
        self.panels.iter().position(|panel| panel.kind == kind)
    }

    #[must_use]
    pub fn get(&self, kind: PanelKind) -> Option<&PanelState> {
        self.panels.iter().find(|panel| panel.kind == kind)
    }

    pub fn get_mut(&mut self, kind: PanelKind) -> Option<&mut PanelState> {
        self.panels.iter_mut().find(|panel| panel.kind == kind)
    }

    /// The panels not on the Overview, which can be added.
    #[must_use]
    pub fn available(&self) -> Vec<PanelKind> {
        PanelKind::ALL
            .into_iter()
            .filter(|kind| self.position(*kind).is_none())
            .collect()
    }

    /// Adds `kind` at the end, if it is not there already.
    pub fn add(&mut self, kind: PanelKind) {
        if self.position(kind).is_none() {
            self.panels.push(PanelState::new(kind));
        }
    }

    /// Takes `kind` away, returning where it was and how it was set, to put it back with
    /// `restore`. The Server panel stays.
    pub fn remove(&mut self, kind: PanelKind) -> Option<(usize, PanelState)> {
        if !kind.removable() {
            return None;
        }
        let at = self.position(kind)?;
        Some((at, self.panels.remove(at)))
    }

    /// Puts back a panel `remove` took, where it was.
    pub fn restore(&mut self, at: usize, panel: PanelState) {
        if self.position(panel.kind).is_none() {
            self.panels.insert(at.min(self.panels.len()), panel);
        }
    }

    /// Moves `kind` to `to`, counted among the other panels, as a drop between them gives.
    pub fn move_to(&mut self, kind: PanelKind, to: usize) {
        if let Some(from) = self.position(kind) {
            let panel = self.panels.remove(from);
            self.panels.insert(to.min(self.panels.len()), panel);
        }
    }

    /// Moves `kind` one place earlier, or later with `later`.
    pub fn nudge(&mut self, kind: PanelKind, later: bool) {
        if let Some(from) = self.position(kind) {
            let to = if later {
                (from + 1).min(self.panels.len() - 1)
            } else {
                from.saturating_sub(1)
            };
            self.panels.swap(from, to);
        }
    }
}

/// One of the Overview's rows: tiles side by side, two half-width panels, or one panel.
/// Positions are into the sequence given to `rows`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Tiles(Vec<usize>),
    /// A half-width panel, and the one beside it if the next is half width too.
    Halves(usize, Option<usize>),
    Full(usize),
}

/// Groups a sequence of panels, each a tile or of a width, into rows: tiles next to each other
/// share a row, as do two half-width panels in turn; a full-width panel has its own. With
/// `narrow`, half-width panels take the whole width, as there is no room for two.
#[must_use]
pub fn rows(sequence: &[(bool, Width)], narrow: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut at = 0;
    while let Some(&(tile, width)) = sequence.get(at) {
        if tile {
            let start = at;
            while sequence.get(at).is_some_and(|(tile, _)| *tile) {
                at += 1;
            }
            rows.push(Row::Tiles((start..at).collect()));
            continue;
        }
        if width == Width::Full || narrow {
            rows.push(Row::Full(at));
            at += 1;
            continue;
        }
        let beside = sequence
            .get(at + 1)
            .filter(|(tile, width)| !tile && *width == Width::Half)
            .map(|_| at + 1);
        rows.push(Row::Halves(at, beside));
        at += if beside.is_some() { 2 } else { 1 };
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::{OverviewLayout, PanelKind, PanelState, Row, Width, rows};
    use crate::dashboard::Kind;

    #[test]
    fn layouts_read_back_and_anything_odd_is_left_out() {
        let mut layout = OverviewLayout::default();
        layout.add(PanelKind::Log);
        layout.add(PanelKind::Bans);
        if let Some(log) = layout.get_mut(PanelKind::Log) {
            log.width = Width::Half;
            log.collapsed = true;
            log.rows = 5;
            log.kinds = vec![Kind::Flag, Kind::Problem];
        }
        let text = layout.format();
        assert!(
            text.contains("log:half:collapsed:rows=5:kinds=flag+problem"),
            "{text}"
        );
        assert_eq!(OverviewLayout::parse(&text), layout);
        // Unknown and repeated panels are dropped, the Server panel comes back, and bad
        // options keep their defaults.
        let odd = OverviewLayout::parse("plaid, activity:wide:rows=6, activity:half, log:full");
        let kinds: Vec<PanelKind> = odd.panels().iter().map(|panel| panel.kind).collect();
        assert_eq!(
            kinds,
            [PanelKind::Server, PanelKind::Activity, PanelKind::Log]
        );
        assert_eq!(
            odd.get(PanelKind::Activity),
            Some(&PanelState::new(PanelKind::Activity))
        );
    }

    /// Two panels saved under one key would come back as the same panel.
    #[test]
    fn every_panel_is_saved_under_a_key_of_its_own() {
        for kind in PanelKind::ALL {
            assert_eq!(PanelKind::from_key(kind.key()), Some(kind));
        }
        // And a layout with all of them reads back whole.
        let mut layout = OverviewLayout::default();
        for kind in PanelKind::ALL {
            layout.add(kind);
        }
        assert_eq!(layout.panels().len(), PanelKind::ALL.len());
        assert_eq!(OverviewLayout::parse(&layout.format()), layout);
    }

    #[test]
    fn panels_are_added_once_and_the_server_stays() {
        let mut layout = OverviewLayout::default();
        assert!(layout.available().contains(&PanelKind::Log));
        layout.add(PanelKind::Log);
        layout.add(PanelKind::Log);
        assert!(!layout.available().contains(&PanelKind::Log));
        assert_eq!(layout.remove(PanelKind::Server), None);
        let (at, removed) = layout
            .remove(PanelKind::TickTile)
            .unwrap_or_else(|| (0, PanelState::new(PanelKind::Server)));
        assert_eq!((at, removed.kind), (2, PanelKind::TickTile));
        layout.restore(at, removed);
        assert_eq!(layout, {
            let mut expected = OverviewLayout::default();
            expected.add(PanelKind::Log);
            expected
        });
    }

    #[test]
    fn panels_move_where_they_are_dropped() {
        let mut layout = OverviewLayout::default();
        // Activity, last, to just after the Server panel.
        layout.move_to(PanelKind::Activity, 1);
        assert_eq!(layout.position(PanelKind::Activity), Some(1));
        layout.nudge(PanelKind::Activity, false);
        assert_eq!(layout.position(PanelKind::Activity), Some(0));
        layout.nudge(PanelKind::Activity, false);
        assert_eq!(layout.position(PanelKind::Activity), Some(0));
        layout.move_to(PanelKind::Activity, 99);
        assert_eq!(layout.position(PanelKind::Activity), Some(5));
        layout.nudge(PanelKind::Activity, true);
        assert_eq!(layout.position(PanelKind::Activity), Some(5));
    }

    #[test]
    fn tiles_share_rows_and_halves_pair_up() {
        let (tile, full, half) = (
            (true, Width::Full),
            (false, Width::Full),
            (false, Width::Half),
        );
        let sequence = [full, tile, tile, half, half, half, full, tile];
        assert_eq!(
            rows(&sequence, false),
            [
                Row::Full(0),
                Row::Tiles(vec![1, 2]),
                Row::Halves(3, Some(4)),
                Row::Halves(5, None),
                Row::Full(6),
                Row::Tiles(vec![7]),
            ]
        );
        // Without room for two, each half-width panel has its own row.
        assert_eq!(rows(&[half, half], true), [Row::Full(0), Row::Full(1)]);
    }
}
