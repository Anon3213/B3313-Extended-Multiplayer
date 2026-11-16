//! Who the players are to each other: their names and roles, what they say, and the commands the
//! host and moderators give in the chat.
//!
//! The server cannot see who is at a keyboard, so a name is only what a bridge sent, made unlike
//! everyone else's, and the host is whoever's bridge knew the host password first, or whoever
//! the host or the server's owner passed the role to.

use crate::commands::{self, Rule};
use crate::limits::Bucket;
use b3313_protocol::{
    Ack, ChatLine, Inbox, MAX_LINES_PER_PACKET, MAX_MESSAGE_LENGTH, MAX_NAME_LENGTH, Message, Name,
    Outbox, Outfit, Packet, PlayerId, Role, RosterEntry, SESSION_SIZE, Stream,
};
use std::time::{Duration, Instant};

/// A bridge that has not said who its player is by this long after joining gets a name made up
/// for it, so a bridge that never says is still listed.
const INTRODUCTION_GRACE: Duration = Duration::from_secs(3);
/// Messages a player may send at once, and how long each further one takes to be allowed: enough
/// for a quick exchange, not for filling everyone's screen.
const SAY_BURST: f64 = 5.0;
const SAY_INTERVAL: Duration = Duration::from_secs(2);

/// What the chat's `/help` says to each kind of player, a line to each message. Each line is
/// short enough to be one row of the chat in the game, which is 200 pixels wide and shows six.
const HELP: &[&str] = &[
    "/list: who is here. /color: your colors.",
    "/name <new name> renames you.",
];
const MODERATOR_HELP: &[&str] = &[
    "/list: who is here. /color: your colors.",
    "/name <new name> renames you.",
    "/kick <player>, /ban <player>.",
    "<player>: a name, or a number in /list.",
];
pub(crate) const HOST_HELP: &[&str] = &[
    "/list: who is here. /color: your colors.",
    "/name <new name> renames you.",
    "/kick <player>, /ban <player>.",
    "/mod, /unmod, /host <player>.",
    "<player>: a name, or a number in /list.",
    "/end ends the session for everyone.",
];
/// How long a player waits between two changes of name: long enough that nobody can flicker
/// through names to confuse whoever moderates.
pub(crate) const RENAME_INTERVAL: Duration = Duration::from_secs(10);

/// A player named in a chat command: by number, as `/list` shows, or by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Number(u8),
    Name(String),
}

/// A command given in the chat, as `/kick Luigi`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatCommand {
    Help,
    List,
    Kick(Target),
    Ban(Target),
    /// Make a player a moderator.
    Mod(Target),
    /// Make a moderator an ordinary player again.
    Unmod(Target),
    /// Pass the host's role on to a player.
    Host(Target),
    /// Wear the outfit named, or list the outfits if none is.
    Color(String),
    /// Take the name given, or be told one's own if none is.
    Name(String),
    /// End the session for everyone.
    End,
}

impl ChatCommand {
    /// Reads a chat message that starts with `/`, by the table of `commands`.
    ///
    /// # Errors
    /// Fails, with a line for the player who typed it, on anything that is not a command.
    pub fn parse(text: &str) -> Result<Self, String> {
        commands::read(text).map(|(command, _)| command)
    }
}

/// What came of something a player said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Spoken {
    /// A repeat, or more than the player may say yet.
    Nothing,
    /// Sent on to everyone.
    Chat(Message),
    /// A command, for the session to carry out if the player's role is one that may give it.
    Command(ChatCommand, Rule),
}

#[derive(Debug)]
struct Member {
    /// None until the bridge has said, or the grace has passed.
    name: Option<Name>,
    role: Role,
    /// None until the player chooses one; the roster then gives the one their number does.
    outfit: Option<Outfit>,
    joined: Instant,
    /// Whether the bridge has introduced itself: it may once.
    introduced: bool,
    /// When the player last changed their name with `/name`.
    renamed: Option<Instant>,
    /// Which of the player's messages have been taken, so repeats are dropped.
    says: Inbox,
    /// How many messages the player may still send.
    say_allowance: Bucket,
    /// Lines to send, each until the player's bridge acknowledges it.
    lines: Outbox<ChatLine>,
    /// The roster version this player was last sent, and its resends still to go.
    roster_sent: u64,
    roster_resends: u8,
}

