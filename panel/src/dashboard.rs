//! What the panel knows of a running session, built from the events its server reports.

use crate::sort::{Column, Sort, SortOrder};
use b3313_protocol::metrics::LinkReport;
use b3313_protocol::{PlayerId, Role};
use b3313_server::movement::MovementReport;
use b3313_server::{Event, TickReport};
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// Entries the log keeps; older ones are dropped. Link reports alone add one a player every few
/// seconds, so this is about an hour of a full session's reports.
pub const LOG_CAPACITY: usize = 5000;

/// What kind of thing a log entry tells, for filtering and color.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Players joining and leaving, and the server starting and stopping.
    Session,
    /// Kicks, bans and unbans.
    Moderation,
    /// Unusual movement or impossible progress, which may be cheating.
    Flag,
    /// Hits and shared progress.
    Gameplay,
    /// What is said in the chat, commands included.
    Chat,
    /// The regular link, movement and tick reports.
    Report,
    /// Something the user asked for that failed.
    Problem,
}

impl Kind {
    pub const ALL: [Self; 7] = [
        Self::Session,
        Self::Moderation,
        Self::Flag,
        Self::Gameplay,
        Self::Chat,
        Self::Report,
        Self::Problem,
    ];

    /// Its place in `ALL`.
    fn index(self) -> usize {
        match self {
            Self::Session => 0,
            Self::Moderation => 1,
            Self::Flag => 2,
            Self::Gameplay => 3,
            Self::Chat => 4,
            Self::Report => 5,
            Self::Problem => 6,
        }
    }

    /// What a list of entries shows until the user chooses: everything but the routine
    /// reports, which would bury the rest.
    #[must_use]
    pub fn shown_by_default() -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|kind| *kind != Self::Report)
            .collect()
    }

    /// Kinds as the panel's preferences save them, as `flag+problem`, in `ALL`'s order.
    #[must_use]
    pub fn format_list(kinds: &[Self]) -> String {
        let keys: Vec<&str> = Self::ALL
            .into_iter()
            .filter(|kind| kinds.contains(kind))
            .map(Self::key)
            .collect();
        keys.join("+")
    }

    /// Reads what `format_list` writes, leaving out keys it does not know.
    #[must_use]
    pub fn parse_list(text: &str) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|kind| text.split('+').any(|key| key.trim() == kind.key()))
            .collect()
    }

    /// How the panel's preferences save it.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Moderation => "moderation",
            Self::Flag => "flag",
            Self::Gameplay => "gameplay",
            Self::Chat => "chat",
            Self::Report => "report",
            Self::Problem => "problem",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Moderation => "Moderation",
            Self::Flag => "Flags",
            Self::Gameplay => "Gameplay",
            Self::Chat => "Chat",
            Self::Report => "Reports",
            Self::Problem => "Problems",
        }
    }

    /// What entries of this kind are, as their filter explains on hover.
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Self::Session => "Players joining and leaving, and the server starting and stopping",
            Self::Moderation => "Kicks, bans and unbans, and moderators appointed and removed",
            Self::Flag => "Unusual movement or impossible progress, which may be cheating",
            Self::Gameplay => "Hits between players and progress everyone shares",
            Self::Chat => "What players and the server's owner say in the chat, commands included",
            Self::Report => {
                "Each player's link and movement, and the server's tick times, every 10 seconds"
            }
            Self::Problem => "Something that failed, such as a file that could not be saved",
        }
    }

    fn of(event: &Event) -> Self {
        match event {
            Event::Joined(..)
            | Event::Left(_)
            | Event::TimedOut(_)
            | Event::WorldSet(_)
            | Event::Named(..)
            // Nothing is wrong but the version of the game they chose.
            | Event::WrongGame { .. }
            | Event::Ended => Self::Session,
            Event::Kicked(_)
            | Event::Banned(..)
            | Event::AddressBanned(_)
            | Event::Unbanned(_)
            | Event::RoleChanged(..) => Self::Moderation,
            Event::Flagged(..)
            | Event::CoinsRefused(_)
            | Event::ProgressRefused(..)
            | Event::HostPasswordWrong(_)
            | Event::HostRefused(..) => Self::Flag,
            Event::Chat { .. } => Self::Chat,
            Event::Hit { .. } | Event::HitRejected { .. } | Event::ProgressAdded(..) => {
                Self::Gameplay
            }
            Event::Link(..) | Event::Movement(..) | Event::Ticks(_) | Event::Players(_) => {
                Self::Report
            }
            Event::CommandFailed(_) => Self::Problem,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    /// Since the panel opened.
    pub at: Duration,
    pub kind: Kind,
    pub text: String,
    /// `text` in lower case, made once, as the log's search and sort compare it on every frame.
    pub folded: String,
    /// The players it is about: none for the panel's own entries, two for a hit.
    pub players: Vec<PlayerId>,
}

/// Points kept for each chart: half an hour of the server's reports, which come every ten
/// seconds.
pub const HISTORY_POINTS: usize = 180;

/// One measurement's recent values, oldest first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Series(VecDeque<f64>);

impl Series {
    pub fn push(&mut self, value: f64) {
        if self.0.len() == HISTORY_POINTS {
            self.0.pop_front();
        }
        self.0.push_back(value);
    }

    #[must_use]
    pub fn values(&self) -> &VecDeque<f64> {
        &self.0
    }
}

/// The session's measurements over time, one point per tick report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionTrends {
    pub players: Series,
    /// Mean tick time, in milliseconds.
    pub tick_ms: Series,
    /// Bytes per second to all players.
    pub traffic_out: Series,
    /// Bytes per second from all players.
    pub traffic_in: Series,
    pub rate_limited: Series,
}

