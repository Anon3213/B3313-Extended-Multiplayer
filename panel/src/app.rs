//! The panel's window: a sidebar of pages, the server it hosts, and the settings being edited.

mod arrange;
mod bans_page;
mod chat;
mod column_resize;
mod crash_notice;
mod log_page;
mod messages;
mod moderation;
mod overview;
mod overview_panels;
mod page_arrange;
mod play_page;
mod players;
mod preferences_apply;
mod preferences_behavior;
mod preferences_page;
mod server;
mod settings_files;
mod settings_page;
mod sidebar;
mod streamer;

use crate::alert;
use crate::dashboard::{Dashboard, Kind, LogColumn, LogEntry, PlayerColumn};
use crate::format;
use crate::icons::IconName;
use crate::page_layout::ArrangedPage;
use crate::preferences::{Preferences, StartPage, WindowPlacement};
use crate::settings::SettingsForm;
use crate::sort::{Sort, SortOrder};
use crate::system;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{FONT, Theme, scaled};
use crate::widgets::{follow_tooltips, track_mouse_navigation};
use crate::zoom::Zoom;
use b3313_protocol::PlayerId;
use b3313_server::bans::BanList;
use gpui::{
    AnyElement, App, Context, CursorStyle, DispatchPhase, Entity, FocusHandle, Focusable,
    FontWeight, KeyBinding, MouseMoveEvent, MouseUpEvent, PromptLevel, ScrollDelta, ScrollHandle,
    ScrollWheelEvent, SharedString, Subscription, Task, Window, WindowBounds, actions, canvas, div,
    prelude::*, px,
};
use messages::{Toast, Undo};
use server::{Pending, Running};
use std::collections::BTreeMap;
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
    /// The name invites give for the server in place of its address; see
    /// `Preferences::invite_name`.
    invite_name: Entity<TextInput>,
    running: Option<Running>,
    pending: Pending,
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
    log_list: log_page::LogList,
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
        let invite_name = Self::invite_name_input(&preferences, &mut subscriptions, cx);
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
            invite_name,
            running: None,
            pending: Pending::default(),
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
            log_list: log_page::LogList::default(),
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

    /// The field for the name invites give for the server, filled from `preferences` and kept
    /// there as it is typed: the invite beside it is written from it at once.
    fn invite_name_input(
        preferences: &Preferences,
        subscriptions: &mut Vec<Subscription>,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        let name = preferences.invite_name.as_deref().unwrap_or_default();
        let input = cx.new(|cx| TextInput::new(cx, name, "Such as myserver.duckdns.org"));
        subscriptions.push(cx.subscribe(&input, |this, input, event, cx| {
            if *event == InputEvent::Changed {
                let name = input.read(cx).text().trim().to_owned();
                this.preferences.invite_name = (!name.is_empty()).then_some(name);
                this.save_preferences();
                cx.notify();
            }
        }));
        input
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

    /// When an entry happened, on the user's clock and in their format.
    fn entry_time(&self, entry: &LogEntry) -> String {
        format::time_of_day(self.dashboard.time_of_day(entry), self.hour24)
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
        // The Overview's Game panel shows the same list.
        if matches!(page, Page::Play | Page::Overview) {
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

#[cfg(test)]
mod tests {
    use super::window_title;

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
