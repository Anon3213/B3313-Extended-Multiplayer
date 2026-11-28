//! The bot API: a program on this PC plays the game through its bridge. It connects to the
//! bridge's port on 127.0.0.1 and sends requests, one JSON object a line, and the bridge
//! answers each, one a line, and tells it of what happens: chat, players moving between areas,
//! and how what it sent Mario to do went. `BOTS.md` is the guide for whoever writes one.
//!
//! The bridge takes one bot at a time, and only when the player asked it to (`Options::bot_port`):
//! the port is opened for this PC alone, and a connection whose first line is not a `hello`
//! is closed, so a web page that has a browser send something to the port does nothing. Any
//! program on the PC can reach the game's memory as the bridge does, so the port gives it
//! nothing it could not take.
//!
//! What a bot can do is what a player can: see their own game, hold its controller (through
//! the mailbox's `padIn`, which the game reads in place of the controller), chat, and send Mario
//! somewhere (`travel`). It sees other players only as the bridge draws them.

use crate::game::{Area, Mario};
use crate::json::{self, Value};
use crate::mailbox::HeldPad;
use crate::travel::{self, Order, Report, Seen, flat_distance};
use b3313_protocol::PlayerId;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

/// The port the panel opens for a bot unless the player names another: the game's name, and a
/// zero.
pub const DEFAULT_PORT: u16 = 33130;
/// What a bot is told this API is, for one written for another to notice.
pub const API_VERSION: u8 = 1;
/// The longest line a bot may send, and the most the bridge keeps for one that does not read
/// what it is sent before it gives the connection up.
const LINE_MOST: usize = 64 * 1024;
const OUTBOX_MOST: usize = 4 * 1024 * 1024;
/// How often a bot may be sent the state it watches, at most: as often as the session looks
/// at the game for a bot that only watches (`assist`).
const WATCH_FASTEST: Duration = Duration::from_millis(100);
/// The longest one press may hold the controller, in the game's frames: half a minute at the
/// game's own speed, so a bot whose bridge is gone leaves no button held for long.
pub const PAD_FRAMES_MOST: u16 = 900;

/// The controller's buttons by name, as the game's `OSContPad` has them.
pub const BUTTONS: [(&str, u16); 14] = [
    ("A", 0x8000),
    ("B", 0x4000),
    ("Z", 0x2000),
    ("START", 0x1000),
    ("D_UP", 0x0800),
    ("D_DOWN", 0x0400),
    ("D_LEFT", 0x0200),
    ("D_RIGHT", 0x0100),
    ("L", 0x0020),
    ("R", 0x0010),
    ("C_UP", 0x0008),
    ("C_DOWN", 0x0004),
    ("C_LEFT", 0x0002),
    ("C_RIGHT", 0x0001),
];

/// What a bot asked for that the session must answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    /// Who and where it is: the first request.
    Hello,
    /// The state of the game now.
    Look,
    /// The areas of the level and their warps.
    Map,
    /// The controller held so, for `frames` of the game's frames.
    Pad(HeldPad),
    /// A line said in the chat.
    Say(String),
    /// Mario sent somewhere, or stopped.
    Travel(Order),
}

/// A request, with the ticket its answers are sent under.
#[derive(Clone, Debug, PartialEq)]
pub struct Asked {
    pub request: Request,
    pub ticket: u32,
}

fn number(request: &Value, key: &str) -> Result<Option<f64>, String> {
    match request.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .map(Some)
            .ok_or_else(|| format!("{key} is not a number")),
    }
}

fn whole<T: TryFrom<i64>>(request: &Value, key: &str) -> Result<Option<T>, String> {
    let Some(value) = number(request, key)? else {
        return Ok(None);
    };
    let wrong = || format!("{key} is not a whole number in range");
    if value.fract() != 0.0 || value.abs() > 1e9 {
        return Err(wrong());
    }
    // A float has no checked conversion to an integer; the shortest decimal of a whole one,
    // which is its digits, does.
    format!("{value}")
        .parse::<i64>()
        .ok()
        .and_then(|value| T::try_from(value).ok())
        .map(Some)
        .ok_or_else(wrong)
}

