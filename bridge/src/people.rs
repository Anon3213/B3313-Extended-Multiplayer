//! Who the players of a session are to each other, as its bridge knows it: its own player's
//! introduction and outfit, asked for until the server's roster shows them, the roster the game
//! is told of, and the chat the game draws, with its rows, how long they are shown, the line
//! the player is typing there, and the rows before, which Page Up scrolls back to.
//!
//! A line the player sends that is a command for their own game (`travel::command`) is kept
//! for the session to carry out, and never reaches the server.

use crate::client::{Connection, Received, slot_for};
use crate::mailbox::{CHAT_SHOWN_MOST, Mailbox, Person, RemoteSlot};
use crate::ram::ProcessMemory;
use crate::session::{Event, Input, Options};
use crate::text::{self, ChatSize, ROW_BYTES, TEXT_END};
use crate::travel::{self, Order};
use crate::typing::{CHAT_LINES_MIN, Draft, Key};
use b3313_protocol::{Message, Name, Outfit, PlayerId, RosterEntry};
use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// How often a bridge says who its player is, until the server lists them.
const INTRODUCTION_INTERVAL: Duration = Duration::from_secs(1);
/// How often the game is told again who is here, though nothing changed, which puts the names
/// back in a game that was reset.
const PEOPLE_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// How often the game is told that a line is still being typed. It takes that to hold for a
/// second of play (`MP_TYPING_LEASE`), which is a third as long in the test emulators.
const TYPING_RENEW_INTERVAL: Duration = Duration::from_millis(100);
/// How many of the chat's rows are kept to scroll back through.
const HISTORY_ROWS: usize = 200;
/// How far Page Up and Page Down scroll: the rows the game shows, but for the one that says
/// how far back they are.
const SCROLL_STEP: usize = SHOWN_ROWS - 1;
/// `CHAT_SHOWN_MOST`, as a count of rows.
const SHOWN_ROWS: usize = crate::layout::len::CHAT_SHOWN_MOST;

/// Who the players are to each other, as this bridge knows it: its own player's introduction, and
/// the names the server last listed, to put to the chat lines that follow.
pub(crate) struct People {
    name: Option<Name>,
    outfit: Option<Outfit>,
    /// An outfit chosen since joining that the roster does not show yet, and when the server
    /// was last asked for it.
    wanted: Option<(Outfit, Instant)>,
    host_password: String,
    last_introduction: Option<Instant>,
    /// Whether the server's roster has listed this player, so the introduction arrived.
    listed: bool,
    roster: Vec<RosterEntry>,
    /// When the game was last told who is here; None once that has changed.
    told_game: Option<Instant>,
    /// Chat rows the game has yet to be given, and how many it has been given in all.
    rows: Vec<[u8; ROW_BYTES]>,
    chat_count: Option<u32>,
    /// How many of the newest rows the game is to draw while nobody types, and when it is to
    /// stop.
    shown: u8,
    hide_at: Option<Instant>,
    /// The most rows the game draws as lines arrive, as the player chose.
    shown_most: u8,
    /// The character whose key opens the chat in the game, if the keyboard is read for it,
    /// and whether the player has been told of it there.
    chat_key: Option<char>,
    hinted: bool,
    /// How long the game shows the chat's rows after the newest arrived.
    shown_for: Duration,
    /// While joining again: the seconds left that the game's chat was last told of.
    told_left: Option<u64>,
    /// The line the player is typing in the game.
    draft: Draft,
    /// Whether the keys are the chat's and not the game's: while a line is typed, and after it
    /// until the keys that were down as it ended are let go.
    holds_keys: bool,
    /// What the game was last given to draw of the chat: the line being typed, and how many
    /// rows with whether one is; and when, as the last is given again while one is.
    told_draft: Option<[u8; ROW_BYTES]>,
    told_view: Option<(u8, bool)>,
    told_view_at: Option<Instant>,
    /// How large the game draws the chat, which its rows are cut for.
    size: ChatSize,
    /// The rows written to the game, and how far back the player has scrolled them.
    scroll: Scroll,
    /// Commands the player sent for their own game, and what is wrong with any that is not
    /// one, for the session to carry out or say.
    orders: Vec<Result<Order, String>>,
    /// The lines of chat heard since the session last asked, by whom, for a bot.
    heard: Vec<(Option<String>, String)>,
}

/// The chat's rows as written to the game, to scroll back through while a line is typed.
#[derive(Debug, Default)]
struct Scroll {
    /// The rows, oldest first, up to `HISTORY_ROWS`.
    history: VecDeque<[u8; ROW_BYTES]>,
    /// How many rows back from the newest the lowest shown is, and as the game was last given
    /// them: 0 is the newest, as written.
    back: usize,
    told_back: usize,
}

