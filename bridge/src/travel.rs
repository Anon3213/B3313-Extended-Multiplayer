//! Where the local Mario goes when he is sent: to a level and an area, to another player, after
//! another player, or on foot to a point. A bot sends him through the bridge's bot API
//! (`bots`), and a player with a command in the chat (`command`): `/warp`, `/goto`, `/follow`
//! and `/stop`, which this player's own bridge carries out and the server never sees.
//!
//! A warp is the game's own: the bridge writes where to, as a painting or a pipe would, and the
//! game goes there (`game::warp`). A level is started afresh, as from the file select, because a
//! warp to a node an area lacks stops the game, and only the level loaded now can be asked
//! which nodes its areas have; within it, the area is then reached by one of its own nodes.
//!
//! On foot, Mario is steered by the stick, which the game turns by the camera's angle, as a
//! player's would be (`stick_toward`); he jumps when he is stuck or the way goes up. Following
//! a player who leaves the area, he goes to where they were last seen and jumps there, which
//! takes him through a painting or a door as it took them, and warps after them if that does
//! not take him there soon.

use crate::game::{self, Area, Mario, PAINTING_ENTRY_NODE};
use crate::mailbox::{HeldPad, Mailbox};
use crate::ram::ProcessMemory;
use b3313_protocol::PlayerId;
use std::f32::consts::TAU;
use std::io;
use std::ops::RangeInclusive;
use std::time::{Duration, Instant};

/// The level numbers the game's table of levels has room for.
pub const LEVELS: RangeInclusive<i16> = 1..=38;
/// The level numbers of that range that are no level Mario plays in, with what each is.
/// Warped to, the game stopped at each of these, or at 25, Super Mario 64's ending, would end
/// the game: found by warping a game to every number in turn (`logs/level_probe.py`,
/// 2026-11-27), 1 to 3 in A2AE and 1.0.2, the rest in A2AE. Levels 6 to 31 were not each tried
/// that way; 6, 9 and 26 were warped to in play.
const MENUS: [(i16, &str); 8] = [
    (1, "the title screen"),
    (2, "a screen that stops the game"),
    (3, "a screen that stops the game"),
    (25, "the game's ending"),
    (32, "a screen that stops the game"),
    (35, "a screen that stops the game"),
    (37, "a screen that stops the game"),
    (38, "a screen that stops the game"),
];

/// The levels a warp may go to: those of `LEVELS` that are not `MENUS`.
#[must_use]
pub fn warp_levels() -> Vec<i16> {
    LEVELS
        .filter(|level| MENUS.iter().all(|(menu, _)| menu != level))
        .collect()
}
/// The controller's A button, which jumps.
pub const A_BUTTON: u16 = 0x8000;
/// The controller's Start button, which goes on from the title screen and the file select.
pub const START_BUTTON: u16 = 0x1000;
/// How often a button is pressed to get into a level from the game's menus: long enough for a
/// menu or a text box to act on one press before the next, as the test scripts press them.
const ENTER_PRESS_EVERY: Duration = Duration::from_secs(1);
/// How long pressing through the menus is kept at before it is given up.
const ENTER_LIMIT: Duration = Duration::from_secs(180);
/// How far the stick is pushed, of the 80 or so a real one reaches: Mario runs.
pub const STICK_EDGE: f32 = 80.0;
/// How many of the game's frames each hold lasts: more than pass between two of the bridge's
/// turns in the test emulators, which run the game at 2.6 times its speed, so Mario is never
/// let go between them, and few enough that he stops soon after the bridge does.
const HOLD_FRAMES: u16 = 8;
/// How long a jump holds A, and how long after one before the next.
const JUMP_FOR: Duration = Duration::from_millis(250);
const JUMP_EVERY: Duration = Duration::from_millis(700);
/// Mario is stuck when he has moved less than this in `STUCK_AFTER` with the stick held.
const STUCK_DISTANCE: f32 = 40.0;
const STUCK_AFTER: Duration = Duration::from_secs(1);
/// A point higher than him by this, within the next distance, is jumped up to.
const CLIMB_HEIGHT: f32 = 150.0;
const CLIMB_REACH: f32 = 600.0;
/// Near enough to a point gone to, and to a player followed.
const ARRIVED_WITHIN: f32 = 100.0;
const FOLLOW_NEAR: f32 = 300.0;
/// Near enough to where a followed player left to jump there, as into a painting.
const EXIT_REACH: f32 = 150.0;
/// How long a follower tries to leave by where the player left, before warping after them.
const EXIT_PATIENCE: Duration = Duration::from_secs(15);
/// A walk that comes no nearer by `PROGRESS` in `GIVE_UP_AFTER` is not getting there: going to
/// a point fails, and following puts Mario beside the player.
const PROGRESS: f32 = 200.0;
const GIVE_UP_AFTER: Duration = Duration::from_secs(20);
/// How long a warp may take before it is written again, as the game does not take one while
/// it is busy with another thing, and before it is given up.
const WARP_AGAIN_AFTER: Duration = Duration::from_secs(5);
const WARP_LIMIT: Duration = Duration::from_secs(30);
/// How long Mario is left to come out of a warp before he is put beside a player gone to.
const SETTLE_FOR: Duration = Duration::from_millis(1500);
/// How long a followed player may be out of sight, at a menu or gone, before following ends.
const LOST_AFTER: Duration = Duration::from_secs(20);
/// How high above a player Mario is put beside them, so he lands on their floor, and how far
/// behind them.
const PLACE_ABOVE: f32 = 50.0;
const BEHIND: f32 = 100.0;

/// Where to send Mario.
#[derive(Clone, Debug, PartialEq)]
pub enum Order {
    /// To `area` of `level`, or to where the level starts if none is named.
    Warp { level: i16, area: Option<u8> },
    /// To the area another player is in now, and beside them, by their name.
    Goto(String),
    /// After another player, by their name, until stopped.
    Follow(String),
    /// On foot to a point of the area he is in.
    Go([f32; 3]),
    /// Nowhere: whatever he was sent to do ends, and the controller is the player's again.
    Stop,
}

/// What a line typed for the chat asks of this player's own game, if it is one of the
/// commands the bridge carries out itself: None for any other line, which goes to the server.
/// A command with something wrong in it is the words that say what.
#[must_use]
pub fn command(line: &str) -> Option<Result<Order, String>> {
    let mut words = line.split_whitespace();
    let name = words.next()?.to_ascii_lowercase();
    let rest: Vec<&str> = words.collect();
    let player = rest.join(" ");
    let order = match name.as_str() {
        "/warp" => warp_command(&rest),
        "/goto" | "/follow" if player.is_empty() => Err(format!("Usage: {name} <player>")),
        "/goto" => Ok(Order::Goto(player)),
        "/follow" => Ok(Order::Follow(player)),
        "/stop" => Ok(Order::Stop),
        _ => return None,
    };
    Some(order)
}

