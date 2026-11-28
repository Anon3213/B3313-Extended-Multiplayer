//! What a bridge does for its player beyond the session itself: the commands they send for
//! their own game (`/warp`, `/goto`, `/follow`, `/stop`; `travel`), and a bot that plays the
//! game through the bot API (`bots`). The session's loop gives it a turn each time round
//! (`Assist::turn`).

use crate::bots::{self, Arrivals, Asked, Bots, Request};
use crate::client::Connection;
use crate::game;
use crate::interpolation::Interpolator;
use crate::mailbox::{HeldPad, Mailbox};
use crate::people::People;
use crate::ram::ProcessMemory;
use crate::travel::{GameMoves, Order, Origin, Other, Report, Seen, Travel};
use b3313_protocol::{Message, PlayerId, PlayerState, Whereabouts};
use std::io;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

/// How often the game is looked at for a bot that asks nothing, and Mario is sent nowhere: for
/// the news of who moved where, and the state it watches.
const WATCH_EVERY: Duration = Duration::from_millis(100);

/// What a session reports of its bot API, for the player to know.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BotNotice {
    /// Bots may connect here.
    Listening(SocketAddr),
    /// The port asked for could not be opened, for this reason: no bot can connect.
    Unavailable(String),
    /// A bot said hello.
    Connected,
    /// The bot connected went.
    Left,
}

impl BotNotice {
    /// The notice in a sentence.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Listening(address) => format!("A bot can connect to this game at {address}."),
            Self::Unavailable(why) => format!("No bot can connect to this game: {why}."),
            Self::Connected => "A bot is playing this game.".to_owned(),
            Self::Left => "The bot playing this game left.".to_owned(),
        }
    }
}

/// The player's travels and their bot, from one turn of the session to the next.
#[derive(Debug, Default)]
pub(crate) struct Assist {
    travel: Travel,
    bots: Option<Bots>,
    arrivals: Arrivals,
    /// Whether the bot holds the controller, which must be let go when it leaves.
    bot_holds: bool,
    /// When the game was last looked at for a bot that only watches.
    looked: Option<Instant>,
}

impl Assist {
    /// Opens the bot API on `port`, if one is given, and tells `notices` where bots connect,
    /// or why none can.
    pub(crate) fn new(port: Option<u16>, notices: &mut impl FnMut(BotNotice)) -> Self {
        let Some(port) = port else {
            return Self::default();
        };
        let (bots, notice) = match Bots::open(port).and_then(|bots| Ok((bots.address()?, bots))) {
            Ok((address, bots)) => (Some(bots), BotNotice::Listening(address)),
            Err(error) => (
                None,
                BotNotice::Unavailable(format!("port {port} could not be opened ({error})")),
            ),
        };
        notices(notice);
        Self {
            bots,
            ..Self::default()
        }
    }

