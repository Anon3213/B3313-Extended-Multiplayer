//! The server the panel hosts: starting, stopping and restarting it, its port on the router,
//! and taking in what it reports.

use super::messages::{ToastTone, Undo};
use super::{Page, Panel, chat};
use crate::dashboard::Kind;
use crate::host::{self, Host, Notice};
use crate::icons::IconName;
use crate::network::{self, Reach};
use crate::port_map::{self, PortMap};
use crate::preferences;
use crate::settings::SettingsForm;
use crate::system;
use crate::widgets::{Button, Tone};
use b3313_server::bans::{BanList, read_ban_file};
use b3313_server::{Config, Event};
use gpui::{AnyElement, Context, Window, prelude::*};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// The server while it runs, with the settings it started with.
#[derive(Debug)]
pub(super) struct Running {
    pub(super) host: Host,
    pub(super) config: Config,
    /// Whether the settings being edited differ from `config`, so that a restart is needed
    /// to use them: worked out as either changes (`Panel::note_restart_needed`), not as each
    /// page asks, which reads every setting in the form again.
    restart_needed: bool,
    pub(super) started: Instant,
    /// Who can join, and the address to give them; found once, as the server starts.
    pub(super) reach: Reach,
    /// Whether the router lets players over the internet in.
    pub(super) port: RouterPort,
}

/// What the panel has in hand for a server that is not running it yet, or is waiting to
/// hear back about.
#[derive(Debug, Default)]
pub(super) struct Pending {
    /// The host password of a server stopped to be started again, for the next to take.
    password: Option<String>,
    /// The router's port of a server stopped to be started again, with the address on this
    /// network it leads to, for the next to keep.
    port: Option<(SocketAddr, PortMap)>,
    /// The address the router has been asked to open a port for and has not yet answered.
    port_asked: Option<SocketAddr>,
    /// Bans and unbans asked for from the panel that the server has yet to report: news of
    /// one beyond these is of someone else's doing, as a moderator's in the game.
    pub(super) bans_asked: u32,
}

/// The server's port on its owner's router.
#[derive(Debug)]
pub(super) enum RouterPort {
    /// Not asked for: the server is for this PC alone, or the user turned asking off.
    NotAsked,
    Asking,
    /// Open for as long as this is kept.
    Open(PortMap),
    /// Why the router did not open it, in a sentence for the user.
    Refused(String),
}

