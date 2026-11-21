//! The panel's window: a sidebar of pages, the server it hosts, and the settings being edited.

mod arrange;
mod bans_page;
mod chat;
mod column_resize;
mod crash_notice;
mod log_page;
mod messages;
mod overview;
mod overview_panels;
mod page_arrange;
mod play_page;
mod players;
mod preferences_behavior;
mod preferences_page;
mod settings_page;
mod sidebar;

use crate::alert;
use crate::dashboard::{Dashboard, Kind, LogColumn, LogEntry, PlayerColumn};
use crate::format;
use crate::host::{Host, Notice};
use crate::icons::IconName;
use crate::network::{self, Reach};
use crate::overview_layout::{OverviewLayout, PanelState};
use crate::page_layout::ArrangedPage;
use crate::port_map::{self, PortMap};
use crate::preferences::{self, Preferences, StartPage, WindowPlacement};
use crate::privacy;
use crate::settings::{FieldKind, GROUPS, SettingsForm};
use crate::sort::{Sort, SortOrder};
use crate::system;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{FONT, Mode, Theme, ThemeName, scaled};
use crate::widgets::{
    Button, Tone, TooltipDuration, dismiss_tooltips, follow_tooltips, track_mouse_navigation,
};
use crate::zoom::Zoom;
use b3313_protocol::{Message, PlayerId};
use b3313_server::bans::{BanList, read_ban_file};
use b3313_server::{Command, Config, Event};
use gpui::{
    AnyElement, App, Context, CursorStyle, DispatchPhase, Entity, FocusHandle, Focusable,
    FontWeight, KeyBinding, MouseMoveEvent, MouseUpEvent, PathPromptOptions, PromptLevel,
    ScrollDelta, ScrollHandle, ScrollWheelEvent, SharedString, Subscription, Task,
    UniformListScrollHandle, Window, WindowBounds, actions, canvas, div, prelude::*, px,
};
use std::collections::{BTreeMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};

actions!(
    panel,
    [
        NewConfig,
        OpenConfig,
        SaveConfig,
        SaveConfigAs,
        FocusNext,
        FocusPrevious,
        ZoomIn,
        ZoomOut,
        ResetZoom,
        GoBack,
        FindInLog,
        ToggleStreamerMode,
        ToggleSidebar,
        ShowPlay,
        ShowOverview,
        ShowPlayers,
        ShowBans,
        ShowSettings,
        ShowLog,
        ShowPreferences
    ]
);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("secondary-n", NewConfig, None),
        KeyBinding::new("secondary-o", OpenConfig, None),
        KeyBinding::new("secondary-s", SaveConfig, None),
        KeyBinding::new("secondary-shift-s", SaveConfigAs, None),
        KeyBinding::new("tab", FocusNext, None),
        KeyBinding::new("shift-tab", FocusPrevious, None),
        // As browsers do: Ctrl and = (or +, on the same key) for larger, - for smaller.
        KeyBinding::new("secondary-=", ZoomIn, None),
        KeyBinding::new("secondary-+", ZoomIn, None),
        KeyBinding::new("secondary--", ZoomOut, None),
        KeyBinding::new("secondary-0", ResetZoom, None),
        KeyBinding::new("escape", GoBack, None),
        KeyBinding::new("secondary-f", FindInLog, None),
        KeyBinding::new("secondary-shift-h", ToggleStreamerMode, None),
        KeyBinding::new("secondary-b", ToggleSidebar, None),
        // The pages in the sidebar's order, as `Page::shortcut` names them.
        KeyBinding::new("secondary-1", ShowPlay, None),
        KeyBinding::new("secondary-2", ShowOverview, None),
        KeyBinding::new("secondary-3", ShowPlayers, None),
        KeyBinding::new("secondary-4", ShowBans, None),
        KeyBinding::new("secondary-5", ShowSettings, None),
        KeyBinding::new("secondary-6", ShowLog, None),
        KeyBinding::new("secondary-7", ShowPreferences, None),
    ]);
}

/// Every shortcut, as the Preferences page lists them: keys, then what they do.
pub(crate) const SHORTCUTS: [(&str, &str); 14] = [
    (
        "Ctrl+1 to Ctrl+7",
        "Play, Overview, Players, Bans, Server settings, Log, Preferences",
    ),
    ("Ctrl+N", "Start new server settings from the defaults"),
    ("Ctrl+O", "Open a settings file"),
    ("Ctrl+S", "Save the server settings"),
    ("Ctrl+Shift+S", "Save the server settings as a new file"),
    ("Ctrl+F", "Search the log"),
    (
        "Ctrl+Shift+H",
        "Streamer mode on or off: hides IP addresses, server names and your user folder",
    ),
    ("Ctrl+B", "Sidebar with only icons, or in full"),
    ("Ctrl+= or Ctrl+wheel up", "Larger interface"),
    ("Ctrl+− or Ctrl+wheel down", "Smaller interface"),
    ("Ctrl+0", "Interface size back to 100%"),
    (
        "Esc",
        "Back: from a player's details, from arranging a page, or from the recent messages",
    ),
    ("Tab / Shift+Tab", "Move between buttons and fields"),
    (
        "Enter",
        "Press the focused button; in a field, do what it is for: join, send the chat line, \
         start the server, or ban",
    ),
];

/// The address offered at first: this PC only. Others must be let in deliberately.
const DEFAULT_ADDRESS: &str = "127.0.0.1:41313";
/// How often the panel collects what the server reported.
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const SUGGESTED_FILE_NAME: &str = "server.conf";
/// What every page calls settings that have no file yet, so they read as one state.
const NOT_SAVED: &str = "Not saved to a file";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    /// Joining a session with this PC's game, as a player.
    Play,
    Overview,
    Players,
    Bans,
    /// The server's settings file.
    Settings,
    Log,
    /// The panel's own: how it looks, alerts and starts.
    Preferences,
}

impl From<StartPage> for Page {
    fn from(page: StartPage) -> Self {
        match page {
            StartPage::Play => Self::Play,
            StartPage::Overview => Self::Overview,
            StartPage::Players => Self::Players,
            StartPage::Bans => Self::Bans,
            StartPage::Settings => Self::Settings,
            StartPage::Log => Self::Log,
            StartPage::Preferences => Self::Preferences,
        }
    }
}

impl Page {
    const ALL: [Self; 7] = [
        Self::Play,
        Self::Overview,
        Self::Players,
        Self::Bans,
        Self::Settings,
        Self::Log,
        Self::Preferences,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Play => "Play",
            Self::Overview => "Overview",
            Self::Players => "Players",
            Self::Bans => "Bans",
            Self::Settings => "Server settings",
            Self::Log => "Log",
            Self::Preferences => "Preferences",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Play => IconName::Gamepad,
            Self::Overview => IconName::LayoutDashboard,
            Self::Players => IconName::Users,
            Self::Bans => IconName::Ban,
            Self::Settings => IconName::Settings,
            Self::Log => IconName::ScrollText,
            Self::Preferences => IconName::SlidersHorizontal,
        }
    }

    /// The keys that show it, by its place in the sidebar.
    fn shortcut(self) -> &'static str {
        match self {
            Self::Play => "Ctrl+1",
            Self::Overview => "Ctrl+2",
            Self::Players => "Ctrl+3",
            Self::Bans => "Ctrl+4",
            Self::Settings => "Ctrl+5",
            Self::Log => "Ctrl+6",
            Self::Preferences => "Ctrl+7",
        }
    }
}

/// The player whose details are open, with when they joined, so their entries are found even
/// after they leave and someone else gets their number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Selected {
    id: PlayerId,
    joined_at: Duration,
}

/// The server while it runs, with the settings it started with.
#[derive(Debug)]
struct Running {
    host: Host,
    config: Config,
    started: Instant,
    /// Who can join, and the address to give them; found once, as the server starts.
    reach: Reach,
    /// Whether the router lets players over the internet in.
    port: RouterPort,
}