fn text<'a>(request: &'a Value, key: &str) -> Result<&'a str, String> {
    request
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} is missing, or not text"))
}

fn buttons(request: &Value) -> Result<u16, String> {
    let Some(given) = request.get("buttons") else {
        return Ok(0);
    };
    let names = given
        .as_array()
        .ok_or("buttons is not a list of names, as [\"A\", \"Z\"]")?;
    names.iter().try_fold(0, |held, name| {
        let name = name.as_str().ok_or("a button is not a name")?;
        let wanted = name.trim().to_ascii_uppercase();
        BUTTONS
            .iter()
            .find(|(known, _)| *known == wanted)
            .map(|(_, bit)| held | bit)
            .ok_or_else(|| format!("there is no button {name}"))
    })
}

fn stick(request: &Value) -> Result<[i8; 2], String> {
    let Some(given) = request.get("stick") else {
        return Ok([0, 0]);
    };
    match given.as_array() {
        Some([Value::Number(x), Value::Number(y)]) if x.abs() <= 1.0 && y.abs() <= 1.0 => {
            let share = |value: f64| format!("{value}").parse::<f32>().unwrap_or(0.0);
            Ok([
                travel::stick_value(share(*x)),
                travel::stick_value(share(*y)),
            ])
        }
        _ => Err("stick is not [x, y], each from -1 to 1".to_owned()),
    }
}

/// What a line from a bot asks; the words that say what is wrong with it if it asks nothing.
///
/// # Errors
/// What is wrong with the request, in words.
pub fn read_request(request: &Value) -> Result<Request, String> {
    let kind = text(request, "type")?;
    Ok(match kind {
        "hello" => Request::Hello,
        "look" => Request::Look,
        "map" => Request::Map,
        "pad" => {
            let frames = whole::<u16>(request, "frames")?.unwrap_or(1);
            if frames > PAD_FRAMES_MOST {
                return Err(format!("frames is at most {PAD_FRAMES_MOST}"));
            }
            Request::Pad(HeldPad {
                buttons: buttons(request)?,
                stick: stick(request)?,
                frames,
            })
        }
        "say" => Request::Say(text(request, "text")?.to_owned()),
        "warp" => Request::Travel(Order::Warp {
            level: whole(request, "level")?.ok_or("level is missing")?,
            area: whole(request, "area")?,
        }),
        "goto" => Request::Travel(Order::Goto(text(request, "player")?.to_owned())),
        "follow" => Request::Travel(Order::Follow(text(request, "player")?.to_owned())),
        "go" => {
            let point = |key| number(request, key)?.ok_or(format!("{key} is missing"));
            let [x, y, z] = [point("x")?, point("y")?, point("z")?]
                .map(|value| format!("{value}").parse::<f32>().unwrap_or(f32::NAN));
            Request::Travel(Order::Go([x, y, z]))
        }
        "stop" => Request::Travel(Order::Stop),
        other => return Err(format!("there is no request {other}")),
    })
}

/// Adds what `stream` has for reading now to `inbox`; whether the connection has closed.
fn read_into(stream: &mut TcpStream, inbox: &mut Vec<u8>) -> bool {
    let mut buffer = [0; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => return true,
            Ok(read) => inbox.extend(buffer.get(..read).unwrap_or_default()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return false,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return true,
        }
        if inbox.len() > LINE_MOST * 2 {
            return false;
        }
    }
}

/// A connected bot.
#[derive(Debug)]
struct Client {
    stream: TcpStream,
    inbox: Vec<u8>,
    outbox: Vec<u8>,
    greeted: bool,
    /// How often it is sent the state, and when next.
    watch: Option<(Duration, Instant)>,
}

/// The bot API's port, and the bot connected to it, if one is.
#[derive(Debug)]
pub struct Bots {
    listener: TcpListener,
    client: Option<Client>,
    next_ticket: u32,
    /// Each request still to be answered: its ticket, and the `id` the bot gave it, if any.
    open: Vec<(u32, Option<Value>)>,
}

