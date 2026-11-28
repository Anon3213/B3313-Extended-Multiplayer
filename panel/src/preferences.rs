//! The panel's own preferences, remembered between runs: how it looks, how it alerts the user,
//! and what it opens with. These belong to whoever uses the panel, not to a server, so they live
//! apart from the server's settings.

use crate::columns::Widths;
use crate::dashboard::{Kind, LogColumn, PlayerColumn};
use crate::format::RateUnit;
use crate::overview_layout::OverviewLayout;
use crate::page_layout::{ArrangedPage, PageLayouts};
use crate::sort::{Sort, SortOrder};
use crate::system;
use crate::theme::{Mode, ThemeName};
use crate::zoom::Zoom;
use b3313_bridge::text::ChatSize;
use b3313_bridge::typing::{
    DEFAULT_CHAT_KEY, DEFAULT_CHAT_LINES, DEFAULT_CHAT_SECONDS, DEFAULT_MAP_KEY, chat_key,
    chat_lines, chat_seconds,
};
use b3313_protocol::Outfit;
use b3313_protocol::secure::{PublicKey, join_code, split_join_code};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Settings files listed as recent.
pub const RECENT_FILES: usize = 5;

/// Where the window was and how large, in logical pixels, to open there again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowPlacement {
    pub maximized: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WindowPlacement {
    fn parse(text: &str) -> Option<Self> {
        let mut words = text.split_whitespace();
        let maximized = match words.next()? {
            "maximized" => true,
            "windowed" => false,
            _ => return None,
        };
        let mut number = || words.next()?.parse::<f32>().ok().filter(|n| n.is_finite());
        let (x, y, width, height) = (number()?, number()?, number()?, number()?);
        (width > 0.0 && height > 0.0).then_some(Self {
            maximized,
            x,
            y,
            width,
            height,
        })
    }

    fn format(&self) -> String {
        let state = if self.maximized {
            "maximized"
        } else {
            "windowed"
        };
        format!(
            "{state} {:.0} {:.0} {:.0} {:.0}",
            self.x, self.y, self.width, self.height
        )
    }
}

/// How long tooltips stay, in seconds, unless the user chooses otherwise.
pub const TOOLTIP_SECONDS_DEFAULT: u8 = 5;
/// The longest the user can choose; past this a tooltip would just be in the way.
pub const TOOLTIP_SECONDS_MAX: u8 = 10;

/// How long a message stays, in seconds, unless the user chooses otherwise.
pub const MESSAGE_SECONDS_DEFAULT: u8 = 6;
pub const MESSAGE_SECONDS_MIN: u8 = 3;
pub const MESSAGE_SECONDS_MAX: u8 = 20;

/// The expanded sidebar's width in pixels at 100% interface size, unless changed.
pub const SIDEBAR_WIDTH_DEFAULT: u16 = 232;
/// The narrowest it goes, still fitting every page's name and count.
pub const SIDEBAR_WIDTH_MIN: u16 = 180;
/// The widest, leaving the page most of the window.
pub const SIDEBAR_WIDTH_MAX: u16 = 360;

/// The collapsed sidebar's width at 100%: an icon's row and the sidebar's padding.
pub const SIDEBAR_COLLAPSED_WIDTH: u16 = 72;
/// Where dragging the sidebar's edge snaps between icons only and in full: two-thirds of the
/// way from the narrowest full width to the collapsed one, so a drag only a little past the
/// minimum does not collapse it by accident.
pub const SIDEBAR_SNAP_WIDTH: u16 =
    SIDEBAR_WIDTH_MIN - (SIDEBAR_WIDTH_MIN - SIDEBAR_COLLAPSED_WIDTH) * 2 / 3;

/// What a drag of the sidebar's edge to `pixels` from the window's left, at 100%, makes of it:
/// collapsed (None) past the snap point, otherwise in full at that width, within its range.
#[must_use]
pub fn dragged_sidebar(pixels: f32) -> Option<u16> {
    (pixels >= f32::from(SIDEBAR_SNAP_WIDTH)).then(|| sidebar_width(pixels))
}

/// A sidebar width, in pixels at 100%, kept within the range it may have.
#[must_use]
pub fn sidebar_width(pixels: f32) -> u16 {
    // `max` and `min` also turn NaN into the minimum.
    let width = pixels
        .round()
        .max(f32::from(SIDEBAR_WIDTH_MIN))
        .min(f32::from(SIDEBAR_WIDTH_MAX));
    #[expect(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "within SIDEBAR_WIDTH_MIN..=SIDEBAR_WIDTH_MAX, so it fits a u16 exactly"
    )]
    let width = width as u16;
    width
}

