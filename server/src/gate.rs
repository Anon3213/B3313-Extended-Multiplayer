//! The server's front door: rate limits, cookies, the encrypted handshake, and sealing.
//!
//! Everything from the network passes through here before the session sees it. A stranger must
//! return a cookie for its address before anything is kept for it, then prove it holds the session
//! password in the handshake. After that, only packets sealed under its channel reach the session,
//! so a forged source address or a tampered packet gets nowhere.

use crate::Session;
use crate::config::Config;
use crate::limits::Limits;
use b3313_protocol::secure::{Channel, Key, Responder, Salt, SecureError, derive_key};
use b3313_protocol::{DecodeError, Packet, RejectReason, is_unsealed_kind};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher, RandomState};
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// How long a cookie stays valid: this, and up to this again.
const COOKIE_PERIOD: Duration = Duration::from_secs(30);
/// Wrong passwords from one IP address before it is refused for `LOCKOUT`. Each guess already costs
/// the guesser an Argon2 derivation; this caps how many a second the server will answer.
const MAX_FAILURES: u32 = 5;
const LOCKOUT: Duration = Duration::from_secs(60);
/// IP addresses with recent failures remembered at once; more are forgotten oldest first.
const MAX_FAILURE_ENTRIES: usize = 1024;

/// A joined player's channel, and the handshake reply, kept to resend if it was lost.
#[derive(Debug)]
struct Peer {
    channel: Channel,
    accept: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
struct Failures {
    count: u32,
    last: Instant,
}

#[derive(Debug)]
pub struct Gate {
    limits: Limits,
    /// A random key, new for every server run, that cookies are made with.
    cookie_key: RandomState,
    cookie_start: Instant,
    salt: Salt,
    key: Key,
    peers: HashMap<SocketAddr, Peer>,
    failures: HashMap<IpAddr, Failures>,
    rate_limited: u64,
    /// Banned IP addresses, which get no answer at all.
    bans: HashSet<IpAddr>,
}

/// A salt that differs on every server run. It need not be secret, only fresh, so the hashes of
/// two randomly keyed hashers are enough.
fn fresh_salt() -> Salt {
    let mut salt = [0; 16];
    for (chunk, seed) in salt.chunks_mut(8).zip(0_u8..) {
        let hash = RandomState::new().hash_one(seed);
        chunk.copy_from_slice(&hash.to_be_bytes());
    }
    salt
}

impl Gate {
    /// Sets up the gate, deriving the handshake key from the config's password.
    ///
    /// # Errors
    /// Fails only if Argon2 rejects its own parameters.
    pub fn new(config: &Config, now: Instant) -> Result<Self, SecureError> {
        let salt = fresh_salt();
        Ok(Self {
            limits: Limits::new(config.packets_per_second, config.joins_per_second, now),
            cookie_key: RandomState::new(),
            cookie_start: now,
            salt,
            key: derive_key(&config.password, &salt)?,
            peers: HashMap::new(),
            failures: HashMap::new(),
            rate_limited: 0,
            bans: config.bans.iter().map(IpAddr::to_canonical).collect(),
        })
    }