impl Member {
    /// Takes one message from the player's allowance, if it has one.
    fn may_say(&mut self, now: Instant) -> bool {
        let rate = SAY_INTERVAL.as_secs_f64().recip();
        self.say_allowance.take(1.0, rate, SAY_BURST, now)
    }
}

/// The players' names and roles, and the chat between them.
#[derive(Debug, Default)]
pub struct People {
    members: [Option<Member>; SESSION_SIZE],
    /// Raised whenever the roster changes; never 0 once anyone is listed.
    roster_version: u64,
}

fn text(line: &str) -> Option<Message> {
    Message::new(line)
}

impl People {
    fn member(&self, player: PlayerId) -> Option<&Member> {
        self.members.get(usize::from(player.get()))?.as_ref()
    }

    fn member_mut(&mut self, player: PlayerId) -> Option<&mut Member> {
        self.members.get_mut(usize::from(player.get()))?.as_mut()
    }

    /// Takes in a player who has just joined; they are listed once they have a name.
    pub fn join(&mut self, player: PlayerId, now: Instant) {
        if let Some(slot) = self.members.get_mut(usize::from(player.get())) {
            *slot = Some(Member {
                name: None,
                role: Role::Player,
                outfit: None,
                joined: now,
                introduced: false,
                renamed: None,
                says: Inbox::default(),
                say_allowance: Bucket::full(SAY_BURST, now),
                lines: Outbox::default(),
                roster_sent: 0,
                roster_resends: 0,
            });
        }
    }

    /// Forgets a player who has gone, and tells the others, with `how` they went, as "left".
    pub fn leave(&mut self, player: PlayerId, how: &str) {
        let gone = self
            .members
            .get_mut(usize::from(player.get()))
            .and_then(Option::take);
        if let Some(name) = gone.and_then(|member| member.name) {
            self.roster_version += 1;
            self.announce(&format!("{} {how}", name.as_str()));
        }
    }

    /// The player's name, or what they are called until they have one.
    #[must_use]
    pub fn name_of(&self, player: PlayerId) -> String {
        self.member(player)
            .and_then(|member| member.name.as_ref())
            .map_or_else(|| default_name(player), |name| name.as_str().to_owned())
    }

    #[must_use]
    pub fn role_of(&self, player: PlayerId) -> Role {
        self.member(player)
            .map_or(Role::Player, |member| member.role)
    }

    /// Who has the name `wanted`, whatever the case of its letters, other than `player`, who
    /// may have it themself.
    #[must_use]
    pub fn holder_of(&self, wanted: &str, player: PlayerId) -> Option<PlayerId> {
        let index = self.members.iter().enumerate().position(|(index, slot)| {
            index != usize::from(player.get())
                && slot
                    .as_ref()
                    .and_then(|member| member.name.as_ref())
                    .is_some_and(|name| name.as_str().eq_ignore_ascii_case(wanted))
        })?;
        PlayerId::new(u8::try_from(index).ok()?)
    }

    /// `wanted`, or `wanted` with the lowest number after it that makes it unlike every other
    /// player's name, whatever the case of the letters.
    fn unlike_the_others(&self, player: PlayerId, wanted: &Name) -> Name {
        let taken = |candidate: &str| self.holder_of(candidate, player).is_some();
        if !taken(wanted.as_str()) {
            return wanted.clone();
        }
        (2..=SESSION_SIZE + 1)
            .filter_map(|number| {
                let suffix = format!(" {number}");
                let room = MAX_NAME_LENGTH.saturating_sub(suffix.len());
                let base: String = wanted.as_str().chars().take(room).collect();
                Name::new(&format!("{}{suffix}", base.trim_end()))
            })
            .find(|candidate| !taken(candidate.as_str()))
            .unwrap_or_else(|| wanted.clone())
    }

    /// Gives a player a name and lists them, announcing them if they were not listed before.
    fn set_name(&mut self, player: PlayerId, wanted: &Name) {
        let name = self.unlike_the_others(player, wanted);
        let Some(member) = self.member_mut(player) else {
            return;
        };
        let is_new = member.name.is_none();
        member.name = Some(name.clone());
        self.roster_version += 1;
        if is_new {
            self.announce(&format!("{} joined", name.as_str()));
        }
    }

