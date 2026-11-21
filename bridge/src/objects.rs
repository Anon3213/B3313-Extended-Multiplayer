//! Object sync on the bridge: which of the game's owned objects to send, and passing on what
//! other owners sent.

use crate::client::{Connection, Received};
use crate::interpolation::Interpolator;
use crate::layout::len;
use crate::mailbox::{COIN_ENTRIES, Mailbox, ObjectsOut, Place, RingCoin};
use crate::object_motion::ObjectMotion;
use crate::ram::ProcessMemory;
use b3313_protocol::objects::object_id;
use b3313_protocol::{
    GONE_CHILD_SENDS, MAX_GONE_CHILDREN, MAX_OBJECTS_PER_PACKET, ObjectSet, ObjectState,
    ObjectUpdate,
};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::{Duration, Instant};

/// An owned object that has not changed is still sent this often, so a player who arrives later,
/// or missed a packet, learns where it is.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// Where `timer` and `animFrame` sit in a state's network part (`struct MpObjectState` from `id`).
const TIMER: std::ops::Range<usize> = network_part(len::OBJECT_STATE_TIMER_OFFSET, 2);
const ANIM_FRAME: std::ops::Range<usize> = network_part(len::OBJECT_STATE_ANIM_FRAME_OFFSET, 2);

/// Where the `size` bytes at `offset` of the game's `struct MpObjectState` are in a state as
/// the network carries it, which begins at the struct's `id`.
const fn network_part(offset: usize, size: usize) -> std::ops::Range<usize> {
    let start = offset - len::OBJECT_STATE_ID_OFFSET;
    start..start + size
}

/// Whether two states of one object differ in more than their timers and animation frames. Both
/// count frames, so counting them as a change would send every object every time; each game's copy
/// counts on by itself, and refreshes and real changes bring it back in step.
fn differs(a: &ObjectState, b: &ObjectState) -> bool {
    let counts_frames = |index: &usize| TIMER.contains(index) || ANIM_FRAME.contains(index);
    a.iter()
        .zip(b)
        .enumerate()
        .any(|(index, (x, y))| x != y && !counts_frames(&index))
}

/// Picks which owned states go in each send: changed ones first, then unchanged ones due a
/// refresh, each group longest-waiting first, up to a packet's worth.
#[derive(Debug, Default)]
pub struct ObjectSender {
    place: Option<Place>,
    /// Each object's state as last sent, and when.
    sent: HashMap<u16, (ObjectState, Instant)>,
}

impl ObjectSender {
    pub fn pick(&mut self, out: &ObjectsOut, now: Instant) -> Vec<ObjectState> {
        if self.place != Some(out.place) {
            self.place = Some(out.place);
            self.sent.clear();
        }
        // (unchanged, when last sent): changed states sort first, and never-sent ones first of all.
        let mut due: Vec<((bool, Option<Instant>), ObjectState)> = out
            .states
            .iter()
            .filter_map(|state| {
                let last = self.sent.get(&object_id(state));
                let unchanged = last.is_some_and(|(sent, _)| !differs(sent, state));
                let refresh_due = last
                    .is_none_or(|&(_, at)| now.saturating_duration_since(at) >= REFRESH_INTERVAL);
                (!unchanged || refresh_due).then(|| ((unchanged, last.map(|&(_, at)| at)), *state))
            })
            .collect();
        due.sort_by_key(|&(order, _)| order);
        due.truncate(MAX_OBJECTS_PER_PACKET);
        for (_, state) in &due {
            self.sent.insert(object_id(state), (*state, now));
        }
        due.into_iter().map(|(_, state)| state).collect()
    }
}

/// What the server last said about the objects where this player is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectNews {
    pub place: Place,
    pub owned: ObjectSet,
    pub gone: ObjectSet,
    /// Every spawned enemy alive there, as the games that run them last said; None if there
    /// are too many to list.
    pub children: Option<Vec<u16>>,
    /// Other owners' states, oldest first, each with its owner and when it was read.
    pub states: Vec<ObjectUpdate>,
}

impl ObjectNews {
    /// Adds a later packet's news: its sets and its list replace these, and its states follow,
    /// unless it is about another place, which replaces everything.
    pub fn merge(&mut self, later: Self) {
        if later.place == self.place {
            self.owned = later.owned;
            self.gone = later.gone;
            self.children = later.children;
            self.states.extend(later.states);
        } else {
            *self = later;
        }
    }
}