/// The server's port on its owner's router.
#[derive(Debug)]
enum RouterPort {
    /// Not asked for: the server is for this PC alone, or the user turned asking off.
    NotAsked,
    Asking,
    /// Open for as long as this is kept.
    Open(PortMap),
    /// Why the router did not open it, in a sentence for the user.
    Refused(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ToastTone {
    Success,
    Error,
}

/// A short message about the last thing done, shown for a few seconds.
#[derive(Debug)]
struct Toast {
    tone: ToastTone,
    text: String,
    shown: Instant,
    /// What its Undo button reverses, for an action the user may regret.
    undo: Option<Undo>,
    /// Tells it from the one before, so that a new message fades in too.
    serial: u64,
    /// When it started fading away.
    leaving: Option<Instant>,
}

/// Puts back what an action changed, offered in the message that reports the action: quicker
/// than asking first, and it costs only the one who made the mistake.
#[derive(Debug)]
enum Undo {
    /// Lets in an address just banned.
    Unban(IpAddr),
    /// Refuses again an address just let in.
    Reban(IpAddr),
    /// Puts back a log just cleared.
    RestoreLog(VecDeque<LogEntry>),
    /// Puts back an Overview panel just removed, where it was.
    RestorePanel(usize, PanelState),
    /// Puts back the Overview's layout from before Reset.
    RestoreLayout(OverviewLayout),
    /// Puts back the preferences from before Reset all.
    RestorePreferences(Box<Preferences>),
    /// Puts back the settings as they were edited before Revert.
    RestoreSettings(Box<SettingsForm>),
}

#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent flags (following the log, closing, an alert due, the clock), not \
              states of one thing"
)]
pub struct Panel {
    focus: FocusHandle,
    page: Page,
    address: Entity<TextInput>,
    running: Option<Running>,
    /// The host password of a server stopped to be started again, for the next to take.
    restart_password: Option<String>,
    dashboard: Dashboard,
    bans: BanList,
    ban_input: Entity<TextInput>,
    /// What the server's owner is typing to say to everyone in the chat.
    say_input: Entity<TextInput>,
    /// What players and the server's owner have said since it started, as the Players page shows.
    server_chat: chat::ChatLog,
    /// The page whose panels show the strip that moves them, while it is being arranged.
    page_arranging: Option<ArrangedPage>,
    form: SettingsForm,
    fields: Vec<(&'static str, Entity<TextInput>)>,
    field_errors: BTreeMap<&'static str, String>,
    /// Which kinds of log entry are shown.
    log_kinds: Vec<Kind>,
    follow_log: bool,
    log_scroll: UniformListScrollHandle,
    /// How far sideways the Players page's table is scrolled.
    players_scroll: ScrollHandle,
    toast: Option<Toast>,
    /// The uptime last drawn, in whole seconds, so the clock redraws once a second.
    shown_uptime: u64,
    /// Set once the user has agreed to close, so closing does not ask again.
    closing: bool,
    preferences: Preferences,
    /// Whether the preferences' file could not be read as the panel started: it is then
    /// never saved, as the usual preferences would replace whatever it has.
    preferences_unread: bool,
    /// The player whose details the Players page shows instead of the table.
    selected: Option<Selected>,
    log_search: Entity<TextInput>,
    /// Ctrl+wheel movement not yet enough for a step of interface size.
    wheel_zoom: f32,
    /// Something happened that the user should see, to flash the taskbar for if the panel is
    /// in the background.
    alert_due: bool,
    player_sort: Sort<PlayerColumn>,
    log_sort: Sort<LogColumn>,
    /// The banned addresses' one column, by address.
    ban_order: SortOrder,
    /// Times of day on the 24-hour clock; looked up once rather than on every redraw.
    hour24: bool,
    /// Whether a Windows contrast theme is on: asked as the panel opens and when Windows says
    /// its appearance changed, rather than on every redraw.
    high_contrast: bool,
    /// The user's folder, which streamer mode hides in paths.
    home: Option<String>,
    /// While the sidebar's edge is dragged, the full sidebar's width when the drag began, kept
    /// if the drag collapses it.
    sidebar_drag: Option<u16>,
    /// The table column whose edge is being dragged to change its width.
    column_drag: Option<column_resize::ColumnDrag>,
    /// The sidebar sliding between icons only and in full.
    sidebar_slide: Option<sidebar::Slide>,
    history: messages::History,
    play: play_page::Play,
    arranging: arrange::Arranging,
    /// The window's title as last set, so it is set again only when it changes.
    shown_title: String,
    crash_notice: Option<crash_notice::CrashNotice>,
    _subscriptions: Vec<Subscription>,
    _poll: Task<()>,
}

impl Panel {
    /// Lets the panel decide whether the window closes, as with settings not yet saved.
    fn ask_before_closing(window: &mut Window, cx: &mut Context<Self>) {
        let this = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |this, cx| this.should_close(window, cx))
                .unwrap_or(true)
        });
    }

    /// A panel editing `form`, drawn as `preferences` say; `problem` is shown if opening the
    /// settings the user asked for failed. With the preferences comes why their file could
    /// not be read, if it could not: the panel then saves none, which would replace it.
    pub fn new(
        form: SettingsForm,
        problem: Option<String>,
        (preferences, unread): (Preferences, Option<String>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let listen = preferences
            .listen_address
            .clone()
            .unwrap_or_else(|| DEFAULT_ADDRESS.to_owned());
        let address = cx.new(|cx| TextInput::new(cx, &listen, DEFAULT_ADDRESS));
        let ban_input = cx.new(|cx| TextInput::new(cx, "", "IP address, such as 203.0.113.7"));
        let say_input = cx.new(|cx| TextInput::new(cx, "", "Say something to everyone"));
        let log_search = cx.new(|cx| TextInput::new(cx, "", "Search the log"));
        let mut subscriptions = vec![
            cx.subscribe(&say_input, |this, _, event, cx| {
                if *event == InputEvent::Submitted {
                    this.say_as_server(cx);
                }
            }),
            cx.subscribe(&log_search, |this, _, event, cx| {
                if *event == InputEvent::Changed {
                    // A new search shows its newest matches.
                    this.follow_log = true;
                    cx.notify();
                }
            }),
            cx.subscribe_in(&address, window, |this, _, event, window, cx| {
                if *event == InputEvent::Submitted {
                    this.start(window, cx);
                }
            }),
            cx.subscribe_in(
                &ban_input,
                window,
                |this, input, event, window, cx| match event {
                    InputEvent::Submitted => this.ban_address(window, cx),
                    InputEvent::Changed => {
                        input.update(cx, |input, cx| input.set_invalid(false, cx));
                    }
                },
            ),
            // Also when a Windows contrast theme is turned on or off, which changes it too.
            cx.observe_window_appearance(window, |this, window, cx| {
                this.high_contrast = system::high_contrast_on();
                this.apply_theme(window, cx);
            }),
        ];
        let fields = Self::setting_fields(&form, &mut subscriptions, cx);
        let poll = cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                if this.update_in(cx, Self::poll).is_err() {
                    break;
                }
            }
        });
        Self::ask_before_closing(window, cx);

        let now = Instant::now();
        let mut panel = Self {
            focus: cx.focus_handle(),
            page: Page::from(preferences.start_page),
            address,
            running: None,
            restart_password: None,
            dashboard: Dashboard::new(now).with_time_of_day(system::local_time_of_day()),
            bans: BanList::default(),
            ban_input,
            say_input,
            server_chat: chat::ChatLog::default(),
            page_arranging: None,
            form,
            fields,
            field_errors: BTreeMap::new(),
            log_kinds: preferences.log_kinds.clone(),
            follow_log: true,
            log_scroll: UniformListScrollHandle::new(),
            players_scroll: ScrollHandle::new(),
            toast: None,
            shown_uptime: 0,
            closing: false,
            preferences_unread: unread.is_some(),
            selected: None,
            log_search,
            wheel_zoom: 0.0,
            alert_due: false,
            // As last kept; the defaults unless the user keeps them.
            player_sort: preferences.player_sort,
            log_sort: preferences.log_sort,
            ban_order: preferences.ban_order,
            hour24: preferences.uses_24_hour(),
            high_contrast: system::high_contrast_on(),
            home: std::env::var("USERPROFILE").ok(),
            sidebar_drag: None,
            column_drag: None,
            sidebar_slide: None,
            history: messages::History::default(),
            play: play_page::Play::new(preferences.servers.first(), &preferences.player_name, cx),
            arranging: arrange::Arranging::default(),
            shown_title: String::new(),
            crash_notice: None,
            preferences,
            _subscriptions: subscriptions,
            _poll: poll,
        };
        panel.open(unread, problem, window, cx);
        panel
    }

    /// What opening does once the panel exists: the settings checked, the bans read, the
    /// server started if the user wants it, and any crash reports from last time offered.
    /// `unread` is why the preferences could not be read, and `problem` why the settings
    /// asked for could not be opened, if either is so.
    fn open(
        &mut self,
        unread: Option<String>,
        problem: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(unread) = unread {
            self.report_problem(unread);
        }
        window.set_rem_size(px(self.preferences.zoom.rem_size()));
        self.refresh_validation(cx);
        if let Err(message) = self.sync_bans() {
            self.report_problem(message);
        }
        if let Some(path) = self.form.path().map(Path::to_owned) {
            self.preferences.remember_file(&path);
            self.save_preferences();
        }
        match problem {
            Some(problem) => self.report_problem(problem),
            // Not when the settings meant to start with failed to open: the defaults in their
            // place are not what the user chose to serve.
            None if self.preferences.start_server => self.start(window, cx),
            None => {}
        }
        self.mask_fields(cx);
        self.find_crash_reports(false);
        self.share_state();
        window.focus(&self.focus);
    }

    /// A field for each server setting, filled from `form`, each updating the form as it is
    /// typed in.
    fn setting_fields(
        form: &SettingsForm,
        subscriptions: &mut Vec<Subscription>,
        cx: &mut Context<Self>,
    ) -> Vec<(&'static str, Entity<TextInput>)> {
        let mut fields = Vec::new();
        for field in GROUPS.iter().flat_map(|group| group.fields) {
            let key = field.key;
            let input = cx.new(|cx| {
                let input = TextInput::new(cx, form.value(key), "");
                if field.kind == FieldKind::Password {
                    input.masked()
                } else {
                    input
                }
            });
            subscriptions.push(cx.subscribe(&input, move |this, input, event, cx| {
                if *event == InputEvent::Changed {
                    let text = input.read(cx).text().to_owned();
                    this.form.set(key, &text);
                    this.refresh_validation(cx);
                }
            }));
            fields.push((key, input));
        }
        fields
    }

    // --- The server ---

    fn start(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        let Some(config) = self.settings_to_serve(cx) else {
            return;
        };
        if let Err(message) = self.sync_bans() {
            self.report_problem(message);
            cx.notify();
            return;
        }
        // A server started again keeps the host password it made up before, which the
        // host's own bridge still holds; the settings' own, if they have one, stands.
        let made_up = self.restart_password.take().unwrap_or_default();
        let host_password = if config.host_password.is_empty() {
            made_up
        } else {
            config.host_password.clone()
        };
        let serving = Config {
            bans: self.bans.bans().to_vec(),
            host_password,
            ..config.clone()
        };
        let address = self.address.read(cx).text().to_owned();
        match Host::start(&address, serving) {
            Ok(host) => {
                let message = format!("Server started on {}", host.address());
                self.dashboard
                    .note(Kind::Session, message.clone(), Instant::now());
                self.show_toast(ToastTone::Success, message);
                self.address
                    .update(cx, |input, cx| input.set_disabled(true, cx));
                if self.preferences.listen_address.as_deref() != Some(address.as_str()) {
                    self.preferences.listen_address = Some(address);
                    self.save_preferences();
                }
                // The chat is this server's own: the last one's lines would take their
                // speakers' marks from whoever has those players' numbers now.
                self.server_chat.clear();
                let reach = network::reach(host.address(), network::local_network_ip());
                let started = Instant::now();
                self.running = Some(Running {
                    host,
                    config,
                    started,
                    reach,
                    port: RouterPort::NotAsked,
                });
                if let (Reach::Network(local), true) = (reach, self.preferences.open_port) {
                    self.open_router_port(local, started, cx);
                }
            }
            Err(message) => self.report_problem(message),
        }
        cx.notify();
    }

    /// Asks the router to send the server's port to `local`, this PC's address on its network,
    /// and notes the answer for the server started at `started`, if it is still the one
    /// running. Waiting on the router happens off the panel's thread.
    fn open_router_port(&mut self, local: SocketAddr, started: Instant, cx: &mut Context<Self>) {
        if let Some(running) = &mut self.running {
            running.port = RouterPort::Asking;
        }
        let asked = cx
            .background_executor()
            .spawn(async move { port_map::open(local) });
        cx.spawn(async move |this, cx| {
            let answer = asked.await;
            let _ = this.update(cx, |this, cx| {
                let Some(running) = this.running.as_mut().filter(|r| r.started == started) else {
                    // That server has stopped; dropping the answer closes the port again.
                    return;
                };
                let (port, message) = match answer {
                    Ok(map) => {
                        let message = format!("The router opened port {}", local.port());
                        (RouterPort::Open(map), message)
                    }
                    Err(reason) => (RouterPort::Refused(reason.clone()), reason),
                };
                running.port = port;
                this.dashboard.note(Kind::Session, message, Instant::now());
                cx.notify();
            });
        })
        .detach();
    }

    /// Has the router close the server's port and waits a moment for it to hear: before the
    /// panel exits, when nothing else would be left to tell it.
    fn close_router_port(&mut self) {
        if let Some(running) = &mut self.running
            && let RouterPort::Open(map) =
                std::mem::replace(&mut running.port, RouterPort::NotAsked)
        {
            map.close();
        }
    }

    /// The settings to serve with, or `None` after saying what in them must be fixed first.
    fn settings_to_serve(&mut self, cx: &mut Context<Self>) -> Option<Config> {
        match self.form.config() {
            Ok(config) => Some(config),
            Err(message) => {
                self.show_toast(
                    ToastTone::Error,
                    format!("Fix the settings first: {message}"),
                );
                self.go_to(Page::Settings, cx);
                None
            }
        }
    }

    fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.unless_players(window, cx, "Stop the server?", "Stop", |this, _, cx| {
            this.stop_now(cx);
        });
    }

    fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Looked at before the server stops: one that could not start again would leave its
        // players joining nothing.
        let Some(config) = self.settings_to_serve(cx) else {
            return;
        };
        if let Some(path) = config.ban_file {
            let usable = match self.bans.path() {
                Some(_) => read_ban_file(&path).map(|_| ()),
                // Bans made with no file go into the one named since, and now: a file that
                // cannot take them would keep the server from starting.
                None => BanList::open(path, self.bans.bans()).map(|kept| self.bans = kept),
            };
            if let Err(message) = usable {
                self.report_problem(message);
                cx.notify();
                return;
            }
        }
        self.unless_players(
            window,
            cx,
            "Restart the server?",
            "Restart",
            |this, window, cx| {
                this.stop_server(true, cx);
                this.start(window, cx);
            },
        );
    }

    /// Asks first if anyone is playing, since they would be disconnected.
    fn unless_players(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        question: &str,
        confirm: &str,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        match self.dashboard.player_count() {
            // Nobody to disconnect, or the user chose not to be asked.
            _ if !self.preferences.confirm_stop => then(self, window, cx),
            0 => then(self, window, cx),
            count => {
                let detail = format!(
                    "{count} {} playing. Stopping ends the session for them; restarting has \
                     them join again by themselves.",
                    if count == 1 {
                        "player is"
                    } else {
                        "players are"
                    }
                );
                Self::confirm(window, cx, question, &detail, confirm, then);
            }
        }
    }

    fn stop_now(&mut self, cx: &mut Context<Self>) {
        self.stop_server(false, cx);
    }

    /// Stops the server. To start it again at once, if `restarting`: its players' bridges are
    /// then told to join again by themselves, and the host's own stays the host's, by the
    /// next server's taking this one's host password.
    fn stop_server(&mut self, restarting: bool, cx: &mut Context<Self>) {
        if let Some(running) = self.running.take() {
            let notices = if restarting {
                self.restart_password = Some(running.host.host_password().to_owned());
                running.host.stop_to_restart()
            } else {
                running.host.stop()
            };
            for notice in notices {
                self.handle(notice);
            }
            self.server_stopped();
            self.dashboard
                .note(Kind::Session, "Server stopped".to_owned(), Instant::now());
        }
        self.address
            .update(cx, |input, cx| input.set_disabled(false, cx));
        cx.notify();
    }

    /// What follows the server's no longer running, however it came to stop.
    fn server_stopped(&mut self) {
        self.dashboard.server_stopped();
        // The settings may have changed under the server: the bans are theirs again. Only
        // where a file kept them: bans kept by none go into a file when the server is next
        // started, not into one whose name may be half typed as the server stops.
        if self.bans.path().is_some()
            && let Err(message) = self.sync_bans()
        {
            self.report_problem(message);
        }
    }

    fn poll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let notices = self
            .running
            .as_ref()
            .map(|running| running.host.drain())
            .unwrap_or_default();
        let mut changed = !notices.is_empty();
        for notice in notices {
            self.handle(notice);
        }
        if self.running.is_none() {
            self.address
                .update(cx, |input, cx| input.set_disabled(false, cx));
        }
        let lasts = (
            self.preferences.message_duration(false),
            self.preferences.message_duration(true),
        );
        changed |= self.poll_play();
        if crate::crash::take_new_report() {
            self.find_crash_reports(true);
            changed = true;
        }
        changed |= self.age_toast(|toast| {
            if toast.undo.is_some() {
                lasts.1
            } else {
                lasts.0
            }
        });
        let uptime = self.uptime().map_or(0, |uptime| uptime.as_secs());
        if uptime != self.shown_uptime {
            self.shown_uptime = uptime;
            changed = true;
        }
        if std::mem::take(&mut self.alert_due) {
            if self.preferences.alerts && !window.is_window_active() {
                alert::flash_taskbar(window);
            }
            // Heard from the game too, where the taskbar is out of sight.
            if self.preferences.alert_sound {
                alert::sound();
            }
        }
        if changed {
            self.share_state();
            cx.notify();
        }
    }

    /// Takes in one thing the server reported, or that the panel did itself while it is stopped.
    fn handle(&mut self, notice: Notice) {
        let now = Instant::now();
        match notice {
            Notice::Event(event) => {
                match &event {
                    Event::Kicked(_) | Event::RoleChanged(..) => self.report(event.to_string()),
                    Event::Chat { from, name, text } => {
                        let speaker = from.and_then(|from| self.dashboard.player(from));
                        self.server_chat.push(chat::ChatEntry {
                            at: system::local_time_of_day(),
                            from: *from,
                            role: speaker.map(|row| row.role),
                            name: name.clone(),
                            text: text.as_str().to_owned(),
                        });
                    }
                    Event::Banned(_, ip) | Event::AddressBanned(ip) => {
                        self.show_undoable(event.to_string(), Undo::Unban(*ip));
                    }
                    Event::Unbanned(ip) => {
                        self.show_undoable(event.to_string(), Undo::Reban(*ip));
                    }
                    Event::CommandFailed(message) => {
                        self.show_toast(ToastTone::Error, message.clone());
                    }
                    _ => {}
                }
                self.dashboard.apply(&event, now);
                if self
                    .dashboard
                    .log()
                    .back()
                    .is_some_and(|entry| self.preferences.alert_kinds.contains(&entry.kind))
                {
                    self.alert_due = true;
                }
                // After the event is logged, so a ban file that cannot be written shows as
                // a problem with the ban it failed to keep.
                if let Err(message) = self.bans.update(&event) {
                    self.report_problem(message);
                }
            }
            Notice::Failed(message) => {
                self.running = None;
                self.server_stopped();
                self.report_problem(message);
            }
            // The log already has who ended it, from the server's own event.
            Notice::Ended => {
                self.running = None;
                self.server_stopped();
                self.dashboard
                    .note(Kind::Session, "Server stopped".to_owned(), now);
                let told = "The session was ended; the server has stopped".to_owned();
                self.show_toast(ToastTone::Success, told);
            }
        }
    }

    fn uptime(&self) -> Option<Duration> {
        self.running
            .as_ref()
            .map(|running| running.started.elapsed())
    }

    /// Whether the settings being edited differ from those the running server started with.
    fn needs_restart(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| self.form.config().ok().as_ref() != Some(&running.config))
    }

    // --- Moderation ---

    fn kick(&mut self, player: PlayerId) {
        if let Some(running) = &self.running {
            running.host.send(Command::Kick(player));
        }
    }

    /// Makes a player a moderator, who may kick and ban from the chat, or an ordinary player
    /// again. The server reports the change, or why it could not be made.
    fn set_moderator(&mut self, player: PlayerId, moderator: bool) {
        if let Some(running) = &self.running {
            running.host.send(if moderator {
                Command::Mod(player)
            } else {
                Command::Unmod(player)
            });
        }
    }

    /// This PC's own player in the server it hosts, if it has joined its own server: the one
    /// person who is both the server's owner and, by the panel's doing, the session's host.
    /// Known by the number and the name being the same in the server's list and the session's.
    fn own_player(&self) -> Option<PlayerId> {
        self.running.as_ref()?;
        let play_page::PlayState::Playing { player, .. } = self.play.state else {
            return None;
        };
        let name = self.play.name_of(player)?;
        let row = self.dashboard.player(player)?;
        (row.name.as_deref() == Some(name)).then_some(player)
    }

    /// Says what is typed in the server chat's field to everyone, as the server, and empties
    /// the field. A line that starts with a slash is a command for the server instead, as it
    /// is in the game's chat: `/end` ends the session from here too.
    fn say_as_server(&mut self, cx: &mut Context<Self>) {
        let typed = self.say_input.read(cx).text().to_owned();
        if self.running.is_none() {
            return;
        }
        let command = if typed.trim_start().starts_with('/') {
            // By name as well as by number, as the players' own chat takes them.
            let parsed = {
                let players = self.dashboard.players();
                let named = players.filter_map(|row| Some((row.id, row.name.as_deref()?)));
                by_number(&typed, named).and_then(|line| Command::parse(&line))
            };
            match parsed {
                Ok(command) => command,
                Err(why) => {
                    // Left in the field, to be put right.
                    self.show_toast(ToastTone::Error, why);
                    cx.notify();
                    return;
                }
            }
        } else if let Some(message) = Message::new(&typed) {
            Command::Say(message)
        } else {
            return;
        };
        if let Some(running) = &self.running {
            running.host.send(command);
        }
        self.say_input
            .update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    /// The IP address the player numbered `player` plays from, if one is in the session.
    fn ip_of(&self, player: PlayerId) -> Option<IpAddr> {
        let mut players = self.dashboard.players();
        players
            .find(|row| row.id == player)
            .map(|row| row.address.ip())
    }

    fn ban_player(&mut self, player: PlayerId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ip) = self.ip_of(player) else {
            return;
        };
        let question = format!("Ban player {}?", player.get());
        let detail = format!(
            "{} will be refused from now on, including anyone else playing from it. \
             The Bans page can let it in again.",
            self.shown_ip(ip)
        );
        Self::confirm(window, cx, &question, &detail, "Ban", move |this, _, cx| {
            // The server bans whoever has the number when the command arrives. While the
            // question stood, the player asked about may have left and another taken it.
            if this.ip_of(player) != Some(ip) {
                let gone = format!("Player {} has left; nobody was banned", player.get());
                this.show_toast(ToastTone::Error, gone);
                cx.notify();
            } else if let Some(running) = &this.running {
                running.host.send(Command::Ban(player));
            }
        });
    }

    fn ban_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.ban_input.read(cx).text().trim().to_owned();
        let Ok(ip) = text.parse::<IpAddr>() else {
            self.ban_input
                .update(cx, |input, cx| input.set_invalid(true, cx));
            let message = if text.is_empty() {
                "Type the IP address to ban".to_owned()
            } else if self.preferences.streamer_mode {
                // What was typed is hidden in its field, and is not to show here.
                "That is not an IP address".to_owned()
            } else {
                format!("{text} is not an IP address")
            };
            self.show_toast(ToastTone::Error, message);
            cx.notify();
            return;
        };
        if self.bans.bans().contains(&ip) {
            self.show_toast(ToastTone::Error, format!("{ip} is already banned"));
            cx.notify();
            return;
        }
        let playing = self
            .dashboard
            .players()
            .filter(|row| row.address.ip() == ip)
            .count();
        let ban = move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
            this.ban_now(ip);
            this.ban_input
                .update(cx, |input, cx| input.set_text("", cx));
            cx.notify();
        };
        if playing == 0 {
            ban(self, window, cx);
        } else {
            let question = format!("Ban {}?", self.shown_ip(ip));
            let detail = format!(
                "{playing} {} from it will be removed.",
                if playing == 1 {
                    "player playing"
                } else {
                    "players playing"
                }
            );
            Self::confirm(window, cx, &question, &detail, "Ban", ban);
        }
    }

    /// Refuses `ip`, removing anyone playing from it, without asking.
    fn ban_now(&mut self, ip: IpAddr) {
        match &self.running {
            Some(running) => running.host.send(Command::BanAddress(ip)),
            None => self.handle(Notice::Event(Event::AddressBanned(ip))),
        }
    }

    fn unban(&mut self, ip: IpAddr, cx: &mut Context<Self>) {
        match &self.running {
            Some(running) => running.host.send(Command::Unban(ip)),
            None => self.handle(Notice::Event(Event::Unbanned(ip))),
        }
        cx.notify();
    }

    /// Reads the bans from the ban file the settings name, unless the server is running, when
    /// its events keep them current. Bans made while no file kept them are written into it.
    ///
    /// Without a ban file, bans stay in the panel until it closes, across restarts.
    fn sync_bans(&mut self) -> Result<(), String> {
        if self.running.is_some() {
            return Ok(());
        }
        let Ok(config) = self.form.config() else {
            return Ok(());
        };
        self.bans = match config.ban_file {
            Some(path) => {
                // Bans made while no file kept them go into the file named since.
                let unkept = match self.bans.path() {
                    Some(_) => &[],
                    None => self.bans.bans(),
                };
                BanList::open(path, unkept)?
            }
            None => BanList::new(None, self.bans.bans().to_vec()),
        };
        Ok(())
    }

    // --- Settings ---

    fn refresh_validation(&mut self, cx: &mut Context<Self>) {
        self.field_errors = self.form.errors();
        for (key, input) in &self.fields {
            let invalid = self.field_errors.contains_key(key);
            input.update(cx, |input, cx| input.set_invalid(invalid, cx));
        }
        cx.notify();
    }

    /// Shows a new form in the fields, as after opening a file.
    fn show_form(&mut self, form: SettingsForm, cx: &mut Context<Self>) {
        self.form = form;
        self.refresh_fields(cx);
    }

    /// Puts the form's values into the fields.
    fn refresh_fields(&mut self, cx: &mut Context<Self>) {
        for (key, input) in &self.fields {
            let value = self.form.value(key).to_owned();
            input.update(cx, |input, cx| input.set_text(&value, cx));
        }
        self.refresh_validation(cx);
        if let Err(message) = self.sync_bans() {
            self.report_problem(message);
        }
    }

    fn new_config(&mut self, _: &NewConfig, window: &mut Window, cx: &mut Context<Self>) {
        self.unless_unsaved(window, cx, "Start from the defaults?", |this, _, cx| {
            this.show_form(SettingsForm::new(&Config::default()), cx);
            this.show_toast(
                ToastTone::Success,
                "Settings reset to the defaults".to_owned(),
            );
        });
    }

    fn open_config(&mut self, _: &OpenConfig, window: &mut Window, cx: &mut Context<Self>) {
        self.unless_unsaved(
            window,
            cx,
            "Open another settings file?",
            |_, window, cx| {
                before_dialog(window);
                let chosen = cx.prompt_for_paths(PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("Open".into()),
                });
                cx.spawn_in(window, async move |this, cx| {
                    let path = match chosen.await {
                        Ok(Ok(Some(paths))) => paths.into_iter().next(),
                        Ok(Err(error)) => {
                            let message = format!("cannot show the file picker: {error}");
                            let _ = this.update(cx, |this, cx| {
                                this.report_problem(message);
                                cx.notify();
                            });
                            None
                        }
                        // Cancelled.
                        Ok(Ok(None)) | Err(_) => None,
                    };
                    if let Some(path) = path {
                        let _ = this.update(cx, |this, cx| this.open_path(&path, cx));
                    }
                })
                .detach();
            },
        );
    }

    fn open_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        match SettingsForm::open(path) {
            Ok(form) => {
                self.show_form(form, cx);
                self.show_toast(ToastTone::Success, format!("Opened {}", display_name(path)));
                self.preferences.remember_file(path);
                self.save_preferences();
            }
            Err(message) => self.report_problem(message),
        }
        cx.notify();
    }

    fn save_config(&mut self, _: &SaveConfig, window: &mut Window, cx: &mut Context<Self>) {
        match self.form.path().map(Path::to_owned) {
            Some(path) => self.save_to(&path, cx),
            None => self.save_config_as(&SaveConfigAs, window, cx),
        }
    }

    fn save_config_as(&mut self, _: &SaveConfigAs, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(message) = self.form.config() {
            self.show_toast(
                ToastTone::Error,
                format!("Fix the settings first: {message}"),
            );
            self.go_to(Page::Settings, cx);
            return;
        }
        let directory = self
            .form
            .path()
            .and_then(Path::parent)
            .map(Path::to_owned)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();
        let suggested = self
            .form
            .path()
            .map_or_else(|| SUGGESTED_FILE_NAME.to_owned(), display_name);
        before_dialog(window);
        let chosen = cx.prompt_for_new_path(&directory, Some(&suggested));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = chosen.await {
                let _ = this.update(cx, |this, cx| this.save_to(&path, cx));
            }
        })
        .detach();
    }

    fn save_to(&mut self, path: &Path, cx: &mut Context<Self>) {
        match self.form.save_as(path) {
            Ok(()) => {
                self.show_toast(ToastTone::Success, format!("Saved {}", display_name(path)));
                self.preferences.remember_file(path);
                self.save_preferences();
                if let Err(message) = self.sync_bans() {
                    self.report_problem(message);
                }
            }
            Err(message) => {
                self.show_toast(ToastTone::Error, message);
                self.go_to(Page::Settings, cx);
            }
        }
        cx.notify();
    }

    fn revert_config(&mut self, cx: &mut Context<Self>) {
        let yours = self.form.clone();
        self.form.revert();
        self.refresh_fields(cx);
        // Every change since the last save goes at once, so the way back is offered.
        self.show_undoable(
            "Settings back to those last saved".to_owned(),
            Undo::RestoreSettings(Box::new(yours)),
        );
    }

    /// Shows the password in the settings' field `field` as text, or as dots again.
    fn toggle_password(&mut self, field: &str, cx: &mut Context<Self>) {
        // Streamer mode keeps it hidden; its Show button is disabled then, and says why.
        if self.preferences.streamer_mode {
            return;
        }
        if let Some((_, input)) = self.fields.iter().find(|(key, _)| *key == field) {
            input.update(cx, |input, cx| {
                let masked = input.is_masked();
                input.set_masked(!masked, cx);
            });
        }
    }

    // --- Streamer mode ---

    /// `text` as the screen may show it: with IP addresses, the servers the player joins and
    /// the user's folder hidden in streamer mode.
    fn shown_text(&self, text: &str) -> String {
        if self.preferences.streamer_mode {
            let text = privacy::hide_servers(text, self.known_servers());
            privacy::hide_in_text(&text, self.home.as_deref())
        } else {
            text.to_owned()
        }
    }

    /// The servers the player has joined, as typed: the recent ones and the one now joined.
    fn known_servers(&self) -> impl Iterator<Item = &str> {
        self.preferences
            .servers
            .iter()
            .map(|server| server.address.as_str())
            .chain(std::iter::once(self.play.server()))
    }

    /// A log entry as one line on screen, hidden as streamer mode needs.
    fn entry_text(&self, entry: &LogEntry) -> String {
        self.shown_text(&entry.text).replace('\n', " · ")
    }

    fn shown_address(&self, address: SocketAddr) -> String {
        if self.preferences.streamer_mode {
            privacy::hide_address(address)
        } else {
            address.to_string()
        }
    }

    fn shown_ip(&self, ip: IpAddr) -> String {
        if self.preferences.streamer_mode {
            privacy::hide_ip(ip)
        } else {
            ip.to_string()
        }
    }

    fn shown_path(&self, path: &Path) -> String {
        self.shown_text(&path.display().to_string())
    }

    /// Turns streamer mode on or off, and remembers it.
    fn set_streamer_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        self.preferences.streamer_mode = on;
        // A tooltip already open was written before, and could show what is now hidden.
        dismiss_tooltips(cx);
        self.mask_fields(cx);
        self.save_preferences();
        self.show_toast(
            ToastTone::Success,
            if on {
                "Streamer mode on: IP addresses, server names and your user folder are hidden"
                    .to_owned()
            } else {
                "Streamer mode off".to_owned()
            },
        );
        cx.notify();
    }

    /// Shows the fields that can hold an address or a path as dots in streamer mode: the
    /// listen address, the address to ban, and paths among the settings. The password is
    /// always dots in streamer mode, and goes back to dots when it ends.
    fn mask_fields(&mut self, cx: &mut Context<Self>) {
        let on = self.preferences.streamer_mode;
        if on {
            self.play.hide_password(cx);
        }
        for input in [&self.address, &self.ban_input, self.play.address_input()] {
            input.update(cx, |input, cx| input.set_masked(on, cx));
        }
        for (key, input) in &self.fields {
            let kind = GROUPS
                .iter()
                .flat_map(|group| group.fields)
                .find(|field| field.key == *key)
                .map(|field| field.kind);
            match kind {
                Some(FieldKind::Path) => input.update(cx, |input, cx| input.set_masked(on, cx)),
                Some(FieldKind::Password) => {
                    input.update(cx, |input, cx| input.set_masked(true, cx));
                }
                _ => {}
            }
        }
    }

    /// Asks before discarding unsaved settings.
    fn unless_unsaved(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        question: &str,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        if self.form.is_dirty() {
            let detail = "The settings have changes that are not saved.";
            Self::confirm(window, cx, question, detail, "Discard changes", then);
        } else {
            then(self, window, cx);
        }
    }

    // --- The window ---

    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut losses = Vec::new();
        if self.running.is_some() {
            losses.push("The server will stop, disconnecting everyone playing.");
        } else if self.play.in_session() {
            // The session is the panel's to keep: a window closed by mistake would take the
            // player out of it without a word.
            losses.push("You will leave the session; your game plays on alone.");
        }
        if self.form.is_dirty() {
            losses.push("Unsaved settings will be lost.");
        }
        if self.closing || losses.is_empty() {
            self.remember_window(window);
            return true;
        }
        Self::confirm(
            window,
            cx,
            "Close the panel?",
            &losses.join(" "),
            "Close",
            |this, window, cx| {
                this.closing = true;
                this.close_router_port();
                this.stop_now(cx);
                this.remember_window(window);
                window.remove_window();
            },
        );
        false
    }

    /// Saves where the window is and its size, to open there next time.
    fn remember_window(&mut self, window: &Window) {
        let (maximized, bounds) = match window.window_bounds() {
            WindowBounds::Windowed(bounds) => (false, bounds),
            // Full screen reopens maximized: the panel offers no full screen of its own.
            WindowBounds::Maximized(bounds) | WindowBounds::Fullscreen(bounds) => (true, bounds),
        };
        self.preferences.window = Some(WindowPlacement {
            maximized,
            x: f32::from(bounds.origin.x),
            y: f32::from(bounds.origin.y),
            width: f32::from(bounds.size.width),
            height: f32::from(bounds.size.height),
        });
        self.save_preferences();
    }

    /// Shows a native question; `then` runs if the user picks `confirm`.
    fn confirm(
        window: &mut Window,
        cx: &mut Context<Self>,
        question: &str,
        detail: &str,
        confirm: &str,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        before_dialog(window);
        let answer = window.prompt(
            PromptLevel::Warning,
            question,
            Some(detail),
            &[confirm, "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update_in(cx, then);
            }
        })
        .detach();
    }

    fn show_toast(&mut self, tone: ToastTone, text: String) {
        self.show_message(tone, text, None);
    }

    /// Reports what someone else did, as a moderator in the game: in the list behind the
    /// bell at once, and as a message unless one with an Undo is up, which it would take
    /// away for good. What the user does themself replaces that message, and so retires an
    /// Undo that no longer fits what they have done since.
    fn report(&mut self, text: String) {
        let holds_undo = self
            .toast
            .as_ref()
            .is_some_and(|shown| shown.undo.is_some() && shown.leaving.is_none());
        if holds_undo {
            self.history.remember(ToastTone::Success, &text);
        } else {
            self.show_toast(ToastTone::Success, text);
        }
    }

    /// Reports an action with an Undo button, shown longer so there is time to use it.
    fn show_undoable(&mut self, text: String, undo: Undo) {
        self.show_message(ToastTone::Success, text, Some(undo));
    }

    fn show_message(&mut self, tone: ToastTone, text: String, undo: Option<Undo>) {
        let serial = self.history.remember(tone, &text);
        self.toast = Some(Toast {
            tone,
            text,
            shown: Instant::now(),
            undo,
            serial,
            leaving: None,
        });
    }

    /// Reverses the action the message reports, and closes it.
    fn undo(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(undo) = self.toast.take().and_then(|toast| toast.undo) else {
            return;
        };
        match undo {
            Undo::Unban(ip) => self.unban(ip, cx),
            Undo::Reban(ip) => self.ban_now(ip),
            Undo::RestoreLog(log) => {
                self.dashboard.restore_log(log);
                self.show_toast(ToastTone::Success, "Log restored".to_owned());
            }
            Undo::RestorePanel(at, panel) => self.restore_panel(at, panel),
            Undo::RestoreLayout(layout) => self.restore_layout(layout),
            Undo::RestorePreferences(preferences) => {
                self.preferences = *preferences;
                self.apply_preferences(window, cx);
                self.save_preferences();
            }
            Undo::RestoreSettings(form) => self.show_form(*form, cx),
        }
        cx.notify();
    }

    /// Everything on the Preferences page back to its default, offering to put it back.
    fn reset_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn apply_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    fn remember_views(&mut self) {
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
    fn set_remember_views(&mut self, on: bool, cx: &mut Context<Self>) {
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

    /// When an entry happened, on the user's clock and in their format.
    fn entry_time(&self, entry: &LogEntry) -> String {
        format::time_of_day(self.dashboard.time_of_day(entry), self.hour24)
    }

    /// Writes times of day on the 24-hour clock or the 12-hour one, and remembers it.
    fn set_clock(&mut self, hour24: bool, cx: &mut Context<Self>) {
        self.hour24 = hour24;
        self.preferences.clock_24h = Some(hour24);
        self.save_preferences();
        cx.notify();
    }

    /// Empties the log, offering to put it back.
    fn clear_log(&mut self, cx: &mut Context<Self>) {
        let log = self.dashboard.take_log();
        let count = log.len();
        let text = if count == 1 {
            "Cleared 1 entry".to_owned()
        } else {
            format!("Cleared {count} entries")
        };
        self.show_undoable(text, Undo::RestoreLog(log));
        cx.notify();
    }

    /// Logs a problem and shows it, alerting the user if the panel is in the background.
    fn report_problem(&mut self, message: String) {
        self.dashboard
            .note(Kind::Problem, message.clone(), Instant::now());
        self.show_toast(ToastTone::Error, message);
        self.alert_due = true;
    }

    /// Draws everything at `zoom` from now on, and remembers it for the next run.
    fn set_zoom(&mut self, zoom: Zoom, window: &mut Window, cx: &mut Context<Self>) {
        if zoom == self.preferences.zoom {
            return;
        }
        self.preferences.zoom = zoom;
        window.set_rem_size(px(zoom.rem_size()));
        self.save_preferences();
        cx.notify();
    }

    /// Colors everything in `theme` from now on, and remembers it for the next run.
    fn set_theme(&mut self, theme: ThemeName, window: &mut Window, cx: &mut Context<Self>) {
        if theme == self.preferences.theme {
            return;
        }
        self.preferences.theme = theme;
        self.apply_theme(window, cx);
        self.save_preferences();
    }

    /// Draws light, dark, or as Windows is set, from now on, and remembers it.
    fn set_mode(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        if mode == self.preferences.mode {
            return;
        }
        self.preferences.mode = mode;
        self.apply_theme(window, cx);
        self.save_preferences();
    }

    /// Colors the panel as the preferences and Windows' settings now say.
    fn apply_theme(&mut self, window: &Window, cx: &mut Context<Self>) {
        let preferences = &self.preferences;
        cx.set_global(Theme::for_user(
            preferences.theme,
            preferences.mode,
            window.appearance(),
            self.high_contrast,
        ));
        cx.notify();
    }

    /// One step of interface size per wheel notch, or per 50 pixels of a touchpad's smooth
    /// scrolling; up for larger, as browsers do.
    fn wheel_zoom(&mut self, delta: ScrollDelta, window: &mut Window, cx: &mut Context<Self>) {
        self.wheel_zoom += match delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => f32::from(pixels.y) / 50.0,
        };
        let zoom = self.preferences.zoom;
        if self.wheel_zoom >= 1.0 {
            self.wheel_zoom = 0.0;
            self.set_zoom(zoom.larger(), window, cx);
        } else if self.wheel_zoom <= -1.0 {
            self.wheel_zoom = 0.0;
            self.set_zoom(zoom.smaller(), window, cx);
        }
    }

    fn save_preferences(&mut self) {
        if self.preferences_unread {
            return;
        }
        if let Some(path) = preferences::path()
            && let Err(message) = self.preferences.save(&path)
        {
            self.report_problem(message);
        }
    }

    /// Shows `page`; choosing the page already shown goes back to its start, as from a
    /// player's details to the table.
    fn go_to(&mut self, page: Page, cx: &mut Context<Self>) {
        if page == self.page {
            self.selected = None;
        }
        // Customize belongs to the Overview; leaving it is done arranging.
        if page != Page::Overview {
            self.arranging.on = false;
        }
        if page != self.page {
            // Arranging is of the page in view; leaving it is done arranging.
            self.page_arranging = None;
            crate::crash::note("panel", &format!("Showed {}", page.title()));
        }
        self.page = page;
        // What the Play page lists as changed in the emulator is what a join would change,
        // which a preference set on another page meanwhile, as the chat's key, alters.
        if page == Page::Play {
            self.refresh_emulator_changes();
        }
        self.share_state();
        cx.notify();
    }
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        let page = match self.page {
            Page::Play => self.render_play(window, cx),
            Page::Overview => self.render_overview(window, cx),
            Page::Players => self.render_players(window, cx),
            Page::Bans => self.render_bans(cx),
            Page::Settings => self.render_settings(window, cx),
            Page::Log => self.render_log(cx),
            Page::Preferences => self.render_preferences(cx),
        };
        self.update_title(window);
        // A click that removes the focused element, as Start does by becoming Stop, leaves
        // nothing focused, and keys then reach nothing: the panel takes them back.
        if window.focused(cx).is_none() {
            window.focus(&self.focus);
        }
        div()
            .relative()
            .flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .font_family(FONT)
            .text_size(scaled(14.))
            .track_focus(&self.focus)
            .key_context("Panel")
            .on_action(cx.listener(Self::new_config))
            .on_action(cx.listener(Self::open_config))
            .on_action(cx.listener(Self::save_config))
            .on_action(cx.listener(Self::save_config_as))
            .on_action(cx.listener(|_, _: &FocusNext, window, _| window.focus_next()))
            .on_action(cx.listener(|_, _: &FocusPrevious, window, _| window.focus_prev()))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| {
                this.set_zoom(this.preferences.zoom.larger(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ZoomOut, window, cx| {
                this.set_zoom(this.preferences.zoom.smaller(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &ResetZoom, window, cx| {
                this.set_zoom(Zoom::default(), window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| {
                // Whatever is over the page closes first.
                if this.history.open {
                    this.close_history(cx);
                } else if this.page == Page::Overview && this.arranging.on {
                    this.set_arranging(false, cx);
                } else if (this.page == Page::Players && this.selected.take().is_some())
                    || this.page_arranging.take().is_some()
                {
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleStreamerMode, _, cx| {
                let on = !this.preferences.streamer_mode;
                this.set_streamer_mode(on, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                let collapsed = !this.preferences.sidebar_collapsed;
                this.set_sidebar_collapsed(collapsed, cx);
            }))
            .on_action(cx.listener(|this, _: &ShowPlay, _, cx| this.go_to(Page::Play, cx)))
            .on_action(cx.listener(|this, _: &ShowOverview, _, cx| this.go_to(Page::Overview, cx)))
            .on_action(cx.listener(|this, _: &ShowPlayers, _, cx| this.go_to(Page::Players, cx)))
            .on_action(cx.listener(|this, _: &ShowBans, _, cx| this.go_to(Page::Bans, cx)))
            .on_action(cx.listener(|this, _: &ShowSettings, _, cx| this.go_to(Page::Settings, cx)))
            .on_action(cx.listener(|this, _: &ShowLog, _, cx| this.go_to(Page::Log, cx)))
            .on_action(cx.listener(|this, _: &ShowPreferences, _, cx| {
                this.go_to(Page::Preferences, cx);
            }))
            .on_action(cx.listener(|this, _: &FindInLog, window, cx| {
                this.go_to(Page::Log, cx);
                window.focus(&this.log_search.focus_handle(cx));
            }))
            .child(self.window_listeners(cx))
            .child(self.render_sidebar(window, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .children(self.render_crash_notice(&theme, cx))
                    .child(page),
            )
            .children(self.render_toast(cx))
            .children(self.render_history(cx))
    }
}

impl Panel {
    /// Puts the server's state in the window's title, where the taskbar and Alt+Tab show it.
    fn update_title(&mut self, window: &mut Window) {
        let state = self
            .running
            .as_ref()
            .map(|_| (self.dashboard.player_count(), self.needs_restart()));
        // By name, as the sidebar says it, once the server has listed the player.
        let playing = match &self.play.state {
            play_page::PlayState::Playing { player, .. } => Some(self.play.called(*player)),
            _ => None,
        };
        let title = window_title(state, playing.as_deref());
        if title != self.shown_title {
            window.set_window_title(&title);
            self.shown_title = title;
        }
    }

    /// Listeners for the whole window, registered as it paints. Ctrl+wheel resizes the
    /// interface; listening before anything else does (the capture phase) keeps the page under
    /// the mouse from scrolling as well. A drag of the sidebar's edge, or of a table column's,
    /// goes on wherever the mouse goes, with the resize cursor, until the button is let go.
    fn window_listeners(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let panel = cx.entity().downgrade();
        let resizing = self.sidebar_drag.is_some() || self.column_drag.is_some();
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                follow_tooltips(window);
                track_mouse_navigation(window);
                if resizing {
                    window.set_window_cursor_style(CursorStyle::ResizeLeftRight);
                    let moved = panel.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            // Each follows only a drag of its own.
                            let _ = moved.update(cx, |this, cx| {
                                this.drag_sidebar(event, window, cx);
                                this.drag_column(event, window, cx);
                            });
                        }
                    });
                    let released = panel.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = released.update(cx, |this, cx| {
                                this.finish_sidebar_drag(cx);
                                this.finish_column_drag(cx);
                            });
                        }
                    });
                }
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && event.modifiers.secondary() {
                        cx.stop_propagation();
                        let _ = panel.update(cx, |this, cx| {
                            this.wheel_zoom(event.delta, window, cx);
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full()
    }
}

/// A command typed into the server chat, with a player it names by name given by number
/// instead, which is how the server's console takes one: `/kick Luigi` as `/kick 3`. The
/// players' own chat takes either, so the server owner's must, or the same line would work in
/// one chat and not the other. Anything else is as typed.
///
/// Fails, with a line for whoever typed it, if nobody playing has the name.
fn by_number<'a>(
    typed: &str,
    mut players: impl Iterator<Item = (PlayerId, &'a str)>,
) -> Result<String, String> {
    let line = typed.trim();
    let (command, who) = line.split_once(' ').unwrap_or((line, ""));
    let who = who.trim();
    let takes_a_player = command
        .strip_prefix('/')
        .is_some_and(b3313_server::commands::names_a_player);
    // A number is a player's already, and an address is one to ban.
    let is_named =
        !who.is_empty() && who.parse::<u8>().is_err() && who.parse::<std::net::IpAddr>().is_err();
    if !(takes_a_player && is_named) {
        return Ok(line.to_owned());
    }
    players
        .find(|(_, name)| name.eq_ignore_ascii_case(who))
        .map(|(player, _)| format!("{command} {}", player.get()))
        .ok_or_else(|| format!("Nobody playing is called {who}."))
}

/// The window's title: the server's state first, as the taskbar cuts titles short. `state` is
/// the players in the session and whether a restart is waiting, while it runs; `playing`, who
/// this PC joined another's server as, said when it hosts nothing.
fn window_title(state: Option<(usize, bool)>, playing: Option<&str>) -> String {
    const NAME: &str = "B3313 Extended Multiplayer";
    match state {
        None => match playing {
            Some(player) => format!("Playing as {player} — {NAME}"),
            None => format!("Stopped — {NAME}"),
        },
        Some((players, restart)) => {
            let players = match players {
                0 => "nobody playing".to_owned(),
                1 => "1 player".to_owned(),
                players => format!("{players} players"),
            };
            let restart = if restart { " · restart to apply" } else { "" };
            format!("Running · {players}{restart} — {NAME}")
        }
    }
}

/// A page's title and actions above, its content scrolling beneath.
fn page_frame(
    theme: &Theme,
    title: &str,
    subtitle: String,
    actions: Vec<AnyElement>,
    body: impl IntoElement,
) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .size_full()
        .child(
            div()
                .flex()
                .items_center()
                .gap_4()
                .px_8()
                .pt_6()
                .pb_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(
                            div()
                                .text_size(scaled(22.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title.to_owned()),
                        )
                        .child(
                            div()
                                .text_size(scaled(13.))
                                .text_color(theme.text_muted)
                                .child(subtitle),
                        ),
                )
                .child(div().flex().gap_2().children(actions)),
        )
        .child(
            div()
                .id(SharedString::from(format!("{title}-body")))
                .flex_1()
                .overflow_y_scroll()
                .px_8()
                .pb_8()
                .child(body),
        )
        .into_any_element()
}

/// Takes focus from the button that opens a native dialog. Otherwise Enter, which confirms the
/// dialog as the key goes down, reaches that button as it comes up, and presses it again.
fn before_dialog(window: &mut Window) {
    window.blur();
}

/// A file's name, for showing where the full path would not fit.
fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// The Start, Restart and Stop buttons, shared by the pages that control the server.
fn server_buttons(panel: &Panel, cx: &mut Context<Panel>) -> Vec<AnyElement> {
    if panel.running.is_some() {
        vec![
            Button::new("restart", "Restart")
                .icon(IconName::RotateCw)
                .enabled(panel.field_errors.is_empty())
                .tooltip(if panel.field_errors.is_empty() {
                    "Stops and starts the server, with the settings as they are now"
                } else {
                    "Fix the settings marked in red on Server settings first"
                })
                .on_click(cx.listener(|this, _, window, cx| this.restart(window, cx)))
                .into_any_element(),
            Button::new("stop", "Stop server")
                .icon(IconName::Square)
                .tone(Tone::Danger)
                .tooltip("Ends the session: everyone in it is told, and can join the next one")
                .on_click(cx.listener(|this, _, window, cx| this.stop(window, cx)))
                .into_any_element(),
        ]
    } else {
        vec![
            Button::new("start", "Start server")
                .icon(IconName::Play)
                .tone(Tone::Primary)
                .tooltip("Starts serving on the listen address (Enter in that field)")
                .on_click(cx.listener(|this, _, window, cx| this.start(window, cx)))
                .into_any_element(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{by_number, window_title};
    use b3313_protocol::PlayerId;

    #[test]
    fn the_server_chat_takes_a_player_by_name_as_the_players_chat_does() {
        let luigi = PlayerId::new(3);
        let players = || luigi.map(|id| (id, "Luigi")).into_iter();

        assert_eq!(
            by_number("/kick luigi", players()),
            Ok("/kick 3".to_owned())
        );
        assert_eq!(
            by_number(" /mod  Luigi ", players()),
            Ok("/mod 3".to_owned())
        );
        assert_eq!(
            by_number("/host Luigi", players()),
            Ok("/host 3".to_owned())
        );
        // A number, an address and a command that names nobody are as typed.
        assert_eq!(by_number("/kick 5", players()), Ok("/kick 5".to_owned()));
        assert_eq!(
            by_number("/ban 192.168.1.20", players()),
            Ok("/ban 192.168.1.20".to_owned())
        );
        assert_eq!(
            by_number("/say Luigi", players()),
            Ok("/say Luigi".to_owned())
        );
        assert_eq!(by_number("/end", players()), Ok("/end".to_owned()));
        assert!(by_number("/kick Peach", players()).is_err_and(|why| why.contains("Peach")));
    }

    #[test]
    fn the_title_says_the_servers_state_first() {
        assert_eq!(
            window_title(None, None),
            "Stopped — B3313 Extended Multiplayer"
        );
        assert_eq!(
            window_title(Some((0, false)), None),
            "Running · nobody playing — B3313 Extended Multiplayer"
        );
        assert_eq!(
            window_title(Some((1, false)), Some("Luigi")),
            "Running · 1 player — B3313 Extended Multiplayer"
        );
        assert_eq!(
            window_title(Some((3, true)), None),
            "Running · 3 players · restart to apply — B3313 Extended Multiplayer"
        );
        // Joined to someone else's server, hosting nothing.
        assert_eq!(
            window_title(None, Some("Luigi")),
            "Playing as Luigi — B3313 Extended Multiplayer"
        );
    }
}