#[derive(Clone, Debug, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent on/off preferences, each its own switch, not states of one thing"
)]
pub struct Preferences {
    pub zoom: Zoom,
    pub theme: ThemeName,
    /// Light, dark, or as Windows is set.
    pub mode: Mode,
    /// The 24-hour clock rather than the 12-hour one; unset, as the user's region writes times.
    pub clock_24h: Option<bool>,
    /// Where the window was when the panel last closed.
    pub window: Option<WindowPlacement>,
    /// How long hover text stays, in seconds; zero for none.
    pub tooltip_seconds: u8,
    /// The expanded sidebar's width, in pixels at 100% interface size.
    pub sidebar_width: u16,
    /// The sidebar shows only icons.
    pub sidebar_collapsed: bool,
    /// What the Overview shows, and how.
    pub overview: OverviewLayout,
    /// The order of each page's panels.
    pub pages: PageLayouts,
    /// Flash the taskbar while the panel is in the background, for entries of `alert_kinds`.
    pub alerts: bool,
    /// Play a short sound for the same things, heard while the game is in front.
    pub alert_sound: bool,
    pub alert_kinds: Vec<Kind>,
    /// How long a message stays at the corner, in seconds; one with Undo stays longer.
    pub message_seconds: u8,
    /// Ask before stopping or restarting the server while players are connected.
    pub confirm_stop: bool,
    /// Keep the tables' sorting and the Log's filters for next time.
    pub remember_views: bool,
    pub player_sort: Sort<PlayerColumn>,
    pub log_sort: Sort<LogColumn>,
    pub ban_order: SortOrder,
    pub log_kinds: Vec<Kind>,
    /// The widths the user has given the tables' columns.
    pub player_widths: Widths<PlayerColumn>,
    pub log_widths: Widths<LogColumn>,
    /// Traffic in bytes or bits.
    pub rate_unit: RateUnit,
    /// Tighter rows in lists.
    pub compact: bool,
    /// The page shown when the panel opens.
    pub start_page: StartPage,
    /// The multiplayer ROM the Play page joins with.
    pub rom: Option<PathBuf>,
    /// The player's Super Mario 64 ROM and the B3313 patch, which the multiplayer ROM is built
    /// from: kept so that a newer panel can build its own ROM again without asking.
    pub sm64_rom: Option<PathBuf>,
    pub b3313_patch: Option<PathBuf>,
    /// The name other players see, as typed on the Play page; empty leaves it to the server.
    pub player_name: String,
    /// The colors the player's Mario wears, as chosen on the Play page or with `/color`; none
    /// leaves it to the server, which gives one by the player's number.
    pub player_outfit: Option<Outfit>,
    /// The key that opens the chat in the game, where the player then types a line; none
    /// leaves the chat to the panel.
    pub chat_key: Option<char>,
    /// How many seconds the game shows the chat's lines after the newest arrived.
    pub chat_seconds: u8,
    /// How many of the chat's rows the game shows at once as lines arrive.
    pub chat_lines: u8,
    /// Whether the mouse resting on the game's chat shows it.
    pub chat_on_hover: bool,
    /// How large the game draws the chat.
    pub chat_size: ChatSize,
    /// Whether the game shows the minimap of the walls and players around the player's Mario
    /// as a session begins.
    pub minimap: bool,
    /// The key that shows or hides the minimap in the game; none leaves that to `/map`.
    pub map_key: Option<char>,
    /// The port a bot on this PC may play the player's game through, while they are in a
    /// session; none for no bot.
    pub bot_port: Option<u16>,
    /// Servers joined, most recent first.
    pub servers: Vec<SavedServer>,
    /// Open the most recent settings file when the panel opens without one named.
    pub reopen_settings: bool,
    /// Start the server as soon as the panel opens.
    pub start_server: bool,
    /// Hide IP addresses, server names and the user's folder, for streaming or sharing the
    /// screen.
    pub streamer_mode: bool,
    /// Ask the router to open the server's port when the server starts on the network.
    pub open_port: bool,
    /// The address the server last started on.
    pub listen_address: Option<String>,
    /// The name invites give for the server in place of its address, if its owner has one
    /// that follows their address as it changes (dynamic DNS).
    pub invite_name: Option<String>,
    /// Settings files opened or saved, most recent first.
    pub recent_files: Vec<PathBuf>,
    /// The newest crash report the user has been told of, by file name without extension.
    pub seen_crash: Option<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            zoom: Zoom::default(),
            theme: ThemeName::default(),
            mode: Mode::default(),
            clock_24h: None,
            window: None,
            tooltip_seconds: TOOLTIP_SECONDS_DEFAULT,
            sidebar_width: SIDEBAR_WIDTH_DEFAULT,
            sidebar_collapsed: false,
            overview: OverviewLayout::default(),
            pages: PageLayouts::default(),
            alerts: true,
            alert_sound: false,
            // Possible cheating, and anything that went wrong, the server stopping on its own
            // among them.
            alert_kinds: vec![Kind::Flag, Kind::Problem],
            message_seconds: MESSAGE_SECONDS_DEFAULT,
            confirm_stop: true,
            remember_views: true,
            player_sort: Sort::by(PlayerColumn::Player),
            log_sort: Sort::by(LogColumn::Time),
            ban_order: SortOrder::Ascending,
            log_kinds: Kind::shown_by_default(),
            player_widths: Widths::default(),
            log_widths: Widths::default(),
            rate_unit: RateUnit::default(),
            compact: false,
            start_page: StartPage::default(),
            rom: None,
            sm64_rom: None,
            b3313_patch: None,
            player_name: String::new(),
            player_outfit: None,
            chat_key: Some(DEFAULT_CHAT_KEY),
            chat_seconds: DEFAULT_CHAT_SECONDS,
            chat_lines: DEFAULT_CHAT_LINES,
            chat_on_hover: false,
            chat_size: ChatSize::Normal,
            minimap: true,
            map_key: Some(DEFAULT_MAP_KEY),
            bot_port: None,
            servers: Vec::new(),
            reopen_settings: true,
            start_server: false,
            streamer_mode: false,
            open_port: true,
            listen_address: None,
            invite_name: None,
            recent_files: Vec::new(),
            seen_crash: None,
        }
    }
}

/// Servers kept as recent on the Play page: three rows of chips or so, and more than the
/// servers most players go back to. A server dropped from the end is forgotten with its key,
/// and is then joined as a new one would be, so the list is not kept short.
pub const RECENT_SERVERS: usize = 10;

/// Where the server kept or typed as `server` is, and its key if that is written after it,
/// as in an invite. What follows a `#` and is no key is left as part of the place, which
/// then fails to be looked up, and says so to the player.
#[must_use]
pub fn server_parts(server: &str) -> (&str, Option<PublicKey>) {
    split_join_code(server).unwrap_or((server, None))
}

/// A server the player joined, and its passwords if they asked for them to be remembered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedServer {
    /// As it is joined: the address, and after a `#` the server's key once it is known, from
    /// an invite or from having joined it.
    pub address: String,
    /// Kept in the preferences file as plain text, so only when the player chooses.
    pub password: Option<String>,
    /// The server's host password, if its owner gave the player one to join as the host
    /// with. Kept as the password is, and only when that is.
    pub host_password: Option<String>,
}