impl Panel {
    pub(super) fn start(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.running.is_some() {
            return;
        }
        // The router's port of a server stopped to be started again: kept open for this
        // one if it listens where that one did, and closed as this returns if it does not
        // start, or listens elsewhere.
        let kept_port = self.pending.port.take();
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
        let made_up = self.pending.password.take().unwrap_or_default();
        let host_password = if config.host_password.is_empty() {
            made_up
        } else {
            config.host_password.clone()
        };
        // The key players know this server by, the same at every start. Not kept with the
        // settings shown as running, which are compared with those in the form.
        let key = match host::server_key(&config, preferences::path().as_deref()) {
            Ok(key) => key,
            Err(message) => {
                self.report_problem(message);
                cx.notify();
                return;
            }
        };
        let serving = Config {
            bans: self.bans.bans().to_vec(),
            host_password,
            key,
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
                    // It was started with the form's settings, a moment ago.
                    restart_needed: false,
                    started,
                    reach,
                    port: RouterPort::NotAsked,
                });
                match (reach, self.preferences.open_port, kept_port) {
                    // Still open, for the same address: closing it and asking again could
                    // have the old closing reach the router after the new opening.
                    (Reach::Network(local), true, Some((mapped, map))) if mapped == local => {
                        if let Some(running) = &mut self.running {
                            running.port = RouterPort::Open(map);
                        }
                    }
                    (Reach::Network(local), true, _) => self.open_router_port(local, cx),
                    _ => {}
                }
            }
            Err(message) => self.report_problem(message),
        }
        cx.notify();
    }

    /// Asks the router to send the server's port to `local`, this PC's address on its network,
    /// and notes the answer for whichever server is then running there and waiting for it:
    /// the one that asked, or one started in its place meanwhile. Waiting on the router
    /// happens off the panel's thread.
    fn open_router_port(&mut self, local: SocketAddr, cx: &mut Context<Self>) {
        if let Some(running) = &mut self.running {
            running.port = RouterPort::Asking;
        }
        // The router was asked for this already, for a server since stopped, and has yet to
        // answer: that answer is this server's. A second asking would make two keepers of
        // one port, and the first to go would close it under the other.
        if self.pending.port_asked == Some(local) {
            return;
        }
        self.pending.port_asked = Some(local);
        let asked = cx
            .background_executor()
            .spawn(async move { port_map::open(local) });
        cx.spawn(async move |this, cx| {
            let answer = asked.await;
            let _ = this.update(cx, |this, cx| {
                this.pending.port_asked = None;
                let waiting = |running: &&mut Running| {
                    running.reach == Reach::Network(local)
                        && matches!(running.port, RouterPort::Asking)
                };
                let Some(running) = this.running.as_mut().filter(waiting) else {
                    // No server waits for it; dropping the answer closes the port again.
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
    pub(super) fn close_router_port(&mut self) {
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

    pub(super) fn restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Looked at before the server stops: one that could not start again would leave its
        // players joining nothing.
        let Some(config) = self.settings_to_serve(cx) else {
            return;
        };
        if let Err(message) = host::server_key(&config, preferences::path().as_deref()) {
            self.report_problem(message);
            cx.notify();
            return;
        }
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

    pub(super) fn stop_now(&mut self, cx: &mut Context<Self>) {
        self.stop_server(false, cx);
    }

    /// Stops the server. To start it again at once, if `restarting`: its players' bridges are
    /// then told to join again by themselves, and the host's own stays the host's, by the
    /// next server's taking this one's host password.
    fn stop_server(&mut self, restarting: bool, cx: &mut Context<Self>) {
        if let Some(running) = self.running.take() {
            let notices = if restarting {
                // The router's port stays open for the next server: see `start`.
                if let (RouterPort::Open(map), Reach::Network(local)) =
                    (running.port, running.reach)
                {
                    self.pending.port = Some((local, map));
                }
                // Only one the panel made up: the settings' own is theirs to change, and
                // one taken out of them to revoke it must not live on in the next server.
                if running.config.host_password.is_empty() {
                    self.pending.password = Some(running.host.host_password().to_owned());
                }
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

    /// Takes in one thing the server reported, or that the panel did itself while it is stopped.
    pub(super) fn handle(&mut self, notice: Notice) {
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
                        self.report_ban(event.to_string(), Undo::Unban(*ip));
                    }
                    Event::Unbanned(ip) => {
                        self.report_ban(event.to_string(), Undo::Reban(*ip));
                    }
                    Event::CommandFailed(message) => {
                        self.show_toast(ToastTone::Error, message.clone());
                    }
                    // The answer to `/list`, which the Log keeps among its reports, out of
                    // sight unless asked for: shown, or the command would seem to do nothing.
                    Event::Players(_) => self.show_toast(ToastTone::Success, event.to_string()),
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

    pub(super) fn uptime(&self) -> Option<Duration> {
        self.running
            .as_ref()
            .map(|running| running.started.elapsed())
    }

    /// Whether the settings being edited differ from those the running server started with.
    pub(super) fn needs_restart(&self) -> bool {
        self.running
            .as_ref()
            .is_some_and(|running| running.restart_needed)
    }

    /// Works out again whether the running server needs a restart to use the settings being
    /// edited. Called wherever the form changes; a server just started needs none.
    pub(super) fn note_restart_needed(&mut self) {
        if let Some(running) = &mut self.running {
            running.restart_needed = differs_from(&self.form, &running.config);
        }
    }
}

/// Whether the settings in `form` are other than `served`, those a server runs with. Settings
/// that cannot be used are other than any a server runs with.
fn differs_from(form: &SettingsForm, served: &Config) -> bool {
    form.config().ok().as_ref() != Some(served)
}

/// The Start, Restart and Stop buttons, shared by the pages that control the server.
pub(super) fn server_buttons(panel: &Panel, cx: &mut Context<Panel>) -> Vec<AnyElement> {
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
    use super::differs_from;
    use crate::settings::SettingsForm;
    use b3313_server::Config;

    #[test]
    fn a_restart_is_needed_once_the_settings_are_other_than_the_servers() {
        let served = Config::default();
        let mut form = SettingsForm::new(&served);
        assert!(!differs_from(&form, &served));
        form.set("max_players", "6");
        assert!(differs_from(&form, &served));
        // Typed back to what the server has, none is needed after all.
        let as_served = SettingsForm::new(&served);
        form.set("max_players", as_served.value("max_players"));
        assert!(!differs_from(&form, &served));
        // Settings that cannot be used are not the server's either.
        form.set("max_players", "many");
        assert!(differs_from(&form, &served));
    }
}