    /// The host, if the session has one. There is never more than one.
    #[must_use]
    pub fn host(&self) -> Option<PlayerId> {
        let index = self
            .members
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|m| m.role == Role::Host))?;
        PlayerId::new(u8::try_from(index).ok()?)
    }

    /// Gives `player` the host's role, which whoever had it loses, so that one player has it.
    fn crown(&mut self, player: PlayerId) {
        for member in self.members.iter_mut().flatten() {
            if member.role == Role::Host {
                member.role = Role::Player;
            }
        }
        if let Some(member) = self.member_mut(player) {
            member.role = Role::Host;
        }
        self.roster_version += 1;
    }

    /// Takes a bridge's word on who its player is: the name, or one made up if it gave none, and
    /// the host's role if `is_host`, from whoever had it. A bridge may say once; anything after
    /// is ignored.
    ///
    /// Returns the name the player now has, if this was their introduction.
    pub fn introduce(
        &mut self,
        player: PlayerId,
        name: Option<&Name>,
        is_host: bool,
    ) -> Option<Name> {
        let member = self.member_mut(player)?;
        if member.introduced {
            return None;
        }
        member.introduced = true;
        if is_host {
            self.crown(player);
        }
        let fallback = Name::new(&default_name(player))?;
        self.set_name(player, name.unwrap_or(&fallback));
        let given = self.member(player)?.name.clone()?;
        // Said, or a player would only wonder at the number after their name.
        if let Some(wanted) = name.filter(|&wanted| *wanted != given) {
            self.tell(
                player,
                &format!(
                    "{} is taken, so you are {}. /name changes it.",
                    wanted.as_str(),
                    given.as_str()
                ),
            );
        }
        Some(given)
    }

    /// Carries out `/name` for a player: gives them the name `wanted` and tells everyone, or
    /// tells them the name they have if it asks for none. Returns the new name, if there is
    /// one.
    ///
    /// # Errors
    /// Fails, with a line for the player, if the name is not one a player can have, is another
    /// player's, is their own already, or comes too soon after their last change.
    pub fn rename(
        &mut self,
        player: PlayerId,
        wanted: &str,
        now: Instant,
    ) -> Result<Option<Name>, String> {
        let current = self.name_of(player);
        if wanted.is_empty() {
            self.tell(
                player,
                &format!("You are {current}. /name and a new name changes it."),
            );
            return Ok(None);
        }
        // Asked for by name, so refused where a name at joining would be cut or cleaned.
        let name = Name::new(wanted)
            .filter(|name| name.as_str() == wanted)
            .ok_or_else(|| {
                format!("A name is 1 to {MAX_NAME_LENGTH} plain characters, with no accents.")
            })?;
        if name.as_str() == current {
            return Err(format!("You are {current} already."));
        }
        if self.holder_of(name.as_str(), player).is_some() {
            return Err(format!(
                "{} is taken; /list shows the names in use.",
                name.as_str()
            ));
        }
        let member = self
            .member_mut(player)
            .ok_or_else(|| format!("There is no player {}.", player.get()))?;
        let is_due = |last: Instant| now.saturating_duration_since(last) >= RENAME_INTERVAL;
        if !member.renamed.is_none_or(is_due) {
            return Err("Wait a moment before changing your name again.".to_owned());
        }
        member.renamed = Some(now);
        member.name = Some(name.clone());
        self.roster_version += 1;
        self.announce(&format!("{current} is now {}", name.as_str()));
        Ok(Some(name))
    }

    /// Passes the host's role to a player and tells everyone. Whoever had it is an ordinary
    /// player again, and is returned.
    ///
    /// # Errors
    /// Fails, with a line for whoever asked, if the player is not here or is the host already.
    pub fn make_host(&mut self, player: PlayerId) -> Result<Option<PlayerId>, String> {
        let name = self.name_of(player);
        let member = self
            .member(player)
            .ok_or_else(|| format!("There is no player {}.", player.get()))?;
        if member.role == Role::Host {
            return Err(format!("{name} is the host already."));
        }
        let former = self.host();
        self.crown(player);
        self.announce(&format!("{name} is now the host"));
        Ok(former)
    }

    /// Names the players whose bridges have had long enough to say and have not, and returns
    /// them with their names.
    pub fn name_the_silent(&mut self, now: Instant) -> Vec<(PlayerId, Name)> {
        let silent: Vec<PlayerId> = self
            .members
            .iter()
            .enumerate()
            .filter(|(_, slot)| {
                slot.as_ref().is_some_and(|member| {
                    member.name.is_none()
                        && now.saturating_duration_since(member.joined) >= INTRODUCTION_GRACE
                })
            })
            .filter_map(|(index, _)| PlayerId::new(u8::try_from(index).ok()?))
            .collect();
        silent
            .into_iter()
            .filter_map(|player| {
                let name = Name::new(&default_name(player))?;
                self.set_name(player, &name);
                Some((player, self.member(player)?.name.clone()?))
            })
            .collect()
    }

    /// Makes a player a moderator, or an ordinary player again, and tells everyone.
    ///
    /// # Errors
    /// Fails, with a line for whoever asked, if the player is not here, is the host, or already
    /// is what was asked.
    pub fn set_moderator(&mut self, player: PlayerId, moderator: bool) -> Result<(), String> {
        let name = self.name_of(player);
        let member = self
            .member_mut(player)
            .ok_or_else(|| format!("There is no player {}.", player.get()))?;
        let (role, news) = match (member.role, moderator) {
            (Role::Host, _) => return Err(format!("{name} is the host.")),
            (Role::Moderator, true) => return Err(format!("{name} is already a moderator.")),
            (Role::Player, false) => return Err(format!("{name} is not a moderator.")),
            (_, true) => (Role::Moderator, "is now a moderator"),
            (_, false) => (Role::Player, "is no longer a moderator"),
        };
        member.role = role;
        self.roster_version += 1;
        self.announce(&format!("{name} {news}"));
        Ok(())
    }

    /// Has a player wear an outfit, which every game is then told with the roster. Asking for
    /// the one already worn changes nothing, so a bridge may ask until it sees it worn.
    pub fn wear(&mut self, player: PlayerId, outfit: Outfit) {
        if let Some(member) = self.member_mut(player)
            && member.outfit != Some(outfit)
        {
            member.outfit = Some(outfit);
            self.roster_version += 1;
        }
    }

    /// Carries out `/color` for a player: has them wear the outfit called `wanted`, or tells
    /// them the outfits there are if it names none.
    ///
    /// # Errors
    /// Fails, with a line for the player, if no outfit is called that.
    pub fn choose_outfit(&mut self, player: PlayerId, wanted: &str) -> Result<(), String> {
        if wanted.is_empty() {
            // Two lines, as one would be longer than a chat message may be.
            let names: Vec<&str> = Outfit::all().map(Outfit::name).collect();
            let (first, second) = names.split_at(names.len() / 2);
            self.tell(player, &format!("/color and one of: {},", first.join(", ")));
            self.tell(player, &format!("{}.", second.join(", ")));
            return Ok(());
        }
        let outfit = Outfit::named(wanted)
            .ok_or_else(|| format!("{wanted} is not a color; /color lists them."))?;
        self.wear(player, outfit);
        self.tell(player, &format!("You now wear {}.", outfit.name()));
        Ok(())
    }

    /// The player a command named, by number or, whatever the case of the letters, by name.
    ///
    /// # Errors
    /// Fails, with a line for whoever asked, if nobody here is so numbered or named.
    pub fn find(&self, target: &Target) -> Result<PlayerId, String> {
        let listed = |player: PlayerId| self.member(player).is_some_and(|m| m.name.is_some());
        match target {
            Target::Number(number) => PlayerId::new(*number)
                .filter(|&player| listed(player))
                .ok_or_else(|| format!("There is no player {number}; /list shows who is here.")),
            Target::Name(wanted) => self
                .roster()
                .into_iter()
                .find(|entry| entry.name.as_str().eq_ignore_ascii_case(wanted))
                .map(|entry| entry.player)
                .ok_or_else(|| format!("Nobody here is called {wanted}; /list shows who is.")),
        }
    }

    /// Everyone who has a name, in order of their numbers.
    #[must_use]
    pub fn roster(&self) -> Vec<RosterEntry> {
        self.members
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let member = slot.as_ref()?;
                let player = PlayerId::new(u8::try_from(index).ok()?)?;
                Some(RosterEntry {
                    player,
                    role: member.role,
                    outfit: member.outfit.unwrap_or(Outfit::of_player(player)),
                    name: member.name.clone()?,
                })
            })
            .collect()
    }

    /// Who is here, as `/list` tells it: each player's number, name and role, in as many lines as
    /// it takes.
    #[must_use]
    pub fn listing(&self) -> Vec<String> {
        let mut lines = vec![String::new()];
        for entry in self.roster() {
            let role = match entry.role {
                Role::Player => "",
                Role::Moderator => " (moderator)",
                Role::Host => " (host)",
            };
            let item = format!("{} {}{role}", entry.player.get(), entry.name.as_str());
            match lines.last_mut() {
                Some(line) if line.is_empty() => *line = item,
                Some(line) if line.len() + 2 + item.len() <= MAX_MESSAGE_LENGTH => {
                    line.push_str(", ");
                    line.push_str(&item);
                }
                _ => lines.push(item),
            }
        }
        lines
    }

    /// What `/help` tells `player`, a line to each message.
    #[must_use]
    pub fn help_for(&self, player: PlayerId) -> &'static [&'static str] {
        match self.role_of(player) {
            Role::Player => HELP,
            Role::Moderator => MODERATOR_HELP,
            Role::Host => HOST_HELP,
        }
    }

    fn queue(member: &mut Member, from: Option<PlayerId>, text: &Message) {
        member.lines.push(|id| ChatLine {
            id,
            from,
            text: text.clone(),
        });
    }

    /// What to answer a `Say` from `player` with: how far their messages have been taken.
    #[must_use]
    pub fn ack(&self, player: PlayerId) -> Option<Ack> {
        let member = self.members.get(usize::from(player.get()))?.as_ref()?;
        member.says.ack(Stream::Chat)
    }

    /// Takes a bridge's word that it has every chat line up to number `id`.
    pub fn acknowledged(&mut self, player: PlayerId, id: u16) {
        if let Some(member) = self.member_mut(player) {
            member.lines.acknowledge(id);
        }
    }

    /// Sends a line from the server to one player, as the answer to a command.
    pub fn tell(&mut self, player: PlayerId, line: &str) {
        if let (Some(member), Some(text)) = (self.member_mut(player), text(line)) {
            Self::queue(member, None, &text);
        }
    }

    /// Sends a line to everyone who is listed: from a player, or from the server.
    fn send_to_all(&mut self, from: Option<PlayerId>, text: &Message) {
        for member in self.members.iter_mut().flatten() {
            if member.name.is_some() {
                Self::queue(member, from, text);
            }
        }
    }

    /// Sends a line from the server to everyone.
    pub fn announce(&mut self, line: &str) {
        if let Some(text) = text(line) {
            self.send_to_all(None, &text);
        }
    }

    /// Takes something a player said: a command if it starts with `/`, and otherwise chat, which
    /// goes on to everyone, the player included.
    pub fn say(&mut self, player: PlayerId, id: u16, said: &Message, now: Instant) -> Spoken {
        let Some(member) = self.member_mut(player) else {
            return Spoken::Nothing;
        };
        if member.name.is_none() || !member.says.take(id) {
            return Spoken::Nothing;
        }
        if !member.may_say(now) {
            self.tell(player, "You are sending messages too fast; wait a moment.");
            return Spoken::Nothing;
        }
        if said.as_str().starts_with('/') {
            return match commands::read(said.as_str()) {
                Ok((command, rule)) => Spoken::Command(command, rule),
                Err(why) => {
                    self.tell(player, &why);
                    Spoken::Nothing
                }
            };
        }
        self.send_to_all(Some(player), said);
        Spoken::Chat(said.clone())
    }

    /// The roster, if `player` is due it, and the chat lines waiting for them.
    ///
    /// `resend` and `quiet_resend` say whether this tick is one for resending what a player has
    /// had, as the session's world is resent.
    pub fn packets_for(
        &mut self,
        player: PlayerId,
        resend: bool,
        quiet_resend: bool,
    ) -> Vec<Packet> {
        let version = self.roster_version;
        let Some(member) = self.member_mut(player) else {
            return Vec::new();
        };
        let is_new = member.roster_sent < version;
        if is_new {
            member.roster_sent = version;
            member.roster_resends = crate::SETTLING_RESENDS;
        }
        let is_resend_due =
            resend && (crate::session::take_resend(&mut member.roster_resends) || quiet_resend);
        let mut packets = Vec::new();
        // Made only when it is to go: this is asked for every player on every tick, and the
        // roster is a name copied for each of them.
        if version > 0 && (is_new || is_resend_due) {
            packets.push(Packet::Roster(self.roster()));
        }
        let Some(member) = self.member_mut(player) else {
            return packets;
        };
        if !member.lines.is_empty() {
            packets.push(Packet::Said(member.lines.batch(MAX_LINES_PER_PACKET)));
        }
        packets
    }
}