impl Bots {
    /// Opens the port on 127.0.0.1, for programs on this PC alone.
    ///
    /// # Errors
    /// Fails if the port cannot be opened, as when another program has it.
    pub fn open(port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            client: None,
            next_ticket: 1,
            open: Vec::new(),
        })
    }

    /// Where bots connect.
    ///
    /// # Errors
    /// Fails if the system cannot say, which it always can for an open port.
    pub fn address(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Whether a bot is connected and has said hello.
    #[must_use]
    pub fn has_bot(&self) -> bool {
        self.client.as_ref().is_some_and(|client| client.greeted)
    }

    /// Takes a new connection, and what the bot sent since the last call: the requests the
    /// session must answer. The bot is let go if it closed the connection, or sent a line that
    /// is not JSON before its hello; the returned flag says one was, so that what it held is
    /// let go too.
    pub fn poll(&mut self, now: Instant) -> (Vec<Asked>, bool) {
        self.accept();
        let Some(client) = &mut self.client else {
            return (Vec::new(), false);
        };
        let mut closed = read_into(&mut client.stream, &mut client.inbox);
        let mut lines = Vec::new();
        while let Some(end) = client.inbox.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = client.inbox.drain(..=end).collect();
            lines.push(String::from_utf8_lossy(&line).trim().to_owned());
        }
        // A line this long without its end is no request.
        closed |= client.inbox.len() > LINE_MOST;
        let mut asked = Vec::new();
        for line in lines.iter().filter(|line| !line.is_empty()) {
            match self.take_line(line, now) {
                Ok(Some(one)) => asked.push(one),
                Ok(None) => {}
                Err(()) => {
                    closed = true;
                    break;
                }
            }
        }
        if closed {
            let had_greeted = self.has_bot();
            self.client = None;
            self.open.clear();
            return (asked, had_greeted);
        }
        (asked, false)
    }

    /// Accepts a connection; while a bot is connected, says so to another and closes it.
    fn accept(&mut self) {
        while let Ok((mut stream, _)) = self.listener.accept() {
            if self.client.is_some() {
                let busy = Value::object([
                    ("type", "error".into()),
                    ("error", "another bot is playing this game".into()),
                ]);
                let _ = writeln!(stream, "{busy}");
                continue;
            }
            if stream.set_nonblocking(true).is_err() || stream.set_nodelay(true).is_err() {
                continue;
            }
            self.client = Some(Client {
                stream,
                inbox: Vec::new(),
                outbox: Vec::new(),
                greeted: false,
                watch: None,
            });
        }
    }

    /// One line from the bot: a request for the session, one answered here, or, before the
    /// bot has said hello, anything else, which ends the connection.
    fn take_line(&mut self, line: &str, now: Instant) -> Result<Option<Asked>, ()> {
        let greeted = self.has_bot();
        let parsed = json::parse(line);
        let id = parsed
            .as_ref()
            .ok()
            .and_then(|value| value.get("id"))
            .cloned();
        let request = parsed.and_then(|value| {
            if value.get("type").and_then(Value::as_str) == Some("watch") {
                return Ok(Err(number(&value, "every")?));
            }
            read_request(&value).map(Ok)
        });
        let ticket = self.ticket(id);
        match request {
            Err(_) if !greeted => Err(()),
            Ok(Ok(Request::Hello)) => {
                if let Some(client) = &mut self.client {
                    client.greeted = true;
                }
                Ok(Some(Asked {
                    request: Request::Hello,
                    ticket,
                }))
            }
            _ if !greeted => {
                self.refuse(ticket, "say hello first: {\"type\":\"hello\"}");
                Ok(None)
            }
            Err(why) => {
                self.refuse(ticket, &why);
                Ok(None)
            }
            Ok(Err(every)) => {
                let period = every.filter(|&seconds| seconds > 0.0).map(|seconds| {
                    Duration::try_from_secs_f64(seconds)
                        .unwrap_or(WATCH_FASTEST)
                        .max(WATCH_FASTEST)
                });
                if let Some(client) = &mut self.client {
                    client.watch = period.map(|period| (period, now));
                }
                self.reply(ticket, Value::object([("type", "ok".into())]));
                Ok(None)
            }
            Ok(Ok(request)) => Ok(Some(Asked { request, ticket })),
        }
    }

    fn ticket(&mut self, id: Option<Value>) -> u32 {
        let ticket = self.next_ticket;
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        self.open.push((ticket, id));
        // A bot that sends faster than it is answered is not kept track of without end.
        if self.open.len() > 1024 {
            self.open.remove(0);
        }
        ticket
    }

    /// Answers request `ticket` with `answer`, carrying the `id` the bot gave the request as
    /// `re`. A ticket of a request that is still under way, as a warp, stays open for its
    /// `done`; `close` says this is the last answer.
    fn answer(&mut self, ticket: u32, mut answer: Value, close: bool) {
        let index = self.open.iter().position(|(open, _)| *open == ticket);
        let id = index.and_then(|index| {
            let id = self.open.get(index).and_then(|(_, id)| id.clone());
            if close {
                self.open.remove(index);
            }
            id
        });
        if let Some(id) = id {
            answer.insert("re", id);
        }
        self.send(&answer);
    }

    /// The answer to request `ticket`.
    pub fn reply(&mut self, ticket: u32, answer: Value) {
        self.answer(ticket, answer, true);
    }

    /// An answer to request `ticket` that it has more to come: Mario is on his way.
    pub fn started(&mut self, ticket: u32, words: &str) {
        let answer = Value::object([("type", "ok".into()), ("doing", words.into())]);
        self.answer(ticket, answer, false);
    }

    /// Request `ticket` cannot be done, for the reason given.
    pub fn refuse(&mut self, ticket: u32, why: &str) {
        let answer = Value::object([("type", "error".into()), ("error", why.into())]);
        self.answer(ticket, answer, true);
    }

    /// How an order a bot gave went.
    pub fn report(&mut self, ticket: u32, report: &Report) {
        let answer = Value::object([
            ("type", "done".into()),
            ("ok", report.done.into()),
            ("words", report.words.as_str().into()),
        ]);
        self.answer(ticket, answer, true);
    }

    /// Tells the bot of something that happened, once it has said hello.
    pub fn tell(&mut self, news: &Value) {
        if self.has_bot() {
            self.send(news);
        }
    }

    /// Whether the bot watches the state and is due to be sent it.
    pub fn is_watch_due(&mut self, now: Instant) -> bool {
        let Some(client) = self.client.as_mut().filter(|client| client.greeted) else {
            return false;
        };
        match &mut client.watch {
            Some((period, next)) if now >= *next => {
                *next = now + *period;
                true
            }
            _ => false,
        }
    }

    fn send(&mut self, message: &Value) {
        if let Some(client) = &mut self.client {
            let _ = writeln!(client.outbox, "{message}");
        }
    }

    /// Writes what the bot has yet to be sent, as much as it takes now. A bot that has not
    /// read for so long that too much waits is let go; the returned flag says so.
    pub fn flush(&mut self) -> bool {
        let Some(client) = &mut self.client else {
            return false;
        };
        while !client.outbox.is_empty() {
            match client.stream.write(&client.outbox) {
                Ok(0) => break,
                Ok(written) => {
                    client.outbox.drain(..written);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    client.outbox.clear();
                    break;
                }
            }
        }
        if client.outbox.len() > OUTBOX_MOST {
            let had_greeted = client.greeted;
            self.client = None;
            self.open.clear();
            return had_greeted;
        }
        false
    }
}

