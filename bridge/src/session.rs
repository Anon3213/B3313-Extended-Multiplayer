//! One multiplayer session: attaches to a running game, joins a server, and moves players,
//! hits, objects and progress between them until the session ends. The command-line bridge and
//! the panel's Play page both run it, each showing what it reports in its own way.

use crate::client::{
    Connection, JOIN_TIMEOUT, Received, ShownPlayers, apply_snapshot, clear_all, player_for,
    slot_for,
};
use crate::interpolation::Interpolator;
use crate::keyboard::{Keyboard, WindowsKeys};
use crate::mailbox::{CHAT_SHOWN_MOST, Mailbox, Person, RemoteSlot};
use crate::objects::ObjectSync;
use crate::pacing::{FRAME_INTERVAL, SEND_INTERVAL, Schedule};
use crate::process::WindowsProcess;
use crate::progress::ProgressSync;
use crate::ram::{N64Ram, ProcessMemory, Search, attach};
use crate::text::{self, ROW_BYTES, TEXT_END};
use crate::typing::{Draft, Key};
use b3313_protocol::metrics::LinkReport;
use b3313_protocol::{
    HitKind, Hurt, Message, Name, Outfit, PlayerId, RemoveReason, RosterEntry, WorldInputs,
};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

const WORLD_OFFER_INTERVAL: Duration = Duration::from_secs(1);
/// How often a bridge says who its player is, until the server lists them.
const INTRODUCTION_INTERVAL: Duration = Duration::from_secs(1);
/// How often the game is told again who is here, though nothing changed, which puts the names
/// back in a game that was reset.
const PEOPLE_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
/// How often the game is told that a line is still being typed. It takes that to hold for a
/// second of play (`MP_TYPING_LEASE`), which is a third as long in the test emulators.
const TYPING_RENEW_INTERVAL: Duration = Duration::from_millis(100);
/// How long a bridge whose server has gone silent keeps trying to join it again: long enough
/// for a server to be started again, or a connection to come back, and not for ever. Each try
/// is short, as the game is seen to between them.
const REJOIN_FOR: Duration = Duration::from_secs(120);
const REJOIN_ATTEMPT: Duration = Duration::from_secs(1);
/// A game that has not published its Mario for this many send slots, half a second, has
/// stopped running its objects. It publishes once a frame, about once a slot, so a slot or
/// two without is nothing.
const STALLED_AFTER: u32 = 15;
/// A game that has not marked its mailbox for this long is not about to: it starts within a
/// few seconds, and one being started again is back sooner.
const WRONG_ROM_AFTER: Duration = Duration::from_secs(10);
/// A failure this soon after the last is one of a run, and a run longer than `MOST_FAULTS`
/// ends the session: joining again does not mend whatever fails that often.
const FAULT_WINDOW: Duration = Duration::from_secs(10);
const MOST_FAULTS: u32 = 3;
/// The game's chat says how long is left to join again in, in steps of this many seconds.
const COUNTDOWN_STEP: u64 = 10;
const BOOT_TIMEOUT: Duration = Duration::from_secs(60);
const BOOT_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// How long the game is looked for only in the memory the emulator has in use, and how long
/// between the searches of all its memory after that: see `Search`. A game boots well within
/// this, so one that starts with its bridge is found the cheap way; only a game long running
/// whose code Windows has taken out of memory waits this long and pays for a full search.
const FULL_SEARCH_EVERY: Duration = Duration::from_secs(20);

/// What a session needs: the multiplayer ROM's bytes, to find the game in the emulator's
/// memory, the emulator's process, and the server to join.
#[derive(Clone, Debug)]
pub struct Options {
    pub rom: Vec<u8>,
    pub pid: u32,
    pub server: SocketAddr,
    pub password: String,
    /// The name the player chose; empty leaves it to the server, which makes one up.
    pub name: String,
    /// The outfit the player chose; none leaves it to the server, which gives one by number.
    pub outfit: Option<Outfit>,
    /// The host's own password, which makes this player the host; empty for everyone else.
    pub host_password: String,
    /// The character whose key opens the chat in the game, where the player then types a
    /// line; none leaves the keyboard unread, and the chat to the panel or the console.
    pub chat_key: Option<char>,
    /// How many seconds the game shows the chat's rows after the newest arrived. Timed by
    /// the bridge, in real time: the game's own rate varies with the emulator.
    pub chat_seconds: u8,
}

/// What a session reports as it goes.
#[derive(Clone, Debug)]
pub enum Event {
    /// The emulator is open but its game has not started yet.
    WaitingForGame,
    /// The game is running; joining the server.
    Joining,
    /// In the session, as this player. Again after `Reconnecting`, and not always as the same.
    Joined(PlayerId),
    /// The server went silent, or is being started again; joining it again, for this long
    /// at most.
    Reconnecting(Duration),
    /// In the session, but the game running is not one this bridge can show it in: it marked
    /// itself with this mailbox version, or with none. Said once; `describe_wrong_rom`
    /// puts it in words.
    WrongRom(Option<u32>),
    /// The local player hit another.
    Hit(PlayerId, HitKind),
    /// Another player hit the local one.
    HitBy(PlayerId, HitKind),
    /// How the link went over the last few seconds.
    Link(Box<LinkReport>),
    /// Everyone in the session, with their roles and names, whenever the server sends it.
    Roster(Vec<RosterEntry>),
    /// A line of chat: from a player, with their name, or from the server itself.
    Said {
        from: Option<PlayerId>,
        name: String,
        text: String,
    },
}

/// What `Event::WrongRom` means for the player, and what to do about it.
#[must_use]
pub fn describe_wrong_rom(found: Option<u32>) -> String {
    match found {
        Some(version) => format!(
            "The game running is built for another version of this program (its mailbox is \
             version {version}, and this one's is {}), so the session cannot be shown in it. \
             Set the game up again on the panel's Play page, or start the ROM this version \
             of the panel built.",
            crate::mailbox::VERSION
        ),
        None => "The game running is not the multiplayer ROM, so the session cannot be shown \
                 in it. Start the multiplayer ROM."
            .to_owned(),
    }
}

/// Why a session ended, once it had joined.
#[derive(Debug)]
pub enum End {
    /// Asked to stop, as by the panel's Leave or Ctrl+C at the command line.
    Left,
    GameClosed(io::Error),
    /// The server went silent, and stayed so for as long as the bridge tried to join again.
    ServerSilent,
    Removed(RemoveReason),
    /// The server was there again but would not have this player back.
    Refused(io::Error),
    /// The network failed again each time the bridge had joined again, with the game still
    /// running.
    NetworkFailing(io::Error),
}