impl SavedServer {
    /// Whether either of its passwords is kept with it.
    #[must_use]
    pub const fn keeps_a_password(&self) -> bool {
        self.password.is_some() || self.host_password.is_some()
    }
}

/// The passwords a server is joined with, as the player typed them; an empty one is none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Passwords<'a> {
    /// What lets a player into the session.
    pub session: &'a str,
    /// What makes the player who joins with it the host.
    pub host: &'a str,
}

/// The page shown when the panel opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StartPage {
    /// Where a new player begins: most people who open the panel come to join a session.
    #[default]
    Play,
    Overview,
    Players,
    Bans,
    Settings,
    Log,
    Bots,
    Preferences,
}

impl StartPage {
    pub const ALL: [Self; 8] = [
        Self::Play,
        Self::Overview,
        Self::Players,
        Self::Bans,
        Self::Settings,
        Self::Log,
        Self::Bots,
        Self::Preferences,
    ];

    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Play => "play",
            Self::Overview => "overview",
            Self::Players => "players",
            Self::Bans => "bans",
            Self::Settings => "settings",
            Self::Log => "log",
            Self::Bots => "bots",
            Self::Preferences => "preferences",
        }
    }

    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|page| page.key() == key)
    }
}

fn parse_flag(value: &str) -> Option<bool> {
    match value {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

/// Sets `flag` from `value`, keeping it if `value` is neither true nor false.
fn set_flag(flag: &mut bool, value: &str) {
    if let Some(on) = parse_flag(value) {
        *flag = on;
    }
}

impl Preferences {
    /// Reads saved preferences. A missing or unreadable value, such as one from a later
    /// version, keeps its default: a wrong preference is no reason to stop the panel opening.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut preferences = Self::default();
        for (key, value) in text
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(key, value)| (key.trim(), value.trim()))
        {
            if !preferences.read_look(key, value) {
                preferences.read_behavior(key, value);
            }
        }
        preferences
    }

    /// Reads one of the preferences for how the panel looks, saying whether `key` is one.
    fn read_look(&mut self, key: &str, value: &str) -> bool {
        match key {
            "zoom_percent" => {
                if let Some(zoom) = value.parse().ok().and_then(Zoom::from_percent) {
                    self.zoom = zoom;
                }
            }
            "theme" => {
                if let Some(theme) = ThemeName::from_key(value) {
                    self.theme = theme;
                }
            }
            "mode" => {
                if let Some(mode) = Mode::from_key(value) {
                    self.mode = mode;
                }
            }
            "clock" => {
                self.clock_24h = match value {
                    "24h" => Some(true),
                    "12h" => Some(false),
                    _ => None,
                };
            }
            "window" => self.window = WindowPlacement::parse(value),
            "tooltip_seconds" => {
                if let Some(seconds) = value.parse().ok().filter(|s| *s <= TOOLTIP_SECONDS_MAX) {
                    self.tooltip_seconds = seconds;
                }
            }
            // From before tooltips had a duration: off, or the default.
            "tooltips" => {
                if let Some(on) = parse_flag(value) {
                    self.tooltip_seconds = if on { TOOLTIP_SECONDS_DEFAULT } else { 0 };
                }
            }
            "sidebar_width" => {
                if let Some(width) = value
                    .parse::<u16>()
                    .ok()
                    .filter(|width| (SIDEBAR_WIDTH_MIN..=SIDEBAR_WIDTH_MAX).contains(width))
                {
                    self.sidebar_width = width;
                }
            }
            "sidebar_collapsed" => set_flag(&mut self.sidebar_collapsed, value),
            "overview" => self.overview = OverviewLayout::parse(value),
            "rate_unit" => {
                if let Some(unit) = RateUnit::from_key(value) {
                    self.rate_unit = unit;
                }
            }
            "density" => match value {
                "compact" => self.compact = true,
                "comfortable" => self.compact = false,
                _ => {}
            },
            _ => return self.pages.read(key, value),
        }
        true
    }

    /// Reads one of the preferences for how the panel behaves and what it remembers.
    fn read_behavior(&mut self, key: &str, value: &str) {
        match key {
            "alerts" => set_flag(&mut self.alerts, value),
            "alert_sound" => set_flag(&mut self.alert_sound, value),
            // Empty is a choice too: alert for nothing.
            "alert_kinds" => self.alert_kinds = Kind::parse_list(value),
            "message_seconds" => {
                if let Some(seconds) = value
                    .parse()
                    .ok()
                    .filter(|s| (MESSAGE_SECONDS_MIN..=MESSAGE_SECONDS_MAX).contains(s))
                {
                    self.message_seconds = seconds;
                }
            }
            "confirm_stop" => set_flag(&mut self.confirm_stop, value),
            "remember_views" => set_flag(&mut self.remember_views, value),
            "player_sort" => {
                if let Some(sort) = Sort::from_key(value) {
                    self.player_sort = sort;
                }
            }
            "log_sort" => {
                if let Some(sort) = Sort::from_key(value) {
                    self.log_sort = sort;
                }
            }
            "ban_order" => {
                if let Some(order) = SortOrder::from_key(value) {
                    self.ban_order = order;
                }
            }
            "log_kinds" => self.log_kinds = Kind::parse_list(value),
            "player_columns" => self.player_widths = Widths::parse(value),
            "log_columns" => self.log_widths = Widths::parse(value),
            "start_page" => {
                if let Some(page) = StartPage::from_key(value) {
                    self.start_page = page;
                }
            }
            "rom" if !value.is_empty() => self.rom = Some(PathBuf::from(value)),
            "sm64_rom" if !value.is_empty() => self.sm64_rom = Some(PathBuf::from(value)),
            "b3313_patch" if !value.is_empty() => self.b3313_patch = Some(PathBuf::from(value)),
            "player_name" => value.clone_into(&mut self.player_name),
            "player_color" => self.player_outfit = Outfit::named(value),
            // A letter or digit; anything else, as the "off" this writes, is no key.
            "chat_key" => self.chat_key = chat_key(value),
            "chat_seconds" => {
                if let Some(seconds) = chat_seconds(value) {
                    self.chat_seconds = seconds;
                }
            }
            "chat_lines" => {
                if let Some(lines) = chat_lines(value) {
                    self.chat_lines = lines;
                }
            }
            "chat_on_hover" => set_flag(&mut self.chat_on_hover, value),
            "chat_size" => self.chat_size = ChatSize::named(value).unwrap_or_default(),
            "minimap" => set_flag(&mut self.minimap, value),
            "map_key" => self.map_key = chat_key(value),
            // A port; anything else, as the "off" this writes, is none.
            "bot_port" => self.bot_port = value.parse().ok().filter(|&port| port != 0),
            "server" if !value.is_empty() && self.servers.len() < RECENT_SERVERS => {
                self.servers.push(SavedServer {
                    address: value.to_owned(),
                    password: None,
                    host_password: None,
                });
            }
            // The passwords of the server on the line before them.
            "server_password" if !value.is_empty() => {
                if let Some(server) = self.servers.last_mut() {
                    server.password = Some(value.to_owned());
                }
            }
            "server_host_password" if !value.is_empty() => {
                if let Some(server) = self.servers.last_mut() {
                    server.host_password = Some(value.to_owned());
                }
            }
            "reopen_settings" => set_flag(&mut self.reopen_settings, value),
            "start_server" => set_flag(&mut self.start_server, value),
            "streamer_mode" => set_flag(&mut self.streamer_mode, value),
            "open_port" => set_flag(&mut self.open_port, value),
            "listen_address" if !value.is_empty() => {
                self.listen_address = Some(value.to_owned());
            }
            "invite_name" if !value.is_empty() => self.invite_name = Some(value.to_owned()),
            "seen_crash" if !value.is_empty() => self.seen_crash = Some(value.to_owned()),
            "recent_file" if !value.is_empty() && self.recent_files.len() < RECENT_FILES => {
                self.recent_files.push(PathBuf::from(value));
            }
            _ => {}
        }
    }

    /// The lines for the player's own game: its chat and its minimap.
    fn format_in_game(&self, text: &mut String) {
        // The keys are written either way: without the line, a key is the usual one.
        let key = |key: Option<char>| key.map_or("off".to_owned(), String::from);
        let _ = writeln!(text, "chat_key = {}", key(self.chat_key));
        let _ = writeln!(text, "chat_seconds = {}", self.chat_seconds);
        let _ = writeln!(text, "chat_lines = {}", self.chat_lines);
        let _ = writeln!(text, "chat_on_hover = {}", self.chat_on_hover);
        let _ = writeln!(text, "chat_size = {}", self.chat_size.name());
        let _ = writeln!(text, "minimap = {}", self.minimap);
        let _ = writeln!(text, "map_key = {}", key(self.map_key));
    }

    /// What `parse` reads back.
    #[must_use]
    pub fn format(&self) -> String {
        let mut text = format!(
            "# The control panel's own preferences.\n\
             zoom_percent = {}\ntheme = {}\nmode = {}\ntooltip_seconds = {}\n\
             sidebar_width = {}\nsidebar_collapsed = {}\nalerts = {}\n\
             reopen_settings = {}\nstart_server = {}\nstreamer_mode = {}\nopen_port = {}\n",
            self.zoom.percent(),
            self.theme.key(),
            self.mode.key(),
            self.tooltip_seconds,
            self.sidebar_width,
            self.sidebar_collapsed,
            self.alerts,
            self.reopen_settings,
            self.start_server,
            self.streamer_mode,
            self.open_port,
        );
        // Writing to a String cannot fail.
        let _ = write!(
            text,
            "alert_kinds = {}\nmessage_seconds = {}\nconfirm_stop = {}\nremember_views = {}\n\
             player_sort = {}\nlog_sort = {}\nban_order = {}\nlog_kinds = {}\n\
             rate_unit = {}\ndensity = {}\nstart_page = {}\n",
            Kind::format_list(&self.alert_kinds),
            self.message_seconds,
            self.confirm_stop,
            self.remember_views,
            self.player_sort.key(),
            self.log_sort.key(),
            self.ban_order.key(),
            Kind::format_list(&self.log_kinds),
            self.rate_unit.key(),
            if self.compact {
                "compact"
            } else {
                "comfortable"
            },
            self.start_page.key(),
        );
        let _ = writeln!(text, "player_columns = {}", self.player_widths.format());
        let _ = writeln!(text, "log_columns = {}", self.log_widths.format());
        let _ = writeln!(text, "overview = {}", self.overview.format());
        for page in ArrangedPage::ALL {
            let _ = writeln!(text, "{} = {}", page.key(), self.pages.of(page).format());
        }
        if let Some(clock_24h) = self.clock_24h {
            let _ = writeln!(text, "clock = {}", if clock_24h { "24h" } else { "12h" });
        }
        if let Some(window) = &self.window {
            let _ = writeln!(text, "window = {}", window.format());
        }
        if let Some(address) = &self.listen_address {
            let _ = writeln!(text, "listen_address = {address}");
        }
        if let Some(name) = &self.invite_name {
            let _ = writeln!(text, "invite_name = {name}");
        }
        if let Some(seen) = &self.seen_crash {
            let _ = writeln!(text, "seen_crash = {seen}");
        }
        for path in &self.recent_files {
            let _ = writeln!(text, "recent_file = {}", path.display());
        }
        let files = [
            ("rom", &self.rom),
            ("sm64_rom", &self.sm64_rom),
            ("b3313_patch", &self.b3313_patch),
        ];
        for (key, path) in files {
            if let Some(path) = path {
                let _ = writeln!(text, "{key} = {}", path.display());
            }
        }
        if !self.player_name.is_empty() {
            let _ = writeln!(text, "player_name = {}", self.player_name);
        }
        if let Some(outfit) = self.player_outfit {
            let _ = writeln!(text, "player_color = {}", outfit.name());
        }
        self.format_in_game(&mut text);
        let bot_port = self
            .bot_port
            .map_or("off".to_owned(), |port| port.to_string());
        let _ = writeln!(text, "bot_port = {bot_port}");
        let _ = writeln!(text, "alert_sound = {}", self.alert_sound);
        for server in &self.servers {
            let _ = writeln!(text, "server = {}", server.address);
            if let Some(password) = &server.password {
                let _ = writeln!(text, "server_password = {password}");
            }
            if let Some(password) = &server.host_password {
                let _ = writeln!(text, "server_host_password = {password}");
            }
        }
        text
    }

    /// Puts `address` first among the recent servers, with its passwords if the player wants
    /// them remembered, or forgetting any it had. It takes the place of what was kept of the
    /// same server: under the same address, or under another with the same key, as a server
    /// whose address has changed is still that server.
    pub fn remember_server(&mut self, address: &str, passwords: Option<Passwords<'_>>) {
        let kept = |password: &str| (!password.is_empty()).then(|| password.to_owned());
        let (place, key) = server_parts(address);
        self.servers.retain(|server| {
            let (kept_place, kept_key) = server_parts(&server.address);
            kept_place != place && (key.is_none() || kept_key != key)
        });
        self.servers.insert(
            0,
            SavedServer {
                address: address.to_owned(),
                password: passwords.and_then(|passwords| kept(passwords.session)),
                host_password: passwords.and_then(|passwords| kept(passwords.host)),
            },
        );
        self.servers.truncate(RECENT_SERVERS);
    }

    pub fn forget_server(&mut self, address: &str) {
        self.servers.retain(|server| server.address != address);
    }

    /// What is kept of the server the player typed as `address`, if that is more than they
    /// typed: the same address with the key the server had when it was joined, which it
    /// must still have. An address typed with a key of its own is taken as it is.
    #[must_use]
    pub fn server_with_its_key(&self, address: &str) -> Option<&str> {
        let (place, None) = server_parts(address) else {
            return None;
        };
        self.servers
            .iter()
            .map(|server| server.address.as_str())
            .find(|kept| matches!(server_parts(kept), (kept_place, Some(_)) if kept_place == place))
    }

    /// Keeps `key`, which the server kept as `address` had when it was joined just now, as
    /// that server's key: it is then written with it, as an invite gives it, and only that
    /// server is joined at the address from now on. Returns what it is kept as now, if that
    /// changed.
    ///
    /// Only for a server that was joined: the bridge refuses one whose key is not the one
    /// kept, so a key that differs here is the panel's own server's, set up anew, which the
    /// panel knows without being told.
    pub fn learn_server_key(&mut self, address: &str, key: &PublicKey) -> Option<String> {
        let (place, kept) = server_parts(address);
        if kept.as_ref() == Some(key) {
            return None;
        }
        let server = self
            .servers
            .iter_mut()
            .find(|kept| kept.address == address)?;
        server.address = join_code(place, key);
        Some(server.address.clone())
    }

    /// Whether times of day use the 24-hour clock: as chosen, or as the user's region does.
    #[must_use]
    pub fn uses_24_hour(&self) -> bool {
        self.clock_24h.unwrap_or_else(system::prefers_24_hour)
    }

    /// Puts `path` first among the recent files.
    pub fn remember_file(&mut self, path: &Path) {
        self.recent_files.retain(|recent| recent != path);
        self.recent_files.insert(0, path.to_owned());
        self.recent_files.truncate(RECENT_FILES);
    }

    /// Everything on the Preferences page back to its default. What the panel keeps for itself
    /// stays: where its window was, the recent files, the last listen address, the Play page's
    /// ROM and servers, and the Overview's layout, which its own Reset restores.
    #[must_use]
    pub fn reset(&self) -> Self {
        Self {
            window: self.window,
            recent_files: self.recent_files.clone(),
            rom: self.rom.clone(),
            sm64_rom: self.sm64_rom.clone(),
            b3313_patch: self.b3313_patch.clone(),
            player_name: self.player_name.clone(),
            player_outfit: self.player_outfit,
            servers: self.servers.clone(),
            listen_address: self.listen_address.clone(),
            invite_name: self.invite_name.clone(),
            overview: self.overview.clone(),
            pages: self.pages.clone(),
            // Reports already told of are not news again.
            seen_crash: self.seen_crash.clone(),
            // Never turned off as a side effect: what it hides would show at once, to
            // whoever is watching, the listen address on this very page among it.
            streamer_mode: self.streamer_mode,
            ..Self::default()
        }
    }

    /// How long a message stays at the corner; one with Undo stays longer, for time to use it.
    #[must_use]
    pub fn message_duration(&self, undoable: bool) -> std::time::Duration {
        let extra = if undoable { 6 } else { 0 };
        std::time::Duration::from_secs(u64::from(self.message_seconds) + extra)
    }

    /// The settings file to open at startup, if the user wants the last one back.
    #[must_use]
    pub fn settings_to_reopen(&self) -> Option<&Path> {
        self.reopen_settings
            .then(|| self.recent_files.first())
            .flatten()
            .map(PathBuf::as_path)
    }

    /// Reads the saved preferences, or the defaults if there are none.
    ///
    /// # Errors
    /// Fails, with a message for the user, if there is a file and it cannot be read, as
    /// while another program holds it. The panel must then not save its preferences, or the
    /// defaults would replace whatever the file has.
    pub fn load(path: &Path) -> Result<Self, String> {
        // Read as bytes: a file edited by hand and saved in another encoding still has its
        // settings, where reading it as text would refuse the whole file.
        match std::fs::read(path) {
            Ok(bytes) => Ok(Self::parse(&String::from_utf8_lossy(&bytes))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!(
                "could not read the panel's preferences, so they are the usual ones for now \
                 and changes to them are not kept: {error}"
            )),
        }
    }

    /// Saves the preferences, creating the file's folder if needed.
    ///
    /// # Errors
    /// Fails, with a message for the user, if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| replace_file(path, &self.format()));
        written.map_err(|error| format!("could not save the panel's preferences: {error}"))
    }
}

