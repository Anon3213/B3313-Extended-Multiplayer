//! What the server's owner does to players from the panel: kicking, roles, bans kept in step
//! with the ban file, and the server chat's line, which takes the server's commands too.

use super::messages::ToastTone;
use super::{Panel, play_page};
use crate::host::Notice;
use b3313_protocol::PlayerId;
use b3313_server::bans::BanList;
use b3313_server::{Command, Event};
use gpui::{Context, Window};
use std::net::IpAddr;

impl Panel {
    pub(super) fn kick(&mut self, player: PlayerId) {
        if let Some(running) = &self.running {
            running.host.send(Command::Kick(player));
        }
    }

    /// Makes a player a moderator, who may kick and ban from the chat, or an ordinary player
    /// again. The server reports the change, or why it could not be made.
    pub(super) fn set_moderator(&mut self, player: PlayerId, moderator: bool) {
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
    pub(super) fn own_player(&self) -> Option<PlayerId> {
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
    pub(super) fn say_as_server(&mut self, cx: &mut Context<Self>) {
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
        } else if typed.trim().is_empty() {
            return;
        } else {
            // As the console's `say`, which refuses a line too long for the chat: sent as
            // it is, its end would be cut off for the players and shown whole here.
            match Command::parse(&format!("say {typed}")) {
                Ok(command) => command,
                Err(why) => {
                    let why = why.replacen("say takes", "A line takes", 1).replacen(
                        "say needs",
                        "A line needs",
                        1,
                    );
                    self.show_toast(ToastTone::Error, why);
                    cx.notify();
                    return;
                }
            }
        };
        if matches!(
            command,
            Command::Ban(_) | Command::BanAddress(_) | Command::Unban(_)
        ) {
            self.pending.bans_asked += 1;
        }
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

    pub(super) fn ban_player(
        &mut self,
        player: PlayerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
                this.pending.bans_asked += 1;
                running.host.send(Command::Ban(player));
            }
        });
    }

    pub(super) fn ban_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn ban_now(&mut self, ip: IpAddr) {
        self.pending.bans_asked += 1;
        match &self.running {
            Some(running) => running.host.send(Command::BanAddress(ip)),
            None => self.handle(Notice::Event(Event::AddressBanned(ip))),
        }
    }

    pub(super) fn unban(&mut self, ip: IpAddr, cx: &mut Context<Self>) {
        self.pending.bans_asked += 1;
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
    pub(super) fn sync_bans(&mut self) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::by_number;
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
}