impl End {
    /// What it means for the player, in a sentence.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Left => "You left the session.".to_owned(),
            // What the system said of it is `detail`: long, and the same whenever a game closes.
            Self::GameClosed(_) => "The game closed.".to_owned(),
            Self::ServerSilent => "The server stopped answering.".to_owned(),
            // Not "the host": a moderator may remove a player, and the server's owner, who
            // need not be playing, may end a session.
            Self::Removed(RemoveReason::Kicked) => "You were removed from the session.".to_owned(),
            Self::Removed(RemoveReason::Banned) => "You were banned from the session.".to_owned(),
            Self::Removed(RemoveReason::Ended) => "The session was ended.".to_owned(),
            // Only a bridge that could not join again ends on this; one that can, does.
            Self::Removed(RemoveReason::Restarting) => "The server is starting again.".to_owned(),
            Self::Refused(error) => format!("Could not join the session again: {error}."),
            Self::NetworkFailing(error) => format!("The connection kept failing: {error}."),
        }
    }

    /// What the system said when the game could no longer be reached, for whoever wants to
    /// know more than that it closed.
    #[must_use]
    pub fn detail(&self) -> Option<String> {
        match self {
            Self::GameClosed(error) => Some(error.to_string()),
            Self::Left
            | Self::ServerSilent
            | Self::Removed(_)
            | Self::Refused(_)
            | Self::NetworkFailing(_) => None,
        }
    }
}

/// What the player does in the session from outside the game.
#[derive(Clone, Debug)]
pub enum Input {
    /// A line typed for the chat.
    Say(String),
    /// An outfit chosen to wear from now on.
    Wear(Outfit),
}

/// Runs a session until it ends or `stop` is set, reporting to `events` as it goes. What the
/// player types for the chat, and the outfits they choose, come in on `input`.
///
/// # Errors
/// Fails before joining: the emulator cannot be opened, the game is not found in it in time,
/// or the server cannot be joined, as with a wrong password.
pub fn run(
    options: &Options,
    stop: &AtomicBool,
    input: &Receiver<Input>,
    mut events: impl FnMut(Event),
) -> io::Result<End> {
    let process = WindowsProcess::open(options.pid)?;
    let Some(ram) = attach_when_running(&process, &options.rom, stop, &mut events)? else {
        return Ok(End::Left);
    };
    let mailbox = Mailbox::new(ram);
    events(Event::Joining);
    let joined = Connection::join_unless(
        options.server,
        &options.password,
        mailbox.game()?,
        JOIN_TIMEOUT,
        stop,
    );
    let mut connection = match joined {
        Ok(connection) => connection,
        // Left before the server answered.
        Err(error) if error.kind() == io::ErrorKind::Interrupted => return Ok(End::Left),
        Err(error) => return Err(error),
    };
    events(Event::Joined(connection.player()));
    let mut session = Session::new(Instant::now());
    let mut people = People::new(options);
    let mut keyboard = options
        .chat_key
        .map(|key| Keyboard::new(WindowsKeys::new(options.pid), key));
    let mut faults = Faults::default();
    let end = loop {
        if stop.load(Ordering::Relaxed) {
            break End::Left;
        }
        let now = Instant::now();
        if let Some(keyboard) = &mut keyboard {
            let keys = keyboard.poll(people.draft.is_open(), now);
            let keys = keyboard.has_keyboard().then_some(keys.as_slice());
            let held = keyboard.is_settling(now);
            people.take_keys(keys, held, session.playing, &mut connection, now);
        }
        let stepped = people
            .send(&mut connection, input, now)
            .and_then(|()| session.step(&mailbox, &mut connection, now, &mut people, &mut events));
        let is_lost = match stepped {
            // The game's memory or the network failed. With the game still there to read, it
            // was the network, as when Wi-Fi drops: the way back is to join again.
            Err(error) if mailbox.is_ready().is_err() => break End::GameClosed(error),
            // Unless it fails again as soon as it has joined again, over and over: that is
            // no network coming back. The game is still there, and is told the session is over.
            Err(error) if faults.is_one_too_many(now) => break End::NetworkFailing(error),
            Err(_) | Ok(Some(RemoveReason::Restarting)) => true,
            Ok(Some(reason)) => break End::Removed(reason),
            Ok(None) => !connection.is_server_alive(now),
        };
        if is_lost {
            let mut link = Link {
                connection: &mut connection,
                session: &mut session,
                people: &mut people,
            };
            match rejoin(options, stop, &mailbox, &mut link, &mut events) {
                Ok(()) => {
                    // A key tapped meanwhile must not open the chat now.
                    if let Some(keyboard) = &mut keyboard {
                        keyboard.look_afresh();
                    }
                    continue;
                }
                Err(end) => break end,
            }
        }
        if let Some(report) = connection.report_if_due(now) {
            events(Event::Link(Box::new(report)));
        }
    };
    connection.leave();
    // A game that closed has nothing left to clear.
    if !matches!(end, End::GameClosed(_)) {
        let _ = leave_game(&mailbox, &mut session.shown, &end, people.holds_keys);
    }
    Ok(end)
}

/// The session's failures that were taken for the network's and answered by joining again.
#[derive(Debug, Default)]
struct Faults {
    /// How many came each within `FAULT_WINDOW` of the one before, and when the last did.
    running: u32,
    last: Option<Instant>,
}

impl Faults {
    /// Notes a failure at `now`, and says whether it is one more than joining again is
    /// tried for: the fourth in a row, each soon after the last.
    fn is_one_too_many(&mut self, now: Instant) -> bool {
        let is_soon = self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < FAULT_WINDOW);
        self.running = if is_soon { self.running + 1 } else { 1 };
        self.last = Some(now);
        self.running > MOST_FAULTS
    }
}

/// Whether `error` says this PC has no network to reach the server over just now.
fn is_no_network(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NetworkUnreachable
            | io::ErrorKind::HostUnreachable
            | io::ErrorKind::NetworkDown
            | io::ErrorKind::AddrNotAvailable
    )
}

/// A session's hold on its server, and what it keeps of it, for `rejoin` to make anew.
struct Link<'a> {
    connection: &'a mut Connection,
    session: &'a mut Session,
    people: &'a mut People,
}

