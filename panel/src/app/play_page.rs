//! Joining a session as a player: the server from an invite, this PC's game, and how the
//! session goes once joined. The bridge runs on a thread of its own (`join.rs`).

use super::chat::{Chat, ChatBox, ChatEntry, ChatLog};
use super::overview::{hint, label};
use super::{Page, Panel, ToastTone, before_dialog, display_name, page_frame};
use crate::emulator::{Change, Install};
use crate::format::{self, byte_rate, clock, millis};
use crate::game_setup;
use crate::icons::{IconName, icon};
use crate::join::{Bridge, Notice, Request};
use crate::network::read_invite;
use crate::page_layout::ArrangedPage;
use crate::preferences::SavedServer;
use crate::system;
use crate::text_input::{InputEvent, TextInput};
use crate::theme::{MONO, Theme, scaled};
use crate::widgets::{
    Button, FocusRing as _, OneLine as _, Tip, Tone, card, card_header, dot, switch,
};
use b3313_bridge::process::{EMULATOR, find_processes};
use b3313_bridge::session::Event;
use b3313_protocol::metrics::LinkReport;
use b3313_protocol::{HitKind, Outfit, PlayerId, Role, RosterEntry};
use gpui::{
    AnyElement, Context, Div, Entity, FontWeight, PathPromptOptions, SharedString, Subscription,
    Window, div, prelude::*,
};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How often the page looks for the emulator while not joined.
const LOOK_INTERVAL: Duration = Duration::from_secs(2);
/// Things that happened in the session, listed newest first.
const EVENTS: usize = 8;
/// Where the game comes from and what runs it, under the Game heading here and on the Overview.
pub(super) const GAME_MEANING: &str = "Built once from your own Super Mario 64 ROM and a B3313 \
                                       patch, and run in Parallel Launcher's emulator.";

/// A file the Game card asks the player for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GameFile {
    Sm64Rom,
    B3313Patch,
    /// A multiplayer ROM the player built themself, in place of the one made here.
    OwnRom,
}

/// Where the session is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PlayState {
    Idle,
    WaitingForGame,
    Joining,
    Playing {
        player: PlayerId,
        since: Instant,
    },
    /// The server went silent or is starting again; the bridge is joining it again, and
    /// gives up at `until`.
    Reconnecting {
        until: Instant,
    },
    /// Ended, saying why; `left` when the player chose to. `detail` is what the system said of
    /// it, shown on hover.
    Ended {
        reason: String,
        left: bool,
        detail: Option<String>,
    },
}

/// The Play page's own state.
pub(super) struct Play {
    address: Entity<TextInput>,
    password: Entity<TextInput>,
    show_password: bool,
    remember_password: bool,
    /// The emulator's processes, as last looked, and when.
    emulators: Vec<u32>,
    looked: Option<Instant>,
    /// The one chosen, when there are several.
    chosen: Option<u32>,
    /// Parallel Launcher's emulator, if it is installed, so Join can start the game itself.
    launcher: Option<Install>,
    /// What starting the game changes in that emulator, while the player has asked to see
    /// it: read from their settings as they asked, or why it could not be.
    changes: Option<Result<Vec<Change>, String>>,
    /// Whether the multiplayer ROM is being built from the player's files right now.
    building: bool,
    bridge: Option<Bridge>,
    pub(super) state: PlayState,
    /// The second of the session's clock that was last drawn: see `Panel::poll_play`.
    shown_second: u64,
    /// The server joined, as typed.
    server: String,
    link: Option<Box<LinkReport>>,
    /// What happened, with its time of day, newest first.
    events: VecDeque<(Duration, String)>,
    /// The name other players see.
    name: Entity<TextInput>,
    /// What the player is typing for the chat.
    say: Entity<TextInput>,
    /// The chat since joining.
    pub(super) chat: ChatLog,
    /// Everyone in the session, as the server last listed them.
    roster: Vec<RosterEntry>,
    _subscriptions: Vec<Subscription>,
}