/// Writes `text` as the whole of the file at `path`, which keeps what it had until all of
/// `text` is on the disk: a crash or a power cut half way leaves the old file, not half a new
/// one. It is written beside the file under a name of the panel's own, then moved into place.
///
/// # Errors
/// Fails if the file cannot be written or put in place.
pub fn replace_file(path: &Path, text: &str) -> std::io::Result<()> {
    replace_file_bytes(path, text.as_bytes())
}

/// As `replace_file`, for a file that is not text.
///
/// # Errors
/// Fails if the file cannot be written or put in place.
pub fn replace_file_bytes(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let mut beside = path.as_os_str().to_owned();
    beside.push(".b3313-saving");
    let beside = PathBuf::from(beside);
    let written = std::fs::File::create(&beside).and_then(|mut file| {
        file.write_all(contents)?;
        file.sync_all()
    });
    written
        .and_then(|()| std::fs::rename(&beside, path))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&beside);
        })
}

/// Where the panel keeps its preferences: the user's roaming application data on Windows.
/// None where there is no such folder; they are then forgotten when the panel closes.
#[must_use]
pub fn path() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(base)
            .join("B3313 Extended Multiplayer")
            .join("panel.conf"),
    )
}

#[cfg(test)]
mod tests {
    use super::StartPage;
    use super::{
        Passwords, Preferences, RECENT_FILES, SIDEBAR_SNAP_WIDTH, SIDEBAR_WIDTH_DEFAULT,
        SIDEBAR_WIDTH_MAX, SIDEBAR_WIDTH_MIN, TOOLTIP_SECONDS_DEFAULT, WindowPlacement,
        dragged_sidebar, sidebar_width,
    };
    use crate::columns::Widths;
    use crate::dashboard::{Kind, LogColumn, PlayerColumn};
    use crate::format::RateUnit;
    use crate::overview_layout::{OverviewLayout, PanelKind};
    use crate::page_layout::{ArrangedPage, PageLayouts};
    use crate::sort::{Sort, SortOrder};
    use crate::theme::{Mode, ThemeName};
    use crate::zoom::Zoom;
    use b3313_bridge::text::ChatSize;
    use b3313_protocol::Outfit;
    use b3313_protocol::secure::{SecureError, ServerKey, join_code};
    use std::path::{Path, PathBuf};