/// Joins the server again, after it went silent or said it is being started again, for as
/// long as `REJOIN_FOR`. Meanwhile the game plays on alone: the other players fade out of it,
/// its objects run as they would without a session, and a row of chat says why.
///
/// The player may come back under another number, and the server may be a new one that knows
/// nothing of the session: so the bridge introduces its player again, and offers the world
/// the session had, which every game in it was made with.
///
/// Fails with how the session ended instead, if it did.
fn rejoin<M: ProcessMemory>(
    options: &Options,
    stop: &AtomicBool,
    mailbox: &Mailbox<M>,
    link: &mut Link<'_>,
    events: &mut impl FnMut(Event),
) -> Result<(), End> {
    events(Event::Reconnecting(REJOIN_FOR));
    events(Event::Roster(Vec::new()));
    let me = link.connection.player();
    link.people.lose_link();
    let deadline = Instant::now() + REJOIN_FOR;
    let joined = loop {
        if stop.load(Ordering::Relaxed) {
            return Err(End::Left);
        }
        let now = Instant::now();
        // To the ten seconds above: often enough to be believed, and not a row a second.
        let left = deadline.saturating_duration_since(now).as_secs();
        let left = left.div_ceil(COUNTDOWN_STEP) * COUNTDOWN_STEP;
        play_alone(mailbox, link, me, left, now).map_err(End::GameClosed)?;
        // To a server that is there again but still has this player down as here: so the
        // name they had is free when they ask for it. One that started again ignores it.
        link.connection.leave();
        // Read again each time: the player may have started another game meanwhile.
        let game = mailbox.game().map_err(End::GameClosed)?;
        let attempt = Connection::join_unless(
            options.server,
            &options.password,
            game,
            REJOIN_ATTEMPT,
            stop,
        );
        match attempt {
            Ok(joined) => break joined,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(End::Left),
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                if Instant::now() >= deadline {
                    return Err(End::ServerSilent);
                }
            }
            // No network to ask over, which may come back: said at once, where a silent
            // server is waited out, so the wait between two tries is made here.
            Err(error) if is_no_network(&error) => {
                if Instant::now() >= deadline {
                    return Err(End::ServerSilent);
                }
                thread::sleep(REJOIN_ATTEMPT.saturating_sub(now.elapsed()));
            }
            // A password that is no longer right, a session now full, another version, or a
            // session that now plays another game.
            Err(error) => return Err(End::Refused(error)),
        }
    };
    *link.connection = joined;
    link.session.start_again(Instant::now());
    link.people.rejoined();
    events(Event::Joined(link.connection.player()));
    Ok(())
}

/// Keeps a game going with no server: nobody else in it, every object its own to run, the
/// bridge's lease kept up, and the chat's rows shown, among them the one that says why and
/// the one that says how many seconds are `left` to join again in.
fn play_alone<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    link: &mut Link<'_>,
    me: PlayerId,
    left: u64,
    now: Instant,
) -> io::Result<()> {
    if !mailbox.is_ready()? {
        return Ok(());
    }
    link.people.tell_time_left(mailbox, left, now)?;
    clear_all(mailbox, &mut link.session.shown)?;
    mailbox.run_every_object()?;
    mailbox.renew_lease()?;
    link.people.show(mailbox, me, now)
}

/// Takes the session out of a game that goes on without it: other players must not stay frozen
/// there, nor its chat on the screen, nor its objects waiting on games that no longer say what
/// they do. A player who did not choose to leave is told in the game why the others are gone.
/// `keys_held` says the keys were the chat's as the session ended, and may still be down.
fn leave_game<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    shown: &mut ShownPlayers,
    end: &End,
    keys_held: bool,
) -> io::Result<()> {
    if !mailbox.is_ready()? {
        return Ok(());
    }
    clear_all(mailbox, shown)?;
    let parting = match end {
        End::Left | End::GameClosed(_) => Vec::new(),
        End::ServerSilent | End::Removed(_) | End::Refused(_) | End::NetworkFailing(_) => {
            text::rows(&format!("- {}", end.describe()))
        }
    };
    mailbox.end_session(&parting, keys_held)
}

/// Attaches as soon as the game is running, so a session can start with the emulator. None if
/// asked to stop while waiting.
fn attach_when_running<'a>(
    process: &'a WindowsProcess,
    rom: &[u8],
    stop: &AtomicBool,
    events: &mut impl FnMut(Event),
) -> io::Result<Option<N64Ram<'a, WindowsProcess>>> {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    let mut full_search_due = Instant::now() + FULL_SEARCH_EVERY;
    let mut waiting_said = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            return Ok(None);
        }
        // Said at once: an emulator that has closed reads as one whose game has not started,
        // and would be waited on until the time for that ran out.
        if process.has_exited() {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the emulator closed before its game had started",
            ));
        }
        let search = if Instant::now() >= full_search_due {
            full_search_due = Instant::now() + FULL_SEARCH_EVERY;
            Search::Everything
        } else {
            Search::InUse
        };
        match attach(process, rom, search) {
            // The game's code is not in memory, or not running, until the emulator has booted it.
            Err(error) if error.kind() == io::ErrorKind::NotFound && Instant::now() < deadline => {
                if !waiting_said {
                    events(Event::WaitingForGame);
                    waiting_said = true;
                }
                thread::sleep(BOOT_POLL_INTERVAL);
            }
            result => return result.map(Some),
        }
    }
}