impl std::fmt::Debug for Play {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Play")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Play {
    /// The page as the panel opens, the last server filled in.
    pub(super) fn new(last: Option<&SavedServer>, name: &str, cx: &mut Context<Panel>) -> Self {
        let address = last.map_or("", |server| server.address.as_str());
        let password = last.and_then(|server| server.password.as_deref());
        let address = cx.new(|cx| TextInput::new(cx, address, "Such as 203.0.113.7:41313"));
        let password = cx.new(|cx| {
            let mut input =
                TextInput::new(cx, password.unwrap_or_default(), "If the server has one");
            input.set_masked(true, cx);
            input
        });
        let name = cx.new(|cx| TextInput::new(cx, name, "Up to 16 letters; others see it"));
        let say = cx.new(|cx| TextInput::new(cx, "", "Say something, or /help"));
        // Enter in the server's and the name's fields joins, and in the chat's sends.
        let mut subscriptions: Vec<Subscription> = [&address, &password, &name]
            .into_iter()
            .map(|input| {
                cx.subscribe(input, |this: &mut Panel, _, event, cx| {
                    if *event == InputEvent::Submitted {
                        this.join(cx);
                    }
                })
            })
            .collect();
        subscriptions.push(cx.subscribe(&say, |this: &mut Panel, _, event, cx| {
            if *event == InputEvent::Submitted {
                this.send_chat(cx);
            }
        }));
        Self {
            address,
            password,
            show_password: false,
            remember_password: last.is_some_and(|server| server.password.is_some()),
            emulators: Vec::new(),
            looked: None,
            chosen: None,
            launcher: None,
            changes: None,
            building: false,
            bridge: None,
            state: PlayState::Idle,
            shown_second: 0,
            server: String::new(),
            link: None,
            events: VecDeque::new(),
            name,
            say,
            chat: ChatLog::default(),
            roster: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    /// The name and role the server listed `player` under, if it has.
    fn listed(&self, player: PlayerId) -> Option<&RosterEntry> {
        self.roster.iter().find(|entry| entry.player == player)
    }

    fn hear(&mut self, from: Option<PlayerId>, name: String, text: String) {
        crate::crash::note("chat", &crate::crash::chat_note(&text));
        let speaker = from.and_then(|from| self.listed(from));
        let role = speaker.map(|listed| listed.role);
        self.chat.push(ChatEntry {
            at: system::local_time_of_day(),
            from,
            role,
            name,
            text,
        });
    }

    /// The emulator to join with: the chosen one, or the only one running.
    fn emulator(&self) -> Option<u32> {
        match self.emulators.as_slice() {
            [only] => Some(*only),
            _ => self.chosen.filter(|pid| self.emulators.contains(pid)),
        }
    }

    /// What `player` is called: the name the server listed them under, or their number until
    /// it has.
    pub(super) fn called(&self, player: PlayerId) -> String {
        self.listed(player).map_or_else(
            || format!("player {}", player.get()),
            |entry| entry.name.as_str().to_owned(),
        )
    }

    /// Whether a session is under way, joined or still joining.
    pub(super) const fn in_session(&self) -> bool {
        self.bridge.is_some()
    }

    /// The server joined, or last joined, as typed.
    pub(super) fn server(&self) -> &str {
        &self.server
    }

    /// The field that holds an address, for streamer mode to hide.
    pub(super) fn address_input(&self) -> &Entity<TextInput> {
        &self.address
    }

    /// Shows the password as dots again, as streamer mode keeps it.
    pub(super) fn hide_password(&mut self, cx: &mut Context<Panel>) {
        self.show_password = false;
        self.password
            .update(cx, |input, cx| input.set_masked(true, cx));
    }

    fn note(&mut self, text: String) {
        crate::crash::note("play", &text);
        self.events.push_front((system::local_time_of_day(), text));
        self.events.truncate(EVENTS);
    }
}

impl Panel {
    /// Takes in what the session reported, and looks for the emulator now and then while not
    /// joined. Whether anything changed, to draw again.
    pub(super) fn poll_play(&mut self) -> bool {
        let mut changed = false;
        let notices = self
            .play
            .bridge
            .as_ref()
            .map(Bridge::drain)
            .unwrap_or_default();
        for notice in notices {
            changed = true;
            self.play_notice(notice);
        }
        let due = self
            .play
            .looked
            .is_none_or(|looked| looked.elapsed() >= LOOK_INTERVAL);
        // Only for the pages that show it: the look lists every process on the PC.
        let shows_emulator = matches!(self.page, Page::Play | Page::Overview);
        if self.play.bridge.is_none() && due && shows_emulator {
            self.play.looked = Some(Instant::now());
            let found = find_processes(EMULATOR).unwrap_or_default();
            let launcher = Install::find();
            if found != self.play.emulators || launcher != self.play.launcher {
                self.play.emulators = found;
                self.play.launcher = launcher;
                changed = true;
            }
        }
        // The time played counts up, and the time left to join again in counts down, on the
        // page and in the sidebar: drawn again as the second turns, and not at every look.
        let second = match self.play.state {
            PlayState::Playing { since, .. } => since.elapsed().as_secs(),
            PlayState::Reconnecting { until } => {
                until.saturating_duration_since(Instant::now()).as_secs()
            }
            _ => 0,
        };
        changed |= std::mem::replace(&mut self.play.shown_second, second) != second;
        changed
    }

    fn play_notice(&mut self, notice: Notice) {
        // What a session reports after it was asked to leave, and before it has, is no
        // longer the player's: it would show them joined again, and then its end as a fault.
        let leaving = matches!(self.play.state, PlayState::Ended { left: true, .. });
        if leaving && matches!(notice, Notice::Event(_)) {
            return;
        }
        match notice {
            Notice::SaveNotCopied => self.play.note(
                "No save of this game was found to copy aside. A game not yet played has \
                 none; one you started yourself may keep its save elsewhere"
                    .to_owned(),
            ),
            Notice::Event(Event::WaitingForGame) => self.play.state = PlayState::WaitingForGame,
            Notice::Event(Event::Joining) => self.play.state = PlayState::Joining,
            Notice::Event(Event::Reconnecting(limit)) => {
                self.play.state = PlayState::Reconnecting {
                    until: Instant::now() + limit,
                };
                self.play.link = None;
                // Who was there is not who will be: the server may be a new one, and this
                // player's number another, which an old list would name someone else by.
                self.play.roster.clear();
                self.play
                    .note("Lost the server; joining it again".to_owned());
                // Worth looking up for, from the game or another page.
                self.alert_due = true;
            }
            Notice::Event(Event::Joined(player)) => {
                self.play.state = PlayState::Playing {
                    player,
                    since: Instant::now(),
                };
                let text = format!("Joined {} as player {}", self.play.server, player.get());
                self.play.note(text);
            }
            Notice::Event(Event::WrongRom(found)) => {
                let text = b3313_bridge::session::describe_wrong_rom(found);
                self.play.note(text.clone());
                self.report_problem(text);
            }
            Notice::Event(Event::Hit(target, kind)) => {
                let who = self.play.called(target);
                self.play.note(format!("You {} {who}", hit_word(kind)));
            }
            Notice::Event(Event::HitBy(attacker, kind)) => {
                let who = self.play.called(attacker);
                self.play.note(format!("{who} {} you", hit_word(kind)));
            }
            Notice::Event(Event::Link(report)) => self.play.link = Some(report),
            Notice::Event(Event::Roster(roster)) => {
                self.play.roster = roster;
                // What the server says this player wears is what the picker shows and what
                // is brought to the next session, however it was chosen: here, or by /color.
                let worn = match self.play.state {
                    PlayState::Playing { player, .. } => self.play.listed(player),
                    _ => None,
                }
                .map(|entry| entry.outfit);
                if worn.is_some() && worn != self.preferences.player_outfit {
                    self.preferences.player_outfit = worn;
                    self.save_preferences();
                }
            }
            Notice::Event(Event::Said { from, name, text }) => self.play.hear(from, name, text),
            Notice::Ended { reason, detail } => {
                let left = matches!(self.play.state, PlayState::Ended { left: true, .. });
                // The thread has finished; this only collects it.
                drop(self.play.bridge.take());
                self.play.link = None;
                self.play.roster.clear();
                self.play.note(reason.clone());
                if !left {
                    self.alert_due = true;
                    self.show_toast(ToastTone::Error, reason.clone());
                }
                if let Some(detail) = &detail {
                    crate::crash::note("play", detail);
                }
                self.play.state = PlayState::Ended {
                    reason,
                    left,
                    detail,
                };
            }
        }
    }

    /// Why Join cannot go ahead yet, if it cannot.
    fn join_blocker(&self, cx: &Context<Self>) -> Option<&'static str> {
        if self.play.bridge.is_some() {
            let leaving = matches!(self.play.state, PlayState::Ended { left: true, .. });
            Some(if leaving {
                "Still leaving the last session"
            } else {
                "Already joined"
            })
        } else if self.play.building {
            Some("The game is still being built")
        } else if self.preferences.rom.is_none() {
            Some("Set up the game first: choose your Super Mario 64 ROM and a B3313 patch")
        } else if self.play.emulators.is_empty() && self.play.launcher.is_none() {
            Some("Install Parallel Launcher, or start the multiplayer ROM yourself")
        } else if !self.play.emulators.is_empty() && self.play.emulator().is_none() {
            Some("Choose which RetroArch to join with")
        } else if self.play.address.read(cx).text().trim().is_empty() {
            Some("Paste an invite, or type the server's address")
        } else {
            None
        }
    }

    /// Joins the server in the fields with the chosen ROM and emulator, remembering the server.
    fn join(&mut self, cx: &mut Context<Self>) {
        if let Some(blocker) = self.join_blocker(cx) {
            self.show_toast(ToastTone::Error, blocker.to_owned());
            cx.notify();
            return;
        }
        let Some(rom) = self.preferences.rom.clone() else {
            return;
        };
        // With no emulator running, the session starts the game itself.
        let pid = self.play.emulator();
        if pid.is_none()
            && let Some(Ok(changes)) = self.emulator_changes()
        {
            // On the record, as they are the player's settings; the Game panel lists them.
            self.play.note(format!(
                "Starting the game with {} changes to the emulator's settings, for this run only",
                changes.len()
            ));
        }
        let sources = self
            .preferences
            .sm64_rom
            .clone()
            .zip(self.preferences.b3313_patch.clone());
        let server = self.play.address.read(cx).text().trim().to_owned();
        let password = self.play.password.read(cx).text().to_owned();
        let kept = self.play.remember_password.then_some(password.as_str());
        self.preferences.remember_server(&server, kept);
        let name = self.play.name.read(cx).text().trim().to_owned();
        self.preferences.player_name.clone_from(&name);
        self.save_preferences();
        let hosting = self.running.as_ref().map(|running| {
            let host = &running.host;
            (host.address(), host.host_password().to_owned())
        });
        let request = Request {
            rom,
            sources,
            pid,
            outfit: self.preferences.player_outfit,
            chat_key: self.preferences.chat_key,
            chat_seconds: self.preferences.chat_seconds,
            server: server.clone(),
            password,
            name,
            hosting,
        };
        match Bridge::start(request) {
            Ok(bridge) => {
                self.play.bridge = Some(bridge);
                self.play.server = server;
                self.play.state = PlayState::Joining;
                self.play.chat.clear();
                // The fields stay editable: they apply to the next Join, and this session
                // keeps the server it joined.
                self.play.link = None;
            }
            Err(message) => self.report_problem(message),
        }
        cx.notify();
    }

    /// Sends what is typed in the chat's field, if the session is joined, and empties the field.
    /// The line shows once the server sends it back, as everyone else's do.
    fn send_chat(&mut self, cx: &mut Context<Self>) {
        let typed = self.play.say.read(cx).text().trim().to_owned();
        let Some(bridge) = self.play.bridge.as_ref().filter(|_| !typed.is_empty()) else {
            return;
        };
        bridge.say(typed);
        self.play.say.update(cx, |input, cx| input.set_text("", cx));
        cx.notify();
    }

    /// Leaves the session; the bridge reports the end, as any other.
    fn leave_session(&mut self, cx: &mut Context<Self>) {
        if let Some(bridge) = &self.play.bridge {
            // Asked, and not waited for here: a session still being joined may be building
            // the game or looking the server's name up, and the window would stand still
            // until it had. Its end comes as any other's, and is taken then.
            bridge.ask_to_leave();
            self.play.state = PlayState::Ended {
                reason: String::new(),
                left: true,
                detail: None,
            };
        }
        cx.notify();
    }

    /// Fills the fields from an invite on the clipboard.
    fn paste_invite(&mut self, cx: &mut Context<Self>) {
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        let Some(invite) = read_invite(&text) else {
            self.show_toast(
                ToastTone::Error,
                "The clipboard holds no invite or server address".to_owned(),
            );
            cx.notify();
            return;
        };
        let password = invite.password.unwrap_or_default();
        self.play
            .address
            .update(cx, |input, cx| input.set_text(&invite.address, cx));
        self.play
            .password
            .update(cx, |input, cx| input.set_text(&password, cx));
        let message = if password.is_empty() {
            "Invite pasted".to_owned()
        } else {
            "Invite pasted, with its password".to_owned()
        };
        self.show_toast(ToastTone::Success, message);
        cx.notify();
    }

    /// Fills the fields from a recent server.
    fn use_saved_server(&mut self, saved: &SavedServer, cx: &mut Context<Self>) {
        let password = saved.password.clone().unwrap_or_default();
        self.play
            .address
            .update(cx, |input, cx| input.set_text(&saved.address, cx));
        self.play
            .password
            .update(cx, |input, cx| input.set_text(&password, cx));
        self.play.remember_password = saved.password.is_some();
        cx.notify();
    }

    /// Asks for one of the game's files and keeps the answer. Once the Super Mario 64 ROM and
    /// the B3313 patch are both known, the multiplayer ROM is built from them.
    fn choose_game_file(file: GameFile, window: &mut Window, cx: &mut Context<Self>) {
        before_dialog(window);
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    match file {
                        GameFile::Sm64Rom => this.preferences.sm64_rom = Some(path),
                        GameFile::B3313Patch => this.preferences.b3313_patch = Some(path),
                        GameFile::OwnRom => this.preferences.rom = Some(path),
                    }
                    this.save_preferences();
                    if file != GameFile::OwnRom {
                        this.build_game(cx);
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Builds the multiplayer ROM from the two files, if both are chosen, off the panel's own
    /// thread: it reads and checks some hundred megabytes.
    fn build_game(&mut self, cx: &mut Context<Self>) {
        let (Some(sm64), Some(patch)) = (
            self.preferences.sm64_rom.clone(),
            self.preferences.b3313_patch.clone(),
        ) else {
            return;
        };
        if self.play.building {
            return;
        }
        self.play.building = true;
        let built_from = (Some(sm64.clone()), Some(patch.clone()));
        let built = cx
            .background_executor()
            .spawn(async move { game_setup::set_up(&sm64, &patch) });
        cx.spawn(async move |this, cx| {
            let built = built.await;
            let _ = this.update(cx, |this, cx| {
                this.play.building = false;
                // Another file was chosen while this was built: the game shown as ready
                // must be the one of the files shown, so it is built from those now.
                let chosen = (
                    this.preferences.sm64_rom.clone(),
                    this.preferences.b3313_patch.clone(),
                );
                if chosen != built_from {
                    this.build_game(cx);
                    return;
                }
                match built {
                    Ok(built) => {
                        this.preferences.rom = Some(built.rom);
                        this.save_preferences();
                        // Which version it is decides who the player can share a session with.
                        let ready = format!("The game is ready: B3313 {}", built.game.name);
                        this.show_toast(ToastTone::Success, ready);
                    }
                    Err(message) => this.report_problem(message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_play(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let actions = vec![
            self.arrange_button(ArrangedPage::Play, cx),
            self.play_button(false, cx),
        ];
        let subtitle = match &self.play.state {
            PlayState::Playing { since, .. } => format!("Playing for {}", clock(since.elapsed())),
            PlayState::WaitingForGame | PlayState::Joining => "Joining".to_owned(),
            PlayState::Reconnecting { .. } => "Joining again".to_owned(),
            _ => "Join a session with this PC's game".to_owned(),
        };
        let panels = vec![
            ("status", Some(self.render_play_status(&theme, cx))),
            ("chat", self.render_chat_card(&theme, cx)),
            ("server", Some(self.render_server_card(&theme, cx))),
            ("game", Some(self.render_game_card(&theme, window, cx))),
        ];
        let body = self.arranged(&theme, ArrangedPage::Play, panels, cx);
        page_frame(&theme, "Play", subtitle, actions, body)
    }

    /// Join, or Leave while joined; `small` beside a panel's heading on the Overview.
    pub(super) fn play_button(&self, small: bool, cx: &mut Context<Self>) -> AnyElement {
        let button = if self.play.bridge.is_some() {
            // Asked to leave already, and not gone yet.
            let leaving = matches!(self.play.state, PlayState::Ended { left: true, .. });
            Button::new("leave-session", if leaving { "Leaving" } else { "Leave" })
                .icon(IconName::Square)
                .tone(Tone::Danger)
                .enabled(!leaving)
                .tooltip(if leaving {
                    "Leaving the session, once what it was doing is done"
                } else {
                    "Leaves the session; the game goes on alone"
                })
                .on_click(cx.listener(|this, _, _, cx| this.leave_session(cx)))
        } else {
            let blocker = self.join_blocker(cx);
            Button::new("join-session", "Join")
                .icon(IconName::Play)
                .tone(Tone::Primary)
                .enabled(blocker.is_none())
                .tooltip(blocker.map_or_else(
                    || "Joins the server with this PC's game (Enter in the fields)".to_owned(),
                    str::to_owned,
                ))
                .on_click(cx.listener(|this, _, _, cx| this.join(cx)))
        };
        if small { button.small() } else { button }.into_any_element()
    }

    /// How the session goes: its state, its link, and what happened lately.
    fn render_play_status(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        card(theme)
            .p_5()
            .gap_4()
            .child(self.play_state(theme, cx))
            .children(self.render_link(theme))
            .children(self.render_play_events(theme))
            .into_any_element()
    }

    /// Where the session is, in a heading and a line under it. When the game closed, hovering
    /// the line shows what the system said of it.
    pub(super) fn play_state(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let more = match &self.play.state {
            PlayState::Ended {
                left: false,
                detail: Some(detail),
                ..
            } => Some(self.shown_text(detail)),
            _ => None,
        };
        let (color, title, detail) = match &self.play.state {
            PlayState::Idle => match self.join_blocker(cx) {
                Some(blocker) => (
                    theme.text_faint,
                    "Not joined".to_owned(),
                    blocker.to_owned(),
                ),
                None => (
                    theme.accent,
                    "Ready to join".to_owned(),
                    "Select Join, or press Enter in a field.".to_owned(),
                ),
            },
            PlayState::WaitingForGame => (
                theme.warning,
                "Waiting for the game".to_owned(),
                "RetroArch is open; you join once the game has started.".to_owned(),
            ),
            PlayState::Joining => (
                theme.warning,
                "Joining".to_owned(),
                format!("Reaching {}…", self.shown_text(&self.play.server)),
            ),
            PlayState::Reconnecting { until } => (
                theme.warning,
                "Joining again".to_owned(),
                format!(
                    "{} went silent. Your game plays on alone; trying for {} more.",
                    self.shown_text(&self.play.server),
                    time_left(*until)
                ),
            ),
            PlayState::Playing { player, since } => (
                theme.success,
                format!("Playing as {}", self.shown_text(&self.play.called(*player))),
                format!(
                    "On {} for {}",
                    self.shown_text(&self.play.server),
                    clock(since.elapsed())
                ),
            ),
            PlayState::Ended { reason, left, .. } => (
                if *left {
                    theme.text_faint
                } else {
                    theme.danger
                },
                "Not joined".to_owned(),
                if *left {
                    "You left the session.".to_owned()
                } else {
                    self.shown_text(reason)
                },
            ),
        };
        let line = div()
            .id("play-state-detail")
            .text_size(scaled(13.))
            .text_color(theme.text_muted)
            .child(detail);
        let line = match more {
            Some(more) => line.tip(more, cx),
            None => line,
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div().flex().items_center().gap_2().child(dot(color)).child(
                    div()
                        .text_size(scaled(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                ),
            )
            .child(line)
    }

    /// The link's last report, in a row: round trip, loss, and traffic each way.
    pub(super) fn render_link(&self, theme: &Theme) -> Option<AnyElement> {
        let link = self.play.link.as_ref()?;
        let (sent, received) = link.byte_rates();
        let round_trip = link
            .round_trips
            .mean()
            .map_or_else(|| "—".to_owned(), millis);
        let facts = [
            ("Round trip", round_trip),
            ("Loss", format!("{:.1}%", link.losses.rate() * 100.0)),
            ("Sent", byte_rate(sent)),
            ("Received", byte_rate(received)),
        ];
        Some(
            div()
                .flex()
                .flex_wrap()
                .gap_8()
                .pt_3()
                .border_t_1()
                .border_color(theme.border)
                .children(facts.into_iter().map(|(name, value)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .text_size(scaled(12.))
                                .text_color(theme.text_muted)
                                .child(name),
                        )
                        .child(
                            div()
                                .text_size(scaled(16.))
                                .font_weight(FontWeight::MEDIUM)
                                .child(value),
                        )
                }))
                .into_any_element(),
        )
    }

    fn render_play_events(&self, theme: &Theme) -> Option<AnyElement> {
        if self.play.events.is_empty() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .pt_3()
                .border_t_1()
                .border_color(theme.border)
                .children(self.play_event_rows(theme))
                .into_any_element(),
        )
    }

    /// What happened in the session lately, newest first, a row each.
    pub(super) fn play_event_rows(&self, theme: &Theme) -> Vec<Div> {
        let rows = self.play.events.iter().map(|(at, text)| {
            div()
                .flex()
                .gap_3()
                .when(self.preferences.compact, Styled::py_0p5)
                .when(!self.preferences.compact, Styled::py_1)
                .text_size(scaled(13.))
                .child(
                    div()
                        .w(scaled(96.))
                        .flex_none()
                        .font_family(MONO)
                        .text_size(scaled(12.))
                        .text_color(theme.text_faint)
                        .child(format::time_of_day(*at, self.hour24)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .one_line()
                        .child(self.shown_text(text)),
                )
        });
        rows.collect()
    }

    /// The chat, once joined: who is here, and the chat's box.
    fn render_chat_card(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.play.bridge.is_none() && self.play.chat.is_empty() {
            return None;
        }
        // Where to type it is said here, as the game itself says it only once.
        let in_game = self.preferences.chat_key.map_or_else(String::new, |key| {
            format!(
                " In the game, {} opens the chat and / opens it for a command.",
                key.to_ascii_uppercase()
            )
        });
        let about = format!(
            "Everyone in the session sees what you send, here and in their games.{in_game} \
             /help lists the commands."
        );
        Some(
            card(theme)
                .p_5()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_3()
                        .child(
                            // Free to be narrower than its text, which then wraps and leaves
                            // the button its room.
                            div().flex_1().min_w_0().child(card_header(
                                theme,
                                "Chat",
                                Some(&about),
                            )),
                        )
                        // Beside what they change: the key that opens the chat in the game,
                        // and how long its lines stay there.
                        .child(
                            div().flex_none().child(
                                Button::new("chat-settings", "Chat settings")
                                    .small()
                                    .tone(Tone::Quiet)
                                    .icon(IconName::SlidersHorizontal)
                                    .tooltip(
                                        "The chat's key in the game and how long lines stay there, \
                                     on the Preferences page",
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.go_to(Page::Preferences, cx);
                                    })),
                            ),
                        ),
                )
                .child(self.play_people(theme, cx))
                .child(self.play_chat_box(theme, "play-chat", cx))
                .into_any_element(),
        )
    }

    /// Everyone in the session joined, each with the host's or a moderator's mark.
    pub(super) fn play_people(&self, theme: &Theme, cx: &gpui::App) -> Div {
        let people = self.play.roster.iter().map(|entry| {
            div()
                .id(SharedString::from(format!("person-{}", entry.player.get())))
                .tip(
                    format!(
                        "Player {}, a {}",
                        entry.player.get(),
                        role_meaning(entry.role)
                    ),
                    cx,
                )
                .flex()
                .items_center()
                .gap_1()
                .h(scaled(24.))
                .px_2()
                .rounded_full()
                .border_1()
                .border_color(theme.border)
                .text_size(scaled(12.))
                .children(role_icon(theme, entry.role))
                .child(self.shown_text(entry.name.as_str()))
        });
        div().flex().flex_wrap().gap_2().children(people)
    }

    /// Whether anyone is listed in the session joined.
    pub(super) fn play_has_people(&self) -> bool {
        !self.play.roster.is_empty()
    }

    /// The session's chat in its box, speaking as this PC's player. `id` tells the box on one
    /// page from the same chat's on another.
    pub(super) fn play_chat_box(
        &self,
        theme: &Theme,
        id: &'static str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let joined = self.play.bridge.is_some();
        let lines = self
            .play
            .chat
            .lines()
            .map(|entry| self.chat_line(theme, entry, entry.role))
            .collect();
        let send = Button::new(SharedString::from(format!("{id}-send")), "Send")
            .icon(IconName::MessageSquare)
            .tone(Tone::Primary)
            .enabled(joined)
            .tooltip(if joined {
                "Sends the message to everyone (Enter)"
            } else {
                "Join a session on the Play page to chat"
            })
            .on_click(cx.listener(|this, _, _, cx| this.send_chat(cx)))
            .into_any_element();
        let empty = if joined {
            "Nothing has been said yet."
        } else {
            "Not in a session. Join one on the Play page to chat."
        };
        let chat_box = ChatBox {
            chat: Chat::Session,
            id,
            empty,
        };
        self.chat_box(theme, chat_box, lines, (&self.play.say, send), cx)
    }

    /// The server to join: from an invite, typed, or recent.
    fn render_server_card(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        card(theme)
            .p_5()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().min_w(scaled(240.)).child(card_header(
                        theme,
                        "Server",
                        Some("Copy the invite you were sent, then paste it here."),
                    )))
                    .child(Self::paste_invite_button(false, cx)),
            )
            .child(self.server_fields(theme, cx))
            .into_any_element()
    }

    /// Fills the server's fields from the invite on the clipboard; `small` beside a panel's
    /// heading on the Overview.
    pub(super) fn paste_invite_button(small: bool, cx: &mut Context<Self>) -> AnyElement {
        let button = Button::new("paste-invite", "Paste invite")
            .icon(IconName::Copy)
            .tone(Tone::Primary)
            .tooltip("Fills in the address and password from the invite you copied")
            .on_click(cx.listener(|this, _, _, cx| this.paste_invite(cx)));
        if small { button.small() } else { button }.into_any_element()
    }

    /// The server's address and password, whether the password is kept, and recent servers.
    pub(super) fn server_fields(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let show = Button::new(
            "show-play-password",
            if self.play.show_password {
                "Hide"
            } else {
                "Show"
            },
        )
        .tone(Tone::Quiet)
        .small()
        .enabled(!self.preferences.streamer_mode)
        .tooltip(if self.preferences.streamer_mode {
            "Streamer mode keeps the password hidden"
        } else {
            "Shows or hides the password"
        })
        .on_click(cx.listener(|this, _, _, cx| {
            this.play.show_password = !this.play.show_password;
            let masked = !this.play.show_password;
            this.play
                .password
                .update(cx, |input, cx| input.set_masked(masked, cx));
            cx.notify();
        }));
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_4()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .flex_1()
                            .min_w(scaled(240.))
                            .child(label(theme, "Server address"))
                            .child(div().font_family(MONO).child(self.play.address.clone())),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .flex_1()
                            .min_w(scaled(240.))
                            .child(label(theme, "Password"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().flex_1().child(self.play.password.clone()))
                                    .child(show),
                            ),
                    ),
            )
            .child(switch(
                theme,
                "remember-password",
                "Remember the password for this server",
                "Saved with your preferences in plain text, so only on a PC you trust. Off, you \
                 paste it each time.",
                self.play.remember_password,
                cx.listener(|this, _, _, cx| {
                    this.play.remember_password = !this.play.remember_password;
                    cx.notify();
                }),
            ))
            .children(self.render_saved_servers(theme, cx))
    }

    /// Servers joined before, to fill in with a click, or forget.
    fn render_saved_servers(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.preferences.servers.is_empty() {
            return None;
        }
        let chips: Vec<AnyElement> = self
            .preferences
            .servers
            .iter()
            .enumerate()
            .map(|(index, saved)| self.saved_server_chip(theme, index, saved, cx))
            .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(label(theme, "Recent"))
                .child(div().flex().flex_wrap().gap_2().children(chips))
                .into_any_element(),
        )
    }

    /// One recent server: its address, whether its password is kept, and a way to forget it.
    fn saved_server_chip(
        &self,
        theme: &Theme,
        index: usize,
        saved: &SavedServer,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = theme.accent;
        let fill = saved.clone();
        let forget = saved.address.clone();
        let tip = if saved.password.is_some() {
            "Fills in this server and its saved password"
        } else {
            "Fills in this server"
        };
        let forget_button = div()
            .id(("forget-server", index))
            .tip("Forgets this server, and its password", cx)
            .flex()
            .items_center()
            .justify_center()
            .size(scaled(22.))
            .rounded_full()
            .border_1()
            .border_color(gpui::transparent_black())
            .tab_index(0)
            .cursor_pointer()
            .hover(|style| style.bg(theme.surface_sunken))
            .focus_ring(move |style| style.border_color(accent))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.preferences.forget_server(&forget);
                this.save_preferences();
                cx.notify();
            }))
            .child(icon(IconName::X, 12., theme.text_muted));
        div()
            .id(("saved-server", index))
            .tip(tip, cx)
            .flex()
            .items_center()
            .gap_2()
            .h(scaled(30.))
            .pl_3()
            .pr_1()
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .tab_index(0)
            .cursor_pointer()
            .hover(|style| style.bg(theme.hover))
            .focus_ring(move |style| style.border_color(accent))
            .on_click(cx.listener(move |this, _, _, cx| this.use_saved_server(&fill, cx)))
            .child(
                div()
                    .font_family(MONO)
                    .text_size(scaled(13.))
                    .child(self.shown_text(&saved.address)),
            )
            .when(saved.password.is_some(), |chip| {
                chip.child(
                    div()
                        .text_size(scaled(11.))
                        .text_color(theme.text_faint)
                        .child("password saved"),
                )
            })
            .child(forget_button)
            .into_any_element()
    }

    /// The name other players see, which the next Join uses.
    fn render_name_field(&self, theme: &Theme) -> gpui::Div {
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(label(theme, "Your name"))
            // In a box of its own: the field fills its parent, which in a column is no height.
            .child(div().max_w(scaled(320.)).child(self.play.name.clone()))
            .child(hint(
                theme,
                "Shown over your Mario and in the chat: up to 16 plain characters, with no \
                 accents. Applies the next time you join; in a session, /name and a new name in \
                 the chat changes it. If someone in the session has the name, you get a number \
                 after it and are told so. Empty, the server calls you Player and a number.",
            ))
    }

    /// Has the player's Mario wear `outfit`: at once if a session is joined, and from the
    /// next join on either way.
    fn choose_outfit(&mut self, outfit: Outfit, cx: &mut Context<Self>) {
        self.preferences.player_outfit = Some(outfit);
        self.save_preferences();
        if let Some(bridge) = &self.play.bridge {
            bridge.wear(outfit);
        }
        cx.notify();
    }

    /// The outfits to choose from, each as a swatch of its cap over its overalls, the one worn
    /// marked.
    fn render_outfits(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let color = |[red, green, blue]: [u8; 3]| {
            gpui::rgb(u32::from(red) << 16 | u32::from(green) << 8 | u32::from(blue))
        };
        let accent = theme.accent;
        let swatches = Outfit::all().map(|outfit| {
            let worn = self.preferences.player_outfit == Some(outfit);
            div()
                .id(("outfit", usize::from(outfit.code())))
                .tip(outfit.name(), cx)
                .flex()
                .flex_col()
                .size(scaled(26.))
                .rounded_md()
                .overflow_hidden()
                .border_2()
                .border_color(if worn { accent } else { theme.border })
                .tab_index(0)
                .cursor_pointer()
                .focus_ring(move |style| style.border_color(accent))
                .on_click(cx.listener(move |this, _, _, cx| this.choose_outfit(outfit, cx)))
                .child(div().flex_1().bg(color(outfit.cap())))
                .child(div().flex_1().bg(color(outfit.overalls())))
        });
        let worn = self.preferences.player_outfit.map_or_else(
            || "Not chosen: the server gives you one by your number.".to_owned(),
            |outfit| format!("You wear {}.", outfit.name()),
        );
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(label(theme, "Your colors"))
            .child(div().flex().flex_wrap().gap_1p5().children(swatches))
            .child(hint(
                theme,
                &format!(
                    "{worn} Your Mario's cap and shirt over his overalls, as everyone sees \
                     him. /color in the chat changes it too."
                ),
            ))
    }

    /// One of the game's files: what it is, which file is chosen, and a button to choose it.
    fn game_file_row(&self, theme: &Theme, file: GameFile, cx: &mut Context<Self>) -> Div {
        let preferences = &self.preferences;
        let (id, title, path, missing, tooltip) = match file {
            GameFile::Sm64Rom => (
                "sm64-rom",
                "Super Mario 64 ROM",
                preferences.sm64_rom.as_deref(),
                "Not chosen yet",
                "Picks your own Super Mario 64 (USA) ROM, in any dump format".to_owned(),
            ),
            GameFile::B3313Patch => (
                "b3313-patch",
                "B3313 patch",
                preferences.b3313_patch.as_deref(),
                "Not chosen yet",
                format!(
                    "Picks a B3313 patch, a .bps file, of the version the session plays: {}",
                    game_setup::game_names()
                ),
            ),
            GameFile::OwnRom => (
                "multiplayer-rom",
                "Multiplayer ROM",
                preferences.rom.as_deref(),
                "Made when the two files above are chosen",
                "Picks a multiplayer ROM you built yourself, in place of the one made here"
                    .to_owned(),
            ),
        };
        let text = match path {
            _ if file == GameFile::OwnRom && self.play.building => "Building…".to_owned(),
            Some(path) => display_name(path),
            None => missing.to_owned(),
        };
        let tip = path.map_or_else(|| tooltip.clone(), |path| self.shown_path(path));
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .child(label(theme, title))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .id(SharedString::from(format!("{id}-name")))
                            .tip(tip, cx)
                            .min_w_0()
                            .one_line()
                            .child(text),
                    )
                    .child(
                        Button::new(SharedString::from(format!("{id}-choose")), "Choose…")
                            .small()
                            .tooltip(tooltip)
                            .on_click(cx.listener(move |_, _, window, cx| {
                                Self::choose_game_file(file, window, cx);
                            })),
                    ),
            )
    }

    /// This PC's side: the player, the multiplayer ROM and the emulator running it.
    fn render_game_card(
        &self,
        theme: &Theme,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        card(theme)
            .p_5()
            .gap_4()
            .child(card_header(theme, "Game", Some(GAME_MEANING)))
            .child(self.you_fields(theme, cx))
            .child(self.game_fields(theme, cx))
            .into_any_element()
    }

    /// What starting the game would change in the emulator now, if the panel can start it:
    /// read from the player's settings, with whether they type the chat in the game.
    fn emulator_changes(&self) -> Option<Result<Vec<Change>, String>> {
        let launcher = self.play.launcher.as_ref()?;
        let typing = self.preferences.chat_key.is_some();
        Some(launcher.changes(typing).map_err(|error| error.to_string()))
    }

    /// Reads again what starting the game changes in the emulator, if the list is open.
    pub(super) fn refresh_emulator_changes(&mut self) {
        if self.play.changes.is_some() {
            self.play.changes = self.emulator_changes();
        }
    }

    /// Shows what starting the game changes in the emulator, or hides it again.
    fn toggle_emulator_changes(&mut self, cx: &mut Context<Self>) {
        self.play.changes = match self.play.changes {
            Some(_) => None,
            None => self.emulator_changes(),
        };
        cx.notify();
    }

    /// That the panel changes some of the emulator's settings for a game it starts, and, if
    /// the player asks, each change with its reason. They are the player's own emulator's
    /// settings, so nothing is done to them unsaid.
    fn render_emulator_changes(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let is_open = self.play.changes.is_some();
        let toggle = Button::new(
            "emulator-changes",
            if is_open { "Hide" } else { "Show what changes" },
        )
        .small()
        .tone(Tone::Quiet)
        .icon(if is_open {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        })
        .tooltip("The emulator settings the panel changes for a game it starts, and why")
        .on_click(cx.listener(|this, _, _, cx| this.toggle_emulator_changes(cx)));
        let listed: Vec<AnyElement> = match &self.play.changes {
            None => Vec::new(),
            Some(Err(why)) => vec![
                hint(
                    theme,
                    &format!("The emulator's settings could not be read: {why}"),
                )
                .into_any_element(),
            ],
            Some(Ok(changes)) => changes
                .iter()
                .map(|change| {
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(div().text_size(scaled(13.)).child(change.what.clone()))
                        .child(hint(theme, change.why))
                        .into_any_element()
                })
                .collect(),
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            // One under the other: beside the button, in a row, the text ran off the card's
            // edge where it should have wrapped.
            .child(hint(
                theme,
                "For a game it starts, the panel changes a few of the emulator's settings so \
                 the session keeps in step. Only for that run: your own settings file is never \
                 changed, and games you start from Parallel Launcher are as ever.",
            ))
            .child(div().flex().child(toggle))
            .children(listed)
    }

    /// The player as others see them: their name and their Mario's colors.
    pub(super) fn you_fields(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_name_field(theme))
            .child(self.render_outfits(theme, cx))
    }

    /// The game's three files and the emulator that runs it.
    /// Which version of B3313 the player's game is, and what that decides. Said where the
    /// files are chosen, as two patches of one name can be different versions.
    fn version_hint(&self) -> String {
        let built = self.preferences.rom.as_deref();
        match built.and_then(game_setup::built_at) {
            Some(game) => format!(
                "Your game is B3313 {}. Everyone in a session plays the same version of \
                 B3313; to play another, choose its patch.",
                game.name
            ),
            None => format!(
                "The patch decides which version of B3313 you play, and everyone in a \
                 session plays the same one. These will do: {}.",
                game_setup::game_names()
            ),
        }
    }

    pub(super) fn game_fields(&self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let emulators: AnyElement = match self.play.emulators.as_slice() {
            [] if self.play.launcher.is_some() => {
                hint(theme, "Not running. Join starts the game for you.").into_any_element()
            }
            [] => hint(
                theme,
                "Parallel Launcher's emulator was not found. Install Parallel Launcher and start \
                 any game in it once, or start the multiplayer ROM in RetroArch yourself.",
            )
            .into_any_element(),
            [pid] => div()
                .flex()
                .items_center()
                .gap_2()
                .child(dot(theme.success))
                .child(format!("RetroArch is running (process {pid})"))
                .into_any_element(),
            several => div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .child(hint(theme, "Several are running; which one has the game?"))
                .children(several.iter().map(|pid| {
                    let pid = *pid;
                    Button::new(
                        SharedString::from(format!("emulator-{pid}")),
                        format!("Process {pid}"),
                    )
                    .small()
                    .tone(if self.play.chosen == Some(pid) {
                        Tone::Primary
                    } else {
                        Tone::Secondary
                    })
                    .tooltip("Joins with this RetroArch")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.play.chosen = Some(pid);
                        cx.notify();
                    }))
                }))
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.game_file_row(theme, GameFile::Sm64Rom, cx))
            .child(self.game_file_row(theme, GameFile::B3313Patch, cx))
            .child(self.game_file_row(theme, GameFile::OwnRom, cx))
            .child(hint(theme, &self.version_hint()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(label(theme, "Emulator"))
                    .child(emulators)
                    .when(self.play.launcher.is_some(), |emulator| {
                        emulator.child(self.render_emulator_changes(theme, cx))
                    }),
            )
    }
}