    /// Handles one datagram from `from`, and returns the bytes to send back, if any.
    pub fn receive(
        &mut self,
        session: &mut Session,
        from: SocketAddr,
        data: &[u8],
        now: Instant,
    ) -> Option<Vec<u8>> {
        let packet = Packet::decode(data);
        // A packet sealed under a player's channel proves it is theirs, and spends their
        // allowance. Anything else spends that of an address that has proved nothing, even
        // one that claims a player's address: a forged flood must not cut the player off.
        let opened = match &packet {
            Ok(Packet::Sealed { nonce, sealed }) => {
                let peer = self.peers.get_mut(&from);
                peer.and_then(|peer| peer.channel.open(*nonce, sealed))
            }
            _ => None,
        };
        let allowed = if opened.is_some() {
            self.limits.allow_packet(from, now)
        } else {
            self.limits.allow_unproven(from, now)
        };
        if !allowed {
            self.rate_limited += 1;
            return None;
        }
        let packet = match packet {
            Ok(packet) => packet,
            Err(DecodeError::WrongVersion(_)) => {
                return Some(Packet::Rejected(RejectReason::WrongVersion).encode());
            }
            // Anything else is noise or an attack, and an answer would only help either.
            Err(_) => return None,
        };
        match packet {
            Packet::Sealed { .. } => {
                let inner = opened?;
                session.record_received(from, data.len());
                // Handshake packets travel only unsealed, and admission only through the gate.
                let inner = Packet::decode(&inner).ok()?;
                if is_unsealed_kind(&inner) || matches!(inner, Packet::Welcome { .. }) {
                    return None;
                }
                let reply = session.receive(from, inner, now)?;
                self.seal(from, &reply)
            }
            Packet::Hello {
                cookie,
                game,
                handshake,
            } => self.hello(session, from, cookie, game, &handshake, now),
            // Game packets must be sealed; the rest are the server's own.
            _ => None,
        }
    }

    /// Answers a `Hello` from `from`, which gave `cookie` and plays `game`.
    fn hello(
        &mut self,
        session: &mut Session,
        from: SocketAddr,
        cookie: u64,
        game: u8,
        handshake: &[u8],
        now: Instant,
    ) -> Option<Vec<u8>> {
        if self.bans.contains(&from.ip().to_canonical()) {
            return None;
        }
        // A repeat means the reply was lost.
        if let Some(peer) = self.peers.get(&from) {
            let handshake = peer.accept.clone();
            return Some(Packet::Accept { handshake }.encode());
        }
        // Nothing is kept for a stranger until it proves it receives at its address.
        if !self.is_valid_cookie(from, cookie, now) {
            let cookie = self.cookie(from, self.cookie_period(now));
            let salt = self.salt;
            return Some(Packet::Challenge { cookie, salt }.encode());
        }
        if self.is_locked_out(from.ip(), now) {
            return None;
        }
        if !self.limits.allow_join(now) {
            self.rate_limited += 1;
            return None;
        }
        let Ok(responder) = Responder::accept(&self.key, handshake) else {
            self.record_failure(from.ip(), now);
            return Some(Packet::Rejected(RejectReason::WrongPassword).encode());
        };
        // The game goes on to the session, which admits only those playing the same one.
        let join = Packet::Hello {
            cookie: 0,
            game,
            handshake: Vec::new(),
        };
        // A Welcome, or why not: the session is full, or plays another game.
        let admission = session.receive(from, join, now)?;
        let (channel, accept) = responder.finish(&admission.encode()).ok()?;
        if matches!(admission, Packet::Welcome { .. }) {
            let peer = Peer {
                channel,
                accept: accept.clone(),
            };
            self.peers.insert(from, peer);
        }
        Some(Packet::Accept { handshake: accept }.encode())
    }

    /// Seals `packet` for the player at `to`, or returns `None` if it has no channel.
    pub fn seal(&mut self, to: SocketAddr, packet: &Packet) -> Option<Vec<u8>> {
        let channel = &mut self.peers.get_mut(&to)?.channel;
        let (nonce, sealed) = channel.seal(&packet.encode()).ok()?;
        Some(Packet::Sealed { nonce, sealed }.encode())
    }

    /// Drops the channel to `address` at once, as when its player is removed.
    pub fn forget(&mut self, address: SocketAddr) {
        self.peers.remove(&address);
    }

    /// Refuses `ip` from now on, and drops the channels of everyone joined from it.
    ///
    /// Bans are kept in one form: a socket that hears both kinds of address gives an IPv4
    /// sender's as an IPv6 one, which a ban of the IPv4 address must still match.
    pub fn ban(&mut self, ip: IpAddr) {
        let ip = ip.to_canonical();
        self.bans.insert(ip);
        self.peers
            .retain(|address, _| address.ip().to_canonical() != ip);
    }