/// The chat's help for these commands, which the server's own `/help` does not know of.
pub const COMMANDS_HELP: &str = "In your own game: /warp <level> [area], /goto <player>, \
                                 /follow <player>, /stop.";

fn warp_command(words: &[&str]) -> Result<Order, String> {
    const USAGE: &str = "Usage: /warp <level> [area], as /warp 9 or /warp 9 2.";
    let (level, area) = match words {
        [level] => (*level, None),
        [level, area] => (*level, Some(*area)),
        _ => return Err(USAGE.to_owned()),
    };
    let level = level.parse().map_err(|_| USAGE.to_owned())?;
    let area = area
        .map(|area| area.parse().map_err(|_| USAGE.to_owned()))
        .transpose()?;
    check_place(level, area)?;
    Ok(Order::Warp { level, area })
}

/// Whether a level and area are ones a warp may name.
fn check_place(level: i16, area: Option<u8>) -> Result<(), String> {
    if !LEVELS.contains(&level) {
        return Err(format!(
            "There is no level {level}: levels go from {} to {}.",
            LEVELS.start(),
            LEVELS.end()
        ));
    }
    if let Some((_, menu)) = MENUS.iter().find(|(menu, _)| *menu == level) {
        return Err(format!("Level {level} is {menu}, not a level to play in."));
    }
    if area.is_some_and(|area| area >= game::AREA_SLOTS) {
        return Err(format!(
            "There is no such area: areas go from 0 to {}.",
            game::AREA_SLOTS - 1
        ));
    }
    Ok(())
}

/// Who sent an order, to be told how it went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The player, with a command in the chat.
    Chat,
    /// A bot, by the ticket its request was given.
    Bot(u32),
}

/// How an order went, once it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub origin: Origin,
    pub done: bool,
    pub words: String,
}

/// Another player as this bridge last drew them.
#[derive(Clone, Debug, PartialEq)]
pub struct Other {
    pub player: PlayerId,
    pub name: String,
    pub place: (i16, u8),
    pub position: [f32; 3],
    /// The way they face, in the game's angle units.
    pub yaw: i16,
}

impl Other {
    /// Where Mario is put to be beside them: a little behind them, on the floor they came
    /// over, and not on the very spot, where each would hide the other.
    fn beside(&self) -> [f32; 3] {
        let [x, y, z] = self.position;
        let facing = f32::from(self.yaw) * TAU / 65536.0;
        [
            x - BEHIND * facing.sin(),
            y + PLACE_ABOVE,
            z - BEHIND * facing.cos(),
        ]
    }
}

/// What the bridge sees of the game at one of its turns.
#[derive(Clone, Copy, Debug)]
pub struct Seen<'a> {
    pub now: Instant,
    /// Whether the game is in a level with its Mario running: not at a menu, paused or between
    /// areas.
    pub playing: bool,
    /// Whether the game is in a level at all, Mario running or not: not at the title screen
    /// or another of its menus.
    pub in_level: bool,
    pub place: (i16, u8),
    /// How many areas the game has loaded since it started, which rises when a warp lands.
    pub area_loads: u32,
    pub mario: Mario,
    pub others: &'a [Other],
}

/// What travel does to the game: the game's own state and controller, in tests a record.
pub trait Moves {
    /// # Errors
    /// Fails if the game's memory cannot be read.
    fn areas(&mut self) -> io::Result<Vec<Area>>;
    /// # Errors
    /// Fails if the game's memory cannot be written.
    fn start_level(&mut self, level: i16) -> io::Result<()>;
    /// # Errors
    /// Fails if the game's memory cannot be written.
    fn warp(&mut self, level: u8, area: u8, node: u8) -> io::Result<()>;
    /// # Errors
    /// Fails if the game's memory cannot be written.
    fn teleport(&mut self, to: [f32; 3]) -> io::Result<()>;
    /// # Errors
    /// Fails if the game's memory cannot be written.
    fn hold(&mut self, pad: HeldPad) -> io::Result<()>;
}

/// The stick that sends Mario from `from` toward `to`, with the camera at `camera_yaw`.
///
/// The game turns the stick into the way Mario is to go as `atan2s(-y, x)` plus the camera's
/// angle, in its angle units, where 0 is toward +Z and a quarter turn toward +X (`mario.c`,
/// `update_mario_joystick_inputs`). So the stick for a way is that way less the camera's angle:
/// pushed up, Mario goes away from the camera.
#[must_use]
pub fn stick_toward(from: [f32; 3], to: [f32; 3], camera_yaw: i16) -> [i8; 2] {
    let (dx, dz) = (to[0] - from[0], to[2] - from[2]);
    if dx == 0.0 && dz == 0.0 {
        return [0, 0];
    }
    let turn = dx.atan2(dz) - f32::from(camera_yaw) * TAU / 65536.0;
    [stick_value(turn.sin()), stick_value(-turn.cos())]
}

/// A stick's push, from -1 to 1 of the way to its edge, as the controller reports it.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "clamped to the stick's range first, so the value fits an i8; there is no checked \
              conversion from a float"
)]
#[must_use]
pub fn stick_value(share: f32) -> i8 {
    (share.clamp(-1.0, 1.0) * STICK_EDGE).round() as i8
}

/// How far apart two points are, seen from above.
#[must_use]
pub fn flat_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[2] - b[2])
}

/// Steering on foot: the stick toward a point, and A now and then to jump.
#[derive(Debug, Default)]
struct Steer {
    /// Where Mario was when last checked for being stuck, and when.
    checked: Option<(Instant, [f32; 3])>,
    stuck: bool,
    /// Until when A is held for a jump, and when the next may begin.
    jump_until: Option<Instant>,
    next_jump: Option<Instant>,
}

impl Steer {
    /// The controller that takes `mario` toward `to`; `eager` jumps whenever it may, as into
    /// a painting.
    fn toward(&mut self, mario: &Mario, to: [f32; 3], now: Instant, eager: bool) -> HeldPad {
        self.note_progress(mario.position, now);
        let climb = to[1] - mario.position[1] > CLIMB_HEIGHT
            && flat_distance(mario.position, to) < CLIMB_REACH;
        let may_jump = self.next_jump.is_none_or(|next| now >= next);
        if may_jump && (eager || climb || self.stuck) {
            self.jump_until = Some(now + JUMP_FOR);
            self.next_jump = Some(now + JUMP_EVERY);
            self.stuck = false;
        }
        let jumping = self.jump_until.is_some_and(|until| now < until);
        HeldPad {
            buttons: if jumping { A_BUTTON } else { 0 },
            stick: stick_toward(mario.position, to, mario.camera_yaw.unwrap_or(0)),
            frames: HOLD_FRAMES,
        }
    }