/// What a player is called until their bridge has said, or if it never does.
fn default_name(player: PlayerId) -> String {
    format!("Player {}", player.get())
}

#[cfg(test)]
mod tests {
    use super::{ChatCommand, INTRODUCTION_GRACE, People, SAY_INTERVAL, Spoken, Target};
    use b3313_protocol::{Message, Name, Packet, PlayerId, Role};
    use std::time::{Duration, Instant};

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    fn name(text: &str) -> Name {
        Name::new(text).unwrap_or_else(|| unreachable!("the test's name is not empty"))
    }

    fn message(text: &str) -> Message {
        Message::new(text).unwrap_or_else(|| unreachable!("the test's message is not empty"))
    }

    /// Everyone named in `names`, in order from player 0; the first is the host.
    fn people(names: &[&str], now: Instant) -> People {
        let mut people = People::default();
        for (index, text) in names.iter().enumerate() {
            let player = id(u8::try_from(index).unwrap_or(0));
            people.join(player, now);
            people.introduce(player, Some(&name(text)), index == 0);
        }
        people
    }

    /// The chat lines waiting for `player`, each once, as "name: text" or, from the server, text.
    fn heard(people: &mut People, player: PlayerId) -> Vec<String> {
        let mut lines = Vec::new();
        // Each packet's lines are acknowledged, as a bridge does, so the next brings the rest.
        loop {
            let packets = people.packets_for(player, false, false);
            let said = packets.into_iter().find_map(|packet| match packet {
                Packet::Said(batch) => Some(batch),
                _ => None,
            });
            let Some(batch) = said else {
                return lines;
            };
            for line in &batch {
                let from = line.from.map(|from| format!("{}: ", people.name_of(from)));
                lines.push(format!(
                    "{}{}",
                    from.unwrap_or_default(),
                    line.text.as_str()
                ));
            }
            if let Some(last) = batch.last() {
                people.acknowledged(player, last.id);
            }
        }
    }

