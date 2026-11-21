//! Rate limits, so a flood costs the server little and cannot crowd out players.
//!
//! Every address gets a token bucket, and join attempts from new addresses share one more. The
//! table of addresses is bounded: when it is full, packets from addresses not in it are dropped
//! until idle entries expire. One IP address may have only a small part of the table, so that
//! one machine sending from a thousand ports cannot fill it and keep every newcomer out.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// Addresses tracked at once. Sixteen players need sixteen; the rest is room for strangers.
const MAX_TRACKED: usize = 1024;
/// And at once from one IP address: a full session behind one router, each player's bridge
/// having joined again from a new port, with the old ports not yet forgotten.
const MAX_TRACKED_PER_IP: usize = 48;
/// An address unheard for this long is forgotten.
const IDLE_EXPIRY: Duration = Duration::from_secs(10);

/// An allowance that is spent and comes back with time: `rate` a second on average, and up to
/// `burst` at once. Every limit the server keeps on what a player or an address may do is one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bucket {
    tokens: f64,
    last: Instant,
}

impl Bucket {
    pub(crate) const fn full(burst: f64, now: Instant) -> Self {
        Self {
            tokens: burst,
            last: now,
        }
    }

    /// Takes `cost` from the allowance, if it has that much.
    pub(crate) fn take(&mut self, cost: f64, rate: f64, burst: f64, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last).as_secs_f64();
        self.tokens = (self.tokens + elapsed * rate).min(burst);
        self.last = now;
        if self.tokens < cost {
            return false;
        }
        self.tokens -= cost;
        true
    }

    /// Takes as many of `most` whole ones as the allowance has, and says how many.
    pub(crate) fn take_up_to(&mut self, most: u32, rate: f64, burst: f64, now: Instant) -> u32 {
        let mut taken = 0;
        while taken < most && self.take(1.0, rate, burst, now) {
            taken += 1;
        }
        taken
    }

    /// When the allowance was last asked for.
    const fn last(&self) -> Instant {
        self.last
    }
}

/// One kind of event about one player is logged at most this often, so a cheat cannot flood
/// the console, whose every line is written between two ticks.
pub(crate) const LOG_INTERVAL: Duration = Duration::from_secs(2);

/// Lets one kind of event about one player into the log once every `LOG_INTERVAL`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LogLimit(Option<Instant>);

impl LogLimit {
    /// Whether the event may be logged now, which counts as logging it.
    pub(crate) fn allow(&mut self, now: Instant) -> bool {
        let due = self
            .0
            .is_none_or(|last| now.saturating_duration_since(last) >= LOG_INTERVAL);
        if due {
            self.0 = Some(now);
        }
        due
    }
}

#[derive(Debug)]
pub struct Limits {
    rate: f64,
    burst: f64,
    addresses: HashMap<SocketAddr, Bucket>,
    /// How many of `addresses` each IP address has.
    per_ip: HashMap<IpAddr, usize>,
    join_rate: f64,
    joins: Bucket,
}

impl Limits {
    /// Allows `per_address` packets a second from each address, in bursts of up to half a second's
    /// worth, and `joins` join attempts a second from new addresses together.
    #[must_use]
    pub fn new(per_address: u32, joins: u32, now: Instant) -> Self {
        let rate = f64::from(per_address);
        let join_rate = f64::from(joins);
        Self {
            rate,
            burst: rate / 2.0,
            addresses: HashMap::new(),
            per_ip: HashMap::new(),
            join_rate,
            joins: Bucket::full(join_rate, now),
        }
    }