/// One player's link over time, one point per link report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerTrends {
    pub loss_percent: Series,
    /// The longest wait between updates, in milliseconds.
    pub gap_ms: Series,
    /// Bytes per second from the player.
    pub traffic_in: Series,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerRow {
    pub id: PlayerId,
    /// The name they are listed under, once their bridge has said or the server has made one up.
    pub name: Option<String>,
    pub role: Role,
    pub address: SocketAddr,
    pub joined: Instant,
    /// When they joined, counted as log times are, to find their entries.
    pub joined_at: Duration,
    /// The latest of the reports the server makes every few seconds.
    pub link: Option<LinkReport>,
    pub movement: Option<MovementReport>,
    /// Times the server flagged this player since they joined.
    pub flags: u32,
    pub trends: PlayerTrends,
}

#[derive(Debug)]
pub struct Dashboard {
    opened: Instant,
    /// The local time of day when the panel opened; entries' times of day count from it.
    /// A clock change while the panel is open, as for daylight saving, is not followed.
    opened_time_of_day: Duration,
    players: BTreeMap<u8, PlayerRow>,
    world_from: Option<PlayerId>,
    /// Whether the session has a world, which it keeps after whoever gave it has left.
    has_world: bool,
    ticks: Option<TickReport>,
    trends: SessionTrends,
    log: VecDeque<LogEntry>,
    /// How many entries of each kind the log holds, in `Kind::ALL`'s order.
    kind_counts: [usize; Kind::ALL.len()],
    /// Another number whenever the log changes, so that what was worked out of it, as which
    /// entries a page shows, can be kept until it does.
    log_generation: u64,
}

/// The Players table's columns, by which it can be sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerColumn {
    Player,
    Address,
    Connected,
    Gap,
    Loss,
    Traffic,
    Flags,
}

impl Column for PlayerColumn {
    const ALL: &'static [Self] = &[
        Self::Player,
        Self::Address,
        Self::Connected,
        Self::Gap,
        Self::Loss,
        Self::Traffic,
        Self::Flags,
    ];

    /// Names and addresses in order; measures with the most first, so the longest connected,
    /// the worst links, the busiest and the most flagged come to the top.
    fn first_order(self) -> SortOrder {
        match self {
            Self::Player | Self::Address => SortOrder::Ascending,
            Self::Connected | Self::Gap | Self::Loss | Self::Traffic | Self::Flags => {
                SortOrder::Descending
            }
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Player => "player",
            Self::Address => "address",
            Self::Connected => "connected",
            Self::Gap => "gap",
            Self::Loss => "loss",
            Self::Traffic => "traffic",
            Self::Flags => "flags",
        }
    }
}

/// The log's columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogColumn {
    Time,
    Kind,
    Entry,
}

impl Column for LogColumn {
    const ALL: &'static [Self] = &[Self::Time, Self::Kind, Self::Entry];

    /// Oldest first, as a log reads, newest at the bottom; kinds in their order; text A to Z.
    fn first_order(self) -> SortOrder {
        SortOrder::Ascending
    }