    fn note_progress(&mut self, position: [f32; 3], now: Instant) {
        match self.checked {
            Some((at, from)) if now.saturating_duration_since(at) >= STUCK_AFTER => {
                self.stuck = flat_distance(from, position) < STUCK_DISTANCE;
                self.checked = Some((now, position));
            }
            Some(_) => {}
            None => self.checked = Some((now, position)),
        }
    }

    /// Mario stands: the next walk starts its checks afresh.
    fn rest(&mut self) {
        *self = Self::default();
    }
}

/// A warp under way: to the level, started afresh, and then to the area in it.
#[derive(Debug)]
struct Warping {
    level: i16,
    area: Option<u8>,
    /// Whether the level is reached and the area is being gone to.
    in_level: bool,
    /// When the warp began, when it was last written, and the game's count of areas loaded
    /// then.
    began: Option<Instant>,
    written: Option<(Instant, u32)>,
}

/// Where a warp stands after a turn.
enum WarpStep {
    Underway,
    Arrived,
    Failed(String),
}

impl Warping {
    fn to(level: i16, area: Option<u8>, place: (i16, u8)) -> Self {
        Self {
            level,
            area,
            // Within the level loaded now, its areas can be asked about at once.
            in_level: area.is_some() && place.0 == level,
            began: None,
            written: None,
        }
    }

    fn advance(&mut self, seen: &Seen<'_>, moves: &mut impl Moves) -> io::Result<WarpStep> {
        let began = *self.began.get_or_insert(seen.now);
        if let Some((_, loads)) = self.written
            && loads != seen.area_loads
            && seen.playing
        {
            // The warp landed, here.
            let (level, area) = seen.place;
            if level != self.level {
                return Ok(WarpStep::Failed(format!(
                    "The game went to {level}-{area} and not to level {}.",
                    self.level
                )));
            }
            if self.area.is_none_or(|wanted| wanted == area) {
                return Ok(WarpStep::Arrived);
            }
            if self.in_level {
                return Ok(WarpStep::Failed(format!(
                    "The game went to {level}-{area} and not to area {}.",
                    self.area.unwrap_or(area)
                )));
            }
            // In the level, which can now be asked about its areas: on to the one wanted.
            self.in_level = true;
            self.written = None;
        }
        if seen.now.saturating_duration_since(began) >= WARP_LIMIT {
            return Ok(WarpStep::Failed("The game did not go there.".to_owned()));
        }
        let is_due = self.written.is_none_or(|(at, loads)| {
            loads == seen.area_loads && seen.now.saturating_duration_since(at) >= WARP_AGAIN_AFTER
        });
        if !is_due || !seen.playing {
            return Ok(WarpStep::Underway);
        }
        match (self.in_level, self.area) {
            (true, Some(area)) => match node_into(&moves.areas()?, self.level, area) {
                Ok(node) => moves.warp(u8::try_from(self.level).unwrap_or(0), area, node)?,
                Err(why) => return Ok(WarpStep::Failed(why)),
            },
            _ => moves.start_level(self.level)?,
        }
        self.written = Some((seen.now, seen.area_loads));
        Ok(WarpStep::Underway)
    }
}

/// The warp node of `area` to arrive by, of the areas of `level`, which is loaded: the one
/// paintings enter by, which most areas have, or else the first it has.
fn node_into(areas: &[Area], level: i16, area: u8) -> Result<u8, String> {
    let Some(found) = areas.iter().find(|found| found.index == area) else {
        let have: Vec<String> = areas.iter().map(|found| found.index.to_string()).collect();
        return Err(format!(
            "Level {level} has no area {area}; it has {}.",
            have.join(", ")
        ));
    };
    let entry = found
        .nodes
        .iter()
        .find(|node| node.id == PAINTING_ENTRY_NODE);
    entry
        .or_else(|| found.nodes.first())
        .map(|node| node.id)
        .ok_or_else(|| format!("Area {area} of level {level} has no way in to warp to."))
}

/// The player named `name` of those in a level: the one whose name it is, whatever the case,
/// or the only one whose name begins with it.
fn find<'a>(others: &'a [Other], name: &str) -> Result<&'a Other, String> {
    let wanted = name.trim().to_ascii_lowercase();
    if let Some(exact) = others
        .iter()
        .find(|other| other.name.to_ascii_lowercase() == wanted)
    {
        return Ok(exact);
    }
    let begun: Vec<&Other> = others
        .iter()
        .filter(|other| other.name.to_ascii_lowercase().starts_with(&wanted))
        .collect();
    match begun.as_slice() {
        [only] => Ok(only),
        [] => Err(format!("Nobody in a level is called {name}.")),
        several => {
            let names: Vec<&str> = several.iter().map(|other| other.name.as_str()).collect();
            Err(format!("{name} could be {}.", names.join(" or ")))
        }
    }
}

/// A walk that is not getting nearer, by its best distance so far and when that was.
#[derive(Debug, Default)]
struct Progress {
    best: Option<(f32, Instant)>,
}

impl Progress {
    /// Notes the distance left; whether it has come no nearer by `PROGRESS` for too long.
    fn is_stalled(&mut self, distance: f32, now: Instant) -> bool {
        match self.best {
            Some((best, _)) if distance < best - PROGRESS => self.best = Some((distance, now)),
            Some((_, since)) => return now.saturating_duration_since(since) >= GIVE_UP_AFTER,
            None => self.best = Some((distance, now)),
        }
        false
    }
}

/// Following a player.
#[derive(Debug)]
struct Following {
    player: PlayerId,
    name: String,
    /// Where the player was last seen, and when.
    last: Option<((i16, u8), [f32; 3], Instant)>,
    /// Where they left this area, and since when Mario has tried to leave by it.
    exit: Option<([f32; 3], Instant)>,
    warping: Option<Warping>,
    progress: Progress,
}

/// What Mario was sent to do.
#[derive(Debug)]
enum Kind {
    Warp(Warping),
    Goto {
        player: PlayerId,
        name: String,
        warping: Option<Warping>,
        /// Until when Mario is left to come out of a warp before he is put beside them.
        settle_until: Option<Instant>,
    },
    Follow(Following),
    Go {
        to: [f32; 3],
        progress: Progress,
    },
    /// Into a level from the game's menus first, for an order given there.
    Enter(Entering),
}