/// Gives the game the states of others' objects, as they are to be drawn now. `in_count` is
/// where the bridge is in the game's `objectsIn` ring, from its count when unknown.
fn deliver<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    place: Place,
    states: &[ObjectState],
    in_count: &mut Option<u32>,
) -> io::Result<()> {
    if states.is_empty() {
        return Ok(());
    }
    let count = match *in_count {
        Some(count) => count,
        None => mailbox.objects_in_count()?,
    };
    *in_count = Some(mailbox.write_objects_in(count, place, states)?);
    Ok(())
}

/// Object sync for one session: what to send of the game's objects, and where the bridge is in
/// the game's rings.
#[derive(Debug, Default)]
pub struct ObjectSync {
    sender: ObjectSender,
    /// Where the bridge is in the game's `objectsIn` ring; from the game's count when first needed.
    objects_in: Option<u32>,
    /// Others' objects where this game's player is, which `draw` gives the game as they move.
    motion: ObjectMotion,
    motion_place: Option<Place>,
    /// Where the bridge is in the game's `interactionsOut` ring.
    interactions_seen: Option<u32>,
    /// Where the bridge is in the game's `childrenGoneOut` ring.
    children_gone_seen: Option<u32>,
    /// Spawned enemies the game reported gone, with the sends each has left.
    children_gone: Vec<(u16, u8)>,
    /// Where the bridge is in the game's `coinsOut` and `coinsIn` rings.
    coins_seen: Option<u32>,
    coins_in: Option<u32>,
    /// Others' coins the game has yet to be given: its ring holds only so many at a time, and a
    /// game that has just come to an area is told of every coin collected there before.
    coins_waiting: VecDeque<RingCoin>,
    /// What the game was last told of who runs what, and when.
    told: Option<(Told, Instant)>,
    earlier_coins: EarlierCoins,
}

/// Whether the game is to have the server's word on which coins of its area were collected
/// before it came, which arrives as coins worth nothing for the game to take away.
///
/// The server gives that word with the first state a bridge sends after a break in them, as
/// it cannot tell a game that was at a menu, whose area is loaded again with every coin back,
/// from one that was paused, which still has what it took away. A coin's name is not always
/// one coin's alone, so a game told twice takes away two: coins nobody collected went from a
/// game each time it was paused. The game counts its area loads (`areaLoads`), and the word is
/// passed on only if the area was loaded since it last was.
///
/// Which word a coin belongs to cannot be read off it, so this goes by when each was asked
/// for. A break within a second of the last asking, before its word can have come, or two
/// loads as close, are taken wrongly: rare, and what it costs is a coin too many or too few.
#[derive(Debug, Default)]
struct EarlierCoins {
    /// The game's count of area loads when the server's word was last asked for, by a state
    /// after a break, and when that was.
    asked: Option<(u32, Instant)>,
    /// The word asked for last is one the game has already had.
    is_repeat: bool,
    /// States are going to the server, with no break since the last.
    sending: bool,
    /// Send slots still to go without a state, for the break the server is to see.
    withheld: u8,
}

/// How many send slots go without a state for the server to see a break in them: each is a
/// packet that may be lost, and the server gives its word again only if one arrives.
const BREAK_SLOTS: u8 = 3;
/// How long the server's word takes to come at most, a round trip and the packets it fills.
/// A break sooner than this after asking for it does not make the next word a repeat.
const WORD_ARRIVES_WITHIN: Duration = Duration::from_secs(1);

impl EarlierCoins {
    /// Whether a state may go in this send slot, at `now`, the game having loaded
    /// `area_loads` areas. Not for a few slots if it loaded one since the server's word was
    /// asked for with states going all the while, as when Mario dies: the server would not
    /// give its word again without a break.
    fn state_may_go(&mut self, area_loads: u32, now: Instant) -> bool {
        let loaded_since = self.asked.is_none_or(|(loads, _)| loads != area_loads);
        if self.sending && loaded_since {
            self.sending = false;
            self.withheld = BREAK_SLOTS;
        }
        if self.withheld > 0 {
            self.withheld -= 1;
            return false;
        }
        if !self.sending {
            self.is_repeat = self.asked.is_some_and(|(loads, at)| {
                loads == area_loads && now.saturating_duration_since(at) >= WORD_ARRIVES_WITHIN
            });
            self.asked = Some((area_loads, now));
            self.sending = true;
        }
        true
    }