    fn passwords<'a>(session: &'a str, host: &'a str) -> Passwords<'a> {
        Passwords { session, host }
    }

    #[test]
    fn large_chat_and_a_bots_port_read_back_and_are_off_unless_named() {
        let mut preferences = Preferences {
            chat_size: ChatSize::Large,
            bot_port: Some(40000),
            ..Preferences::default()
        };
        assert_eq!(Preferences::parse(&preferences.format()), preferences);
        preferences.bot_port = None;
        assert_eq!(Preferences::parse(&preferences.format()).bot_port, None);
        assert_eq!(Preferences::parse("").bot_port, None);
        assert_eq!(Preferences::parse("bot_port = 0\n").bot_port, None);
        assert_eq!(
            Preferences::parse("chat_size = huge\n").chat_size,
            ChatSize::Normal
        );
    }

    #[test]
    fn fewer_chat_lines_and_the_chat_on_hover_read_back_and_hover_is_off_unless_named() {
        let preferences = Preferences {
            chat_lines: 2,
            chat_on_hover: true,
            ..Preferences::default()
        };
        assert_eq!(Preferences::parse(&preferences.format()), preferences);
        let usual = Preferences::parse("");
        assert_eq!((usual.chat_lines, usual.chat_on_hover), (6, false));
        assert_eq!(Preferences::parse("chat_lines = 9\n").chat_lines, 6);
    }