/// Who the players are to each other, as this bridge knows it: its own player's introduction, and
/// the names the server last listed, to put to the chat lines that follow.
struct People {
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
    fn new(options: &Options) -> Self {
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
            chat_key: options.chat_key,
            hinted: false,
            shown_for: Duration::from_secs(u64::from(options.chat_seconds)),
            told_left: None,
            draft: Draft::default(),
            holds_keys: false,
            told_draft: None,
            told_view: None,
            told_view_at: None,
        }
    }

    /// The game was started again: what it was told and shown is gone from it, and its chat
    /// ring counts from nothing, so all of that is given anew.
    fn game_started_again(&mut self) {
        self.told_game = None;
        self.chat_count = None;
        self.told_draft = None;
        self.told_view = None;
        self.told_view_at = None;
    }

    /// The server is gone, for now: nobody is listed, a line being typed is given up, and the
    /// game is told why the others have gone.
    fn lose_link(&mut self) {
        self.roster.clear();
        self.listed = false;
        self.told_game = None;
        self.wanted = None;
        self.draft.close();
        self.holds_keys = false;
        self.told_left = None;
        self.rows.extend(text::rows("- Connection lost."));
    }

    /// Says in the game how long the bridge will go on trying to join again: `left` seconds,
    /// in a row of its own that is written over as the time runs down, so that the count
    /// does not push the chat off the screen. The row stays shown for as long as it changes.
    fn tell_time_left<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        left: u64,
        now: Instant,
    ) -> io::Result<()> {
        if self.told_left == Some(left) {
            return Ok(());
        }
        let said = format!("- Joining again, {}:{:02} left.", left / 60, left % 60);
        let Some(row) = text::rows(&said).into_iter().next() else {
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
    fn rejoined(&mut self) {
        self.last_introduction = None;
        self.told_left = None;
        self.rows.extend(text::rows("- Joined again."));
    }

    /// Takes the keys pressed for the chat in the game: they open a line, edit it, and send
    /// it. `keys` is none while the game's window does not have the keyboard. `held` says
    /// whether a key that was down as the last line ended still is, and `playing` whether the
    /// game is in play, with its chat on the screen.
    fn take_keys(
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
            let hint = format!("- Press {key} to chat, or / for a command.");
            self.rows.extend(text::rows(&hint));
        }
        let Some(keys) = keys else {
            // The player has gone to another window. A line left open would keep the
            // controller from Mario for as long as they are away, with nobody typing it.
            self.draft.close();
            self.holds_keys = false;
            return;
        };
        let was_open = self.draft.is_open();
        for &key in keys {
            // A line is begun only in play: before that the game shows no chat to type it in,
            // and would only seem to have stopped taking the controller.
            if !playing && !self.draft.is_open() {
                continue;
            }
            if let Some(line) = self.draft.press(key, now) {
                connection.queue_say(line);
            }
        }
        // A line that ended just now still holds the keys: which of them are down is only
        // known at the next look, and the Enter that sent it is among them.
        self.holds_keys = self.draft.is_open() || was_open || held;
    }

    /// Tells the game what to draw of the chat, when that changes: the line being typed, how
    /// many rows, and whether a line is typed, which it is told again and again while one is.
    fn show_chat<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        now: Instant,
    ) -> io::Result<()> {
        // The line before the word that there is one, so the game never draws the last one.
        let draft = self.draft.row(now).unwrap_or([TEXT_END; ROW_BYTES]);
        if self.told_draft != Some(draft) {
            mailbox.write_chat_draft(&draft)?;
            self.told_draft = Some(draft);
        }
        // While a line is typed, every row there is room for, however old.
        let rows = if self.draft.is_open() {
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
            mailbox.show_chat(rows, self.holds_keys)?;
            self.told_view = Some(view);
            self.told_view_at = Some(now);
        }
        Ok(())
    }

    /// Gives the game what it draws of all this: who is here, when that changes and now and
    /// then, the chat's new rows, which it shows for as long as the player chose, and the line being
    /// typed.
    fn show<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        me: PlayerId,
        now: Instant,
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
                self.shown = self.shown.saturating_add(1).min(CHAT_SHOWN_MOST);
            }
            self.chat_count = Some(count);
            self.hide_at = Some(now + self.shown_for);
        }
        if self.hide_at.is_some_and(|hide_at| now >= hide_at) {
            self.shown = 0;
            self.hide_at = None;
        }
        self.show_chat(mailbox, now)
    }

    /// Introduces this player until the server lists them, queues what they have typed, and
    /// asks for the outfit they chose until the server's roster shows it.
    fn send(
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
                        connection.queue_say(text);
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
    fn receive(&mut self, me: PlayerId, received: &mut Received, events: &mut impl FnMut(Event)) {
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
            self.rows.extend(text::rows(&shown));
            events(Event::Said {
                from: line.from,
                // A player who said something and left before the roster came has no name here.
                name: name.unwrap_or_default(),
                text: line.text.as_str().to_owned(),
            });
        }
    }
}

/// Where the bridge is in the game's two hit rings. Both start from the game's counts when the
/// bridge first finds the mailbox ready, so hits from before it attached are not replayed.
#[derive(Default)]
struct HitRings {
    out_seen: Option<u32>,
    in_count: Option<u32>,
}

/// What a session keeps from one step to the next.
struct Session {
    shown: ShownPlayers,
    interpolator: Interpolator,
    has_session_world: bool,
    /// The world the session last gave this game, which every game in it was made with. A
    /// bridge that joins again offers it, so a server that started again takes up the same.
    world: Option<Box<WorldInputs>>,
    last_offer: Option<Instant>,
    sends: Schedule,
    frames: Schedule,
    rings: HitRings,
    objects: ObjectSync,
    progress: ProgressSync,
    /// Whether the game was in play when the local player was last sent: it has a Mario,
    /// and with him the chat on its screen.
    playing: bool,
    /// The count of its Mario's publishings the game last showed, and for how many send
    /// slots it has stood still: see `is_game_running`.
    published: Option<u32>,
    slots_unpublished: u32,
    /// Since when the game has not been one to write into, and whether the player has been
    /// told that it is the wrong game.
    unready_since: Option<Instant>,
    said_wrong_rom: bool,
}

impl Session {
    fn new(now: Instant) -> Self {
        Self {
            world: None,
            playing: false,
            published: None,
            slots_unpublished: 0,
            unready_since: None,
            said_wrong_rom: false,
            shown: ShownPlayers::default(),
            interpolator: Interpolator::default(),
            has_session_world: false,
            last_offer: None,
            sends: Schedule::new(now, SEND_INTERVAL),
            frames: Schedule::new(now, FRAME_INTERVAL),
            rings: HitRings::default(),
            objects: ObjectSync::default(),
            progress: ProgressSync::default(),
        }
    }

    /// Begins anew with a server joined again, which may know nothing of what went before:
    /// all is as at first, but for who is drawn in the game, the world the session had,
    /// whether the game is in play, whether the player was told it is the wrong game, and
    /// what the player had before the session, which is what its progress is told apart from,
    /// and which coins the game was told were collected before it came.
    fn start_again(&mut self, now: Instant) {
        let mut progress = std::mem::take(&mut self.progress);
        progress.start_again();
        *self = Self {
            objects: self.objects.for_server_joined_again(),
            shown: std::mem::take(&mut self.shown),
            world: self.world.take(),
            playing: self.playing,
            said_wrong_rom: self.said_wrong_rom,
            progress,
            ..Self::new(now)
        };
    }

    /// Whether the game has published its Mario again within the last `STALLED_AFTER` send
    /// slots, which it does in every frame in which it runs its objects. Call it once per
    /// send slot.
    fn is_game_running<M: ProcessMemory>(&mut self, mailbox: &Mailbox<M>) -> io::Result<bool> {
        let published = mailbox.local_player_sequence()?;
        if self.published == Some(published) {
            self.slots_unpublished = self.slots_unpublished.saturating_add(1);
        } else {
            self.published = Some(published);
            self.slots_unpublished = 0;
        }
        Ok(self.slots_unpublished < STALLED_AFTER)
    }