    fn key(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Kind => "kind",
            Self::Entry => "entry",
        }
    }
}

/// How two rows compare in `column`, before the sort's order is applied. Measurements a
/// player has not reported yet come below any value.
fn compare_players(column: PlayerColumn, a: &PlayerRow, b: &PlayerRow) -> Ordering {
    let link = |row: &PlayerRow| row.link;
    match column {
        PlayerColumn::Player => a.id.get().cmp(&b.id.get()),
        PlayerColumn::Address => a.address.cmp(&b.address),
        // How long they have been connected: the earlier joined, the longer.
        PlayerColumn::Connected => b.joined.cmp(&a.joined),
        PlayerColumn::Gap => {
            let gap = |row: &PlayerRow| {
                link(row)
                    .filter(|link| link.gaps.count > 0)
                    .map(|link| link.gaps.max)
            };
            gap(a).cmp(&gap(b))
        }
        PlayerColumn::Loss => {
            let loss = |row: &PlayerRow| link(row).map(|link| link.losses.rate());
            compare_measures(loss(a), loss(b))
        }
        // By what comes in, the first of the column's two numbers.
        PlayerColumn::Traffic => {
            let received = |row: &PlayerRow| link(row).map(|link| link.byte_rates().1);
            compare_measures(received(a), received(b))
        }
        PlayerColumn::Flags => a.flags.cmp(&b.flags),
    }
}

/// Measured values in order, with an unmeasured one below them all.
fn compare_measures(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        (a, b) => a.is_some().cmp(&b.is_some()),
    }
}

/// An event as a crash report's activity keeps it: as the log words it, but without a player's
/// name or what was said in the chat, which a report meant for sharing leaves out.
fn for_crash_report(event: &Event) -> String {
    match event {
        Event::Named(player, _) => format!("Player {} took a name", player.get()),
        Event::Chat { text, .. } => crate::crash::chat_note(text.as_str()),
        // Why a command failed often names the player it was about.
        Event::CommandFailed(why) => format!("A command failed ({} characters)", why.len()),
        other => other.to_string(),
    }
}

/// The players an event is about.
fn players_in(event: &Event) -> Vec<PlayerId> {
    match event {
        Event::Joined(id, _)
        | Event::Left(id)
        | Event::TimedOut(id)
        | Event::WorldSet(id)
        | Event::Link(id, _)
        | Event::Kicked(id)
        | Event::Banned(id, _)
        | Event::Movement(id, _)
        | Event::Flagged(id, _)
        | Event::ProgressAdded(id, _)
        | Event::ProgressRefused(id, _)
        | Event::CoinsRefused(id)
        | Event::Named(id, _)
        | Event::RoleChanged(id, _)
        | Event::HostPasswordWrong(id)
        | Event::HostRefused(id, _) => vec![*id],
        // The server owner's own lines are about nobody.
        Event::Chat { from, .. } => from.iter().copied().collect(),
        Event::Hit {
            attacker, target, ..
        }
        | Event::HitRejected {
            attacker, target, ..
        } => vec![*attacker, *target],
        Event::Ticks(_)
        | Event::AddressBanned(_)
        | Event::Unbanned(_)
        | Event::Players(_)
        | Event::CommandFailed(_)
        // Whoever was refused for their game never became a player.
        | Event::WrongGame { .. }
        | Event::Ended => Vec::new(),
    }
}