    #[test]
    fn the_minimap_is_shown_unless_turned_off_and_its_key_is_m_unless_named() {
        let usual = Preferences::parse("");
        assert_eq!((usual.minimap, usual.map_key), (true, Some('m')));
        let off = Preferences {
            minimap: false,
            map_key: None,
            ..Preferences::default()
        };
        assert_eq!(Preferences::parse(&off.format()), off);
        assert_eq!(Preferences::parse("map_key = N\n").map_key, Some('n'));
    }

    #[test]
    fn preferences_read_back_and_anything_odd_keeps_its_default() {
        let mut preferences = Preferences {
            zoom: Zoom::default().larger().larger(),
            theme: ThemeName::Sepia,
            mode: Mode::Light,
            clock_24h: Some(true),
            window: Some(WindowPlacement {
                maximized: true,
                x: -1200.,
                y: 40.,
                width: 1400.,
                height: 900.,
            }),
            tooltip_seconds: 8,
            sidebar_width: 300,
            sidebar_collapsed: true,
            overview: {
                let mut overview = OverviewLayout::default();
                overview.add(PanelKind::Log);
                overview.move_to(PanelKind::Log, 1);
                overview
            },
            pages: {
                let mut pages = PageLayouts::default();
                pages
                    .of_mut(ArrangedPage::Players)
                    .nudge("chat", true, &["chat", "table"]);
                pages.of_mut(ArrangedPage::Preferences).nudge(
                    "reset",
                    false,
                    &["shortcuts", "reset"],
                );
                pages
            },
            alerts: false,
            alert_sound: true,
            reopen_settings: false,
            start_server: true,
            streamer_mode: true,
            open_port: false,
            listen_address: Some("0.0.0.0:41313".to_owned()),
            invite_name: Some("home.example.org".to_owned()),
            recent_files: Vec::new(),
            seen_crash: Some("crash-2026-11-09-140322".to_owned()),
            alert_kinds: Vec::new(),
            message_seconds: 15,
            confirm_stop: false,
            remember_views: false,
            player_sort: Sort::by(PlayerColumn::Loss).select(PlayerColumn::Loss),
            log_sort: Sort::by(LogColumn::Kind),
            ban_order: SortOrder::Descending,
            log_kinds: vec![Kind::Flag],
            rate_unit: RateUnit::Bits,
            compact: true,
            start_page: StartPage::Log,
            rom: Some(PathBuf::from(r"C:\games\b3313 mp.z64")),
            sm64_rom: Some(PathBuf::from(r"C:\games\Super Mario 64 (USA).z64")),
            b3313_patch: Some(PathBuf::from(r"C:\games\b3313 a2ae.bps")),
            player_name: "Luigi the 2nd".to_owned(),
            player_outfit: Outfit::named("teal"),
            chat_key: Some('y'),
            chat_seconds: 30,
            servers: Vec::new(),
            // The columns' widths, the chat's size and the bot's port: their own tests.
            ..Preferences::default()
        };
        preferences.remember_server("example.net:41313", Some(passwords("a=b c", "")));
        // With a host password too, and with one alone, for a server anyone may join.
        preferences.remember_server("203.0.113.9:41313", Some(passwords("in", "a = crown")));
        preferences.remember_server("open.example:41313", Some(passwords("", "crown")));
        preferences.remember_server("192.168.1.20:41313", None);
        preferences.remember_file(Path::new(r"C:\servers\a b.conf"));
        preferences.remember_file(Path::new(r"C:\servers\other.conf"));
        assert_eq!(Preferences::parse(&preferences.format()), preferences);
        // Turned off, the chat's key stays off, where a file without it has the usual one.
        preferences.chat_key = None;
        assert_eq!(Preferences::parse(&preferences.format()).chat_key, None);
        assert_eq!(Preferences::parse("").chat_key, Some('t'));
        // A time for the chat that cannot be chosen leaves the usual one.
        assert_eq!(Preferences::parse("chat_seconds = 1\n").chat_seconds, 8);
        let odd = Preferences::parse(
            "zoom_percent = 133\ntheme = plaid\nalerts = maybe\nmode = dim\nclock = 13h\n\
             window = windowed 1 2 -3 4\ntooltip_seconds = 99\nsidebar_width = 20\n\
             sidebar_collapsed = yes\nmessage_seconds = 2\nplayer_sort = loss\n\
             rate_unit = nibbles\ndensity = cozy\nstart_page = nowhere\nban_order = up\n",
        );
        assert_eq!(odd, Preferences::default());
        assert_eq!(Preferences::parse(""), Preferences::default());
        // A file from before these preferences existed keeps its size.
        let older = Preferences::parse("zoom_percent = 150\n");
        assert_eq!(older.zoom.percent(), 150);
        assert_eq!(older.theme, ThemeName::Blue);
        assert!(older.alerts);
        assert!(
            !older.streamer_mode,
            "streamer mode is off unless turned on"
        );
        assert_eq!(older.tooltip_seconds, TOOLTIP_SECONDS_DEFAULT);
        assert_eq!(older.sidebar_width, SIDEBAR_WIDTH_DEFAULT);
        assert!(!older.sidebar_collapsed);
        // From when tooltips were only on or off.
        assert_eq!(Preferences::parse("tooltips = false\n").tooltip_seconds, 0);
        assert_eq!(
            Preferences::parse("tooltips = true\n").tooltip_seconds,
            TOOLTIP_SECONDS_DEFAULT
        );
    }