/// Pressing through the game's menus into a level, to carry out an order then.
#[derive(Debug)]
struct Entering {
    then: Order,
    /// Whether Start is pressed next at a menu, where Start and A are pressed in turn.
    start_next: bool,
    next_press: Instant,
    until: Instant,
}

impl Entering {
    fn new(then: Order, now: Instant) -> Self {
        Self {
            then,
            start_next: true,
            next_press: now,
            until: now + ENTER_LIMIT,
        }
    }

    /// The button to press now, if one is due: in a level, as at an act's choice or in a text
    /// box, A; at a menu, Start and A in turn, which take the title screen and the file select.
    fn press(&mut self, seen: &Seen<'_>) -> Option<u16> {
        if seen.now < self.next_press {
            return None;
        }
        self.next_press = seen.now + ENTER_PRESS_EVERY;
        let start = !seen.in_level && self.start_next;
        self.start_next = !start;
        Some(if start { START_BUTTON } else { A_BUTTON })
    }
}

impl Kind {
    /// What it is, in a few words.
    fn describe(&self) -> String {
        match self {
            Self::Enter(entering) => match &entering.then {
                Order::Warp {
                    level,
                    area: Some(area),
                } => format!("getting into the game to warp to {level}-{area}"),
                Order::Warp { level, area: None } => {
                    format!("getting into the game to warp to level {level}")
                }
                Order::Goto(name) => format!("getting into the game to go to {name}"),
                Order::Follow(name) => format!("getting into the game to follow {name}"),
                Order::Go(_) | Order::Stop => "getting into the game".to_owned(),
            },
            Self::Warp(warping) => match warping.area {
                Some(area) => format!("warping to {}-{area}", warping.level),
                None => format!("warping to level {}", warping.level),
            },
            Self::Goto { name, .. } => format!("going to {name}"),
            Self::Follow(following) => format!("following {}", following.name),
            Self::Go { to, .. } => format!("going to {:.0} {:.0} {:.0}", to[0], to[1], to[2]),
        }
    }
}

#[derive(Debug)]
struct Task {
    kind: Kind,
    origin: Origin,
}

/// What a turn of a task came to.
enum Turn {
    Going,
    Done(String),
    Failed(String),
}

/// `words` with its first letter a capital.
fn sentence(words: &str) -> String {
    let mut characters = words.chars();
    characters
        .next()
        .map(|first| first.to_uppercase().chain(characters).collect())
        .unwrap_or_default()
}

/// Mario's travels: the order he is carrying out, if any.
#[derive(Debug, Default)]
pub struct Travel {
    task: Option<Task>,
    steer: Steer,
    /// Whether the controller is held now, and must be let go when nothing needs it.
    holding: bool,
}

impl Travel {
    /// What Mario is doing, in a few words, if anything.
    #[must_use]
    pub fn doing(&self) -> Option<String> {
        Some(self.task.as_ref()?.kind.describe())
    }

    /// Takes an order: what Mario starts doing, in words, or why he cannot. Whatever he was
    /// doing ends, and its sender is told so in the reports returned.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be written.
    pub fn order(
        &mut self,
        order: Order,
        origin: Origin,
        seen: &Seen<'_>,
        moves: &mut impl Moves,
    ) -> io::Result<(Result<String, String>, Vec<Report>)> {
        let kind = match Self::kind_for(order, seen) {
            Ok(Some(kind)) => kind,
            Ok(None) => {
                let reports = self.end("Stopped.");
                self.let_go(moves)?;
                return Ok((Ok("Stopped.".to_owned()), reports));
            }
            Err(why) => return Ok((Err(why), Vec::new())),
        };
        let reports = self.end("Sent elsewhere.");
        self.steer.rest();
        let doing = sentence(&kind.describe());
        self.task = Some(Task { kind, origin });
        Ok((Ok(format!("{doing}.")), reports))
    }

    /// What an order sends Mario to do; none to stop.
    fn kind_for(order: Order, seen: &Seen<'_>) -> Result<Option<Kind>, String> {
        if order == Order::Stop {
            return Ok(None);
        }
        if !seen.playing {
            // From a menu, or a text box, into a level first, where the order is carried out;
            // a point to walk to means nothing until Mario is in an area.
            return match order {
                Order::Warp { level, area } => {
                    check_place(level, area)?;
                    Ok(Some(Kind::Enter(Entering::new(order, seen.now))))
                }
                Order::Goto(_) | Order::Follow(_) => {
                    Ok(Some(Kind::Enter(Entering::new(order, seen.now))))
                }
                Order::Go(_) => Err("Mario is not in a level now.".to_owned()),
                Order::Stop => Ok(None),
            };
        }
        Ok(Some(match order {
            Order::Stop => return Ok(None),
            Order::Warp { level, area } => {
                check_place(level, area)?;
                Kind::Warp(Warping::to(level, area, seen.place))
            }
            Order::Goto(name) => {
                let other = find(seen.others, &name)?;
                Kind::Goto {
                    player: other.player,
                    name: other.name.clone(),
                    warping: None,
                    settle_until: None,
                }
            }
            Order::Follow(name) => {
                let other = find(seen.others, &name)?;
                Kind::Follow(Following {
                    player: other.player,
                    name: other.name.clone(),
                    last: None,
                    exit: None,
                    warping: None,
                    progress: Progress::default(),
                })
            }
            Order::Go(to) if to.iter().all(|value| value.is_finite()) => Kind::Go {
                to,
                progress: Progress::default(),
            },
            Order::Go(_) => return Err("That is no point.".to_owned()),
        }))
    }

    /// Ends what Mario was doing, if anything, with the report for its sender.
    fn end(&mut self, words: &str) -> Vec<Report> {
        self.task
            .take()
            .map(|old| Report {
                origin: old.origin,
                done: false,
                words: words.to_owned(),
            })
            .into_iter()
            .collect()
    }

    /// One turn: Mario goes on with what he is doing, and the reports say what ended.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read or written.
    pub fn step(&mut self, seen: &Seen<'_>, moves: &mut impl Moves) -> io::Result<Vec<Report>> {
        let Some(mut task) = self.task.take() else {
            return Ok(Vec::new());
        };
        let (done, words) = match self.turn(&mut task.kind, seen, moves)? {
            Turn::Going => {
                self.task = Some(task);
                return Ok(Vec::new());
            }
            Turn::Done(words) => (true, words),
            Turn::Failed(words) => (false, words),
        };
        self.let_go(moves)?;
        Ok(vec![Report {
            origin: task.origin,
            done,
            words,
        }])
    }