/// An angle of the game's, where a whole turn is 65536, in degrees.
fn degrees(angle: i16) -> f32 {
    f32::from(angle) * 360.0 / 65536.0
}

fn place_value(place: (i16, u8), position: [f32; 3]) -> Value {
    Value::object([
        ("level", place.0.into()),
        ("area", place.1.into()),
        ("pos", position.into()),
    ])
}

/// What a bot is told when it says hello.
#[must_use]
pub fn welcome(game: u8, player: PlayerId, name: Option<&str>) -> Value {
    let buttons: Vec<Value> = BUTTONS.iter().map(|(name, _)| (*name).into()).collect();
    Value::object([
        ("type", "welcome".into()),
        ("api", API_VERSION.into()),
        ("game", b3313_protocol::game::name(game).into()),
        ("player", player.get().into()),
        ("name", name.into()),
        ("buttons", Value::Array(buttons)),
        ("pad_frames_most", PAD_FRAMES_MOST.into()),
        ("levels", travel::warp_levels().into()),
    ])
}

/// The game's state as a bot is told it.
#[must_use]
pub fn state(seen: &Seen<'_>, doing: Option<String>) -> Value {
    let mario: &Mario = &seen.mario;
    let players: Vec<Value> = seen
        .others
        .iter()
        .map(|other| {
            let distance =
                (other.place == seen.place).then(|| flat_distance(mario.position, other.position));
            Value::object([
                ("name", other.name.as_str().into()),
                ("player", other.player.get().into()),
                ("level", other.place.0.into()),
                ("area", other.place.1.into()),
                ("pos", other.position.into()),
                ("distance", distance.into()),
            ])
        })
        .collect();
    Value::object([
        ("type", "state".into()),
        ("playing", seen.playing.into()),
        ("level", seen.place.0.into()),
        ("area", seen.place.1.into()),
        (
            "mario",
            Value::object([
                ("pos", mario.position.into()),
                ("vel", mario.velocity.into()),
                ("speed", mario.forward_speed.into()),
                ("yaw", degrees(mario.yaw).into()),
                ("action", format!("{:#010x}", mario.action).into()),
                ("in_air", mario.is_in_air().into()),
                ("swimming", mario.is_swimming().into()),
                ("health", (mario.health >> 8).into()),
                ("coins", mario.coins.into()),
                ("stars", mario.stars.into()),
                ("lives", mario.lives.into()),
            ]),
        ),
        ("camera_yaw", mario.camera_yaw.map(degrees).into()),
        ("players", Value::Array(players)),
        ("doing", doing.into()),
    ])
}