impl Scroll {
    fn add(&mut self, row: [u8; ROW_BYTES]) {
        if self.history.len() == HISTORY_ROWS {
            self.history.pop_front();
        }
        self.history.push_back(row);
        // A row scrolled back to stays in its place as others arrive under it.
        if self.back > 0 {
            self.back = (self.back + 1).min(self.furthest());
        }
    }

    /// The furthest back the rows go: far enough that the oldest is the top one shown, and
    /// none at all if all of them are shown already.
    fn furthest(&self) -> usize {
        let rows = self.history.len();
        if rows > SHOWN_ROWS {
            rows - SCROLL_STEP
        } else {
            0
        }
    }

    fn page(&mut self, older: bool) {
        self.back = if older {
            (self.back + SCROLL_STEP).min(self.furthest())
        } else {
            self.back.saturating_sub(SCROLL_STEP)
        };
    }

    /// The rows the game is to show where the newest are, newest first: at the bottom a row
    /// that says how many are below, then the rows from `back` up; or, back at the newest, the
    /// newest, as they were written.
    fn shown(&self, size: ChatSize) -> Vec<[u8; ROW_BYTES]> {
        let newest = self.history.iter().rev();
        if self.back == 0 {
            return newest.take(SHOWN_ROWS).copied().collect();
        }
        let said = format!("- {} newer below: Page Down", self.back);
        let marker = text::rows(&said, size).into_iter().take(1);
        marker
            .chain(newest.skip(self.back).take(SCROLL_STEP).copied())
            .collect()
    }
}

/// A player in the roster, as the game is told of them.
fn person(entry: &RosterEntry) -> Person<'_> {
    Person {
        role: entry.role,
        outfit: entry.outfit,
        name: entry.name.as_str(),
    }
}

impl People {
    pub(crate) fn new(options: &Options) -> Self {
        Self {
            name: Name::new(&options.name),
            outfit: options.outfit,
            wanted: None,
            host_password: options.host_password.clone(),
            last_introduction: None,
            listed: false,
            roster: Vec::new(),
            told_game: None,
            rows: Vec::new(),
            chat_count: None,
            shown: 0,
            hide_at: None,
            shown_most: options.chat_lines.clamp(CHAT_LINES_MIN, CHAT_SHOWN_MOST),
            chat_key: options.chat_key,
            hinted: false,
            shown_for: Duration::from_secs(u64::from(options.chat_seconds)),
            told_left: None,
            draft: Draft::default(),
            holds_keys: false,
            told_draft: None,
            told_view: None,
            told_view_at: None,
            size: options.chat_size,
            scroll: Scroll::default(),
            orders: Vec::new(),
            heard: Vec::new(),
        }
    }

    /// How large the game draws the chat.
    pub(crate) const fn chat_size(&self) -> ChatSize {
        self.size
    }

    /// The name the roster gives `player`, if it lists them.
    pub(crate) fn name_of(&self, player: PlayerId) -> Option<&str> {
        let entry = self.roster.iter().find(|entry| entry.player == player)?;
        Some(entry.name.as_str())
    }

    /// The outfit `player` wears: the roster's, or, if it does not list them yet, the one their
    /// number gives, which the server gives too.
    pub(crate) fn outfit_of(&self, player: PlayerId) -> Outfit {
        self.roster
            .iter()
            .find(|entry| entry.player == player)
            .map_or_else(|| Outfit::of_player(player), |entry| entry.outfit)
    }

    /// Says `words` in the game's chat, to this player alone, as the bridge's own: for what a
    /// command for their own game did.
    pub(crate) fn say_here(&mut self, words: &str) {
        self.rows
            .extend(text::rows(&format!("- {words}"), self.size));
    }

    /// The commands the player sent for their own game since the last call.
    pub(crate) fn take_orders(&mut self) -> Vec<Result<Order, String>> {
        std::mem::take(&mut self.orders)
    }

    /// The lines of chat heard since the last call, by the name of who said each, or none for
    /// the server.
    pub(crate) fn take_heard(&mut self) -> Vec<(Option<String>, String)> {
        std::mem::take(&mut self.heard)
    }

    /// Sends a line the player typed, or, if it is a command for their own game, keeps it for
    /// the session: `/help` goes to the server, and has those commands said here as well.
    fn route(&mut self, line: Message, connection: &mut Connection) {
        if let Some(order) = travel::command(line.as_str()) {
            self.orders.push(order);
            return;
        }
        if line.as_str().trim().eq_ignore_ascii_case("/help") {
            self.say_here(travel::COMMANDS_HELP);
        }
        connection.queue_say(line);
    }

