//! One multiplayer session: attaches to a running game, joins a server, and moves players,
//! hits, objects and progress between them until the session ends. The command-line bridge and
//! the panel's Play page both run it, each showing what it reports in its own way.

use crate::assist::{Assist, BotNotice};
use crate::client::{
    Connection, JOIN_TIMEOUT, ShownPlayers, apply_snapshot, clear_all, player_for, resolve_within,
    slot_for,
};
use crate::interpolation::Interpolator;
use crate::keyboard::{Keyboard, WindowsKeys};
use crate::mailbox::{Mailbox, RemoteSlot};
use crate::objects::ObjectSync;
use crate::pacing::{FRAME_INTERVAL, SEND_INTERVAL, Schedule};
use crate::people::People;
use crate::pointer::WindowsPointer;
use crate::process::WindowsProcess;
use crate::progress::ProgressSync;
use crate::ram::{N64Ram, ProcessMemory, Search, attach};
use crate::text::{self, ChatSize};
use b3313_protocol::metrics::LinkReport;
use b3313_protocol::secure::PublicKey;
use b3313_protocol::{HitKind, Hurt, Outfit, PlayerId, RemoveReason, RosterEntry, WorldInputs};
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::thread;
use std::time::{Duration, Instant};

const WORLD_OFFER_INTERVAL: Duration = Duration::from_secs(1);
/// How long a bridge whose server has gone silent keeps trying to join it again: long enough
/// for a server to be started again, or a connection to come back, and not for ever. Each try
/// is short, as the game is seen to between them.
const REJOIN_FOR: Duration = Duration::from_secs(120);
const REJOIN_ATTEMPT: Duration = Duration::from_secs(1);
/// A game that has not published its Mario for this many send slots, half a second, has
/// stopped running its objects. It publishes once a frame, about once a slot, so a slot or
/// two without is nothing.
const STALLED_AFTER: u32 = 15;
/// A game that has not run a frame for this long is taken for stopped, and its player for
/// away. Long enough for the emulator's menu to be opened and closed, or a hitch as a level
/// loads, without the player's Mario going from the others' games and coming back.
const STOPPED_AFTER: Duration = Duration::from_secs(10);
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
    /// What the player gave as the server, if it was a name: looked up again each time the
    /// bridge joins again, as a home connection's address changes and the name follows it.
    /// None keeps to `server`.
    pub server_name: Option<String>,
    /// The key the server must have, from a join code or an earlier join; none joins whatever
    /// answers at the address. Either way the server joined first is the only one joined
    /// again.
    pub server_key: Option<PublicKey>,
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
    /// How many of the chat's rows the game shows at once as lines arrive, from
    /// `typing::CHAT_LINES_MIN` to `typing::DEFAULT_CHAT_LINES`, all of them; opening the chat
    /// shows all whatever this is.
    pub chat_lines: u8,
    /// Whether the mouse resting on the game's chat shows it, as opening it does.
    pub chat_on_hover: bool,
    /// How large the game draws the chat.
    pub chat_size: ChatSize,
    /// The port on 127.0.0.1 a bot may connect to, to play this game (`crate::bots`); none
    /// opens none.
    pub bot_port: Option<u16>,
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
    /// The key of the server joined, said once, after the first `Joined`: what whoever keeps
    /// the player's servers knows this one by from now on.
    Server(PublicKey),
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
    /// What became of the bot API: where a bot may connect, or why none can, and a bot
    /// connecting and leaving.
    Bot(BotNotice),
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

/// The most characters in the few words that say how a session stands, where there is room
/// for no more: the panel's sidebar shows them on every page, in a box they must not run
/// out of.
pub const HEADLINE_MOST: usize = 20;