    /// Reports whether a packet from `from` may be handled.
    pub fn allow_packet(&mut self, from: SocketAddr, now: Instant) -> bool {
        if !self.addresses.contains_key(&from) {
            // Not swept here, where a flood from made-up addresses would have every one of
            // its packets look through the whole table: the server forgets the idle ones at
            // each tick.
            let of_ip = self.per_ip.get(&from.ip()).copied().unwrap_or(0);
            if self.addresses.len() >= MAX_TRACKED || of_ip >= MAX_TRACKED_PER_IP {
                return false;
            }
            self.addresses.insert(from, Bucket::full(self.burst, now));
            self.per_ip.insert(from.ip(), of_ip + 1);
        }
        let (rate, burst) = (self.rate, self.burst);
        self.addresses
            .get_mut(&from)
            .is_some_and(|bucket| bucket.take(1.0, rate, burst, now))
    }

    /// Reports whether a join attempt from a new address may be handled.
    pub fn allow_join(&mut self, now: Instant) -> bool {
        self.joins.take(1.0, self.join_rate, self.join_rate, now)
    }

    /// Forgets addresses unheard for a while.
    pub fn forget_idle(&mut self, now: Instant) {
        let per_ip = &mut self.per_ip;
        self.addresses.retain(|address, bucket| {
            let kept = now.saturating_duration_since(bucket.last()) < IDLE_EXPIRY;
            if !kept && let Some(of_ip) = per_ip.get_mut(&address.ip()) {
                *of_ip = of_ip.saturating_sub(1);
            }
            kept
        });
        per_ip.retain(|_, of_ip| *of_ip > 0);
    }
}

#[cfg(test)]
mod tests {
    use super::{IDLE_EXPIRY, Limits, MAX_TRACKED, MAX_TRACKED_PER_IP};
    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::{Duration, Instant};

    /// The `number`th of many machines, each sending from one port.
    fn machine(number: usize) -> SocketAddr {
        let [high, low] = u16::try_from(number).unwrap_or(u16::MAX).to_be_bytes();
        SocketAddr::from((Ipv4Addr::new(10, 0, high, low), 1000))
    }

    fn address(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::LOCALHOST, port))
    }

    /// How many of `count` packets from one address at `at` are allowed.
    fn allowed_of(limits: &mut Limits, count: usize, at: Instant) -> usize {
        let from = address(1);
        (0..count).filter(|_| limits.allow_packet(from, at)).count()
    }

    #[test]
    fn a_burst_is_allowed_then_the_rate_holds() {
        let now = Instant::now();
        let mut limits = Limits::new(100, 10, now);
        // Half a second's worth at once.
        assert_eq!(allowed_of(&mut limits, 80, now), 50);

        let later = now + Duration::from_millis(100);
        assert_eq!(allowed_of(&mut limits, 80, later), 10);
        // Other addresses have buckets of their own.
        assert!(limits.allow_packet(address(2), later));
    }

    #[test]
    fn joins_share_one_limit() {
        let now = Instant::now();
        let mut limits = Limits::new(100, 5, now);
        assert_eq!((0..20).filter(|_| limits.allow_join(now)).count(), 5);
        assert!(limits.allow_join(now + Duration::from_millis(200)));
    }

    #[test]
    fn the_address_table_is_bounded_and_idle_addresses_make_room() {
        let now = Instant::now();
        let mut limits = Limits::new(100, 5, now);
        for number in 0..MAX_TRACKED {
            assert!(limits.allow_packet(machine(number), now));
        }
        let full = !limits.allow_packet(address(1), now);
        assert!(full, "the table should be full");
        // As the server does at each tick.
        limits.forget_idle(now + IDLE_EXPIRY);
        assert!(limits.allow_packet(address(1), now + IDLE_EXPIRY));
    }

    #[test]
    fn one_machine_cannot_fill_the_table_and_its_idle_ports_make_room_for_its_next() {
        let now = Instant::now();
        let mut limits = Limits::new(100, 5, now);
        let allowed = (1000..3000).filter(|&port| limits.allow_packet(address(port), now));
        assert_eq!(allowed.count(), MAX_TRACKED_PER_IP);
        // A newcomer elsewhere is heard all the same.
        assert!(limits.allow_packet(machine(1), now));

        limits.forget_idle(now + IDLE_EXPIRY);
        assert!(limits.allow_packet(address(1), now + IDLE_EXPIRY));
    }
}