    #[test]
    fn a_chat_line_is_sent_until_its_player_has_it() {
        let mut people = People::default();
        let now = Instant::now();
        people.join(id(0), now);
        people.tell(id(0), "hello");
        let said = |people: &mut People| -> Vec<u16> {
            let packets = people.packets_for(id(0), false, false);
            let lines = packets.into_iter().find_map(|packet| match packet {
                Packet::Said(batch) => Some(batch),
                _ => None,
            });
            lines
                .unwrap_or_default()
                .iter()
                .map(|line| line.id)
                .collect()
        };
        assert_eq!(said(&mut people), [0]);
        assert_eq!(said(&mut people), [0]);
        people.acknowledged(id(0), 0);
        assert_eq!(said(&mut people), []);
    }

    #[test]
    fn chat_commands_are_read_with_their_players() {
        let kick = ChatCommand::parse("/kick Luigi the Second");
        let named = Target::Name("Luigi the Second".to_owned());
        assert_eq!(kick, Ok(ChatCommand::Kick(named)));
        assert_eq!(
            ChatCommand::parse("/ban 3"),
            Ok(ChatCommand::Ban(Target::Number(3)))
        );
        assert_eq!(
            ChatCommand::parse("/mod  7 "),
            Ok(ChatCommand::Mod(Target::Number(7)))
        );
        assert_eq!(
            ChatCommand::parse("/host Luigi"),
            Ok(ChatCommand::Host(Target::Name("Luigi".to_owned())))
        );
        assert_eq!(ChatCommand::parse("/list"), Ok(ChatCommand::List));
        assert_eq!(ChatCommand::parse("/end"), Ok(ChatCommand::End));
        assert!(ChatCommand::parse("/kick").is_err_and(|why| why.contains("needs a player")));
        assert!(ChatCommand::parse("/dance").is_err_and(|why| why.contains("not a command")));
    }