    /// Whether the player is typing a line in the game.
    pub(crate) fn is_typing(&self) -> bool {
        self.draft.is_open()
    }

    /// Whether the keys are the chat's and not the game's just now.
    pub(crate) fn holds_keys(&self) -> bool {
        self.holds_keys
    }

    /// Whether the server's roster has listed this player.
    #[cfg(test)]
    pub(crate) fn is_listed(&self) -> bool {
        self.listed
    }

    /// Everyone the server last listed.
    #[cfg(test)]
    pub(crate) fn roster(&self) -> &[RosterEntry] {
        &self.roster
    }

    /// The game was started again: what it was told and shown is gone from it, and its chat
    /// ring counts from nothing, so all of that is given anew.
    pub(crate) fn game_started_again(&mut self) {
        self.told_game = None;
        self.chat_count = None;
        self.told_draft = None;
        self.told_view = None;
        self.told_view_at = None;
        self.scroll.back = 0;
        self.scroll.told_back = 0;
    }

    /// The server is gone, for now: nobody is listed, a line being typed is given up, and the
    /// game is told why the others have gone.
    pub(crate) fn lose_link(&mut self) {
        self.roster.clear();
        self.listed = false;
        self.told_game = None;
        self.wanted = None;
        self.draft.close();
        self.holds_keys = false;
        self.told_left = None;
        self.say_here("Connection lost.");
    }