    /// Lets `ip` join again, returning whether it was banned.
    pub fn unban(&mut self, ip: IpAddr) -> bool {
        self.bans.remove(&ip.to_canonical())
    }

    /// Drops the channels of players the session no longer has, and anything else gone stale.
    pub fn forget_departed(&mut self, session: &Session, now: Instant) {
        self.peers.retain(|address, _| session.is_player(*address));
        self.limits.forget_idle(now);
        self.failures
            .retain(|_, failures| now.saturating_duration_since(failures.last) < LOCKOUT);
    }

    /// Packets dropped by the rate limits since the last call.
    pub fn take_rate_limited(&mut self) -> u64 {
        std::mem::take(&mut self.rate_limited)
    }

    /// The cookie for `address` in cookie period `period`: a keyed hash, so only this server can
    /// make one, and it needs nothing stored per address. Never 0, which means "no cookie".
    fn cookie(&self, address: SocketAddr, period: u64) -> u64 {
        let mut hasher = self.cookie_key.build_hasher();
        address.hash(&mut hasher);
        period.hash(&mut hasher);
        hasher.finish() | 1
    }

    fn cookie_period(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.cookie_start).as_secs() / COOKIE_PERIOD.as_secs()
    }

    /// Whether `cookie` is the one `address` was given in this cookie period or the one before.
    fn is_valid_cookie(&self, address: SocketAddr, cookie: u64, now: Instant) -> bool {
        let period = self.cookie_period(now);
        let before = period.checked_sub(1);
        cookie != 0
            && (cookie == self.cookie(address, period)
                || before.is_some_and(|before| cookie == self.cookie(address, before)))
    }

    fn is_locked_out(&self, ip: IpAddr, now: Instant) -> bool {
        self.failures.get(&ip).is_some_and(|failures| {
            failures.count >= MAX_FAILURES && now.saturating_duration_since(failures.last) < LOCKOUT
        })
    }

    fn record_failure(&mut self, ip: IpAddr, now: Instant) {
        if self.failures.len() >= MAX_FAILURE_ENTRIES && !self.failures.contains_key(&ip) {
            let oldest = self
                .failures
                .iter()
                .min_by_key(|(_, failures)| failures.last)
                .map(|(ip, _)| *ip);
            if let Some(oldest) = oldest {
                self.failures.remove(&oldest);
            }
        }
        let failures = self.failures.entry(ip).or_insert(Failures {
            count: 0,
            last: now,
        });
        failures.count += 1;
        failures.last = now;
    }
}