impl End {
    /// What happened, in at most `HEADLINE_MOST` characters; `describe` says it in full.
    #[must_use]
    pub const fn headline(&self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::GameClosed(_) => "Game closed",
            Self::ServerSilent => "Server went silent",
            Self::Removed(RemoveReason::Kicked) => "Removed",
            Self::Removed(RemoveReason::Banned) => "Banned",
            Self::Removed(RemoveReason::Ended) => "Session ended",
            Self::Removed(RemoveReason::Restarting) => "Server restarting",
            Self::Refused(_) => "Could not rejoin",
            Self::NetworkFailing(_) => "Connection failing",
        }
    }

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
        (options.server, options.server_key),
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
    events(Event::Server(connection.server_key()));
    let mut session = Session::new(Instant::now());
    let mut people = People::new(options);
    let (mut keyboard, mut pointer) = player_devices(options);
    let mut assist = Assist::new(options.bot_port, &mut |notice| events(Event::Bot(notice)));
    let mut faults = Faults::default();
    let end = loop {
        if stop.load(Ordering::Relaxed) {
            break End::Left;
        }
        let now = Instant::now();
        let over_chat = take_player_input(
            (keyboard.as_mut(), pointer.as_mut()),
            &mut people,
            &mut connection,
            session.playing,
            now,
        );
        let stepped = people
            .send(&mut connection, input, now)
            .and_then(|()| {
                session.step(
                    &mailbox,
                    &mut connection,
                    now,
                    &mut people,
                    &mut events,
                    over_chat,
                )
            })
            .and_then(|removed| {
                let mut notices = |notice| events(Event::Bot(notice));
                let drawn = (&session.interpolator, session.playing);
                if removed.is_none() {
                    assist.turn(&mailbox, &mut connection, &mut people, drawn, &mut notices)?;
                }
                Ok(removed)
            });
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
        let size = people.chat_size();
        let _ = leave_game(
            &mailbox,
            &mut session.shown,
            &end,
            people.holds_keys(),
            size,
        );
    }
    Ok(end)
}

/// What the session reads of the player at the PC: the keyboard, for a player who types in the
/// game, and the mouse, for one who asked for the chat to show under it.
fn player_devices(options: &Options) -> (Option<Keyboard<WindowsKeys>>, Option<WindowsPointer>) {
    let keyboard = options
        .chat_key
        .map(|key| Keyboard::new(WindowsKeys::new(options.pid), key));
    let pointer = options
        .chat_on_hover
        .then(|| WindowsPointer::new(options.pid));
    (keyboard, pointer)
}