    #[test]
    fn column_widths_are_kept_and_resetting_gives_the_usual_ones_back() {
        let mut preferences = Preferences::default();
        preferences.player_widths.set(PlayerColumn::Player, 210.);
        preferences.log_widths.set(LogColumn::Kind, 60.);
        let text = preferences.format();
        assert!(text.contains("player_columns = player=210\n"), "{text}");
        assert!(text.contains("log_columns = kind=60\n"), "{text}");
        assert_eq!(Preferences::parse(&text), preferences);
        assert_eq!(preferences.reset().player_widths, Widths::default());
    }

    #[test]
    fn sidebar_widths_stay_in_range() {
        assert_eq!(sidebar_width(250.4), 250);
        assert_eq!(sidebar_width(-40.), SIDEBAR_WIDTH_MIN);
        assert_eq!(sidebar_width(5000.), SIDEBAR_WIDTH_MAX);
        assert_eq!(sidebar_width(f32::NAN), SIDEBAR_WIDTH_MIN);
    }

    #[test]
    fn dragging_far_enough_collapses_the_sidebar() {
        assert_eq!(SIDEBAR_SNAP_WIDTH, 108);
        // Past the minimum but short of the snap point, it stays in full at the minimum.
        assert_eq!(dragged_sidebar(150.), Some(SIDEBAR_WIDTH_MIN));
        assert_eq!(dragged_sidebar(108.), Some(SIDEBAR_WIDTH_MIN));
        assert_eq!(dragged_sidebar(107.), None);
        assert_eq!(dragged_sidebar(-20.), None);
        assert_eq!(dragged_sidebar(260.), Some(260));
        assert_eq!(dragged_sidebar(9000.), Some(SIDEBAR_WIDTH_MAX));
    }