    /// One turn of the session: sends what is due, draws the others, and takes in what arrived.
    /// Returns why the server removed this player, if it did.
    fn step<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        connection: &mut Connection,
        now: Instant,
        people: &mut People,
        events: &mut impl FnMut(Event),
    ) -> io::Result<Option<RemoveReason>> {
        let ready = mailbox.is_ready()?;
        if !ready {
            let since = *self.unready_since.get_or_insert(now);
            if !self.said_wrong_rom && now.saturating_duration_since(since) >= WRONG_ROM_AFTER {
                // Without this the player is in the session, sees nobody, and is told nothing.
                self.said_wrong_rom = true;
                events(Event::WrongRom(mailbox.mailbox_version()?));
            }
        } else if self.unready_since.take().is_some() {
            // Marked now and not at the last step: the game was reset and has started again.
            // Its rings count from nothing and nobody is shown in it, so what is kept of the
            // game that was would be wrong for this one.
            self.rings = HitRings::default();
            self.shown = ShownPlayers::default();
            self.objects.game_started_again();
            people.game_started_again();
            // The session's world again, before the game makes its first level: the server
            // sends it only now and then, and a level made from the game's own world numbers
            // its objects unlike everyone else's.
            if let Some(world) = &self.world {
                mailbox.write_session_world(world)?;
            }
        }
        if self.sends.is_due(now) {
            let state = if ready {
                mailbox.read_local_player()?
            } else {
                None
            };
            let playing = state.is_some();
            self.playing = playing;
            // A game that is paused, or has stopped for anything else, runs no objects: its
            // state is not sent, so the server has others run the ones it had, where they
            // would stand still for everyone until it went on.
            let running = playing && self.is_game_running(mailbox)?;
            // Asked after the state was read: an area the state is in was loaded before it.
            let state = match state.filter(|_| running) {
                Some(state) if self.objects.state_may_go(mailbox)? => Some(state),
                _ => None,
            };
            if let Some(state) = state {
                connection.send_state(state)?;
            } else {
                // In menus there is nothing to show, but the player must stay in the session.
                self.objects.no_state_went();
                connection.keep_alive()?;
            }
            if ready {
                collect_hits(mailbox, connection, &mut self.rings, events)?;
            }
            connection.send_hits()?;
            connection.send_says()?;
            if ready {
                self.objects.send(mailbox, connection, now, running)?;
                self.progress.send(mailbox, connection, now)?;
            }
        }
        // Until the session has a world, offer one; the server keeps the owner's. This game's
        // own, or, joining again, the one the session had.
        let offer_due = self
            .last_offer
            .is_none_or(|sent| now.saturating_duration_since(sent) >= WORLD_OFFER_INTERVAL);
        if ready && !self.has_session_world && offer_due {
            let world = match &self.world {
                Some(world) => Some(world.clone()),
                None => mailbox.read_local_world()?.map(Box::new),
            };
            if let Some(world) = world {
                connection.offer_world(&world)?;
                self.last_offer = Some(now);
            }
        }
        connection.ping_if_due(now)?;
        // Only into a game that has marked its mailbox: any other uses that RAM as its own.
        if ready && self.frames.is_due(now) {
            mailbox.renew_lease()?;
            let players = self.interpolator.sample(now);
            apply_snapshot(mailbox, connection.player(), &players, &mut self.shown)?;
            self.objects.draw(mailbox, &self.interpolator, now)?;
        }
        // Waiting in `poll` paces the loop, and times pongs as they arrive.
        let mut received = connection.poll(self.sends.next().min(self.frames.next()))?;
        if let Some(snapshot) = received.snapshot.take() {
            self.interpolator
                .receive(snapshot.arrived, snapshot.present, &snapshot.players);
        }
        if ready && let Some(world) = received.world.take() {
            // The server sends the same world every few seconds for the whole session. It
            // is written when it is news: the game reads it once for each step of making a
            // level, with no second try, and a read that fell in the middle of a write made
            // that step from the player's own world, unlike everyone else's.
            if !self.has_session_world || self.world.as_ref() != Some(&world) {
                mailbox.write_session_world(&world)?;
            }
            self.has_session_world = true;
            self.world = Some(world);
        }
        people.receive(connection.player(), &mut received, events);
        if ready {
            people.show(mailbox, connection.player(), now)?;
        }
        if ready {
            deliver_hurts(
                mailbox,
                connection.player(),
                &received.hurts,
                &mut self.rings,
                events,
            )?;
            self.objects
                .receive(mailbox, &received, &self.interpolator, now)?;
            self.progress.receive(mailbox, &received)?;
        }
        Ok(received.removed)
    }
}

/// Queues the hits the local player landed since the last call.
fn collect_hits<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    connection: &mut Connection,
    rings: &mut HitRings,
    events: &mut impl FnMut(Event),
) -> io::Result<()> {
    let seen = match rings.out_seen {
        Some(seen) => seen,
        None => mailbox.hits_out_count()?,
    };
    let (hits, count) = mailbox.read_hits_out(seen)?;
    rings.out_seen = Some(count);
    let me = connection.player();
    for (slot, code) in hits {
        let target = RemoteSlot::new(slot).and_then(|slot| player_for(me, slot));
        if let (Some(target), Some(kind)) = (target, HitKind::from_code(code)) {
            events(Event::Hit(target, kind));
            connection.queue_hit(target, kind);
        }
    }
    Ok(())
}