    /// Says in the game how long the bridge will go on trying to join again: `left` seconds,
    /// in a row of its own that is written over as the time runs down, so that the count
    /// does not push the chat off the screen. The row stays shown for as long as it changes.
    pub(crate) fn tell_time_left<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        left: u64,
        now: Instant,
    ) -> io::Result<()> {
        if self.told_left == Some(left) {
            return Ok(());
        }
        let said = format!("- Joining again, {}:{:02} left.", left / 60, left % 60);
        let Some(row) = text::rows(&said, self.size).into_iter().next() else {
            return Ok(());
        };
        match (self.told_left, self.chat_count) {
            (Some(_), Some(count)) => {
                mailbox.rewrite_chat_row(count, &row)?;
                self.shown = self.shown.max(1);
                self.hide_at = Some(now + self.shown_for);
            }
            // The first time, a row like any other, which `show` writes.
            _ => self.rows.push(row),
        }
        self.told_left = Some(left);
        Ok(())
    }

    /// The server is joined again, as a player it has yet to be told the name of.
    pub(crate) fn rejoined(&mut self) {
        self.last_introduction = None;
        self.told_left = None;
        self.say_here("Joined again.");
    }

    /// Takes the keys pressed for the chat in the game: they open a line, edit it, and send
    /// it. `keys` is none while the game's window does not have the keyboard. `held` says
    /// whether a key that was down as the last line ended still is, and `playing` whether the
    /// game is in play, with its chat on the screen.
    pub(crate) fn take_keys(
        &mut self,
        keys: Option<&[Key]>,
        held: bool,
        playing: bool,
        connection: &mut Connection,
        now: Instant,
    ) {
        // Said once, and where it will be seen: no manual is open beside a game.
        if playing
            && let Some(key) = self.chat_key
            && !std::mem::replace(&mut self.hinted, true)
        {
            let key = key.to_ascii_uppercase();
            self.say_here(&format!("Press {key} to chat, or / for a command."));
        }
        let Some(keys) = keys else {
            // The player has gone to another window. A line left open would keep the
            // controller from Mario for as long as they are away, with nobody typing it.
            self.draft.close();
            self.holds_keys = false;
            self.scroll.back = 0;
            return;
        };
        let was_open = self.draft.is_open();
        for &key in keys {
            // A line is begun only in play: before that the game shows no chat to type it in,
            // and would only seem to have stopped taking the controller.
            if !playing && !self.draft.is_open() {
                continue;
            }
            match key {
                Key::PageUp | Key::PageDown => self.scroll.page(key == Key::PageUp),
                _ => {
                    if let Some(line) = self.draft.press(key, now) {
                        self.route(line, connection);
                    }
                }
            }
        }
        // Closed, the chat shows its newest rows again.
        if !self.draft.is_open() {
            self.scroll.back = 0;
        }
        // A line that ended just now still holds the keys: which of them are down is only
        // known at the next look, and the Enter that sent it is among them.
        self.holds_keys = self.draft.is_open() || was_open || held;
    }

    /// Tells the game what to draw of the chat, when that changes: the line being typed, how
    /// many rows, and whether a line is typed, which it is told again and again while one is.
    /// `hovering` says the mouse rests on the chat, which shows its rows as typing does.
    fn show_chat<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        now: Instant,
        hovering: bool,
    ) -> io::Result<()> {
        // The line before the word that there is one, so the game never draws the last one.
        let draft = self
            .draft
            .row(now, self.size)
            .unwrap_or([TEXT_END; ROW_BYTES]);
        if self.told_draft != Some(draft) {
            mailbox.write_chat_draft(&draft)?;
            self.told_draft = Some(draft);
        }
        // While a line is typed, or the mouse rests on the chat, every row there is room for,
        // however old.
        let rows = if self.draft.is_open() || hovering {
            CHAT_SHOWN_MOST
        } else {
            self.shown
        };
        let view = (rows, self.holds_keys);
        let is_renewal_due = self.holds_keys
            && self
                .told_view_at
                .is_none_or(|told| now.saturating_duration_since(told) >= TYPING_RENEW_INTERVAL);
        if self.told_view != Some(view) || is_renewal_due {
            mailbox.show_chat(rows, self.holds_keys, self.size)?;
            self.told_view = Some(view);
            self.told_view_at = Some(now);
        }
        Ok(())
    }

    /// Puts the rows the player has scrolled to where the game draws the newest, when that
    /// changes, and the newest back when they scroll down to them again.
    fn show_scrolled<M: ProcessMemory>(&mut self, mailbox: &Mailbox<M>) -> io::Result<()> {
        if self.scroll.back == self.scroll.told_back {
            return Ok(());
        }
        let count = match self.chat_count {
            Some(count) => count,
            None => mailbox.chat_count()?,
        };
        self.chat_count = Some(count);
        mailbox.rewrite_chat_rows(count, &self.scroll.shown(self.size))?;
        self.scroll.told_back = self.scroll.back;
        Ok(())
    }

    /// Gives the game what it draws of all this: who is here, when that changes and now and
    /// then, the chat's new rows, which it shows for as long as the player chose, and the line
    /// being typed, with the rows scrolled back to. `hovering` says the mouse rests on the
    /// chat's corner of the game's window.
    pub(crate) fn show<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        me: PlayerId,
        now: Instant,
        hovering: bool,
    ) -> io::Result<()> {
        let is_due = self
            .told_game
            .is_none_or(|told| now.saturating_duration_since(told) >= PEOPLE_REFRESH_INTERVAL);
        if is_due && let Some(mine) = self.roster.iter().find(|entry| entry.player == me) {
            let others: Vec<(RemoteSlot, Person<'_>)> = self
                .roster
                .iter()
                .filter_map(|entry| Some((slot_for(me, entry.player)?, person(entry))))
                .collect();
            mailbox.write_people(&person(mine), &others)?;
            self.told_game = Some(now);
        }
        if !self.rows.is_empty() {
            let mut count = match self.chat_count {
                Some(count) => count,
                None => mailbox.chat_count()?,
            };
            for row in self.rows.drain(..) {
                count = mailbox.write_chat_row(count, &row)?;
                self.scroll.add(row);
                self.shown = self.shown.saturating_add(1).min(self.shown_most);
            }
            self.chat_count = Some(count);
            self.hide_at = Some(now + self.shown_for);
            // The new rows went where the newest are shown; those scrolled to go back over
            // them.
            if self.scroll.back > 0 {
                self.scroll.told_back = 0;
            }
        }
        if self.hide_at.is_some_and(|hide_at| now >= hide_at) {
            self.shown = 0;
            self.hide_at = None;
        }
        self.show_scrolled(mailbox)?;
        self.show_chat(mailbox, now, hovering)
    }

    /// Introduces this player until the server lists them, queues what they have typed, and
    /// asks for the outfit they chose until the server's roster shows it.
    pub(crate) fn send(
        &mut self,
        connection: &mut Connection,
        input: &Receiver<Input>,
        now: Instant,
    ) -> io::Result<()> {
        let due = self
            .last_introduction
            .is_none_or(|sent| now.saturating_duration_since(sent) >= INTRODUCTION_INTERVAL);
        if !self.listed && due {
            connection.introduce(self.name.as_ref(), self.outfit, &self.host_password)?;
            self.last_introduction = Some(now);
        }
        for given in input.try_iter() {
            match given {
                // What cannot be sent, as a line with no plain letters in it, is dropped.
                Input::Say(typed) => {
                    if let Some(text) = Message::new(&typed) {
                        self.route(text, connection);
                    }
                }
                Input::Wear(outfit) => {
                    // Also what an introduction still to be heard says, so the two agree.
                    self.outfit = Some(outfit);
                    connection.wear(outfit)?;
                    self.wanted = Some((outfit, now));
                }
            }
        }
        // The roster is the only answer, so a lost request shows as one that is never worn.
        if let Some((outfit, asked)) = self.wanted
            && self.listed
            && now.saturating_duration_since(asked) >= INTRODUCTION_INTERVAL
        {
            connection.wear(outfit)?;
            self.wanted = Some((outfit, now));
        }
        Ok(())
    }

    /// Takes the roster and the chat lines the server sent, and reports them.
    pub(crate) fn receive(
        &mut self,
        me: PlayerId,
        received: &mut Received,
        events: &mut impl FnMut(Event),
    ) {
        if let Some(roster) = received.roster.take() {
            let mine = roster.iter().find(|entry| entry.player == me);
            self.listed = mine.is_some();
            if self.wanted.map(|(outfit, _)| outfit) == mine.map(|entry| entry.outfit) {
                self.wanted = None;
            }
            // What they wear now is what they come back in, should they have to join again:
            // under another number, the server would give them another.
            if let Some(mine) = mine {
                self.outfit = Some(mine.outfit);
            }
            self.roster.clone_from(&roster);
            self.told_game = None;
            events(Event::Roster(roster));
        }
        for line in received.said.drain(..) {
            let name = line.from.and_then(|from| {
                let entry = self.roster.iter().find(|entry| entry.player == from)?;
                Some(entry.name.as_str().to_owned())
            });
            // In the game, a player's line begins with their name, and the server's own with a
            // dash, as there is no other way there to tell them apart.
            let shown = match &name {
                Some(name) => format!("{name}: {}", line.text.as_str()),
                None => format!("- {}", line.text.as_str()),
            };
            self.rows.extend(text::rows(&shown, self.size));
            self.heard
                .push((name.clone(), line.text.as_str().to_owned()));
            // A bot that is never asked for them is no reason to keep them without end.
            if self.heard.len() > HISTORY_ROWS {
                self.heard.remove(0);
            }
            events(Event::Said {
                from: line.from,
                // A player who said something and left before the roster came has no name here.
                name: name.unwrap_or_default(),
                text: line.text.as_str().to_owned(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{People, TYPING_RENEW_INTERVAL};
    use crate::client::Connection;
    use crate::layout;
    use crate::mailbox::Mailbox;
    use crate::mailbox::testing::{game, make_ready, raw};
    use crate::ram::N64Ram;
    use crate::session::testing::{
        CHAT_COUNT, CHAT_DRAFT, CHAT_ROWS, CHAT_SHOWN, options, text_at,
    };
    use crate::session::{Event, Input, Options};
    use crate::text::{self, ChatSize};
    use crate::typing::Key;
    use b3313_protocol::{Outfit, PlayerId, RosterEntry};
    use b3313_server::{Config, StopOnDrop, serve};
    use std::io;
    use std::net::{Ipv4Addr, UdpSocket};
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    /// The player's side of a session with no game: what `run` does with names and outfits,
    /// against a real server, until `done` says the roster and the state are as wanted.
    fn play_until(
        people: &mut People,
        connection: &mut Connection,
        input: &mpsc::Receiver<Input>,
        done: impl Fn(&People, &[RosterEntry]) -> bool,
    ) -> io::Result<bool> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut roster = Vec::new();
        while Instant::now() < deadline {
            people.send(connection, input, Instant::now())?;
            let mut received = connection.poll(Instant::now() + Duration::from_millis(33))?;
            people.receive(connection.player(), &mut received, &mut |event| {
                if let super::Event::Roster(listed) = event {
                    roster = listed;
                }
            });
            if done(people, &roster) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    #[test]
    fn an_outfit_chosen_in_a_session_is_asked_for_until_the_roster_shows_it() -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server = socket.local_addr()?;
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| serve(&socket, Config::default(), &stop, |_| {}));
            let _stopping = StopOnDrop(&stop);
            let options = Options {
                rom: Vec::new(),
                pid: 0,
                server,
                server_name: None,
                server_key: None,
                password: String::new(),
                name: "Luigi".to_owned(),
                outfit: Outfit::named("green"),
                host_password: String::new(),
                chat_key: None,
                chat_seconds: 8,
                chat_lines: 6,
                chat_on_hover: false,
                chat_size: ChatSize::Normal,
                minimap: true,
                bot_port: None,
            };
            let mut people = People::new(&options);
            let mut connection = Connection::join(server, "", 0)?;
            let me = connection.player();
            let (choose, input) = mpsc::channel();
            let wears = |roster: &[RosterEntry], name: &str| {
                let mine = roster.iter().find(|entry| entry.player == me);
                mine.map(|entry| entry.outfit) == Outfit::named(name)
            };

            // He comes in the green he chose before joining.
            let came = play_until(&mut people, &mut connection, &input, |_, roster| {
                wears(roster, "green")
            })?;
            assert!(came);

            // A choice made now is worn, and once it is, no longer asked for.
            let gold = Outfit::named("gold").ok_or_else(|| io::Error::other("no gold"))?;
            choose
                .send(Input::Wear(gold))
                .map_err(|_| io::Error::other("the session's input closed"))?;
            let changed = play_until(&mut people, &mut connection, &input, |people, roster| {
                wears(roster, "gold") && people.wanted.is_none()
            })?;
            assert!(changed);
            Ok(())
        })
    }

    #[test]
    fn the_game_shows_chat_for_as_long_as_the_player_chose_and_no_longer() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let server = (Ipv4Addr::LOCALHOST, 9).into();
        let chosen = Options {
            chat_seconds: 20,
            ..options(server)
        };
        let mut people = People::new(&chosen);
        let me = PlayerId::new(0).ok_or_else(|| io::Error::other("no player 0"))?;
        let start = Instant::now();
        let shown = || Ok::<_, io::Error>(raw(&process, CHAT_SHOWN, 4)?.first().copied());

        people
            .rows
            .extend(text::rows("Luigi: hello", ChatSize::Normal));
        people.show(&mailbox, me, start, false)?;
        assert_eq!(shown()?, Some(1));
        people.show(&mailbox, me, start + Duration::from_secs(19), false)?;
        assert_eq!(shown()?, Some(1));
        people.show(&mailbox, me, start + Duration::from_secs(20), false)?;
        assert_eq!(shown()?, Some(0));
        // The mouse on the chat shows it again, all there is room for, for as long as it rests
        // there.
        let later = start + Duration::from_secs(30);
        people.show(&mailbox, me, later, true)?;
        assert_eq!(shown()?, Some(6));
        people.show(&mailbox, me, later, false)?;
        assert_eq!(shown()?, Some(0));
        Ok(())
    }

    #[test]
    fn the_game_shows_no_more_rows_at_once_than_the_player_chose() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let server = (Ipv4Addr::LOCALHOST, 9).into();
        let chosen = Options {
            chat_lines: 2,
            ..options(server)
        };
        let mut people = People::new(&chosen);
        let me = PlayerId::new(0).ok_or_else(|| io::Error::other("no player 0"))?;
        let now = Instant::now();
        let shown = || Ok::<_, io::Error>(raw(&process, CHAT_SHOWN, 4)?.first().copied());

        for line in ["Luigi: one", "Luigi: two", "Luigi: three"] {
            people.rows.extend(text::rows(line, ChatSize::Normal));
        }
        people.show(&mailbox, me, now, false)?;
        assert_eq!(shown()?, Some(2));
        // The mouse on the chat, as opening it, still shows all there is room for.
        people.show(&mailbox, me, now, true)?;
        assert_eq!(shown()?, Some(6));
        Ok(())
    }

    #[test]
    fn page_up_scrolls_the_open_chat_back_and_it_comes_back_down_as_it_closes() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let server = (Ipv4Addr::LOCALHOST, 9).into();
        let mut people = People::new(&options(server));
        let me = PlayerId::new(0).ok_or_else(|| io::Error::other("no player 0"))?;
        let now = Instant::now();
        for line in 1..=12 {
            people.say_here(&format!("line {line}"));
        }
        people.show(&mailbox, me, now, false)?;
        // The newest rows, newest at the bottom.
        let shown_rows = |count: u32| -> io::Result<Vec<String>> {
            (1..=6)
                .map(|back| {
                    let slot = usize::try_from((count - back) % 8).unwrap_or(0);
                    text_at(&process, CHAT_ROWS + slot * text::ROW_BYTES)
                })
                .collect()
        };
        assert_eq!(
            shown_rows(12)?.first().map(String::as_str),
            Some("- line 12")
        );

        people.draft.press(Key::Chat, now);
        people.scroll.page(true);
        people.show(&mailbox, me, now, false)?;
        assert_eq!(
            shown_rows(12)?,
            [
                "- 5 newer below: Page Down",
                "- line 7",
                "- line 6",
                "- line 5",
                "- line 4",
                "- line 3"
            ]
        );
        // As far back as the first line, and no further.
        people.scroll.page(true);
        people.scroll.page(true);
        people.show(&mailbox, me, now, false)?;
        assert_eq!(shown_rows(12)?.last().map(String::as_str), Some("- line 1"));

        // A line arriving meanwhile goes under, and what was scrolled to stays.
        people.say_here("line 13");
        people.show(&mailbox, me, now, false)?;
        assert_eq!(shown_rows(13)?.last().map(String::as_str), Some("- line 1"));
        assert_eq!(
            shown_rows(13)?.first().map(String::as_str),
            Some("- 8 newer below: Page Down")
        );

        // Closed, the newest are where they were.
        people.draft.close();
        people.scroll.back = 0;
        people.show(&mailbox, me, now, false)?;
        assert_eq!(
            shown_rows(13)?.first().map(String::as_str),
            Some("- line 13")
        );
        assert_eq!(shown_rows(13)?.last().map(String::as_str), Some("- line 8"));
        Ok(())
    }

    #[test]
    fn a_command_for_the_players_own_game_is_kept_and_large_text_is_told_to_the_game()
    -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let server = (Ipv4Addr::LOCALHOST, 9).into();
        let large = Options {
            chat_size: ChatSize::Large,
            ..options(server)
        };
        let mut people = People::new(&large);
        let me = PlayerId::new(0).ok_or_else(|| io::Error::other("no player 0"))?;
        let (typed, input) = mpsc::channel();
        typed
            .send(Input::Say("/warp 9 2".into()))
            .map_err(|_| io::Error::other("closed"))?;
        typed
            .send(Input::Say("/warp nowhere".into()))
            .map_err(|_| io::Error::other("closed"))?;
        // No server is joined here: a line for one would need a connection, which `send` is
        // given only to queue it. The commands never reach it.
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            let address = socket.local_addr()?;
            scope.spawn(|| serve(&socket, Config::default(), &stop, |_| {}));
            let _stopping = StopOnDrop(&stop);
            let mut connection = Connection::join(address, "", 0)?;
            people.send(&mut connection, &input, Instant::now())?;
            let orders = people.take_orders();
            assert_eq!(
                orders.first(),
                Some(&Ok(crate::travel::Order::Warp {
                    level: 9,
                    area: Some(2)
                }))
            );
            assert!(matches!(orders.get(1), Some(Err(_))));
            Ok::<_, io::Error>(())
        })?;
        people.draft.press(Key::Chat, Instant::now());
        people.show(&mailbox, me, Instant::now(), false)?;
        assert_eq!(raw(&process, CHAT_SHOWN, 4)?.get(2), Some(&1));
        Ok(())
    }

    /// Where the game keeps the rows shown, as an address, to write there as the game does.
    const CHAT_SHOWN_ADDRESS: u32 =
        layout::MAILBOX_ADDR + layout::MAILBOX_CHAT_OFFSET + layout::CHAT_SHOWN_OFFSET;

    #[test]
    fn a_line_typed_in_the_game_is_drawn_there_and_reaches_the_chat() -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server = socket.local_addr()?;
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| serve(&socket, Config::default(), &stop, |_| {}));
            let _stopping = StopOnDrop(&stop);
            let process = game();
            make_ready(&process)?;
            let mailbox = Mailbox::new(N64Ram::new(&process, 0));
            let mut people = People::new(&options(server));
            let mut connection = Connection::join(server, "", 0)?;
            let me = connection.player();
            let now = Instant::now();
            let view = || raw(&process, CHAT_SHOWN, 4);

            // Before the game is in play there is no chat to type into: the key does nothing.
            people.take_keys(Some(&[Key::Chat]), false, false, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(view()?.get(1), Some(&0));

            // In play, it opens the chat: the game draws a box, with the line under every
            // row it has, and keeps the keys from Mario.
            people.take_keys(Some(&[Key::Chat]), false, true, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(view()?, [6, 30, 0, 0]);
            assert_eq!(text_at(&process, CHAT_DRAFT)?, "> _");

            // The player turns to another window: the line is given up, and Mario has the
            // controller back, as nobody is typing.
            people.take_keys(None, false, true, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(text_at(&process, CHAT_DRAFT)?, "");
            assert_eq!(view()?.get(1), Some(&0));

            people.take_keys(Some(&[Key::Chat]), false, true, &mut connection, now);
            let typed = [Key::Character('h'), Key::Character('i')];
            people.take_keys(Some(&typed), false, true, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(text_at(&process, CHAT_DRAFT)?, "> hi_");

            // Enter sends it. The box goes at once, but Enter is still down, and is the
            // game's Start button: the game is shown no key until it is let go.
            people.take_keys(Some(&[Key::Enter]), false, true, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(text_at(&process, CHAT_DRAFT)?, "");
            assert_eq!(view()?.get(1), Some(&30));
            people.take_keys(Some(&[]), true, true, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(view()?.get(1), Some(&30));
            people.take_keys(Some(&[]), false, true, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(view()?.get(1), Some(&0));

            // And the line comes back from the server as chat, as one typed anywhere does.
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut heard = Vec::new();
            while heard.is_empty() && Instant::now() < deadline {
                people.send(&mut connection, &mpsc::channel().1, Instant::now())?;
                connection.send_says()?;
                let mut received = connection.poll(Instant::now() + Duration::from_millis(33))?;
                // From a player: the server's own line that this one joined is not it.
                people.receive(me, &mut received, &mut |event| {
                    if let Event::Said {
                        from: Some(_),
                        text,
                        ..
                    } = event
                    {
                        heard.push(text);
                    }
                });
            }
            assert_eq!(heard, ["hi"]);
            Ok(())
        })
    }

    #[test]
    fn the_game_is_told_a_line_is_being_typed_for_as_long_as_one_is() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let server = (Ipv4Addr::LOCALHOST, 9).into();
        let mut people = People::new(&options(server));
        let me = PlayerId::new(0).ok_or_else(|| io::Error::other("no player 0"))?;
        let start = Instant::now();
        // Opened without a connection to say it through: `take_keys` would want one.
        people.draft.press(Key::Chat, start);
        people.holds_keys = true;
        people.show(&mailbox, me, start, false)?;
        let ram = N64Ram::new(&process, 0);

        // The game counts it down as it runs; it is put back before it runs out, and not
        // at every turn of the bridge's loop.
        ram.write(CHAT_SHOWN_ADDRESS, &[6, 5, 0, 0])?;
        people.show(&mailbox, me, start + Duration::from_millis(1), false)?;
        assert_eq!(raw(&process, CHAT_SHOWN, 4)?.get(1), Some(&5));
        people.show(&mailbox, me, start + TYPING_RENEW_INTERVAL, false)?;
        assert_eq!(raw(&process, CHAT_SHOWN, 4)?.get(1), Some(&30));
        Ok(())
    }

    #[test]
    fn the_key_that_opens_the_chat_is_told_once_in_the_game() -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server = socket.local_addr()?;
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| serve(&socket, Config::default(), &stop, |_| {}));
            let _stopping = StopOnDrop(&stop);
            let process = game();
            make_ready(&process)?;
            let mailbox = Mailbox::new(N64Ram::new(&process, 0));
            let mut people = People::new(&options(server));
            let mut connection = Connection::join(server, "", 0)?;
            let me = connection.player();
            let now = Instant::now();
            let count = || Ok::<_, io::Error>(raw(&process, CHAT_COUNT, 4)?.last().copied());

            // Not in the menus before play, where it would be gone before it could be used.
            people.take_keys(Some(&[]), false, false, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(count()?, Some(0));

            people.take_keys(Some(&[]), false, true, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(count()?, Some(1));
            assert!(text_at(&process, CHAT_ROWS)?.contains("Press T to chat"));

            people.take_keys(Some(&[]), false, true, &mut connection, now);
            people.show(&mailbox, me, now, false)?;
            assert_eq!(count()?, Some(1));
            Ok(())
        })
    }

    #[test]
    fn the_time_left_to_join_again_counts_down_in_one_row_of_the_chat() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let server = (Ipv4Addr::LOCALHOST, 9).into();
        let mut people = People::new(&options(server));
        let me = PlayerId::new(0).ok_or_else(|| io::Error::other("no player 0"))?;
        let start = Instant::now();
        let count = || Ok::<_, io::Error>(raw(&process, CHAT_COUNT, 4)?.last().copied());
        let second_row = CHAT_ROWS + text::ROW_BYTES;

        people.lose_link();
        people.tell_time_left(&mailbox, 120, start)?;
        people.show(&mailbox, me, start, false)?;
        assert_eq!(text_at(&process, CHAT_ROWS)?, "- Connection lost.");
        assert_eq!(
            text_at(&process, second_row)?,
            "- Joining again, 2:00 left."
        );
        assert_eq!(count()?, Some(2));

        // Ten seconds on: the same row says so, no new one, and it is still on the screen
        // though longer has passed than a row is shown for.
        let later = start + Duration::from_secs(10);
        people.tell_time_left(&mailbox, 110, later)?;
        people.show(&mailbox, me, later, false)?;
        assert_eq!(
            text_at(&process, second_row)?,
            "- Joining again, 1:50 left."
        );
        assert_eq!(count()?, Some(2));
        assert!(
            raw(&process, CHAT_SHOWN, 4)?
                .first()
                .is_some_and(|&rows| rows > 0)
        );
        Ok(())
    }
}