/// The level's areas and their ways out, as a bot is told them.
#[must_use]
pub fn map(place: (i16, u8), areas: &[Area]) -> Value {
    let areas: Vec<Value> = areas
        .iter()
        .map(|area| {
            let exits: Vec<Value> = area
                .nodes
                .iter()
                .map(|node| {
                    Value::object([
                        ("node", node.id.into()),
                        (
                            "to",
                            Value::object([
                                ("level", node.to_level.into()),
                                ("area", node.to_area.into()),
                                ("node", node.to_node.into()),
                            ]),
                        ),
                        ("pos", node.position.into()),
                    ])
                })
                .collect();
            Value::object([("area", area.index.into()), ("exits", Value::Array(exits))])
        })
        .collect();
    Value::object([
        ("type", "map".into()),
        ("level", place.0.into()),
        ("area", place.1.into()),
        ("areas", Value::Array(areas)),
    ])
}

/// A line of chat, as a bot is told it: from a player, by name, or from the server.
#[must_use]
pub fn chat(from: Option<&str>, words: &str) -> Value {
    Value::object([
        ("type", "chat".into()),
        ("from", from.into()),
        ("text", words.into()),
    ])
}

/// How long a player must have been in an area they came to before they are said to have
/// moved: in the first moments the game may tell of their place before their position.
const ARRIVED_AFTER: Duration = Duration::from_millis(500);

/// A place and a position in it.
type Spot = ((i16, u8), [f32; 3]);

/// Where one player is known to be, and where they are arriving, since when.
#[derive(Debug, Default)]
struct Track {
    settled: Option<Spot>,
    arriving: Option<((i16, u8), Instant)>,
}

impl Track {
    /// Notes where the player is now; where they came from and to, once they have arrived
    /// somewhere new.
    fn observe(&mut self, now: Spot, at: Instant) -> Option<(Spot, Spot)> {
        let Some(settled) = self.settled else {
            self.settled = Some(now);
            return None;
        };
        if now.0 == settled.0 {
            self.arriving = None;
            self.settled = Some(now);
            return None;
        }
        match self.arriving {
            Some((place, since)) if place == now.0 => {
                if at.saturating_duration_since(since) < ARRIVED_AFTER {
                    return None;
                }
                self.arriving = None;
                self.settled = Some(now);
                Some((settled, now))
            }
            _ => {
                self.arriving = Some((now.0, at));
                None
            }
        }
    }
}