/// Gives the game the hits on the local player that the server accepted.
fn deliver_hurts<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    me: PlayerId,
    hurts: &[Hurt],
    rings: &mut HitRings,
    events: &mut impl FnMut(Event),
) -> io::Result<()> {
    if hurts.is_empty() {
        return Ok(());
    }
    let mut count = match rings.in_count {
        Some(count) => count,
        None => mailbox.hits_in_count()?,
    };
    for hurt in hurts {
        if let Some(slot) = slot_for(me, hurt.attacker) {
            events(Event::HitBy(hurt.attacker, hurt.kind));
            count = mailbox.write_hit_in(count, slot, hurt.kind.code())?;
        }
    }
    rings.in_count = Some(count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        End, Event, Input, Link, Options, People, Session, TYPING_RENEW_INTERVAL, leave_game,
        rejoin,
    };
    use crate::client::{Connection, ShownPlayers, apply_snapshot};
    use crate::layout::{self, len};
    use crate::mailbox::testing::{game, make_ready, raw};
    use crate::mailbox::{Mailbox, Person};
    use crate::ram::N64Ram;
    use crate::text;
    use crate::typing::Key;
    use b3313_protocol::{
        ObjectSet, Outfit, PLAYER_STATE_SIZE, PROGRESS_SIZE, PlayerId, Progress, RemoveReason,
        Role, RosterEntry, TICK_INTERVAL, WORLD_INPUTS_SIZE,
    };
    use b3313_server::{Command, Config, StopOnDrop, serve, serve_with_console};
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
                password: String::new(),
                name: "Luigi".to_owned(),
                outfit: Outfit::named("green"),
                host_password: String::new(),
                chat_key: None,
                chat_seconds: 8,
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

    fn options(server: std::net::SocketAddr) -> Options {
        Options {
            rom: Vec::new(),
            pid: 0,
            server,
            password: String::new(),
            name: String::new(),
            outfit: None,
            host_password: String::new(),
            chat_key: Some('t'),
            chat_seconds: 8,
        }
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

        people.rows.extend(text::rows("Luigi: hello"));
        people.show(&mailbox, me, start)?;
        assert_eq!(shown()?, Some(1));
        people.show(&mailbox, me, start + Duration::from_secs(19))?;
        assert_eq!(shown()?, Some(1));
        people.show(&mailbox, me, start + Duration::from_secs(20))?;
        assert_eq!(shown()?, Some(0));
        Ok(())
    }

    /// Where the game keeps what a session gave it, from the mailbox's start, as
    /// `TrollEngine/Multiplayer/mailbox.h` lays it out.
    const BRIDGE_LEASE: usize = len::MAILBOX_BRIDGE_LEASE_OFFSET;
    const FIRST_REMOTE_PLAYER: usize = len::MAILBOX_PLAYERS_OFFSET;
    const OBJECTS_OWNED: usize = len::MAILBOX_OBJECTS_OWNED_OFFSET;
    const PEOPLE: usize = len::MAILBOX_PEOPLE_OFFSET;
    const CHAT_COUNT: usize = len::MAILBOX_CHAT_OFFSET;
    /// The rows shown, then the frames a line is typed for.
    const CHAT_SHOWN: usize = CHAT_COUNT + len::CHAT_SHOWN_OFFSET;
    const CHAT_ROWS: usize = CHAT_COUNT + len::CHAT_ROWS_OFFSET;
    const CHAT_DRAFT: usize = CHAT_COUNT + len::CHAT_DRAFT_OFFSET;
    /// The same two as addresses, to write there as the game does.
    const BRIDGE_LEASE_ADDRESS: u32 = layout::MAILBOX_ADDR + layout::MAILBOX_BRIDGE_LEASE_OFFSET;
    const CHAT_SHOWN_ADDRESS: u32 =
        layout::MAILBOX_ADDR + layout::MAILBOX_CHAT_OFFSET + layout::CHAT_SHOWN_OFFSET;

    /// The text in the game's memory at `offset`, up to its end code.
    fn text_at(process: &crate::ram::testing::FakeProcess, offset: usize) -> io::Result<String> {
        let field = raw(process, offset, text::ROW_BYTES)?;
        let end = field.iter().position(|&byte| byte == 0);
        let text = field.get(..end.unwrap_or(field.len())).unwrap_or_default();
        Ok(String::from_utf8_lossy(text).into_owned())
    }

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
            people.show(&mailbox, me, now)?;
            assert_eq!(view()?.get(1), Some(&0));

            // In play, it opens the chat: the game draws a box, with the line under every
            // row it has, and keeps the keys from Mario.
            people.take_keys(Some(&[Key::Chat]), false, true, &mut connection, now);
            people.show(&mailbox, me, now)?;
            assert_eq!(view()?, [6, 30, 0, 0]);
            assert_eq!(text_at(&process, CHAT_DRAFT)?, "> _");

            // The player turns to another window: the line is given up, and Mario has the
            // controller back, as nobody is typing.
            people.take_keys(None, false, true, &mut connection, now);
            people.show(&mailbox, me, now)?;
            assert_eq!(text_at(&process, CHAT_DRAFT)?, "");
            assert_eq!(view()?.get(1), Some(&0));

            people.take_keys(Some(&[Key::Chat]), false, true, &mut connection, now);
            let typed = [Key::Character('h'), Key::Character('i')];
            people.take_keys(Some(&typed), false, true, &mut connection, now);
            people.show(&mailbox, me, now)?;
            assert_eq!(text_at(&process, CHAT_DRAFT)?, "> hi_");

            // Enter sends it. The box goes at once, but Enter is still down, and is the
            // game's Start button: the game is shown no key until it is let go.
            people.take_keys(Some(&[Key::Enter]), false, true, &mut connection, now);
            people.show(&mailbox, me, now)?;
            assert_eq!(text_at(&process, CHAT_DRAFT)?, "");
            assert_eq!(view()?.get(1), Some(&30));
            people.take_keys(Some(&[]), true, true, &mut connection, now);
            people.show(&mailbox, me, now)?;
            assert_eq!(view()?.get(1), Some(&30));
            people.take_keys(Some(&[]), false, true, &mut connection, now);
            people.show(&mailbox, me, now)?;
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
        people.show(&mailbox, me, start)?;
        let ram = N64Ram::new(&process, 0);

        // The game counts it down as it runs; it is put back before it runs out, and not
        // at every turn of the bridge's loop.
        ram.write(CHAT_SHOWN_ADDRESS, &[6, 5, 0, 0])?;
        people.show(&mailbox, me, start + Duration::from_millis(1))?;
        assert_eq!(raw(&process, CHAT_SHOWN, 4)?.get(1), Some(&5));
        people.show(&mailbox, me, start + TYPING_RENEW_INTERVAL)?;
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
            people.show(&mailbox, me, now)?;
            assert_eq!(count()?, Some(0));

            people.take_keys(Some(&[]), false, true, &mut connection, now);
            people.show(&mailbox, me, now)?;
            assert_eq!(count()?, Some(1));
            assert!(text_at(&process, CHAT_ROWS)?.contains("Press T to chat"));

            people.take_keys(Some(&[]), false, true, &mut connection, now);
            people.show(&mailbox, me, now)?;
            assert_eq!(count()?, Some(1));
            Ok(())
        })
    }

    /// Steps a session until `done`, for five seconds at most, and returns why the server
    /// removed its player, if it did.
    fn play_on(
        link: &mut Link<'_>,
        mailbox: &Mailbox<'_, crate::ram::testing::FakeProcess>,
        done: impl Fn(&Session, &People) -> bool,
    ) -> io::Result<Option<RemoveReason>> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let (_, nothing_typed) = mpsc::channel();
        while Instant::now() < deadline && !done(link.session, link.people) {
            let now = Instant::now();
            link.people.send(link.connection, &nothing_typed, now)?;
            let removed =
                link.session
                    .step(mailbox, link.connection, now, link.people, &mut |_| {})?;
            if removed.is_some() {
                return Ok(removed);
            }
        }
        Ok(None)
    }

    #[test]
    fn a_bridge_whose_server_starts_again_joins_the_new_one() -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server = socket.local_addr()?;
        let stop = AtomicBool::new(false);
        let (commands, received) = mpsc::channel();
        thread::scope(|scope| {
            let (socket, stop) = (&socket, &stop);
            let first = scope.spawn(move || {
                serve_with_console(socket, Config::default(), stop, &received, |_| {})
            });
            let _stopping = StopOnDrop(stop);
            let process = game();
            make_ready(&process)?;
            let mailbox = Mailbox::new(N64Ram::new(&process, 0));
            let options = Options {
                name: "Luigi".to_owned(),
                ..options(server)
            };
            let mut people = People::new(&options);
            let mut connection = Connection::join(server, "", 0)?;
            let mut session = Session::new(Instant::now());
            // As if the session had given this game its world, which the first server did.
            session.world = Some(Box::new([7; WORLD_INPUTS_SIZE]));
            let mut link = Link {
                connection: &mut connection,
                session: &mut session,
                people: &mut people,
            };
            play_on(&mut link, &mailbox, |_, people| people.listed)?;
            assert!(link.people.listed);

            // The server's owner restarts it: it says so and stops, and another takes its place.
            commands
                .send(Command::Restart)
                .map_err(|_| io::Error::other("the first server has gone"))?;
            let told = play_on(&mut link, &mailbox, |_, _| false)?;
            assert_eq!(told, Some(RemoveReason::Restarting));
            first
                .join()
                .map_err(|_| io::Error::other("the first server panicked"))??;
            scope.spawn(move || serve(socket, Config::default(), stop, |_| {}));

            let mut said = Vec::new();
            let never = AtomicBool::new(false);
            let rejoined = rejoin(
                &options,
                &never,
                &mailbox,
                &mut link,
                &mut |event| match event {
                    Event::Reconnecting(_) => said.push("reconnecting"),
                    Event::Joined(_) => said.push("joined"),
                    _ => {}
                },
            );
            assert!(rejoined.is_ok(), "{rejoined:?}");
            assert_eq!(said, ["reconnecting", "joined"]);

            // The new server knows nothing of the session: the bridge says again who its
            // player is, and gives it the world the session had, which it takes for its own.
            play_on(&mut link, &mailbox, |session, people| {
                people.listed && session.has_session_world
            })?;
            assert!(link.people.listed);
            assert!(link.session.has_session_world);
            let name = link.people.roster.first().map(|entry| entry.name.as_str());
            assert_eq!(name, Some("Luigi"));
            // And the player in the game was told what went on.
            let rows = raw(&process, CHAT_ROWS, 8 * text::ROW_BYTES)?;
            let rows = String::from_utf8_lossy(&rows).into_owned();
            assert!(rows.contains("Connection lost"), "{rows}");
            assert!(rows.contains("Joined again"), "{rows}");
            Ok(())
        })
    }

    #[test]
    fn a_player_who_leaves_while_the_server_is_gone_is_let_go_at_once() -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server = socket.local_addr()?;
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| serve(&socket, Config::default(), &stop, |_| {}));
            let process = game();
            make_ready(&process)?;
            let mailbox = Mailbox::new(N64Ram::new(&process, 0));
            let options = options(server);
            let mut people = People::new(&options);
            let mut connection = Connection::join(server, "", 0)?;
            let mut session = Session::new(Instant::now());
            let me = connection.player();
            let other = PlayerId::new(1).ok_or_else(|| io::Error::other("no player 1"))?;
            let drawn = [(other, [7; PLAYER_STATE_SIZE])];
            apply_snapshot(&mailbox, me, &drawn, &mut session.shown)?;
            // The server goes for good, and the player presses Leave.
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            let leaving = AtomicBool::new(true);
            let mut link = Link {
                connection: &mut connection,
                session: &mut session,
                people: &mut people,
            };

            let ended = rejoin(&options, &leaving, &mailbox, &mut link, &mut |_| {});

            assert!(matches!(ended, Err(End::Left)), "{ended:?}");
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
        people.show(&mailbox, me, start)?;
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
        people.show(&mailbox, me, later)?;
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

    #[test]
    fn a_player_the_session_leaves_is_told_why_in_the_game() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let ended = End::Removed(b3313_protocol::RemoveReason::Ended);

        leave_game(&mailbox, &mut ShownPlayers::default(), &ended, false)?;

        assert_eq!(text_at(&process, CHAT_ROWS)?, "- The session was ended.");
        assert_eq!(raw(&process, CHAT_SHOWN, 4)?, [1, 0, 0, 0]);
        // No bridge is left to take the row off again. The game does, when it has counted
        // the lease down, so the lease is left running.
        let lease = raw(&process, BRIDGE_LEASE, 4)?;
        assert!(lease.last().is_some_and(|&frames| frames > 100));
        Ok(())
    }

    #[test]
    fn a_session_that_ends_as_a_line_is_sent_leaves_the_keys_to_be_let_go() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let ended = End::Removed(b3313_protocol::RemoveReason::Ended);
        mailbox.write_chat_draft(&text::field("> /end_"))?;

        // The host typed /end: the Enter that sent it is the game's Start button, and is
        // still down when the server's answer ends the session.
        leave_game(&mailbox, &mut ShownPlayers::default(), &ended, true)?;

        // No line is left to draw a box round, and the game counts the keys' lease down
        // by itself, with no bridge left to.
        assert_eq!(text_at(&process, CHAT_DRAFT)?, "");
        assert_eq!(raw(&process, CHAT_SHOWN, 4)?, [1, 30, 0, 0]);
        Ok(())
    }

    /// A paused game publishes nothing: its bridge is then to send no state, so the server
    /// has other games run the objects it had.
    #[test]
    fn a_game_that_stops_publishing_its_mario_is_not_running() -> io::Result<()> {
        let process = game();
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let mut session = Session::new(Instant::now());
        let state = [1; PLAYER_STATE_SIZE];

        // A frame or two without a new publishing is nothing; half a second is a stop.
        crate::mailbox::testing::publish(&process, &state, 2)?;
        let mut running = Vec::new();
        for _ in 0..=super::STALLED_AFTER {
            running.push(session.is_game_running(&mailbox)?);
        }
        assert_eq!(running.pop(), Some(false));
        assert!(running.iter().all(|&running| running));

        // It goes on as soon as the game does.
        crate::mailbox::testing::publish(&process, &state, 4)?;
        assert!(session.is_game_running(&mailbox)?);
        Ok(())
    }

    #[test]
    fn joining_again_is_given_up_only_for_failures_in_a_quick_run() {
        let start = Instant::now();
        let mut faults = super::Faults::default();
        // Now and then, as a network that drops does: joined again every time.
        for minute in 0..10 {
            assert!(!faults.is_one_too_many(start + Duration::from_mins(minute)));
        }
        // One after another: the fourth ends it.
        let run = start + Duration::from_hours(1);
        let ends: Vec<bool> = (0..4)
            .map(|second| faults.is_one_too_many(run + Duration::from_secs(second)))
            .collect();
        assert_eq!(ends, [false, false, false, true]);
    }

    #[test]
    fn leaving_takes_the_session_out_of_the_game() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let me = PlayerId::new(0).ok_or_else(|| io::Error::other("no player 0"))?;
        let other = PlayerId::new(1).ok_or_else(|| io::Error::other("no player 1"))?;
        let gold = Outfit::named("gold").ok_or_else(|| io::Error::other("no gold"))?;

        // A session as the game has it: another player, a name and an outfit, a line of chat
        // on screen, and an area whose objects all but one are other games' to run.
        let mut shown = ShownPlayers::default();
        apply_snapshot(&mailbox, me, &[(other, [7; PLAYER_STATE_SIZE])], &mut shown)?;
        let mine = Person {
            role: Role::Host,
            outfit: gold,
            name: "Luigi",
        };
        mailbox.write_people(&mine, &[])?;
        for row in text::rows("Luigi: hello") {
            mailbox.write_chat_row(0, &row)?;
        }
        mailbox.show_chat(1, false)?;
        let mut owned = ObjectSet::default();
        owned.insert(3);
        mailbox.write_objects_owned((9, 1), &owned, &ObjectSet::default())?;
        mailbox.write_session_progress(&Progress::from_bytes([0xFF; PROGRESS_SIZE]))?;

        leave_game(&mailbox, &mut shown, &End::Left, false)?;

        // The game takes a sequence of nothing for no progress to add to a save.
        let progress = len::MAILBOX_PROGRESS_IN_OFFSET + len::PROGRESS_SEQUENCE_OFFSET;
        assert_eq!(raw(&process, progress, 4)?, [0; 4]);

        assert_eq!(raw(&process, FIRST_REMOTE_PLAYER, 4)?.first(), Some(&0));
        assert_eq!(raw(&process, CHAT_SHOWN, 4)?.first(), Some(&0));
        // Nobody has a name, a role or an outfit: Mario wears the game's own colors again.
        let people = raw(&process, PEOPLE, len::PEOPLE_SEQUENCE_OFFSET)?;
        assert!(people.iter().all(|&byte| byte == 0));
        // The game takes its word on who runs what only for the level it is in, and no level
        // has a negative number, so every object runs as it would in a game alone.
        let place = raw(&process, OBJECTS_OWNED, 4)?;
        let level = place.first_chunk().map(|bytes| i16::from_be_bytes(*bytes));
        assert!(level.is_some_and(|level| level < 0));
        // Each record is whole: an odd sequence would have the game ignore it.
        for sequence_at in [
            OBJECTS_OWNED + len::OBJECTS_OWNED_SEQUENCE_OFFSET,
            PEOPLE + len::PEOPLE_SEQUENCE_OFFSET,
        ] {
            let sequence = raw(&process, sequence_at, 4)?;
            assert_eq!(sequence.last().map(|low| low % 2), Some(0));
        }
        Ok(())
    }

    /// Steps a session with no other player in it until `done`, for a second at most.
    fn step_until(
        session: &mut Session,
        mailbox: &Mailbox<'_, crate::ram::testing::FakeProcess>,
        connection: &mut Connection,
        people: &mut People,
        done: impl Fn() -> io::Result<bool>,
    ) -> io::Result<bool> {
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            session.step(mailbox, connection, Instant::now(), people, &mut |_| {})?;
            if done()? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// A game that is reset counts its rings from nothing: a place the bridge kept in one
    /// would be far ahead of the game's, and what the bridge wrote there never taken.
    #[test]
    fn a_game_started_again_has_its_rings_read_anew() -> io::Result<()> {
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
            let mut session = Session::new(Instant::now());
            let mut advance = |session: &mut Session| {
                let now = Instant::now();
                session.step(&mailbox, &mut connection, now, &mut people, &mut |_| {})
            };
            advance(&mut session)?;

            // Far into the game's rings, as after a long session; then the game is reset,
            // and for a moment has no mailbox.
            let far = Some(4000);
            session.rings.in_count = far;
            N64Ram::new(&process, 0).write(layout::MAILBOX_ADDR, &[0; 4])?;
            advance(&mut session)?;
            assert_eq!(session.rings.in_count, far, "kept while there is no game");

            make_ready(&process)?;
            advance(&mut session)?;
            assert_ne!(session.rings.in_count, far);
            Ok(())
        })
    }

    #[test]
    fn the_game_is_told_its_bridge_is_here_for_as_long_as_it_is() -> io::Result<()> {
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
            let mut session = Session::new(Instant::now());
            let lease = || -> io::Result<Option<u8>> {
                Ok(raw(&process, BRIDGE_LEASE, 4)?.last().copied())
            };
            let is_fresh = || Ok(lease()?.is_some_and(|frames| frames > 100));

            let given = step_until(
                &mut session,
                &mailbox,
                &mut connection,
                &mut people,
                is_fresh,
            )?;
            assert!(given);

            // The game counts it down as it runs, and the bridge puts it back.
            N64Ram::new(&process, 0).write(BRIDGE_LEASE_ADDRESS, &1_u32.to_be_bytes())?;
            let renewed = step_until(
                &mut session,
                &mailbox,
                &mut connection,
                &mut people,
                is_fresh,
            )?;
            assert!(renewed);

            // A bridge that leaves says so at once, and does not leave the game to find out.
            leave_game(&mailbox, &mut session.shown, &End::Left, false)?;
            assert_eq!(lease()?, Some(0));
            Ok(())
        })
    }

    #[test]
    fn leaving_writes_nothing_to_a_game_that_is_not_the_multiplayer_one() -> io::Result<()> {
        let process = game();
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));

        leave_game(&mailbox, &mut ShownPlayers::default(), &End::Left, false)?;

        assert!(process.bytes.borrow().iter().all(|&byte| byte == 0));
        Ok(())
    }

    #[test]
    fn other_players_are_not_written_to_a_game_that_is_not_the_multiplayer_one() -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server = socket.local_addr()?;
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            scope.spawn(|| serve(&socket, Config::default(), &stop, |_| {}));
            let _stopping = StopOnDrop(&stop);
            // Never made ready: a game without the mailbox, whose RAM here is its own.
            let process = game();
            let mailbox = Mailbox::new(N64Ram::new(&process, 0));
            let mut people = People::new(&options(server));
            let mut connection = Connection::join(server, "", 0)?;
            let mut other = Connection::join(server, "", 0)?;
            let mut session = Session::new(Instant::now());

            // Long enough for the other player, who is elsewhere to a game in no level, to be
            // sent and then drawn.
            let deadline = Instant::now() + Duration::from_millis(1500);
            let mut next_state = Instant::now();
            while Instant::now() < deadline {
                let now = Instant::now();
                if now >= next_state {
                    other.send_state([1; PLAYER_STATE_SIZE])?;
                    next_state = now + TICK_INTERVAL;
                }
                other.poll(now)?;
                session.step(&mailbox, &mut connection, now, &mut people, &mut |_| {})?;
            }

            assert!(process.bytes.borrow().iter().all(|&byte| byte == 0));

            // The player, who sees nobody, is told why: once, and that the game has no
            // mailbox at all.
            let mut told = Vec::new();
            for later in [super::WRONG_ROM_AFTER, super::WRONG_ROM_AFTER * 2] {
                let now = Instant::now() + later;
                let mut note = |event| {
                    if let Event::WrongRom(found) = event {
                        told.push(found);
                    }
                };
                session.step(&mailbox, &mut connection, now, &mut people, &mut note)?;
            }
            assert_eq!(told, [None]);
            Ok(())
        })
    }
}