    fn turn(
        &mut self,
        kind: &mut Kind,
        seen: &Seen<'_>,
        moves: &mut impl Moves,
    ) -> io::Result<Turn> {
        if let Kind::Enter(entering) = kind
            && seen.playing
        {
            // In a level now: the order it was given for begins.
            let order = entering.then.clone();
            self.let_go(moves)?;
            return Ok(match Self::kind_for(order, seen) {
                Ok(Some(next)) => {
                    *kind = next;
                    Turn::Going
                }
                Ok(None) => Turn::Done("In a level.".to_owned()),
                Err(why) => Turn::Failed(why),
            });
        }
        match kind {
            Kind::Enter(entering) => {
                if seen.now >= entering.until {
                    return Ok(Turn::Failed(
                        "Could not get past the game's menus into a level.".to_owned(),
                    ));
                }
                if let Some(button) = entering.press(seen) {
                    let pad = HeldPad {
                        buttons: button,
                        stick: [0, 0],
                        frames: HOLD_FRAMES,
                    };
                    self.hold(moves, pad)?;
                }
                Ok(Turn::Going)
            }
            Kind::Warp(warping) => Ok(match warping.advance(seen, moves)? {
                WarpStep::Underway => Turn::Going,
                WarpStep::Arrived => Turn::Done(format!("At {}-{}.", seen.place.0, seen.place.1)),
                WarpStep::Failed(why) => Turn::Failed(why),
            }),
            Kind::Go { to, progress } => {
                let distance = flat_distance(seen.mario.position, *to);
                if distance <= ARRIVED_WITHIN {
                    return Ok(Turn::Done("Arrived.".to_owned()));
                }
                if progress.is_stalled(distance, seen.now) {
                    return Ok(Turn::Failed("Could not get there on foot.".to_owned()));
                }
                if seen.playing {
                    let pad = self.steer.toward(&seen.mario, *to, seen.now, false);
                    self.hold(moves, pad)?;
                } else {
                    self.let_go(moves)?;
                }
                Ok(Turn::Going)
            }
            Kind::Goto {
                player,
                name,
                warping,
                settle_until,
            } => {
                if let Some(going) = warping {
                    match going.advance(seen, moves)? {
                        WarpStep::Underway => return Ok(Turn::Going),
                        WarpStep::Failed(why) => return Ok(Turn::Failed(why)),
                        WarpStep::Arrived => {
                            *warping = None;
                            *settle_until = Some(seen.now + SETTLE_FOR);
                        }
                    }
                }
                let Some(other) = seen.others.iter().find(|other| other.player == *player) else {
                    return Ok(Turn::Failed(format!("{name} is not in a level now.")));
                };
                if other.place != seen.place {
                    // Elsewhere, or gone elsewhere since: after them.
                    *warping = Some(Warping::to(other.place.0, Some(other.place.1), seen.place));
                    return Ok(Turn::Going);
                }
                if !seen.playing || settle_until.is_some_and(|until| seen.now < until) {
                    return Ok(Turn::Going);
                }
                moves.teleport(other.beside())?;
                Ok(Turn::Done(format!("Beside {name}.")))
            }
            Kind::Follow(following) => self.follow(following, seen, moves),
        }
    }

    fn follow(
        &mut self,
        following: &mut Following,
        seen: &Seen<'_>,
        moves: &mut impl Moves,
    ) -> io::Result<Turn> {
        if let Some(going) = &mut following.warping {
            match going.advance(seen, moves)? {
                WarpStep::Underway => return Ok(Turn::Going),
                WarpStep::Failed(why) => return Ok(Turn::Failed(why)),
                WarpStep::Arrived => {
                    following.warping = None;
                    following.exit = None;
                    following.progress = Progress::default();
                }
            }
        }
        let player = following.player;
        let Some(other) = seen.others.iter().find(|other| other.player == player) else {
            let lost_since = following.last.map_or(seen.now, |(_, _, at)| at);
            if seen.now.saturating_duration_since(lost_since) >= LOST_AFTER {
                let name = &following.name;
                return Ok(Turn::Failed(format!(
                    "Lost {name}: not in a level any more."
                )));
            }
            self.let_go(moves)?;
            return Ok(Turn::Going);
        };
        let before = following
            .last
            .replace((other.place, other.position, seen.now));
        if !seen.playing {
            self.let_go(moves)?;
            return Ok(Turn::Going);
        }
        if other.place == seen.place {
            following.exit = None;
            let distance = flat_distance(seen.mario.position, other.position);
            if distance <= FOLLOW_NEAR {
                following.progress = Progress::default();
                self.steer.rest();
                self.let_go(moves)?;
            } else if following.progress.is_stalled(distance, seen.now) {
                // Walls between, or a gap: beside them, and on from there.
                moves.teleport(other.beside())?;
                following.progress = Progress::default();
            } else {
                let pad = self
                    .steer
                    .toward(&seen.mario, other.position, seen.now, false);
                self.hold(moves, pad)?;
            }
            return Ok(Turn::Going);
        }
        // They are elsewhere: by the way they left, if it was from here, or else by a warp.
        if following.exit.is_none()
            && let Some((place, position, _)) = before
            && place == seen.place
        {
            following.exit = Some((position, seen.now));
        }
        match following.exit {
            Some((at, since)) if seen.now.saturating_duration_since(since) < EXIT_PATIENCE => {
                let near = flat_distance(seen.mario.position, at) <= EXIT_REACH;
                let pad = self.steer.toward(&seen.mario, at, seen.now, near);
                self.hold(moves, pad)?;
            }
            _ => {
                self.let_go(moves)?;
                following.exit = None;
                following.warping =
                    Some(Warping::to(other.place.0, Some(other.place.1), seen.place));
            }
        }
        Ok(Turn::Going)
    }

    fn hold(&mut self, moves: &mut impl Moves, pad: HeldPad) -> io::Result<()> {
        self.holding = true;
        moves.hold(pad)
    }

    /// Gives the controller back, if travel was holding it.
    fn let_go(&mut self, moves: &mut impl Moves) -> io::Result<()> {
        if std::mem::take(&mut self.holding) {
            moves.hold(HeldPad::default())?;
        }
        Ok(())
    }
}

/// `Moves` on the game itself, through its RAM.
#[derive(Debug)]
pub struct GameMoves<'r, 'a, M> {
    pub mailbox: &'r Mailbox<'a, M>,
}