/// Who went from one area to another, by watching where everyone is from turn to turn.
#[derive(Debug, Default)]
pub struct Arrivals {
    mine: Track,
    theirs: Vec<(PlayerId, Track)>,
}

impl Arrivals {
    /// What has moved since the last turn, as news for a bot: `{"type":"moved", ...}` for each
    /// player who has arrived in another area than they were last in, with where they left
    /// from, which is where a painting, a door or a pipe took them, and where they came to.
    pub fn moved(&mut self, seen: &Seen<'_>, my_name: Option<&str>) -> Vec<Value> {
        let mut news = Vec::new();
        if seen.playing
            && let Some((from, to)) = self
                .mine
                .observe((seen.place, seen.mario.position), seen.now)
        {
            news.push(moved(my_name, true, from, to));
        }
        let mut theirs = Vec::with_capacity(seen.others.len());
        for other in seen.others {
            let known = self
                .theirs
                .iter()
                .position(|(player, _)| *player == other.player);
            let mut track = match known {
                Some(index) => self.theirs.swap_remove(index).1,
                None => Track::default(),
            };
            if let Some((from, to)) = track.observe((other.place, other.position), seen.now) {
                news.push(moved(Some(&other.name), false, from, to));
            }
            theirs.push((other.player, track));
        }
        self.theirs = theirs;
        news
    }
}

fn moved(
    name: Option<&str>,
    me: bool,
    from: ((i16, u8), [f32; 3]),
    to: ((i16, u8), [f32; 3]),
) -> Value {
    Value::object([
        ("type", "moved".into()),
        ("name", name.into()),
        ("you", me.into()),
        ("from", place_value(from.0, from.1)),
        ("to", place_value(to.0, to.1)),
    ])
}

#[cfg(test)]
mod tests {
    use super::{Arrivals, Asked, Bots, Request, read_request, state, welcome};
    use crate::game::Mario;
    use crate::json::{self, Value};
    use crate::mailbox::HeldPad;
    use crate::travel::{Order, Other, Seen};
    use b3313_protocol::PlayerId;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::time::{Duration, Instant};

    fn request(line: &str) -> Result<Request, String> {
        read_request(&json::parse(line)?)
    }