    /// One turn: carries out what the player and the bot asked, moves Mario on with what he
    /// was sent to do, and tells the bot what happened. `session` holds the other players as
    /// the session draws them, and whether the game is in a level with its Mario. With nothing
    /// asked and nothing under way, it reads nothing of the game.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read or written.
    pub(crate) fn turn<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        connection: &mut Connection,
        people: &mut People,
        (drawn, playing): (&Interpolator, bool),
        notices: &mut impl FnMut(BotNotice),
    ) -> io::Result<()> {
        let now = Instant::now();
        let orders = people.take_orders();
        let heard = people.take_heard();
        let (asked, bot_left) = match &mut self.bots {
            Some(bots) => bots.poll(now),
            None => (Vec::new(), false),
        };
        let watching = self.bots.as_ref().is_some_and(Bots::has_bot);
        let asked_nothing = orders.is_empty() && asked.is_empty() && !bot_left;
        let is_idle = asked_nothing && self.travel.doing().is_none();
        // A bot that only watches is told of moves soon enough at a tenth of a second.
        let is_look_due = watching
            && self
                .looked
                .is_none_or(|looked| now.saturating_duration_since(looked) >= WATCH_EVERY);
        if is_idle && !is_look_due || !mailbox.is_ready()? {
            return Ok(());
        }
        self.looked = Some(now);
        let me = connection.player();
        let others = named(people, me, &drawn.sample(now));
        // The place and position the game published for its Mario, which it does only while
        // he runs, all of a piece. The level and area it keeps change one after the other
        // while an area loads: Mario is taken to be playing only when the two agree, so that
        // nothing is done, and nobody said to have moved, in a place half loaded.
        let mut mario = game::mario(mailbox.ram())?;
        let current = game::place(mailbox.ram())?;
        let published = mailbox
            .read_local_player()?
            .map(|state| Whereabouts::of(&state));
        let place = match published {
            Some(whereabouts) => {
                mario.position = whereabouts.position;
                (whereabouts.level, whereabouts.area)
            }
            None => current,
        };
        let seen = Seen {
            now,
            playing: playing && published.is_some() && place == current,
            in_level: published.is_some(),
            place,
            area_loads: mailbox.area_loads()?,
            mario,
            others: &others,
        };
        let mut moves = GameMoves { mailbox };
        if bot_left {
            notices(BotNotice::Left);
            self.release(mailbox)?;
            if self.travel.doing().is_some() {
                let (_, reports) =
                    self.travel
                        .order(Order::Stop, Origin::Chat, &seen, &mut moves)?;
                self.report(people, &reports);
            }
        }
        for order in orders {
            let said = match order {
                Ok(order) => {
                    let (result, reports) =
                        self.travel.order(order, Origin::Chat, &seen, &mut moves)?;
                    self.report(people, &reports);
                    result.unwrap_or_else(|why| why)
                }
                Err(why) => why,
            };
            people.say_here(&said);
        }
        for one in asked {
            self.answer(one, &seen, mailbox, connection, people, notices)?;
        }
        let reports = self.travel.step(&seen, &mut moves)?;
        self.report(people, &reports);
        if let Some(bots) = &mut self.bots {
            for (from, words) in heard {
                bots.tell(&bots::chat(from.as_deref(), &words));
            }
            for news in self.arrivals.moved(&seen, people.name_of(me)) {
                bots.tell(&news);
            }
            if bots.is_watch_due(now) {
                bots.tell(&bots::state(&seen, self.travel.doing()));
            }
            if bots.flush() {
                notices(BotNotice::Left);
                self.release(mailbox)?;
            }
        }
        Ok(())
    }

    /// Answers one request of the bot's.
    fn answer<M: ProcessMemory>(
        &mut self,
        asked: Asked,
        seen: &Seen<'_>,
        mailbox: &Mailbox<M>,
        connection: &mut Connection,
        people: &mut People,
        notices: &mut impl FnMut(BotNotice),
    ) -> io::Result<()> {
        let Asked { request, ticket } = asked;
        let me = connection.player();
        let mut moves = GameMoves { mailbox };
        let Some(bots) = &mut self.bots else {
            return Ok(());
        };
        match request {
            Request::Hello => {
                notices(BotNotice::Connected);
                let welcome = bots::welcome(mailbox.game()?, me, people.name_of(me));
                bots.reply(ticket, welcome);
            }
            Request::Look => bots.reply(ticket, bots::state(seen, self.travel.doing())),
            Request::Map => bots.reply(ticket, bots::map(seen.place, &game::areas(mailbox.ram())?)),
            Request::Pad(pad) => {
                // The bot takes the controller back from whatever it sent Mario to do.
                if self.travel.doing().is_some() {
                    let (_, reports) =
                        self.travel
                            .order(Order::Stop, Origin::Chat, seen, &mut moves)?;
                    for report in &reports {
                        if let Origin::Bot(ticket) = report.origin {
                            bots.report(ticket, report);
                        } else {
                            people.say_here(&report.words);
                        }
                    }
                }
                mailbox.hold_pad(&pad)?;
                self.bot_holds = pad.frames > 0;
                bots.reply(ticket, ok());
            }
            Request::Say(words) => match Message::new(&words) {
                Some(line) => {
                    connection.queue_say(line);
                    bots.reply(ticket, ok());
                }
                None => bots.refuse(ticket, "nothing to say: a line needs a letter or digit"),
            },
            Request::Travel(order) => {
                let stopping = order == Order::Stop;
                let (result, reports) =
                    self.travel
                        .order(order, Origin::Bot(ticket), seen, &mut moves)?;
                match result {
                    Ok(_) if stopping => bots.reply(ticket, ok()),
                    Ok(doing) => bots.started(ticket, &doing),
                    Err(why) => bots.refuse(ticket, &why),
                }
                self.report(people, &reports);
            }
        }
        Ok(())
    }

    /// Tells whoever sent each order how it went: the player in the chat, a bot by its ticket.
    fn report(&mut self, people: &mut People, reports: &[Report]) {
        for report in reports {
            match (report.origin, &mut self.bots) {
                (Origin::Bot(ticket), Some(bots)) => bots.report(ticket, report),
                (Origin::Bot(_), None) => {}
                (Origin::Chat, _) => people.say_here(&report.words),
            }
        }
    }

    /// Lets go of the controller the bot held.
    fn release<M: ProcessMemory>(&mut self, mailbox: &Mailbox<M>) -> io::Result<()> {
        if std::mem::take(&mut self.bot_holds) {
            mailbox.hold_pad(&HeldPad::default())?;
        }
        Ok(())
    }
}

fn ok() -> crate::json::Value {
    crate::json::Value::object([("type", "ok".into())])
}

/// The other players in a level, by the names the roster gives them.
pub(crate) fn named(
    people: &People,
    me: PlayerId,
    states: &[(PlayerId, PlayerState)],
) -> Vec<Other> {
    states
        .iter()
        .filter(|(player, _)| *player != me)
        .filter_map(|(player, state)| {
            let whereabouts = Whereabouts::of(state);
            let in_level =
                whereabouts.level > 0 && whereabouts.position.iter().all(|value| value.is_finite());
            // The yaw is the second of the state's three angles, at 0x12 (`MpPlayerSlot`).
            let yaw = state
                .get(0x12..0x14)
                .and_then(|bytes| bytes.try_into().ok())
                .map_or(0, i16::from_be_bytes);
            in_level.then(|| Other {
                player: *player,
                name: people.name_of(*player).unwrap_or("?").to_owned(),
                place: (whereabouts.level, whereabouts.area),
                position: whereabouts.position,
                yaw,
            })
        })
        .collect()
}