    #[test]
    fn a_server_is_kept_by_its_key_wherever_its_address_goes() -> Result<(), SecureError> {
        let key = ServerKey::generate()?.public();
        let other = ServerKey::generate()?.public();
        let kept = |preferences: &Preferences| -> Vec<String> {
            let servers = preferences.servers.iter();
            servers.map(|server| server.address.clone()).collect()
        };
        let mut preferences = Preferences::default();

        // Joined by its address alone: the key it then had is kept with it, and goes with
        // the address the next time the player types it.
        preferences.remember_server("home.example:1", None);
        assert_eq!(preferences.server_with_its_key("home.example:1"), None);
        let with_key = join_code("home.example:1", &key);
        let learned = preferences.learn_server_key("home.example:1", &key);
        assert_eq!(learned.as_deref(), Some(with_key.as_str()));
        assert_eq!(
            preferences.server_with_its_key("home.example:1"),
            Some(with_key.as_str())
        );
        // Joined again with the same key, nothing changes; and an address given with a key
        // of its own is taken as it is.
        assert_eq!(preferences.learn_server_key(&with_key, &key), None);
        assert_eq!(preferences.server_with_its_key(&with_key), None);
        assert_eq!(kept(&preferences), std::slice::from_ref(&with_key));
        // The panel's own server, set up anew and joined: its new key takes the old one's
        // place, and the old one is put back for what follows.
        let anew = join_code("home.example:1", &other);
        let learned = preferences.learn_server_key(&with_key, &other);
        assert_eq!(learned.as_deref(), Some(anew.as_str()));
        assert_eq!(kept(&preferences), std::slice::from_ref(&anew));
        assert_eq!(
            preferences.learn_server_key(&anew, &key).as_deref(),
            Some(with_key.as_str())
        );

        // An invite to the same server at a new address takes its place; another server
        // is another entry, though it is at the first one's old address.
        let moved = join_code("203.0.113.5:1", &key);
        preferences.remember_server(&moved, None);
        assert_eq!(kept(&preferences), std::slice::from_ref(&moved));
        let stranger = join_code("home.example:1", &other);
        preferences.remember_server(&stranger, None);
        assert_eq!(kept(&preferences), [stranger, moved]);

        // And the keys survive the preferences file.
        let read = Preferences::parse(&preferences.format());
        assert_eq!(kept(&read), kept(&preferences));
        Ok(())
    }

    #[test]
    fn recent_servers_keep_their_passwords_only_when_asked() {
        let mut preferences = Preferences::default();
        preferences.remember_server("a:1", Some(passwords("secret", "crown")));
        preferences.remember_server("b:2", None);
        let saved = |preferences: &Preferences| -> Vec<(String, Option<String>, Option<String>)> {
            let servers = preferences.servers.iter().cloned();
            servers
                .map(|server| (server.address, server.password, server.host_password))
                .collect()
        };
        let kept = |password: &str| Some(password.to_owned());
        assert_eq!(
            saved(&preferences),
            [
                ("b:2".to_owned(), None, None),
                ("a:1".to_owned(), kept("secret"), kept("crown"))
            ]
        );
        // Joining again without remembering the passwords forgets both.
        preferences.remember_server("a:1", None);
        assert_eq!(
            saved(&preferences),
            [
                ("a:1".to_owned(), None, None),
                ("b:2".to_owned(), None, None)
            ]
        );
        for n in 0..10 {
            preferences.remember_server(&format!("host{n}:1"), None);
        }
        assert_eq!(preferences.servers.len(), super::RECENT_SERVERS);
        preferences.forget_server("host9:1");
        assert_eq!(
            preferences.servers.first().map(|s| s.address.as_str()),
            Some("host8:1")
        );
    }

    #[test]
    fn recent_files_are_newest_first_without_repeats() {
        let mut preferences = Preferences::default();
        for name in ["a", "b", "c", "d", "e", "f", "b"] {
            preferences.remember_file(Path::new(name));
        }
        let names: Vec<PathBuf> = ["b", "f", "e", "d", "c"].map(PathBuf::from).to_vec();
        assert_eq!(preferences.recent_files, names);
        assert_eq!(preferences.recent_files.len(), RECENT_FILES);
        assert_eq!(preferences.settings_to_reopen(), Some(Path::new("b")));
        preferences.reopen_settings = false;
        assert_eq!(preferences.settings_to_reopen(), None);
    }

    #[test]
    fn resetting_keeps_what_the_panel_remembers_for_itself() {
        let mut preferences = Preferences {
            theme: ThemeName::Sepia,
            compact: true,
            message_seconds: 15,
            listen_address: Some("0.0.0.0:41313".to_owned()),
            streamer_mode: true,
            ..Preferences::default()
        };
        preferences.remember_file(Path::new("a.conf"));
        preferences.overview.add(PanelKind::Log);
        let reset = preferences.reset();
        // What streamer mode hides is not shown for a reset of the theme.
        assert!(reset.streamer_mode);
        assert_eq!(reset.theme, ThemeName::Blue);
        assert!(!reset.compact);
        assert_eq!(reset.message_seconds, 6);
        assert_eq!(reset.listen_address, preferences.listen_address);
        assert_eq!(reset.recent_files, preferences.recent_files);
        assert_eq!(reset.overview, preferences.overview);
        // A message with Undo stays six seconds longer.
        assert_eq!(reset.message_duration(true).as_secs(), 12);
    }

    #[test]
    fn saving_creates_the_folder_and_loading_reads_it_back() -> Result<(), String> {
        let folder = std::env::temp_dir().join(format!("b3313-prefs-{}", std::process::id()));
        let path = folder.join("panel.conf");
        // A missing file is the defaults.
        assert_eq!(Preferences::load(&path), Ok(Preferences::default()));
        let preferences = Preferences {
            zoom: Zoom::default().larger(),
            theme: ThemeName::Comfort,
            ..Preferences::default()
        };
        let saved = preferences.save(&path);
        let loaded = Preferences::load(&path);
        // What is there and cannot be read is not the defaults: here, the folder itself.
        let unreadable = Preferences::load(&folder);
        let _ = std::fs::remove_dir_all(&folder);
        saved?;
        assert_eq!(loaded, Ok(preferences));
        assert!(unreadable.is_err());
        Ok(())
    }

    #[test]
    fn a_file_saved_in_another_encoding_keeps_its_settings() {
        let path = std::env::temp_dir().join(format!("b3313-prefs-ansi-{}", std::process::id()));
        let saved = Preferences {
            theme: ThemeName::Comfort,
            ..Preferences::default()
        };
        // As Notepad's ANSI writes an "é" in a comment: not UTF-8.
        let mut bytes = b"# pr\xE9f\xE9rences\n".to_vec();
        bytes.extend(saved.format().bytes());
        let written = std::fs::write(&path, bytes);
        let loaded = Preferences::load(&path);
        let _ = std::fs::remove_file(&path);
        assert!(written.is_ok());
        assert_eq!(loaded.map(|loaded| loaded.theme), Ok(ThemeName::Comfort));
    }
}