    #[test]
    fn requests_are_read_with_their_values_checked() {
        assert_eq!(
            request(r#"{"type":"pad","buttons":["a","Z"],"stick":[0,1],"frames":30}"#),
            Ok(Request::Pad(HeldPad {
                buttons: 0xA000,
                stick: [0, 80],
                frames: 30
            }))
        );
        assert_eq!(
            request(r#"{"type":"pad"}"#),
            Ok(Request::Pad(HeldPad {
                buttons: 0,
                stick: [0, 0],
                frames: 1
            }))
        );
        assert_eq!(
            request(r#"{"type":"warp","level":9,"area":2}"#),
            Ok(Request::Travel(Order::Warp {
                level: 9,
                area: Some(2)
            }))
        );
        assert_eq!(
            request(r#"{"type":"go","x":1.5,"y":0,"z":-3}"#),
            Ok(Request::Travel(Order::Go([1.5, 0.0, -3.0])))
        );
        assert_eq!(
            request(r#"{"type":"follow","player":"Luigi"}"#),
            Ok(Request::Travel(Order::Follow("Luigi".into())))
        );
        for wrong in [
            r#"{"type":"pad","buttons":["X"]}"#,
            r#"{"type":"pad","stick":[2,0]}"#,
            r#"{"type":"pad","frames":901}"#,
            r#"{"type":"pad","frames":1.5}"#,
            r#"{"type":"warp","level":9.5}"#,
            r#"{"type":"warp"}"#,
            r#"{"type":"say"}"#,
            r#"{"type":"fly"}"#,
            r#"{"kind":"look"}"#,
        ] {
            assert!(request(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn the_state_says_where_everyone_is_in_the_bots_own_words() -> Result<(), String> {
        let others = [Other {
            player: PlayerId::new(2).unwrap_or_else(|| unreachable!()),
            name: "Luigi".into(),
            place: (9, 1),
            position: [300.0, 0.0, 400.0],
            yaw: 0,
        }];
        let seen = Seen {
            now: Instant::now(),
            playing: true,
            in_level: true,
            place: (9, 1),
            area_loads: 3,
            mario: Mario {
                position: [0.0; 3],
                velocity: [0.0; 3],
                forward_speed: 0.0,
                yaw: 0x4000,
                action: 0x0C40_0201,
                health: 0x880,
                coins: 3,
                stars: 1,
                lives: 4,
                camera_yaw: Some(-0x8000),
            },
            others: &others,
        };
        let told = json::parse(&state(&seen, Some("following Luigi".into())).to_string())?;
        let mario = told.get("mario").ok_or("no mario")?;
        assert_eq!(mario.get("yaw").and_then(Value::as_f64), Some(90.0));
        assert_eq!(mario.get("health").and_then(Value::as_f64), Some(8.0));
        assert_eq!(
            mario.get("action").and_then(Value::as_str),
            Some("0x0c400201")
        );
        assert_eq!(told.get("camera_yaw").and_then(Value::as_f64), Some(-180.0));
        let luigi = told
            .get("players")
            .and_then(Value::as_array)
            .and_then(<[Value]>::first)
            .ok_or("no players")?;
        assert_eq!(luigi.get("distance").and_then(Value::as_f64), Some(500.0));
        assert_eq!(
            told.get("doing").and_then(Value::as_str),
            Some("following Luigi")
        );
        Ok(())
    }

    #[test]
    fn a_player_who_changes_area_is_news_with_where_they_left() {
        fn seen(others: &[Other], place: (i16, u8), mario: Mario, now: Instant) -> Seen<'_> {
            Seen {
                now,
                playing: true,
                in_level: true,
                place,
                area_loads: 0,
                mario,
                others,
            }
        }
        let player = PlayerId::new(3).unwrap_or_else(|| unreachable!());
        let at = |place: (i16, u8), x: f32| Other {
            player,
            name: "Luigi".into(),
            place,
            position: [x, 0.0, 0.0],
            yaw: 0,
        };
        let mario = Mario {
            position: [0.0; 3],
            velocity: [0.0; 3],
            forward_speed: 0.0,
            yaw: 0,
            action: 0,
            health: 0,
            coins: 0,
            stars: 0,
            lives: 0,
            camera_yaw: None,
        };
        let start = Instant::now();
        let mut arrivals = Arrivals::default();
        let first = [at((9, 1), 10.0)];
        assert_eq!(
            arrivals.moved(&seen(&first, (9, 1), mario, start), Some("Bot")),
            []
        );
        let second = [at((9, 1), 20.0)];
        assert_eq!(
            arrivals.moved(&seen(&second, (9, 1), mario, start), Some("Bot")),
            []
        );
        // Elsewhere, but only just: not yet news, as the place may be told before the
        // position.
        let just = [at((6, 1), 0.0)];
        assert_eq!(
            arrivals.moved(&seen(&just, (9, 2), mario, start), Some("Bot")),
            []
        );
        let settled = [at((6, 1), 99.0)];
        let later = start + Duration::from_secs(1);
        let news = arrivals.moved(&seen(&settled, (9, 2), mario, later), Some("Bot"));
        let text: Vec<String> = news.iter().map(Value::to_string).collect();
        assert_eq!(
            text,
            [
                r#"{"type":"moved","name":"Bot","you":true,"from":{"level":9,"area":1,"pos":[0,0,0]},"to":{"level":9,"area":2,"pos":[0,0,0]}}"#,
                r#"{"type":"moved","name":"Luigi","you":false,"from":{"level":9,"area":1,"pos":[20,0,0]},"to":{"level":6,"area":1,"pos":[99,0,0]}}"#,
            ]
        );
        // And news once only.
        assert_eq!(
            arrivals.moved(&seen(&settled, (9, 2), mario, later), Some("Bot")),
            []
        );
    }

    /// Reads the next line the bridge sent, as JSON.
    fn next(reader: &mut BufReader<TcpStream>) -> Result<Value, String> {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        json::parse(line.trim())
    }

    /// Polls until what the bridge has taken in all, or its loss of the bot, is enough, or a
    /// while has passed.
    fn poll_until(bots: &mut Bots, done: impl Fn(&[Asked], bool) -> bool) -> (Vec<Asked>, bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut all = Vec::new();
        loop {
            let (asked, lost) = bots.poll(Instant::now());
            all.extend(asked);
            bots.flush();
            if done(&all, lost) || Instant::now() > deadline {
                return (all, lost);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_bot_says_hello_asks_and_is_answered_under_its_id() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut bots = Bots::open(0)?;
        let mut bot = TcpStream::connect(bots.address()?)?;
        bot.set_read_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(bot.try_clone()?);
        writeln!(bot, r#"{{"type":"look","id":1}}"#)?;
        writeln!(bot, r#"{{"type":"hello","id":"h"}}"#)?;
        writeln!(bot, r#"{{"type":"say","text":"hi","id":2}}"#)?;
        let (asked, _) = poll_until(&mut bots, |asked, _| asked.len() >= 2);
        let requests: Vec<&Request> = asked.iter().map(|one| &one.request).collect();
        assert_eq!(requests, [&Request::Hello, &Request::Say("hi".into())]);
        assert!(bots.has_bot());
        // Asked before hello: refused, under its id, and the bot stays.
        let refused = next(&mut reader)?;
        assert_eq!(refused.get("re"), Some(&Value::Number(1.0)));
        assert_eq!(refused.get("type").and_then(Value::as_str), Some("error"));

        let hello = asked.first().map(|one| one.ticket).unwrap_or_default();
        let player = PlayerId::new(1).unwrap_or_else(|| unreachable!());
        bots.reply(hello, welcome(1, player, Some("Bot")));
        bots.flush();
        let welcomed = next(&mut reader)?;
        assert_eq!(welcomed.get("re").and_then(Value::as_str), Some("h"));
        assert_eq!(welcomed.get("player").and_then(Value::as_f64), Some(1.0));

        // Not JSON, after hello: refused, and the bot stays.
        writeln!(bot, "nonsense")?;
        let until = Instant::now() + Duration::from_millis(200);
        poll_until(&mut bots, |_, _| Instant::now() > until);
        let wrong = next(&mut reader)?;
        assert_eq!(wrong.get("type").and_then(Value::as_str), Some("error"));
        assert_eq!(wrong.get("re"), None);
        assert!(bots.has_bot());

        // A second bot is told this game has one, and the first goes on.
        let second = TcpStream::connect(bots.address()?)?;
        second.set_read_timeout(Some(Duration::from_secs(5)))?;
        bots.poll(Instant::now());
        let mut second = BufReader::new(second);
        assert_eq!(
            next(&mut second)?.get("type").and_then(Value::as_str),
            Some("error")
        );
        assert!(bots.has_bot());

        // Gone, it is let go, and what it held must be too.
        drop(reader);
        drop(bot);
        let (_, lost) = poll_until(&mut bots, |_, lost| lost);
        assert!(lost);
        assert!(!bots.has_bot());
        Ok(())
    }

    #[test]
    fn a_connection_that_does_not_begin_with_hello_is_closed()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut bots = Bots::open(0)?;
        let mut browser = TcpStream::connect(bots.address()?)?;
        browser.set_read_timeout(Some(Duration::from_millis(20)))?;
        write!(
            browser,
            "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n{{\"type\":\"say\",\"text\":\"spam\"}}\n"
        )?;
        let mut reader = BufReader::new(browser);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut taken = Vec::new();
        // Closed without a word: the read ends, and nothing was taken from it.
        let closed = loop {
            taken.extend(bots.poll(Instant::now()).0);
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => break true,
                _ if Instant::now() > deadline => break false,
                _ => {}
            }
        };
        assert!(closed);
        assert_eq!(taken, []);
        Ok(())
    }
}