impl<M: ProcessMemory> Moves for GameMoves<'_, '_, M> {
    fn areas(&mut self) -> io::Result<Vec<Area>> {
        game::areas(self.mailbox.ram())
    }

    fn start_level(&mut self, level: i16) -> io::Result<()> {
        game::start_level(self.mailbox.ram(), level)
    }

    fn warp(&mut self, level: u8, area: u8, node: u8) -> io::Result<()> {
        game::warp(self.mailbox.ram(), level, area, node)
    }

    fn teleport(&mut self, to: [f32; 3]) -> io::Result<()> {
        game::teleport(self.mailbox.ram(), to)
    }

    fn hold(&mut self, pad: HeldPad) -> io::Result<()> {
        self.mailbox.hold_pad(&pad)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        A_BUTTON, ENTER_LIMIT, ENTER_PRESS_EVERY, EXIT_PATIENCE, GIVE_UP_AFTER, Moves, Order,
        Origin, Other, Report, START_BUTTON, Seen, Travel, WARP_AGAIN_AFTER, command, stick_toward,
    };
    use crate::game::{Area, Mario, WarpNode};
    use crate::mailbox::HeldPad;
    use b3313_protocol::PlayerId;
    use std::io;
    use std::time::{Duration, Instant};

    /// What travel did to a game, and the areas that game's level has.
    #[derive(Default)]
    struct Record {
        areas: Vec<Area>,
        done: Vec<String>,
        held: Vec<HeldPad>,
    }

    impl Moves for Record {
        fn areas(&mut self) -> io::Result<Vec<Area>> {
            Ok(self.areas.clone())
        }

        fn start_level(&mut self, level: i16) -> io::Result<()> {
            self.done.push(format!("start {level}"));
            Ok(())
        }

        fn warp(&mut self, level: u8, area: u8, node: u8) -> io::Result<()> {
            self.done
                .push(format!("warp {level}-{area} node {node:#x}"));
            Ok(())
        }

        fn teleport(&mut self, to: [f32; 3]) -> io::Result<()> {
            self.done
                .push(format!("teleport {:.0} {:.0} {:.0}", to[0], to[1], to[2]));
            Ok(())
        }

        fn hold(&mut self, pad: HeldPad) -> io::Result<()> {
            self.held.push(pad);
            Ok(())
        }
    }

    /// Gives `travel` an order it must take.
    fn send(
        travel: &mut Travel,
        order: Order,
        origin: Origin,
        seen: &Seen<'_>,
        record: &mut Record,
    ) -> io::Result<()> {
        let (started, _) = travel.order(order, origin, seen, record)?;
        assert!(started.is_ok(), "{started:?}");
        Ok(())
    }

    fn mario_at(position: [f32; 3]) -> Mario {
        Mario {
            position,
            velocity: [0.0; 3],
            forward_speed: 0.0,
            yaw: 0,
            action: 0,
            health: 0x880,
            coins: 0,
            stars: 0,
            lives: 4,
            camera_yaw: Some(0),
        }
    }