    #[test]
    fn a_name_already_taken_gets_a_number_and_a_silent_bridge_gets_one_made_up() {
        let now = Instant::now();
        let mut people = people(&["Mario", "mario"], now);
        people.join(id(2), now);
        assert_eq!(people.name_the_silent(now), []);
        let late = now + INTRODUCTION_GRACE;
        assert_eq!(people.name_the_silent(late), [(id(2), name("Player 2"))]);

        let names: Vec<String> = people
            .roster()
            .iter()
            .map(|entry| entry.name.as_str().to_owned())
            .collect();
        assert_eq!(names, ["Mario", "mario 2", "Player 2"]);
        assert_eq!(people.role_of(id(0)), Role::Host);
        assert_eq!(people.role_of(id(1)), Role::Player);
    }

    #[test]
    fn a_bridge_introduces_itself_once() {
        let now = Instant::now();
        let mut people = people(&["Mario", "Luigi"], now);
        // Not the host by saying so later, and not renamed.
        assert_eq!(people.introduce(id(1), Some(&name("Wario")), true), None);
        assert_eq!(people.name_of(id(1)), "Luigi");
        assert_eq!(people.role_of(id(1)), Role::Player);
    }

    #[test]
    fn what_a_player_says_reaches_everyone_once_and_commands_reach_nobody() {
        let now = Instant::now();
        let mut people = people(&["Mario", "Luigi"], now);
        heard(&mut people, id(0));
        heard(&mut people, id(1));

        let hello = message("hello");
        assert_eq!(
            people.say(id(1), 0, &hello, now),
            Spoken::Chat(hello.clone())
        );
        // The same message again, as each is sent more than once.
        assert_eq!(people.say(id(1), 0, &hello, now), Spoken::Nothing);
        assert_eq!(heard(&mut people, id(0)), ["Luigi: hello"]);
        assert_eq!(heard(&mut people, id(1)), ["Luigi: hello"]);

        let list = people.say(id(1), 1, &message("/list"), now);
        assert!(matches!(list, Spoken::Command(ChatCommand::List, _)));
        assert_eq!(
            people.say(id(1), 2, &message("/dance"), now),
            Spoken::Nothing
        );
        assert_eq!(heard(&mut people, id(0)), Vec::<String>::new());
        assert_eq!(heard(&mut people, id(1)).len(), 1);
    }