    /// What is kept of this for a server joined again, with the game where it was: the word
    /// a server that still has the session gives is one the game has had.
    fn for_server_joined_again(&self) -> Self {
        Self {
            asked: self.asked,
            ..Self::default()
        }
    }
}

/// What the server says of an area's objects besides their states, as the game is told it.
#[derive(Debug, PartialEq, Eq)]
struct Told {
    place: Place,
    owned: ObjectSet,
    gone: ObjectSet,
    children: Option<Vec<u16>>,
}

/// The most of others' coins kept for a game that is not taking them, as while it is paused.
const COINS_WAITING_MOST: usize = 1024;

impl ObjectSync {
    /// The game was started again: its rings count from nothing, so each place kept in one
    /// is read from the game anew, and nothing queued for the game that was is kept.
    pub fn game_started_again(&mut self) {
        *self = Self::default();
    }

    /// Whether the game's state may go to the server in this send slot, which is asked in each
    /// slot it would: see `EarlierCoins`.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read.
    pub fn state_may_go<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        now: Instant,
    ) -> io::Result<bool> {
        Ok(self.earlier_coins.state_may_go(mailbox.area_loads()?, now))
    }

    /// Object sync for a server joined again, with the same game: nothing kept of the server
    /// that was, but which of its words on earlier coins the game has had.
    #[must_use]
    pub fn for_server_joined_again(&self) -> Self {
        Self {
            earlier_coins: self.earlier_coins.for_server_joined_again(),
            ..Self::default()
        }
    }

    /// Notes that no state went in this send slot: the game is paused, or at a menu.
    pub const fn no_state_went(&mut self) {
        self.earlier_coins.sending = false;
    }

    /// Reports the game's objects and its Mario's interactions with others'. Call it once per send
    /// slot while the game is ready; `playing` says whether it is in a level.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read, or the socket fails.
    pub fn send<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        connection: &mut Connection,
        now: Instant,
        playing: bool,
    ) -> io::Result<()> {
        self.collect_children_gone(mailbox)?;
        if playing && let Some(out) = mailbox.read_objects_out()? {
            let states = self.sender.pick(&out, now);
            let gone_children = self.next_children_gone();
            connection.send_objects(&out, states, gone_children)?;
        }
        collect_interactions(mailbox, connection, &mut self.interactions_seen)?;
        connection.send_interactions()?;
        self.collect_coins(mailbox, connection)?;
        connection.send_coins()
    }

    /// Gives the game the objects other games run, as each is to be drawn at `now`: the moment
    /// its owner's player is drawn at. Call it as often as players are drawn.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read or written.
    pub fn draw<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        players: &Interpolator,
        now: Instant,
    ) -> io::Result<()> {
        let Some(place) = self.motion_place else {
            return Ok(());
        };
        let states = self.motion.sample(now, |owner| players.clock(owner));
        deliver(mailbox, place, &states, &mut self.objects_in)
    }

    /// Queues the coins the game's Mario collected since the last call.
    fn collect_coins<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        connection: &mut Connection,
    ) -> io::Result<()> {
        let seen = match self.coins_seen {
            Some(seen) => seen,
            None => mailbox.coins_out_count()?,
        };
        let (coins, count) = mailbox.read_coins_out(seen)?;
        self.coins_seen = Some(count);
        for coin in &coins {
            connection.queue_coin(coin);
        }
        Ok(())
    }

    /// Queues the coins other players collected for the game. None is left out for being one
    /// this game's Mario collected too: a name is not always one coin's alone, so that cannot
    /// be told, and a coin two players reach together counts for each of them twice.
    fn queue_coins(&mut self, coins: &[RingCoin]) {
        // Worth nothing: one collected before this game came to its area. See `EarlierCoins`.
        let wanted = coins
            .iter()
            .filter(|coin| coin.value != 0 || !self.earlier_coins.is_repeat);
        for coin in wanted {
            if self.coins_waiting.len() >= COINS_WAITING_MOST {
                self.coins_waiting.pop_front();
            }
            self.coins_waiting.push_back(*coin);
        }
    }

    /// Gives the game the waiting coins its ring has room for, beyond those it has yet to take.
    fn deliver_coins<M: ProcessMemory>(&mut self, mailbox: &Mailbox<M>) -> io::Result<()> {
        if self.coins_waiting.is_empty() {
            return Ok(());
        }
        let mut count = match self.coins_in {
            Some(count) => count,
            None => mailbox.coins_in_count()?,
        };
        let taken = mailbox.coins_in_taken()?;
        // A game started again counts from nothing, and would never catch up with the count
        // kept here: the ring would look full for the rest of the session.
        if count.wrapping_sub(taken) > COIN_ENTRIES {
            count = taken;
        }
        while count.wrapping_sub(taken) < COIN_ENTRIES
            && let Some(coin) = self.coins_waiting.pop_front()
        {
            count = mailbox.write_coin_in(count, &coin)?;
        }
        self.coins_in = Some(count);
        Ok(())
    }

    /// Queues the spawned enemies the game reported gone since the last call, each to be sent
    /// `GONE_CHILD_SENDS` times, as UDP may lose one.
    fn collect_children_gone<M: ProcessMemory>(&mut self, mailbox: &Mailbox<M>) -> io::Result<()> {
        let seen = match self.children_gone_seen {
            Some(seen) => seen,
            None => mailbox.children_gone_out_count()?,
        };
        let (names, count) = mailbox.read_children_gone_out(seen)?;
        self.children_gone_seen = Some(count);
        self.children_gone
            .extend(names.into_iter().map(|name| (name, GONE_CHILD_SENDS)));
        Ok(())
    }

    /// The queued enemies gone that this send carries, counting down each one's sends.
    fn next_children_gone(&mut self) -> Vec<u16> {
        let names = self
            .children_gone
            .iter_mut()
            .take(MAX_GONE_CHILDREN)
            .map(|(name, sends)| {
                *sends -= 1;
                *name
            })
            .collect();
        self.children_gone.retain(|&(_, sends)| sends > 0);
        names
    }

    /// Gives the game what the server said about objects since the last poll: what it owns and
    /// what is gone, at once. Others' states are kept for `draw`, each placed in time by its
    /// owner's clock in `players`.
    ///
    /// # Errors
    /// Fails if the game's memory cannot be read or written.
    pub fn receive<M: ProcessMemory>(
        &mut self,
        mailbox: &Mailbox<M>,
        received: &Received,
        players: &Interpolator,
        now: Instant,
    ) -> io::Result<()> {
        if let Some(news) = &received.objects {
            // The server says who runs what with every state it sends, thirty times a second
            // while anything moves, and mostly says the same. Written again only when it
            // differs, and now and then in case the game let the session go meanwhile and
            // took it up again with nothing kept: each write is eight system calls, and makes
            // the game's read of the record fail if it falls in the middle.
            let told = Told {
                place: news.place,
                owned: news.owned,
                gone: news.gone,
                children: news.children.clone(),
            };
            let is_news = self.told.as_ref().is_none_or(|(last, at)| {
                *last != told || now.saturating_duration_since(*at) >= REFRESH_INTERVAL
            });
            if is_news {
                mailbox.write_objects_owned(news.place, &news.owned, &news.gone)?;
            }
            if self.motion_place != Some(news.place) {
                self.motion_place = Some(news.place);
                self.motion.clear();
            }
            for update in &news.states {
                self.motion
                    .receive(update, players.clock(update.owner), now);
            }
            // What this game runs now, or is gone, is no longer another's to draw.
            for name in news.owned.ids().chain(news.gone.ids()) {
                self.motion.forget(name);
            }
            // Before `draw` next gives the game a state: an enemy no longer alive must not be
            // made again from one.
            if is_news {
                mailbox.write_live_children(news.place, news.children.as_deref())?;
                self.told = Some((told, now));
            }
            if let Some(alive) = &news.children {
                self.motion.forget_children_but(alive);
            }
        }
        self.queue_coins(&received.coins);
        self.deliver_coins(mailbox)
    }
}