impl Dashboard {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            opened: now,
            opened_time_of_day: Duration::ZERO,
            players: BTreeMap::new(),
            world_from: None,
            has_world: false,
            ticks: None,
            trends: SessionTrends::default(),
            log: VecDeque::new(),
            kind_counts: [0; Kind::ALL.len()],
            log_generation: 0,
        }
    }

    /// Counts entries' times of day from `opened`, the local time of day at `new`'s `now`.
    #[must_use]
    pub fn with_time_of_day(mut self, opened: Duration) -> Self {
        self.opened_time_of_day = opened;
        self
    }

    /// When an entry happened, as time since midnight.
    #[must_use]
    pub fn time_of_day(&self, entry: &LogEntry) -> Duration {
        crate::system::wrap_to_day(self.opened_time_of_day + entry.at)
    }

    pub fn apply(&mut self, event: &Event, now: Instant) {
        match event {
            Event::Joined(id, address) => {
                let row = PlayerRow {
                    id: *id,
                    name: None,
                    role: Role::Player,
                    address: *address,
                    joined: now,
                    joined_at: self.since_opened(now),
                    link: None,
                    movement: None,
                    flags: 0,
                    trends: PlayerTrends::default(),
                };
                self.players.insert(id.get(), row);
            }
            Event::Left(id) | Event::TimedOut(id) | Event::Kicked(id) => {
                self.players.remove(&id.get());
                // Whoever has the number next is not the one the world came from.
                if self.world_from == Some(*id) {
                    self.world_from = None;
                }
            }
            Event::WorldSet(id) => {
                self.world_from = Some(*id);
                self.has_world = true;
            }
            Event::Link(id, report) => {
                if let Some(row) = self.players.get_mut(&id.get()) {
                    row.link = Some(**report);
                    let trends = &mut row.trends;
                    trends.loss_percent.push(report.losses.rate() * 100.0);
                    trends.gap_ms.push(report.gaps.max.as_secs_f64() * 1000.0);
                    trends.traffic_in.push(report.byte_rates().1);
                }
            }
            Event::Movement(id, report) => {
                if let Some(row) = self.players.get_mut(&id.get()) {
                    row.movement = Some(*report);
                }
            }
            Event::Flagged(id, _)
            | Event::CoinsRefused(id)
            | Event::ProgressRefused(id, _)
            | Event::HostPasswordWrong(id)
            | Event::HostRefused(id, _) => {
                if let Some(row) = self.players.get_mut(&id.get()) {
                    row.flags += 1;
                }
            }
            Event::Named(id, name) => {
                if let Some(row) = self.players.get_mut(&id.get()) {
                    row.name = Some(name.as_str().to_owned());
                }
            }
            Event::RoleChanged(id, role) => {
                if let Some(row) = self.players.get_mut(&id.get()) {
                    row.role = *role;
                }
            }
            Event::Ticks(report) => {
                self.ticks = Some(*report);
                self.sample_trends(report);
            }
            Event::Players(present) => {
                self.players
                    .retain(|_, row| present.iter().any(|(id, _)| *id == row.id));
            }
            Event::Hit { .. }
            | Event::HitRejected { .. }
            | Event::Banned(..)
            | Event::AddressBanned(_)
            | Event::Unbanned(_)
            | Event::CommandFailed(_)
            | Event::ProgressAdded(..)
            | Event::Chat { .. }
            | Event::WrongGame { .. }
            | Event::Ended => {}
        }
        let kind = Kind::of(event);
        // What led to a crash, but not the routine reports, which would push it out.
        if kind != Kind::Report {
            crate::crash::note(kind.key(), &for_crash_report(event));
        }
        self.record(kind, event.to_string(), players_in(event), now);
    }

    /// Adds the panel's own entry to the log.
    pub fn note(&mut self, kind: Kind, text: String, now: Instant) {
        crate::crash::note(kind.key(), &text);
        self.record(kind, text, Vec::new(), now);
    }

    fn record(&mut self, kind: Kind, text: String, players: Vec<PlayerId>, now: Instant) {
        if self.log.len() == LOG_CAPACITY
            && let Some(dropped) = self.log.pop_front()
        {
            self.count_kind(dropped.kind, false);
        }
        let at = self.since_opened(now);
        self.count_kind(kind, true);
        self.log_changed();
        self.log.push_back(LogEntry {
            at,
            kind,
            folded: text.to_lowercase(),
            text,
            players,
        });
    }

    /// Notes that the log is no longer what it was: every change to it comes through here.
    const fn log_changed(&mut self) {
        self.log_generation = self.log_generation.wrapping_add(1);
    }

    /// A number that is another one after every change to the log: an entry added, which at
    /// the log's capacity also moves every entry one place, or the log cleared or put back.
    #[must_use]
    pub const fn log_generation(&self) -> u64 {
        self.log_generation
    }

    /// Keeps the count of each kind in step with the log, so the filters need not count it.
    fn count_kind(&mut self, kind: Kind, added: bool) {
        if let Some(count) = self.kind_counts.get_mut(kind.index()) {
            *count = if added {
                count.saturating_add(1)
            } else {
                count.saturating_sub(1)
            };
        }
    }

    /// Counts each kind afresh, after the log changed wholesale.
    fn recount_kinds(&mut self) {
        let mut counts = [0; Kind::ALL.len()];
        for entry in &self.log {
            if let Some(count) = counts.get_mut(entry.kind.index()) {
                *count += 1;
            }
        }
        self.kind_counts = counts;
    }

    /// Entries of `kind` in the log.
    #[must_use]
    pub fn kind_count(&self, kind: Kind) -> usize {
        self.kind_counts.get(kind.index()).copied().unwrap_or(0)
    }

    fn since_opened(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.opened)
    }

    /// One point on each session chart, as each tick report comes.
    fn sample_trends(&mut self, report: &TickReport) {
        // Counts past u32::MAX are beyond any session, so a chart may stop at it.
        let count = |count: u64| u32::try_from(count).map_or(f64::from(u32::MAX), f64::from);
        let players = count(u64::try_from(self.players.len()).unwrap_or(u64::MAX));
        let tick_ms = report
            .took
            .mean()
            .map_or(0.0, |mean| mean.as_secs_f64() * 1000.0);
        let (sent, received) = self.traffic();
        self.trends.players.push(players);
        self.trends.tick_ms.push(tick_ms);
        self.trends.traffic_out.push(sent);
        self.trends.traffic_in.push(received);
        self.trends.rate_limited.push(count(report.rate_limited));
    }

    /// Forgets the session, its charts included; the log stays.
    pub fn server_stopped(&mut self) {
        self.players.clear();
        self.world_from = None;
        self.has_world = false;
        self.ticks = None;
        self.trends = SessionTrends::default();
    }

    #[must_use]
    pub fn trends(&self) -> &SessionTrends {
        &self.trends
    }

    #[must_use]
    pub fn player(&self, id: PlayerId) -> Option<&PlayerRow> {
        self.players.get(&id.get())
    }

    /// Entries about `id` since `since`, each with its place in the log, so a player number
    /// used again by someone later does not mix them up with whoever had it before.
    pub fn player_log(
        &self,
        id: PlayerId,
        since: Duration,
    ) -> impl DoubleEndedIterator<Item = (usize, &LogEntry)> {
        self.log
            .iter()
            .enumerate()
            .filter(move |(_, entry)| entry.at >= since && entry.players.contains(&id))
    }

    pub fn players(&self) -> impl Iterator<Item = &PlayerRow> {
        self.players.values()
    }

    /// The players as the table shows them; ties stay in player order.
    #[must_use]
    pub fn sorted_players(&self, sort: Sort<PlayerColumn>) -> Vec<&PlayerRow> {
        let mut rows: Vec<&PlayerRow> = self.players.values().collect();
        rows.sort_by(|a, b| {
            sort.order
                .apply(compare_players(sort.column, a, b))
                .then_with(|| a.id.get().cmp(&b.id.get()))
        });
        rows
    }

    /// Puts log entries, given by their place in the log, in the order the log shows them;
    /// ties stay in the order they happened.
    pub fn sort_log(&self, entries: &mut [usize], sort: Sort<LogColumn>) {
        let log = &self.log;
        entries.sort_by(|&a, &b| {
            let ordering = match (log.get(a), log.get(b)) {
                (Some(first), Some(second)) => match sort.column {
                    LogColumn::Time => a.cmp(&b),
                    LogColumn::Kind => first.kind.cmp(&second.kind),
                    LogColumn::Entry => first.folded.cmp(&second.folded),
                },
                _ => Ordering::Equal,
            };
            sort.order.apply(ordering).then_with(|| a.cmp(&b))
        });
    }

    /// Takes the whole log away, to give back with `restore_log` if the user undoes clearing it.
    pub fn take_log(&mut self) -> VecDeque<LogEntry> {
        self.kind_counts = [0; Kind::ALL.len()];
        self.log_changed();
        std::mem::take(&mut self.log)
    }

    /// Puts back a log `take_log` took, before anything logged since, keeping the newest
    /// entries if together they are more than the log holds.
    pub fn restore_log(&mut self, mut earlier: VecDeque<LogEntry>) {
        earlier.append(&mut self.log);
        let excess = earlier.len().saturating_sub(LOG_CAPACITY);
        earlier.drain(..excess);
        self.log = earlier;
        self.recount_kinds();
        self.log_changed();
    }

    #[must_use]
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// Whose game the session's world came from, if they are still playing.
    #[must_use]
    pub fn world_from(&self) -> Option<PlayerId> {
        self.world_from
            .filter(|id| self.players.contains_key(&id.get()))
    }

    /// Whether the session has a world, whoever it came from.
    #[must_use]
    pub const fn has_world(&self) -> bool {
        self.has_world
    }

    #[must_use]
    pub fn ticks(&self) -> Option<&TickReport> {
        self.ticks.as_ref()
    }

    #[must_use]
    pub fn log(&self) -> &VecDeque<LogEntry> {
        &self.log
    }

    /// Bytes per second the server sends to and receives from all players, by their latest
    /// reports.
    #[must_use]
    pub fn traffic(&self) -> (f64, f64) {
        self.players
            .values()
            .filter_map(|row| row.link.as_ref())
            .map(LinkReport::byte_rates)
            .fold((0.0, 0.0), |(sent, received), (s, r)| {
                (sent + s, received + r)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Dashboard, HISTORY_POINTS, Kind, LOG_CAPACITY, LogColumn, PlayerColumn, for_crash_report,
    };
    use crate::sort::Sort;
    use b3313_protocol::metrics::{LinkReport, Losses, Timings, Traffic};
    use b3313_protocol::{HitKind, Message, Name, PlayerId};
    use b3313_server::movement::Verdict;
    use b3313_server::{Event, TickReport};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::{Duration, Instant};

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    fn address(port: u16) -> SocketAddr {
        (Ipv4Addr::LOCALHOST, port).into()
    }

    #[test]
    fn a_crash_report_keeps_events_without_names_or_what_was_said() {
        let (Some(name), Some(text)) = (Name::new("Weegee"), Message::new("meet at the castle"))
        else {
            unreachable!("the test's name and message are clean");
        };
        let named = for_crash_report(&Event::Named(id(3), name));
        assert!(named.contains("Player 3"), "{named}");
        assert!(!named.contains("Weegee"), "{named}");
        let said = for_crash_report(&Event::Chat {
            from: Some(id(3)),
            name: "Weegee".to_owned(),
            text,
        });
        // How long it was stays: a crash in drawing a line can turn on that.
        assert!(said.contains("18 characters"), "{said}");
        assert!(
            !said.contains("Weegee") && !said.contains("castle"),
            "{said}"
        );
        // Everything else is kept as the log words it.
        let left = Event::Left(id(3));
        assert_eq!(for_crash_report(&left), left.to_string());
    }

    fn ids(dashboard: &Dashboard) -> Vec<u8> {
        dashboard.players().map(|row| row.id.get()).collect()
    }

    #[test]
    fn players_are_listed_from_joining_until_they_go() {
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        for player in 0..4 {
            dashboard.apply(&Event::Joined(id(player), address(9000)), now);
        }
        dashboard.apply(&Event::Left(id(0)), now);
        dashboard.apply(&Event::TimedOut(id(1)), now);
        dashboard.apply(&Event::Kicked(id(3)), now);
        assert_eq!(ids(&dashboard), [2]);
        dashboard.server_stopped();
        assert_eq!(dashboard.player_count(), 0);
        assert_eq!(dashboard.log().len(), 7, "the log outlives the session");
    }

    #[test]
    fn reports_and_flags_attach_to_their_player() {
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        dashboard.apply(&Event::Joined(id(5), address(9005)), now);
        let report = LinkReport {
            elapsed: Duration::from_secs(2),
            sent: Traffic::default(),
            received: Traffic::default(),
            losses: Losses::default(),
            round_trips: Timings::default(),
            gaps: Timings::default(),
        };
        dashboard.apply(&Event::Link(id(5), Box::new(report)), now);
        let unusual = Verdict::Unusual { speed: 9999.0 };
        dashboard.apply(&Event::Flagged(id(5), unusual), now);
        dashboard.apply(&Event::CoinsRefused(id(5)), now);
        // Reports on someone who has gone are not kept.
        dashboard.apply(&Event::Flagged(id(6), unusual), now);

        let row = dashboard.players().next();
        assert_eq!(
            row.map(|row| (row.flags, row.link)),
            Some((2, Some(report)))
        );
        let kinds: Vec<Kind> = dashboard.log().iter().map(|entry| entry.kind).collect();
        let expected = [
            Kind::Session,
            Kind::Report,
            Kind::Flag,
            Kind::Flag,
            Kind::Flag,
        ];
        assert_eq!(kinds, expected);
    }

    #[test]
    fn reports_add_points_to_the_charts_until_the_server_stops() {
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        dashboard.apply(&Event::Joined(id(1), address(9001)), now);
        let link = LinkReport {
            elapsed: Duration::from_secs(10),
            sent: Traffic::default(),
            received: Traffic::default(),
            losses: Losses::default(),
            round_trips: Timings::default(),
            gaps: Timings::default(),
        };
        dashboard.apply(&Event::Link(id(1), Box::new(link)), now);
        let ticks = TickReport {
            took: Timings::default(),
            late: Timings::default(),
            rate_limited: 3,
        };
        for _ in 0..=HISTORY_POINTS {
            dashboard.apply(&Event::Ticks(ticks), now);
        }
        let trends = dashboard.trends();
        assert_eq!(trends.players.values().len(), HISTORY_POINTS);
        assert_eq!(trends.players.values().back(), Some(&1.0));
        assert_eq!(trends.rate_limited.values().back(), Some(&3.0));
        let row = dashboard.player(id(1));
        assert_eq!(
            row.map(|row| row.trends.loss_percent.values().len()),
            Some(1)
        );
        dashboard.server_stopped();
        assert!(dashboard.trends().players.values().is_empty());
    }

    #[test]
    fn a_players_log_is_theirs_alone_since_they_joined() {
        let opened = Instant::now();
        let mut dashboard = Dashboard::new(opened);
        let later = opened + Duration::from_secs(60);
        // Someone else had number 2 first.
        dashboard.apply(&Event::Joined(id(2), address(9002)), opened);
        dashboard.apply(&Event::Left(id(2)), opened);
        dashboard.apply(&Event::Joined(id(2), address(9003)), later);
        dashboard.apply(&Event::Joined(id(4), address(9004)), later);
        let hit = Event::Hit {
            attacker: id(4),
            target: id(2),
            kind: HitKind::Punch,
        };
        dashboard.apply(&hit, later);
        dashboard.note(Kind::Problem, "not about anyone".to_owned(), later);
        let since = dashboard
            .player(id(2))
            .map_or(Duration::ZERO, |row| row.joined_at);
        // The second joining of number 2 and the hit, by their places in the log.
        let places: Vec<usize> = dashboard
            .player_log(id(2), since)
            .map(|(place, _)| place)
            .collect();
        assert_eq!(places, [2, 4]);
    }

    #[test]
    fn the_world_owner_counts_only_while_present() {
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        dashboard.apply(&Event::Joined(id(0), address(9000)), now);
        dashboard.apply(&Event::WorldSet(id(0)), now);
        assert_eq!(dashboard.world_from(), Some(id(0)));
        dashboard.apply(&Event::Left(id(0)), now);
        assert_eq!(dashboard.world_from(), None);
    }

    #[test]
    fn the_log_keeps_the_newest_entries_with_their_time() {
        let opened = Instant::now();
        let mut dashboard = Dashboard::new(opened);
        for count in 0..=LOG_CAPACITY {
            dashboard.note(Kind::Session, count.to_string(), opened);
        }
        assert_eq!(dashboard.log().len(), LOG_CAPACITY);
        assert_eq!(dashboard.log().front().map(|e| e.text.as_str()), Some("1"));
        let later = opened + Duration::from_secs(75);
        dashboard.note(Kind::Problem, "late".to_owned(), later);
        assert_eq!(
            dashboard.log().back().map(|e| e.at),
            Some(Duration::from_secs(75))
        );
        assert_eq!(dashboard.kind_count(Kind::Session), LOG_CAPACITY - 1);
        assert_eq!(dashboard.kind_count(Kind::Problem), 1);
        let taken = dashboard.take_log();
        assert!(dashboard.log().is_empty());
        assert_eq!(dashboard.kind_count(Kind::Session), 0);
        // Undoing a clear puts the old entries back before anything since, newest kept.
        dashboard.note(Kind::Session, "after".to_owned(), later);
        dashboard.restore_log(taken);
        assert_eq!(dashboard.log().len(), LOG_CAPACITY);
        assert_eq!(
            dashboard.log().back().map(|e| e.text.as_str()),
            Some("after")
        );
        // The counts follow the restored log.
        let sessions = dashboard
            .log()
            .iter()
            .filter(|e| e.kind == Kind::Session)
            .count();
        assert_eq!(dashboard.kind_count(Kind::Session), sessions);
    }

    #[test]
    fn every_change_to_the_log_gives_it_another_generation() {
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        let mut seen = vec![dashboard.log_generation()];
        let mut changed = |dashboard: &Dashboard, what: &str| {
            let generation = dashboard.log_generation();
            assert!(!seen.contains(&generation), "{what} left it as it was");
            seen.push(generation);
        };
        dashboard.note(Kind::Session, "noted".to_owned(), now);
        changed(&dashboard, "a note");
        dashboard.apply(&Event::Joined(id(1), address(9001)), now);
        changed(&dashboard, "an event");
        let taken = dashboard.take_log();
        changed(&dashboard, "clearing");
        dashboard.restore_log(taken);
        changed(&dashboard, "restoring");
        // Looking at it changes nothing, nor does the session's end, which keeps the log.
        let before = dashboard.log_generation();
        let _ = dashboard.log().len();
        dashboard.server_stopped();
        assert_eq!(dashboard.log_generation(), before);
    }

    #[test]
    fn times_of_day_count_from_when_the_panel_opened() {
        let opened = Instant::now();
        let evening = Duration::from_mins(23 * 60 + 59);
        let mut dashboard = Dashboard::new(opened).with_time_of_day(evening);
        dashboard.note(
            Kind::Session,
            "past midnight".to_owned(),
            opened + Duration::from_secs(120),
        );
        let entry = dashboard.log().back().cloned();
        let time = entry.map(|entry| dashboard.time_of_day(&entry));
        assert_eq!(
            time,
            Some(Duration::from_secs(60)),
            "it wraps to the next day"
        );
    }

    fn link_losing(missing: u64) -> LinkReport {
        LinkReport {
            elapsed: Duration::from_secs(10),
            sent: Traffic::default(),
            received: Traffic::default(),
            losses: Losses {
                received: 100 - missing,
                missing,
                late: 0,
            },
            round_trips: Timings::default(),
            gaps: Timings::default(),
        }
    }

    #[test]
    fn players_sort_by_any_column_with_the_unmeasured_last() {
        let start = Instant::now();
        let mut dashboard = Dashboard::new(start);
        for (player, port) in [(0, 9003), (1, 9001), (2, 9002)] {
            let joined = start + Duration::from_secs(u64::from(player));
            dashboard.apply(&Event::Joined(id(player), address(port)), joined);
        }
        dashboard.apply(&Event::Link(id(0), Box::new(link_losing(1))), start);
        dashboard.apply(&Event::Link(id(2), Box::new(link_losing(9))), start);
        let order = |sort: Sort<PlayerColumn>| -> Vec<u8> {
            dashboard
                .sorted_players(sort)
                .iter()
                .map(|row| row.id.get())
                .collect()
        };
        assert_eq!(order(Sort::by(PlayerColumn::Player)), [0, 1, 2]);
        assert_eq!(order(Sort::by(PlayerColumn::Address)), [1, 2, 0]);
        // Worst first; player 1 has no report yet, so comes last.
        assert_eq!(order(Sort::by(PlayerColumn::Loss)), [2, 0, 1]);
        assert_eq!(
            order(Sort::by(PlayerColumn::Loss).select(PlayerColumn::Loss)),
            [1, 0, 2]
        );
        // The first to join has been connected longest.
        assert_eq!(order(Sort::by(PlayerColumn::Connected)), [0, 1, 2]);
    }

    #[test]
    fn the_log_sorts_by_time_kind_or_text() {
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        dashboard.note(Kind::Problem, "b".to_owned(), now);
        dashboard.note(Kind::Session, "C".to_owned(), now);
        dashboard.note(Kind::Session, "a".to_owned(), now);
        let order = |sort: Sort<LogColumn>| {
            let mut entries = vec![0, 1, 2];
            dashboard.sort_log(&mut entries, sort);
            entries
        };
        assert_eq!(order(Sort::by(LogColumn::Time)), [0, 1, 2]);
        assert_eq!(
            order(Sort::by(LogColumn::Time).select(LogColumn::Time)),
            [2, 1, 0]
        );
        // Kinds in their order, ties as they happened.
        assert_eq!(order(Sort::by(LogColumn::Kind)), [1, 2, 0]);
        // Text without regard to case.
        assert_eq!(order(Sort::by(LogColumn::Entry)), [2, 0, 1]);
    }
}