    #[test]
    fn a_flood_of_messages_is_cut_off_until_time_passes() {
        let now = Instant::now();
        let mut people = people(&["Mario", "Luigi"], now);
        let spam = message("spam");
        let sent = |people: &mut People, ids: std::ops::Range<u16>, when: Instant| {
            ids.filter(|&id_| people.say(id(1), id_, &spam, when) != Spoken::Nothing)
                .count()
        };
        assert_eq!(sent(&mut people, 0..20, now), 5);
        assert_eq!(sent(&mut people, 20..40, now + SAY_INTERVAL), 1);
        assert_eq!(sent(&mut people, 40..60, now + Duration::from_secs(60)), 5);
    }

    #[test]
    fn moderators_are_made_and_unmade_but_never_the_host() {
        let now = Instant::now();
        let mut people = people(&["Mario", "Luigi"], now);
        assert_eq!(people.set_moderator(id(1), true), Ok(()));
        assert_eq!(people.role_of(id(1)), Role::Moderator);
        assert!(people.set_moderator(id(1), true).is_err());
        assert!(
            people
                .set_moderator(id(0), false)
                .is_err_and(|why| why.contains("host"))
        );
        assert_eq!(people.set_moderator(id(1), false), Ok(()));
        assert!(people.set_moderator(id(1), false).is_err());
        assert!(people.set_moderator(id(9), true).is_err());
    }

    #[test]
    fn players_are_found_by_number_or_by_name_in_any_case() {
        let now = Instant::now();
        let people = people(&["Mario", "Luigi"], now);
        assert_eq!(people.find(&Target::Number(1)), Ok(id(1)));
        assert_eq!(people.find(&Target::Name("LUIGI".to_owned())), Ok(id(1)));
        assert!(people.find(&Target::Number(5)).is_err());
        assert!(people.find(&Target::Name("Peach".to_owned())).is_err());
    }

    #[test]
    fn the_roster_goes_when_it_changes_and_leaving_is_announced() {
        let now = Instant::now();
        let mut people = people(&["Mario", "Luigi"], now);
        let rosters = |people: &mut People| {
            let packets = people.packets_for(id(0), false, false);
            packets
                .iter()
                .filter(|packet| matches!(packet, Packet::Roster(_)))
                .count()
        };
        assert_eq!(rosters(&mut people), 1);
        assert_eq!(rosters(&mut people), 0);
        heard(&mut people, id(0));

        people.leave(id(1), "left");
        assert_eq!(rosters(&mut people), 1);
        assert_eq!(heard(&mut people, id(0)), ["Luigi left"]);
        assert_eq!(people.listing(), ["0 Mario (host)"]);
    }
}