/// Queues the game's claims on objects other games run, which its Mario attacked or picked up
/// since the last call. `seen` is where the bridge is in the game's `interactionsOut` ring, from
/// the game's count when unknown, so nothing from before the bridge attached is replayed.
fn collect_interactions<M: ProcessMemory>(
    mailbox: &Mailbox<M>,
    connection: &mut Connection,
    seen: &mut Option<u32>,
) -> io::Result<()> {
    let from = match *seen {
        Some(seen) => seen,
        None => mailbox.interactions_out_count()?,
    };
    let (interactions, count) = mailbox.read_interactions_out(from)?;
    *seen = Some(count);
    for interaction in &interactions {
        connection.queue_interaction(interaction);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BREAK_SLOTS, ObjectNews, ObjectSender, ObjectSync, REFRESH_INTERVAL, WORD_ARRIVES_WITHIN,
    };
    use crate::layout;
    use crate::mailbox::testing::game;
    use crate::mailbox::{COIN_ENTRIES, Mailbox, ObjectsOut, RingCoin};
    use crate::ram::N64Ram;
    use b3313_protocol::objects::object_id;
    use b3313_protocol::{
        MAX_OBJECTS_PER_PACKET, OBJECT_STATE_SIZE, ObjectSet, ObjectState, ObjectUpdate, PlayerId,
    };
    use std::io;
    use std::time::Instant;

    fn object(id: u16, marker: u8) -> ObjectState {
        let mut state = [marker; OBJECT_STATE_SIZE];
        state[..2].copy_from_slice(&id.to_be_bytes());
        state
    }

    fn out(states: Vec<ObjectState>) -> ObjectsOut {
        ObjectsOut {
            place: (16, 1),
            gone: ObjectSet::default(),
            states,
        }
    }

    fn ids(states: &[ObjectState]) -> Vec<u16> {
        states.iter().map(object_id).collect()
    }

    #[test]
    fn changes_go_at_once_and_still_objects_once_a_refresh_is_due() {
        let start = Instant::now();
        let mut sender = ObjectSender::default();
        let published = out(vec![object(1, 0), object(2, 0)]);
        assert_eq!(ids(&sender.pick(&published, start)), [1, 2]);
        assert_eq!(ids(&sender.pick(&published, start)), []);

        let moved = out(vec![object(1, 1), object(2, 0)]);
        assert_eq!(ids(&sender.pick(&moved, start)), [1]);
        assert_eq!(ids(&sender.pick(&moved, start + REFRESH_INTERVAL)), [1, 2]);
    }

    #[test]
    fn a_full_packet_takes_changes_before_refreshes_and_the_longest_waiting_first() {
        let start = Instant::now();
        let mut sender = ObjectSender::default();
        // A full packet's worth and five more.
        let full = u16::try_from(MAX_OBJECTS_PER_PACKET).unwrap_or(u16::MAX);
        let first: Vec<ObjectState> = (1..=full + 5).map(|id| object(id, 0)).collect();
        let sent = sender.pick(&out(first.clone()), start);
        assert_eq!(ids(&sent), (1..=full).collect::<Vec<_>>());

        // The last five have never been sent, and 1 has changed; the rest are due a refresh too.
        let mut next = first;
        if let Some(state) = next.first_mut() {
            *state = object(1, 1);
        }
        let later = start + REFRESH_INTERVAL;
        let sent = sender.pick(&out(next), later);
        assert_eq!(sent.len(), MAX_OBJECTS_PER_PACKET);
        let mut expected: Vec<u16> = (full + 1..=full + 5).collect();
        expected.push(1);
        assert_eq!(sent.get(..6).map(ids), Some(expected));
    }

    #[test]
    fn counting_frames_is_not_a_change() {
        let now = Instant::now();
        let mut sender = ObjectSender::default();
        let mut state = object(1, 0);
        sender.pick(&out(vec![state]), now);
        state[33] = 1; // The low byte of `timer`.
        state[45] = 1; // And of `animFrame`.
        assert_eq!(ids(&sender.pick(&out(vec![state]), now)), []);
        state[34] = 1; // `subAction`, just after the timer.
        assert_eq!(ids(&sender.pick(&out(vec![state]), now)), [1]);
    }

    fn coin(area: u8, name: u16, value: u8) -> RingCoin {
        RingCoin {
            place: (4, area),
            name,
            value,
        }
    }

    #[test]
    fn coins_wait_for_room_in_the_games_ring() -> io::Result<()> {
        let process = game();
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let mut sync = ObjectSync::default();
        // More than the ring holds at once, as when a game comes to an area others have played.
        let coins: Vec<RingCoin> = (1..=20).map(|name| coin(1, name, 0)).collect();
        sync.queue_coins(&coins);
        sync.deliver_coins(&mailbox)?;
        assert_eq!(mailbox.coins_in_count()?, COIN_ENTRIES);
        sync.deliver_coins(&mailbox)?;
        assert_eq!(mailbox.coins_in_count()?, COIN_ENTRIES);

        // The game takes them all, and says so in `coinsInTaken`.
        let taken = layout::MAILBOX_ADDR + layout::MAILBOX_COINS_IN_TAKEN_OFFSET;
        N64Ram::new(&process, 0).write(taken, &COIN_ENTRIES.to_be_bytes())?;
        sync.deliver_coins(&mailbox)?;
        assert_eq!(mailbox.coins_in_count()?, 20);
        Ok(())
    }

    /// The server names an area's earlier coins again after any break in a game's states. A
    /// game that was only paused has taken them away already, and would take away as many
    /// more of the same names.
    #[test]
    fn earlier_coins_are_passed_on_again_only_once_the_area_was_loaded_again() -> io::Result<()> {
        let process = game();
        let mailbox = Mailbox::new(N64Ram::new(&process, 0));
        let loads = layout::MAILBOX_ADDR + layout::MAILBOX_AREA_LOADS_OFFSET;
        let load_area = |count: u32| N64Ram::new(&process, 0).write(loads, &count.to_be_bytes());
        let mut sync = ObjectSync::default();
        let waiting_after = |sync: &mut ObjectSync, coins: &[RingCoin]| {
            sync.queue_coins(coins);
            let waiting = sync.coins_waiting.len();
            sync.coins_waiting.clear();
            waiting
        };
        let earlier = [coin(1, 7, 0), coin(1, 7, 0)];

        let start = Instant::now();
        let later = start + WORD_ARRIVES_WITHIN;

        // Come to the area: the first state goes, and the server's word is taken.
        load_area(1)?;
        assert!(sync.state_may_go(&mailbox, start)?);
        assert_eq!(waiting_after(&mut sync, &earlier), 2);

        // Paused and on again: the same word comes, and is left out. A coin collected now is not.
        sync.no_state_went();
        assert!(sync.state_may_go(&mailbox, later)?);
        assert_eq!(waiting_after(&mut sync, &earlier), 0);
        assert_eq!(waiting_after(&mut sync, &[coin(1, 7, 1)]), 1);

        // The server is joined again with the game where it was: its word is still a repeat.
        let mut sync = sync.for_server_joined_again();
        assert!(sync.state_may_go(&mailbox, later + WORD_ARRIVES_WITHIN)?);
        assert_eq!(waiting_after(&mut sync, &earlier), 0);

        // The area is loaded again with states going, as after Mario dies: a few slots go
        // without a state, so that the server gives its word again, and then it is taken.
        load_area(2)?;
        let much_later = later + 2 * WORD_ARRIVES_WITHIN;
        for _ in 0..BREAK_SLOTS {
            assert!(!sync.state_may_go(&mailbox, much_later)?);
        }
        assert!(sync.state_may_go(&mailbox, much_later)?);
        assert_eq!(waiting_after(&mut sync, &earlier), 2);
        Ok(())
    }

    #[test]
    fn a_new_place_sends_everything_again() {
        let now = Instant::now();
        let mut sender = ObjectSender::default();
        sender.pick(&out(vec![object(1, 0)]), now);
        let mut elsewhere = out(vec![object(1, 0)]);
        elsewhere.place = (16, 2);
        assert_eq!(ids(&sender.pick(&elsewhere, now)), [1]);
    }

    /// Object `id` as the server passes it on, from player 1's game.
    fn update(id: u16) -> ObjectUpdate {
        ObjectUpdate {
            owner: PlayerId::new(1).unwrap_or_else(|| unreachable!()),
            sequence: 1,
            state: object(id, 0),
        }
    }

    #[test]
    fn news_about_the_same_place_adds_up_and_another_place_replaces_it() {
        let mut owned = ObjectSet::default();
        owned.insert(3);
        let mut news = ObjectNews {
            place: (16, 1),
            owned: ObjectSet::default(),
            gone: ObjectSet::default(),
            children: Some(vec![0x8081, 0x8082]),
            states: vec![update(1)],
        };
        news.merge(ObjectNews {
            place: (16, 1),
            owned,
            gone: ObjectSet::default(),
            children: Some(vec![0x8082]),
            states: vec![update(2)],
        });
        let names: Vec<u16> = news.states.iter().map(|u| object_id(&u.state)).collect();
        assert_eq!((names, news.owned), (vec![1, 2], owned));
        // The later list is the whole list.
        assert_eq!(news.children, Some(vec![0x8082]));

        news.merge(ObjectNews {
            place: (16, 2),
            owned: ObjectSet::default(),
            gone: ObjectSet::default(),
            children: None,
            states: vec![],
        });
        assert_eq!((news.place, news.states.len()), ((16, 2), 0));
        assert_eq!(news.children, None);
    }
}