/// Takes what the player does at the PC for the game: the keys typed into its chat, if the
/// keyboard is read, and whether the pointer rests on its chat, if that is looked at, which is
/// returned.
fn take_player_input(
    (keyboard, pointer): (
        Option<&mut Keyboard<WindowsKeys>>,
        Option<&mut WindowsPointer>,
    ),
    people: &mut People,
    connection: &mut Connection,
    playing: bool,
    now: Instant,
) -> bool {
    if let Some(keyboard) = keyboard {
        let keys = keyboard.poll(people.is_typing(), now);
        let keys = keyboard.has_keyboard().then_some(keys.as_slice());
        let held = keyboard.is_settling(now);
        people.take_keys(keys, held, playing, connection, now);
    }
    pointer.is_some_and(|pointer| pointer.is_over_chat(people.chat_size(), now))
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
    // Only the server that was joined is joined again: what answers at its address now may
    // be another, and the session's world and progress would not be this one's.
    let server_key = link.connection.server_key();
    let mut server = options.server;
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
        // The name may lead elsewhere by now, as when the server's router started again
        // with a new address. Following it unasked is safe because the key is checked. A
        // lookup that fails or takes too long leaves the address last known.
        if let Some(name) = &options.server_name {
            server = resolve_within(name, REJOIN_ATTEMPT).unwrap_or(server);
        }
        let attempt = Connection::join_unless(
            (server, Some(server_key)),
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
    link.people.show(mailbox, me, now, false)
}

/// Takes the session out of a game that goes on without it: other players must not stay frozen
/// there, nor its chat on the screen, nor its objects waiting on games that no longer say what
/// they do. A player who did not choose to leave is told in the game why the others are gone.
/// `keys_held` says the keys were the chat's as the session ended, and may still be down, and
/// `size` is the chat text's.
fn leave_game<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    shown: &mut ShownPlayers,
    end: &End,
    keys_held: bool,
    size: ChatSize,
) -> io::Result<()> {
    if !mailbox.is_ready()? {
        return Ok(());
    }
    clear_all(mailbox, shown)?;
    let parting = match end {
        End::Left | End::GameClosed(_) => Vec::new(),
        End::ServerSilent | End::Removed(_) | End::Refused(_) | End::NetworkFailing(_) => {
            text::rows(&format!("- {}", end.describe()), size)
        }
    };
    mailbox.end_session(&parting, keys_held, size)
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
    /// The game's count of frames run as last read, and since when it has been that: see
    /// `is_game_stopped`.
    frames_run: Option<(u32, Instant)>,
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
            frames_run: None,
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

    /// Whether the game has not run a frame for `STOPPED_AFTER`: it crashed, or its emulator
    /// is held. A paused game runs on, and is not this. Call it in send slots that carry no
    /// state; one that does shows the game runs, and is noted by `is_game_running`.
    fn is_game_stopped<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        now: Instant,
    ) -> io::Result<bool> {
        let frames = mailbox.frames_run()?;
        if self.frames_run.is_none_or(|(seen, _)| seen != frames) {
            self.frames_run = Some((frames, now));
        }
        Ok(self
            .frames_run
            .is_some_and(|(_, since)| now.saturating_duration_since(since) >= STOPPED_AFTER))
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
    /// `over_chat` says the mouse rests on the game's chat, which then shows. Returns why the
    /// server removed this player, if it did.
    fn step<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        connection: &mut Connection,
        now: Instant,
        people: &mut People,
        events: &mut impl FnMut(Event),
        over_chat: bool,
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
                // The player stays in the session all the same. A game that is only paused
                // has its Mario stand where he was for the others; one with no Mario to
                // show, at a menu or stopped for good, is said to be away, and they stop
                // showing him.
                self.objects.no_state_went();
                let away = !playing || self.is_game_stopped(mailbox, now)?;
                connection.keep_alive(away)?;
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
            people.show(mailbox, connection.player(), now, over_chat)?;
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

/// What the tests of a session and of its `People` both use.
#[cfg(test)]
pub(crate) mod testing {
    use super::Options;
    use crate::layout::len;
    use crate::mailbox::testing::raw;
    use crate::text;
    use std::io;

    pub fn options(server: std::net::SocketAddr) -> Options {
        Options {
            rom: Vec::new(),
            pid: 0,
            server,
            server_name: None,
            server_key: None,
            password: String::new(),
            name: String::new(),
            outfit: None,
            host_password: String::new(),
            chat_key: Some('t'),
            chat_seconds: 8,
            chat_lines: crate::typing::DEFAULT_CHAT_LINES,
            chat_on_hover: false,
            chat_size: crate::text::ChatSize::Normal,
            bot_port: None,
        }
    }

    /// Where the game keeps the chat a session gave it, from the mailbox's start, as
    /// `TrollEngine/Multiplayer/mailbox.h` lays it out.
    pub const CHAT_COUNT: usize = len::MAILBOX_CHAT_OFFSET;
    /// The rows shown, then the frames a line is typed for.
    pub const CHAT_SHOWN: usize = CHAT_COUNT + len::CHAT_SHOWN_OFFSET;
    pub const CHAT_ROWS: usize = CHAT_COUNT + len::CHAT_ROWS_OFFSET;
    pub const CHAT_DRAFT: usize = CHAT_COUNT + len::CHAT_DRAFT_OFFSET;

    /// The text in the game's memory at `offset`, up to its end code.
    pub fn text_at(
        process: &crate::ram::testing::FakeProcess,
        offset: usize,
    ) -> io::Result<String> {
        let field = raw(process, offset, text::ROW_BYTES)?;
        let end = field.iter().position(|&byte| byte == 0);
        let text = field.get(..end.unwrap_or(field.len())).unwrap_or_default();
        Ok(String::from_utf8_lossy(text).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{CHAT_DRAFT, CHAT_ROWS, CHAT_SHOWN, options, text_at};
    use super::{End, Event, HEADLINE_MOST, Link, Options, Session, leave_game, rejoin};
    use crate::client::{Connection, ShownPlayers, apply_snapshot, wrong_server};
    use crate::layout::{self, len};
    use crate::mailbox::testing::{game, make_ready, raw};
    use crate::mailbox::{Mailbox, Person};
    use crate::people::People;
    use crate::ram::N64Ram;
    use crate::text::{self, ChatSize};
    use b3313_protocol::secure::ServerKey;
    use b3313_protocol::{
        ObjectSet, Outfit, PLAYER_STATE_SIZE, PROGRESS_SIZE, PlayerId, Progress, RemoveReason,
        Role, TICK_INTERVAL, WORLD_INPUTS_SIZE,
    };
    use b3313_server::{Command, Config, StopOnDrop, serve, serve_with_console};
    use std::io;
    use std::net::{Ipv4Addr, UdpSocket};
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Where the game keeps what a session gave it, from the mailbox's start, as
    /// `TrollEngine/Multiplayer/mailbox.h` lays it out.
    const BRIDGE_LEASE: usize = len::MAILBOX_BRIDGE_LEASE_OFFSET;
    const FIRST_REMOTE_PLAYER: usize = len::MAILBOX_PLAYERS_OFFSET;
    const OBJECTS_OWNED: usize = len::MAILBOX_OBJECTS_OWNED_OFFSET;
    const PEOPLE: usize = len::MAILBOX_PEOPLE_OFFSET;
    /// The lease as an address, to write there as the game does.
    const BRIDGE_LEASE_ADDRESS: u32 = layout::MAILBOX_ADDR + layout::MAILBOX_BRIDGE_LEASE_OFFSET;

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
            let removed = link.session.step(
                mailbox,
                link.connection,
                now,
                link.people,
                &mut |_| {},
                false,
            )?;
            if removed.is_some() {
                return Ok(removed);
            }
        }
        Ok(None)
    }

    /// A server's settings with a key of its own, as a server started again has the same.
    fn with_a_key() -> io::Result<Config> {
        let key = ServerKey::generate().map_err(io::Error::other)?;
        Ok(Config {
            key: Some(key),
            ..Config::default()
        })
    }

    #[test]
    fn a_bridge_joins_again_only_the_server_it_was_in() -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server = socket.local_addr()?;
        let stop = AtomicBool::new(false);
        thread::scope(|scope| {
            let (socket, stop) = (&socket, &stop);
            let first = scope.spawn(move || serve(socket, Config::default(), stop, |_| {}));
            let process = game();
            make_ready(&process)?;
            let mailbox = Mailbox::new(N64Ram::new(&process, 0));
            let options = options(server);
            let mut people = People::new(&options);
            let mut connection = Connection::join(server, "", 0)?;
            let was_in = connection.server_key();
            let mut session = Session::new(Instant::now());
            let mut link = Link {
                connection: &mut connection,
                session: &mut session,
                people: &mut people,
            };

            // The server goes, and another, with a key of its own, answers at its address:
            // as when the address has come to be someone else's, or someone poses as it.
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
            first
                .join()
                .map_err(|_| io::Error::other("the first server panicked"))??;
            stop.store(false, std::sync::atomic::Ordering::Relaxed);
            scope.spawn(move || serve(socket, Config::default(), stop, |_| {}));
            let _stopping = StopOnDrop(stop);

            let never = AtomicBool::new(false);
            let ended = rejoin(&options, &never, &mailbox, &mut link, &mut |_| {});

            let Err(End::Refused(error)) = ended else {
                return Err(io::Error::other(format!("joined or ended as {ended:?}")));
            };
            let wrong = wrong_server(&error).ok_or_else(|| io::Error::other(error))?;
            assert_eq!(wrong.expected, was_in);
            assert_ne!(wrong.met, was_in);
            Ok(())
        })
    }

    #[test]
    fn a_bridge_whose_server_starts_again_joins_the_new_one() -> io::Result<()> {
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))?;
        let server = socket.local_addr()?;
        let stop = AtomicBool::new(false);
        let (commands, received) = mpsc::channel();
        let config = with_a_key()?;
        let again = config.clone();
        thread::scope(|scope| {
            let (socket, stop) = (&socket, &stop);
            let first =
                scope.spawn(move || serve_with_console(socket, config, stop, &received, |_| {}));
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
            play_on(&mut link, &mailbox, |_, people| people.is_listed())?;
            assert!(link.people.is_listed());

            // The server's owner restarts it: it says so and stops, and another takes its place.
            commands
                .send(Command::Restart)
                .map_err(|_| io::Error::other("the first server has gone"))?;
            let told = play_on(&mut link, &mailbox, |_, _| false)?;
            assert_eq!(told, Some(RemoveReason::Restarting));
            first
                .join()
                .map_err(|_| io::Error::other("the first server panicked"))??;
            scope.spawn(move || serve(socket, again, stop, |_| {}));

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
                people.is_listed() && session.has_session_world
            })?;
            assert!(link.people.is_listed());
            assert!(link.session.has_session_world);
            let name = link
                .people
                .roster()
                .first()
                .map(|entry| entry.name.as_str());
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
    fn every_way_a_session_ends_is_said_in_few_enough_words_for_the_sidebar() {
        let failed = || io::Error::other("the system's own words, however long they are");
        let ends = [
            End::Left,
            End::GameClosed(failed()),
            End::ServerSilent,
            End::Removed(RemoveReason::Kicked),
            End::Removed(RemoveReason::Banned),
            End::Removed(RemoveReason::Ended),
            End::Removed(RemoveReason::Restarting),
            End::Refused(failed()),
            End::NetworkFailing(failed()),
        ];
        for end in ends {
            let headline = end.headline();
            let length = headline.chars().count();
            assert!((1..=HEADLINE_MOST).contains(&length), "{headline}");
            // A few words of its own, not the sentence cut short.
            assert!(!headline.ends_with('.'), "{headline}");
        }
    }

    #[test]
    fn a_player_the_session_leaves_is_told_why_in_the_game() -> io::Result<()> {
        let process = game();
        make_ready(&process)?;
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let ended = End::Removed(b3313_protocol::RemoveReason::Ended);

        leave_game(
            &mailbox,
            &mut ShownPlayers::default(),
            &ended,
            false,
            ChatSize::Normal,
        )?;

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
        leave_game(
            &mailbox,
            &mut ShownPlayers::default(),
            &ended,
            true,
            ChatSize::Normal,
        )?;

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
        for row in text::rows("Luigi: hello", ChatSize::Normal) {
            mailbox.write_chat_row(0, &row)?;
        }
        mailbox.show_chat(1, false, ChatSize::Normal)?;
        let mut owned = ObjectSet::default();
        owned.insert(3);
        mailbox.write_objects_owned((9, 1), &owned, &ObjectSet::default())?;
        mailbox.write_session_progress(&Progress::from_bytes([0xFF; PROGRESS_SIZE]))?;

        leave_game(&mailbox, &mut shown, &End::Left, false, ChatSize::Normal)?;

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
            session.step(
                mailbox,
                connection,
                Instant::now(),
                people,
                &mut |_| {},
                false,
            )?;
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
                session.step(
                    &mailbox,
                    &mut connection,
                    now,
                    &mut people,
                    &mut |_| {},
                    false,
                )
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
            leave_game(
                &mailbox,
                &mut session.shown,
                &End::Left,
                false,
                ChatSize::Normal,
            )?;
            assert_eq!(lease()?, Some(0));
            Ok(())
        })
    }

    #[test]
    fn leaving_writes_nothing_to_a_game_that_is_not_the_multiplayer_one() -> io::Result<()> {
        let process = game();
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));

        leave_game(
            &mailbox,
            &mut ShownPlayers::default(),
            &End::Left,
            false,
            ChatSize::Normal,
        )?;

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
                session.step(
                    &mailbox,
                    &mut connection,
                    now,
                    &mut people,
                    &mut |_| {},
                    false,
                )?;
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
                session.step(
                    &mailbox,
                    &mut connection,
                    now,
                    &mut people,
                    &mut note,
                    false,
                )?;
            }
            assert_eq!(told, [None]);
            Ok(())
        })
    }
}