#[cfg(test)]
mod tests {
    use super::{COOKIE_PERIOD, Gate, LOCKOUT, MAX_FAILURES};
    use crate::{Config, Session};
    use b3313_protocol::secure::{Channel, Initiator, derive_key};
    use b3313_protocol::{PLAYER_STATE_SIZE, Packet, PlayerId, RejectReason};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Instant;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn client(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    fn with_password(password: &str) -> Config {
        Config {
            password: password.to_owned(),
            ..Config::default()
        }
    }

    fn decode(reply: Option<Vec<u8>>) -> Option<Packet> {
        Packet::decode(&reply?).ok()
    }

    /// The challenge a stranger at `from` gets, as (cookie, salt).
    fn challenge(
        gate: &mut Gate,
        session: &mut Session,
        from: SocketAddr,
        now: Instant,
    ) -> (u64, [u8; 16]) {
        let hello = Packet::Hello {
            cookie: 0,
            game: 0,
            handshake: vec![],
        };
        match decode(gate.receive(session, from, &hello.encode(), now)) {
            Some(Packet::Challenge { cookie, salt }) => (cookie, salt),
            _ => (0, [0; 16]),
        }
    }

    /// Joins as a client with `password` does, returning the server's answer inside the handshake
    /// and the client's channel, or the server's plain answer.
    fn join(
        gate: &mut Gate,
        session: &mut Session,
        from: SocketAddr,
        password: &str,
        now: Instant,
    ) -> Result<(Packet, Channel), Option<Packet>> {
        join_playing(gate, session, from, (password, 0), now)
    }

    /// As `join`, by a client whose game is the version of B3313 numbered `playing`.
    fn join_playing(
        gate: &mut Gate,
        session: &mut Session,
        from: SocketAddr,
        (password, playing): (&str, u8),
        now: Instant,
    ) -> Result<(Packet, Channel), Option<Packet>> {
        let (cookie, salt) = challenge(gate, session, from, now);
        let key = derive_key(password, &salt).map_err(|_| None)?;
        let (initiator, handshake) = Initiator::start(&key).map_err(|_| None)?;
        let hello = Packet::Hello {
            cookie,
            game: playing,
            handshake,
        };
        let reply = decode(gate.receive(session, from, &hello.encode(), now));
        let Some(Packet::Accept { handshake }) = reply else {
            return Err(reply);
        };
        let (channel, payload) = initiator.finish(&handshake).map_err(|_| None)?;
        let answer = Packet::decode(&payload).map_err(|_| None)?;
        Ok((answer, channel))
    }

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn the_right_password_joins_and_only_sealed_packets_reach_the_session() -> TestResult {
        let now = Instant::now();
        let mut gate = Gate::new(&with_password("hunter2"), now)?;
        let mut session = Session::new();
        let joined = join(&mut gate, &mut session, client(1), "hunter2", now);
        let (answer, mut channel) = joined.map_err(|reply| format!("refused: {reply:?}"))?;
        assert_eq!(answer, Packet::Welcome { player: id(0) });

        // A ping in the clear is ignored; sealed, it is answered, sealed.
        let ping = Packet::Ping { id: 3 }.encode();
        assert_eq!(gate.receive(&mut session, client(1), &ping, now), None);
        let (nonce, sealed) = channel.seal(&ping)?;
        let packet = Packet::Sealed { nonce, sealed }.encode();
        let reply = decode(gate.receive(&mut session, client(1), &packet, now));
        let Some(Packet::Sealed { nonce, sealed }) = reply else {
            return Err(format!("expected a sealed reply, got {reply:?}").into());
        };
        let opened = channel.open(nonce, &sealed);
        let opened = opened.map(|bytes| Packet::decode(&bytes));
        assert_eq!(opened, Some(Ok(Packet::Pong { id: 3 })));
        // The same packet again is a replay.
        assert_eq!(gate.receive(&mut session, client(1), &packet, now), None);
        Ok(())
    }

    #[test]
    fn a_wrong_password_is_refused_and_repeated_guesses_are_locked_out() -> TestResult {
        let start = Instant::now();
        let mut gate = Gate::new(&with_password("hunter2"), start)?;
        let mut session = Session::new();
        let wrong = Some(Packet::Rejected(RejectReason::WrongPassword));
        for port in 0..u16::try_from(MAX_FAILURES)? {
            let refused = join(&mut gate, &mut session, client(port), "letmein", start).err();
            assert_eq!(refused, Some(wrong.clone()));
        }
        // Even the right password from the same machine is now unanswered for a while.
        let locked = join(&mut gate, &mut session, client(9), "hunter2", start).err();
        assert_eq!(locked, Some(None));
        assert!(!session.is_player(client(9)));
        // Another machine is not kept out for those guesses.
        let elsewhere = SocketAddr::from((Ipv4Addr::new(10, 0, 0, 2), 9));
        let joined = join(&mut gate, &mut session, elsewhere, "hunter2", start);
        assert!(joined.is_ok(), "a machine that guessed nothing");
        session.kick(id(0), b3313_protocol::RemoveReason::Kicked);

        let later = start + LOCKOUT;
        let joined = join(&mut gate, &mut session, client(9), "hunter2", later);
        let answer = joined.ok().map(|(answer, _)| answer);
        assert_eq!(answer, Some(Packet::Welcome { player: id(0) }));
        Ok(())
    }

    #[test]
    fn a_stranger_is_challenged_and_kept_out_until_it_returns_its_cookie() -> TestResult {
        let now = Instant::now();
        let mut gate = Gate::new(&Config::default(), now)?;
        let mut session = Session::new();
        let (cookie, _) = challenge(&mut gate, &mut session, client(0), now);
        assert_ne!(cookie, 0, "expected a challenge");
        assert!(!session.is_player(client(0)));

        // A wrong cookie, or one given to another address, is answered with a fresh challenge.
        for (from, guess) in [(client(0), cookie ^ 2), (client(1), cookie)] {
            let hello = Packet::Hello {
                cookie: guess,
                game: 0,
                handshake: vec![1; 48],
            };
            let reply = decode(gate.receive(&mut session, from, &hello.encode(), now));
            assert!(matches!(reply, Some(Packet::Challenge { .. })), "{reply:?}");
        }

        // An old cookie is refused with a fresh challenge, which then works.
        let stale = now + COOKIE_PERIOD * 2;
        let old = Packet::Hello {
            cookie,
            game: 0,
            handshake: vec![1; 48],
        };
        let reply = decode(gate.receive(&mut session, client(0), &old.encode(), stale));
        assert!(matches!(reply, Some(Packet::Challenge { .. })), "{reply:?}");
        let stale_join = join(&mut gate, &mut session, client(0), "", stale);
        assert!(stale_join.is_ok(), "a fresh challenge should let it join");
        assert!(session.is_player(client(0)));
        Ok(())
    }

    #[test]
    fn a_full_session_says_so_inside_the_handshake() -> TestResult {
        let now = Instant::now();
        let config = Config {
            max_players: 1,
            ..Config::default()
        };
        let mut gate = Gate::new(&config, now)?;
        let mut session = Session::with_config(config);
        let first = join(&mut gate, &mut session, client(0), "", now).map(|(answer, _)| answer);
        assert_eq!(first.ok(), Some(Packet::Welcome { player: id(0) }));
        let second = join(&mut gate, &mut session, client(1), "", now).map(|(answer, _)| answer);
        assert_eq!(second.ok(), Some(Packet::Rejected(RejectReason::Full)));
        // The refused client got no channel.
        let state = Packet::State {
            sequence: 1,
            state: [0; PLAYER_STATE_SIZE],
        };
        assert_eq!(gate.seal(client(1), &state), None);
        Ok(())
    }

    #[test]
    fn a_session_playing_another_game_says_which_inside_the_handshake() -> TestResult {
        let now = Instant::now();
        let mut gate = Gate::new(&with_password("hunter2"), now)?;
        let mut session = Session::new();
        let mut answer_to = |port: u16, game: u8| {
            join_playing(
                &mut gate,
                &mut session,
                client(port),
                ("hunter2", game),
                now,
            )
            .map(|(answer, _)| answer)
            .ok()
        };
        assert_eq!(answer_to(0, 2), Some(Packet::Welcome { player: id(0) }));
        let refused = Packet::Rejected(RejectReason::WrongGame(2));
        assert_eq!(answer_to(1, 3), Some(refused));
        assert_eq!(answer_to(2, 2), Some(Packet::Welcome { player: id(1) }));
        // The refused client got no channel.
        assert_eq!(gate.seal(client(1), &Packet::Ping { id: 1 }), None);
        Ok(())
    }

    #[test]
    fn a_banned_address_gets_no_answer_until_unbanned() -> TestResult {
        let now = Instant::now();
        let config = Config {
            bans: vec![client(5).ip()],
            ..Config::default()
        };
        let mut gate = Gate::new(&config, now)?;
        let mut session = Session::new();
        // Banned from the config, at startup.
        let refused = join(&mut gate, &mut session, client(5), "", now).err();
        assert_eq!(refused, Some(None));

        assert!(gate.unban(client(5).ip()));
        let (_, mut channel) = join(&mut gate, &mut session, client(5), "", now)
            .map_err(|reply| format!("refused: {reply:?}"))?;

        // A ban drops the channel at once: what the player sends no longer gets through.
        gate.ban(client(5).ip());
        let (nonce, sealed) = channel.seal(&Packet::Ping { id: 1 }.encode())?;
        let ping = Packet::Sealed { nonce, sealed }.encode();
        assert_eq!(gate.receive(&mut session, client(5), &ping, now), None);
        let elsewhere = Ipv4Addr::new(10, 0, 0, 1).into();
        assert!(!gate.unban(elsewhere), "never banned");

        // A socket that hears both kinds of address gives the same sender as an IPv6 one.
        let as_ipv6 = SocketAddr::from((Ipv4Addr::LOCALHOST.to_ipv6_mapped(), 5));
        let refused = join(&mut gate, &mut session, as_ipv6, "", now).err();
        assert_eq!(refused, Some(None));
        Ok(())
    }

    #[test]
    fn garbage_gets_no_answer_and_other_versions_are_told_why() -> TestResult {
        let now = Instant::now();
        let mut gate = Gate::new(&Config::default(), now)?;
        let mut session = Session::new();
        let mut answer = |data: &[u8]| decode(gate.receive(&mut session, client(0), data, now));
        assert_eq!(answer(b"hello?"), None);
        assert_eq!(answer(b"B3NP\x1A\x04\0"), None);
        assert_eq!(
            answer(b"B3NP\x00\x01"),
            Some(Packet::Rejected(RejectReason::WrongVersion))
        );
        assert_eq!(session.take_events(), []);
        Ok(())
    }

    #[test]
    fn a_flooding_address_is_cut_off_and_counted() -> TestResult {
        let now = Instant::now();
        let config = Config {
            packets_per_second: 100,
            ..Config::default()
        };
        let mut gate = Gate::new(&config, now)?;
        let mut session = Session::new();
        let hello = Packet::Hello {
            cookie: 0,
            game: 0,
            handshake: vec![],
        }
        .encode();
        let answered = (0..200)
            .filter(|_| gate.receive(&mut session, client(0), &hello, now).is_some())
            .count();
        // A burst of half a second's worth of what an address that proved nothing may send.
        assert_eq!(answered, 10);
        assert_eq!(gate.take_rate_limited(), 190);
        assert!(gate.receive(&mut session, client(1), &hello, now).is_some());
        Ok(())
    }

    /// A sender's address is easy to forge. Packets sent under a player's that are not
    /// sealed under the player's channel must not use up what the player may send.
    #[test]
    fn a_flood_forged_under_a_players_address_does_not_cut_the_player_off() -> TestResult {
        let now = Instant::now();
        let mut gate = Gate::new(&Config::default(), now)?;
        let mut session = Session::new();
        let (_, mut channel) = join(&mut gate, &mut session, client(1), "", now)
            .map_err(|reply| format!("refused: {reply:?}"))?;

        let forged = Packet::Sealed {
            nonce: 9,
            sealed: vec![7; 40],
        }
        .encode();
        for _ in 0..500 {
            assert_eq!(gate.receive(&mut session, client(1), &forged, now), None);
        }

        let (nonce, sealed) = channel.seal(&Packet::Ping { id: 3 }.encode())?;
        let ping = Packet::Sealed { nonce, sealed }.encode();
        let answer = gate.receive(&mut session, client(1), &ping, now);
        assert!(answer.is_some(), "the player's own packet is still heard");
        Ok(())
    }
}