    fn seen(
        now: Instant,
        place: (i16, u8),
        loads: u32,
        at: [f32; 3],
        others: &[Other],
    ) -> Seen<'_> {
        Seen {
            now,
            playing: true,
            in_level: true,
            place,
            area_loads: loads,
            mario: mario_at(at),
            others,
        }
    }

    /// Another player, facing +Z, so that behind them is toward -Z.
    fn other(name: &str, place: (i16, u8), position: [f32; 3]) -> Other {
        Other {
            player: PlayerId::new(2).unwrap_or_else(|| unreachable!()),
            name: name.to_owned(),
            place,
            position,
            yaw: 0,
        }
    }

    fn area(index: u8, nodes: &[u8]) -> Area {
        Area {
            index,
            nodes: nodes
                .iter()
                .map(|&id| WarpNode {
                    id,
                    to_level: 6,
                    to_area: 1,
                    to_node: 0x0A,
                    position: None,
                })
                .collect(),
        }
    }

    #[test]
    fn the_chat_commands_are_taken_and_others_left_for_the_server() {
        assert_eq!(
            command("/warp 9"),
            Some(Ok(Order::Warp {
                level: 9,
                area: None
            }))
        );
        assert_eq!(
            command("/WARP 9 2"),
            Some(Ok(Order::Warp {
                level: 9,
                area: Some(2)
            }))
        );
        assert_eq!(
            command("/goto Big Luigi"),
            Some(Ok(Order::Goto("Big Luigi".into())))
        );
        assert_eq!(command("/follow lu"), Some(Ok(Order::Follow("lu".into()))));
        assert_eq!(command("/stop"), Some(Ok(Order::Stop)));
        for wrong in [
            "/warp",
            "/warp nine",
            "/warp 9 2 3",
            "/warp 40",
            "/warp 9 16",
            "/goto",
        ] {
            assert!(matches!(command(wrong), Some(Err(_))), "{wrong}");
        }
        // The title screen is a level by its number, and no level to play in.
        assert_eq!(
            command("/warp 1"),
            Some(Err(
                "Level 1 is the title screen, not a level to play in.".into()
            ))
        );
        assert!(!super::warp_levels().contains(&1));
        assert!(super::warp_levels().contains(&9));
        assert_eq!(command("/kick Wario"), None);
        assert_eq!(command("hello /warp"), None);
        assert_eq!(command(""), None);
    }

    #[test]
    fn the_stick_turns_with_the_camera() {
        let here = [0.0; 3];
        // The camera south of Mario looks north: away from it is up, and to its right is +X.
        assert_eq!(stick_toward(here, [0.0, 0.0, -500.0], 0), [0, 80]);
        assert_eq!(stick_toward(here, [500.0, 0.0, 0.0], 0), [80, 0]);
        assert_eq!(stick_toward(here, [0.0, 0.0, 500.0], 0), [0, -80]);
        // Turned a quarter, the camera east of Mario looks west: west is up.
        assert_eq!(stick_toward(here, [-500.0, 0.0, 0.0], 0x4000), [0, 80]);
        assert_eq!(stick_toward(here, here, 0x1234), [0, 0]);
    }

    #[test]
    fn a_warp_to_another_level_starts_it_and_then_takes_the_area_by_a_node() -> io::Result<()> {
        let start = Instant::now();
        let mut record = Record {
            areas: vec![area(1, &[0x0A]), area(2, &[0xF0, 0x0B])],
            ..Record::default()
        };
        let mut travel = Travel::default();
        let order = Order::Warp {
            level: 9,
            area: Some(2),
        };
        let here = [0.0; 3];
        let (started, _) = travel.order(
            order,
            Origin::Chat,
            &seen(start, (6, 1), 3, here, &[]),
            &mut record,
        )?;
        assert_eq!(started.as_deref(), Ok("Warping to 9-2."));
        travel.step(&seen(start, (6, 1), 3, here, &[]), &mut record)?;
        assert_eq!(record.done, ["start 9"]);
        // The game is in level 9, area 1, where it starts: area 2 has no painting's way in, so
        // its first node.
        let then = start + Duration::from_secs(2);
        assert_eq!(
            travel.step(&seen(then, (9, 1), 4, here, &[]), &mut record)?,
            []
        );
        assert_eq!(record.done, ["start 9", "warp 9-2 node 0xf0"]);
        let reports = travel.step(&seen(then, (9, 2), 5, here, &[]), &mut record)?;
        assert_eq!(
            reports,
            [Report {
                origin: Origin::Chat,
                done: true,
                words: "At 9-2.".into()
            }]
        );
        assert!(travel.doing().is_none());
        Ok(())
    }

    #[test]
    fn a_warp_the_game_did_not_take_is_written_again_and_an_area_it_lacks_is_refused()
    -> io::Result<()> {
        let start = Instant::now();
        let mut record = Record {
            areas: vec![area(1, &[0x0A]), area(3, &[0x0A])],
            ..Record::default()
        };
        let mut travel = Travel::default();
        let here = [0.0; 3];
        let order = Order::Warp {
            level: 9,
            area: None,
        };
        send(
            &mut travel,
            order,
            Origin::Bot(7),
            &seen(start, (9, 1), 3, here, &[]),
            &mut record,
        )?;
        travel.step(&seen(start, (9, 1), 3, here, &[]), &mut record)?;
        let later = start + WARP_AGAIN_AFTER;
        travel.step(&seen(later, (9, 1), 3, here, &[]), &mut record)?;
        assert_eq!(record.done, ["start 9", "start 9"]);

        // Within the level loaded, an area it lacks is said, with those it has.
        let order = Order::Warp {
            level: 9,
            area: Some(2),
        };
        let (started, ended) = travel.order(
            order,
            Origin::Chat,
            &seen(later, (9, 1), 3, here, &[]),
            &mut record,
        )?;
        assert!(started.is_ok());
        assert_eq!(
            ended.first().map(|report| report.origin),
            Some(Origin::Bot(7))
        );
        let reports = travel.step(&seen(later, (9, 1), 3, here, &[]), &mut record)?;
        assert_eq!(
            reports.first().map(|report| report.words.as_str()),
            Some("Level 9 has no area 2; it has 1, 3.")
        );
        Ok(())
    }

    #[test]
    fn a_player_in_the_same_area_is_gone_to_at_once_and_one_elsewhere_by_a_warp() -> io::Result<()>
    {
        let start = Instant::now();
        let mut record = Record {
            areas: vec![area(1, &[0x0A]), area(4, &[0x0A])],
            ..Record::default()
        };
        let here = [0.0; 3];
        let mut travel = Travel::default();
        let luigi = [other("Luigi", (9, 1), [500.0, 20.0, 0.0])];
        send(
            &mut travel,
            Order::Goto("lu".into()),
            Origin::Chat,
            &seen(start, (9, 1), 3, here, &luigi),
            &mut record,
        )?;
        let reports = travel.step(&seen(start, (9, 1), 3, here, &luigi), &mut record)?;
        assert_eq!(record.done, ["teleport 500 70 -100"]);
        assert_eq!(
            reports.first().map(|r| r.words.as_str()),
            Some("Beside Luigi.")
        );

        record.done.clear();
        let luigi = [other("Luigi", (9, 4), [-300.0, 0.0, 10.0])];
        send(
            &mut travel,
            Order::Goto("Luigi".into()),
            Origin::Chat,
            &seen(start, (9, 1), 3, here, &luigi),
            &mut record,
        )?;
        travel.step(&seen(start, (9, 1), 3, here, &luigi), &mut record)?;
        travel.step(&seen(start, (9, 1), 3, here, &luigi), &mut record)?;
        assert_eq!(record.done, ["warp 9-4 node 0xa"]);
        // Arrived, Mario is left to come out of the warp before he is put beside Luigi.
        let landed = start + Duration::from_secs(1);
        assert_eq!(
            travel.step(&seen(landed, (9, 4), 4, here, &luigi), &mut record)?,
            []
        );
        let settled = landed + Duration::from_secs(2);
        travel.step(&seen(settled, (9, 4), 4, here, &luigi), &mut record)?;
        assert_eq!(
            record.done.last().map(String::as_str),
            Some("teleport -300 50 -90")
        );
        Ok(())
    }

    #[test]
    fn a_follower_walks_to_the_player_and_leaves_by_the_way_they_left() -> io::Result<()> {
        let start = Instant::now();
        let mut record = Record {
            areas: vec![area(1, &[0x0A]), area(2, &[0x0A])],
            ..Record::default()
        };
        let mut travel = Travel::default();
        let far = [other("Luigi", (9, 1), [0.0, 0.0, -2000.0])];
        let here = [0.0; 3];
        send(
            &mut travel,
            Order::Follow("Luigi".into()),
            Origin::Chat,
            &seen(start, (9, 1), 3, here, &far),
            &mut record,
        )?;
        travel.step(&seen(start, (9, 1), 3, here, &far), &mut record)?;
        // North, away from the camera: up.
        assert_eq!(record.held.last().map(|pad| pad.stick), Some([0, 80]));

        // Near: let go.
        let near = [other("Luigi", (9, 1), [0.0, 0.0, -200.0])];
        travel.step(&seen(start, (9, 1), 3, here, &near), &mut record)?;
        assert_eq!(record.held.last(), Some(&HeldPad::default()));

        // Luigi goes through a painting to the east: Mario goes there, jumping once he is near.
        let east = [other("Luigi", (9, 1), [1000.0, 0.0, 0.0])];
        travel.step(&seen(start, (9, 1), 3, here, &east), &mut record)?;
        let gone = [other("Luigi", (9, 2), [0.0, 0.0, 0.0])];
        travel.step(&seen(start, (9, 1), 3, here, &gone), &mut record)?;
        assert_eq!(record.held.last().map(|pad| pad.stick), Some([80, 0]));
        let at_painting = [950.0, 0.0, 0.0];
        let soon = start + Duration::from_secs(1);
        travel.step(&seen(soon, (9, 1), 3, at_painting, &gone), &mut record)?;
        assert_eq!(record.held.last().map(|pad| pad.buttons), Some(A_BUTTON));
        assert_eq!(record.done, Vec::<String>::new());

        // That does not take him there in time: he warps after Luigi.
        let late = start + EXIT_PATIENCE + Duration::from_secs(1);
        travel.step(&seen(late, (9, 1), 3, at_painting, &gone), &mut record)?;
        travel.step(&seen(late, (9, 1), 3, at_painting, &gone), &mut record)?;
        assert_eq!(record.done, ["warp 9-2 node 0xa"]);
        assert_eq!(travel.doing().as_deref(), Some("following Luigi"));
        Ok(())
    }

    #[test]
    fn going_to_a_point_ends_there_or_gives_up_when_it_gets_no_nearer() -> io::Result<()> {
        let start = Instant::now();
        let mut record = Record::default();
        let mut travel = Travel::default();
        let here = [0.0; 3];
        let to = [0.0, 0.0, -1000.0];
        send(
            &mut travel,
            Order::Go(to),
            Origin::Bot(1),
            &seen(start, (9, 1), 3, here, &[]),
            &mut record,
        )?;
        travel.step(&seen(start, (9, 1), 3, here, &[]), &mut record)?;
        let reports = travel.step(
            &seen(start, (9, 1), 3, [0.0, 0.0, -950.0], &[]),
            &mut record,
        )?;
        assert_eq!(
            reports.first().map(|r| (r.done, r.origin)),
            Some((true, Origin::Bot(1)))
        );
        assert_eq!(record.held.last(), Some(&HeldPad::default()));

        send(
            &mut travel,
            Order::Go(to),
            Origin::Bot(2),
            &seen(start, (9, 1), 3, here, &[]),
            &mut record,
        )?;
        travel.step(&seen(start, (9, 1), 3, here, &[]), &mut record)?;
        // Stuck against a wall a second: he jumps.
        let second = start + Duration::from_secs(1);
        travel.step(&seen(second, (9, 1), 3, here, &[]), &mut record)?;
        assert_eq!(record.held.last().map(|pad| pad.buttons), Some(A_BUTTON));
        let stuck = start + GIVE_UP_AFTER;
        let reports = travel.step(&seen(stuck, (9, 1), 3, here, &[]), &mut record)?;
        assert_eq!(reports.first().map(|r| r.done), Some(false));
        Ok(())
    }

    #[test]
    fn stop_ends_what_mario_was_doing_and_a_walk_needs_a_level() -> io::Result<()> {
        let start = Instant::now();
        let mut record = Record::default();
        let mut travel = Travel::default();
        let here = [0.0; 3];
        send(
            &mut travel,
            Order::Go([0.0, 0.0, 900.0]),
            Origin::Bot(3),
            &seen(start, (9, 1), 3, here, &[]),
            &mut record,
        )?;
        travel.step(&seen(start, (9, 1), 3, here, &[]), &mut record)?;
        let (stopped, ended) = travel.order(
            Order::Stop,
            Origin::Chat,
            &seen(start, (9, 1), 3, here, &[]),
            &mut record,
        )?;
        assert_eq!(stopped.as_deref(), Ok("Stopped."));
        assert_eq!(
            ended.first().map(|report| report.origin),
            Some(Origin::Bot(3))
        );
        assert_eq!(record.held.last(), Some(&HeldPad::default()));

        let mut menu = seen(start, (9, 1), 3, here, &[]);
        menu.playing = false;
        let (refused, _) = travel.order(
            Order::Go([0.0, 0.0, 900.0]),
            Origin::Chat,
            &menu,
            &mut record,
        )?;
        assert!(refused.is_err());
        let (refused, _) = travel.order(
            Order::Goto("Nobody".into()),
            Origin::Chat,
            &seen(start, (9, 1), 3, here, &[]),
            &mut record,
        )?;
        assert_eq!(
            refused,
            Err("Nobody in a level is called Nobody.".to_owned())
        );
        Ok(())
    }

    #[test]
    fn an_order_at_the_title_screen_presses_into_a_level_and_is_then_carried_out() -> io::Result<()>
    {
        let start = Instant::now();
        let mut record = Record::default();
        let mut travel = Travel::default();
        let here = [0.0; 3];
        let at_menu = |now: Instant| {
            let mut menu = seen(now, (1, 0), 0, here, &[]);
            menu.playing = false;
            menu.in_level = false;
            menu
        };
        let order = Order::Warp {
            level: 9,
            area: None,
        };
        let (taken, _) = travel.order(order, Origin::Bot(4), &at_menu(start), &mut record)?;
        assert_eq!(
            taken.as_deref(),
            Ok("Getting into the game to warp to level 9.")
        );

        // At a menu, Start and A in turn, a press a second.
        let pressed = |record: &Record| record.held.last().map(|pad| pad.buttons);
        travel.step(&at_menu(start), &mut record)?;
        assert_eq!(pressed(&record), Some(START_BUTTON));
        let held_before = record.held.len();
        travel.step(&at_menu(start + Duration::from_millis(500)), &mut record)?;
        assert_eq!(record.held.len(), held_before);
        travel.step(&at_menu(start + ENTER_PRESS_EVERY), &mut record)?;
        assert_eq!(pressed(&record), Some(A_BUTTON));
        // In a level but not running, as at an act's choice: A alone.
        let mut choosing = at_menu(start + ENTER_PRESS_EVERY * 2);
        choosing.in_level = true;
        travel.step(&choosing, &mut record)?;
        travel.step(
            &{
                let mut again = choosing;
                again.now += ENTER_PRESS_EVERY;
                again
            },
            &mut record,
        )?;
        assert_eq!(pressed(&record), Some(A_BUTTON));

        // Running in a level: the controller is let go and the warp begins.
        let later = start + ENTER_PRESS_EVERY * 5;
        let reports = travel.step(&seen(later, (6, 1), 1, here, &[]), &mut record)?;
        assert_eq!(reports.len(), 0);
        assert_eq!(record.held.last(), Some(&HeldPad::default()));
        assert_eq!(travel.doing().as_deref(), Some("warping to level 9"));

        // One that never gets past the menus is given up.
        let mut stuck = Travel::default();
        let follow = Order::Follow("Luigi".into());
        let (_, _) = stuck.order(follow, Origin::Chat, &at_menu(start), &mut record)?;
        let reports = stuck.step(&at_menu(start + ENTER_LIMIT), &mut record)?;
        assert_eq!(reports.first().map(|report| report.done), Some(false));
        Ok(())
    }
}