/// What a role is, in a sentence, for whoever points at its mark. The server's owner is not
/// among them: that is whoever runs the server, from outside the session, and has no role
/// in it.
pub(super) const fn role_meaning(role: Role) -> &'static str {
    match role {
        Role::Host => {
            "host: the player in charge of the session, who can remove anyone, appoint \
             moderators and end it with /end"
        }
        Role::Moderator => {
            "moderator: appointed by the host, and can remove ordinary players with /kick and \
             /ban"
        }
        Role::Player => "player",
    }
}

impl Play {
    /// The name the session lists `player` under, if it has listed them.
    pub(super) fn name_of(&self, player: PlayerId) -> Option<&str> {
        Some(self.listed(player)?.name.as_str())
    }
}

/// What a hit of `kind` did, as "You punched Luigi" says it.
const fn hit_word(kind: HitKind) -> &'static str {
    match kind {
        HitKind::Punch => "punched",
        HitKind::Kick => "kicked",
        HitKind::Trip => "tripped",
    }
}

/// How long is left until `until`, as minutes and seconds: "1:40".
fn time_left(until: Instant) -> String {
    let left = until.saturating_duration_since(Instant::now()).as_secs();
    format!("{}:{:02}", left / 60, left % 60)
}

impl Panel {
    /// The session this PC's player is in, in a few words, with the color that says how it
    /// stands: for the sidebar, where it shows on every page. None while there is none and
    /// has been none.
    pub(super) fn session_summary(&self, theme: &Theme) -> Option<(String, gpui::Hsla)> {
        match &self.play.state {
            // None to speak of: never joined, or left by choice.
            PlayState::Idle | PlayState::Ended { left: true, .. } => None,
            PlayState::WaitingForGame => Some(("Waiting for the game".to_owned(), theme.warning)),
            PlayState::Joining => Some(("Joining".to_owned(), theme.warning)),
            PlayState::Reconnecting { until } => Some((
                format!("Joining again · {}", time_left(*until)),
                theme.warning,
            )),
            PlayState::Playing { player, .. } => {
                let who = self.shown_text(&self.play.called(*player));
                Some((format!("Playing as {who}"), theme.success))
            }
            PlayState::Ended { reason, .. } => Some((self.shown_text(reason), theme.danger)),
        }
    }
}

/// The mark of a host or a moderator beside their name: a crown or a shield, so the shape tells
/// them apart as well as the color. None for an ordinary player.
pub(super) fn role_icon(theme: &Theme, role: Role) -> Option<gpui::Svg> {
    match role {
        Role::Host => Some(icon(IconName::Crown, 13., theme.warning)),
        Role::Moderator => Some(icon(IconName::Shield, 13., theme.accent)),
        Role::Player => None,
    }
}

impl Page {
    /// Whether the session is under way, for the sidebar's mark on Play.
    pub(super) fn play_mark(state: &PlayState, theme: &Theme) -> Option<gpui::Hsla> {
        match state {
            PlayState::Playing { .. } => Some(theme.success),
            PlayState::WaitingForGame | PlayState::Joining | PlayState::Reconnecting { .. } => {
                Some(theme.warning)
            }
            PlayState::Idle | PlayState::Ended { .. } => None,
        }
    }
}
