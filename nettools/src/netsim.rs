//! A UDP proxy that delays, reorders and drops packets between bridges and a server.
//!
//! Each client gets its own socket towards the server, so the server still sees one address per
//! player. Packets are held in a queue until their delivery time, so jitter reorders them the way a
//! real network does.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

/// Larger than any UDP payload, so nothing is cut short.
const BUFFER_SIZE: usize = 65_536;
/// How long the proxy sleeps when nothing is due, which bounds its timing error.
const IDLE_WAIT: Duration = Duration::from_millis(1);
/// A client silent this long is forgotten, and its socket closed.
const CLIENT_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// What the simulated network does to each packet, in each direction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Impairment {
    /// Added to every packet.
    pub delay: Duration,
    /// Up to this much more, chosen evenly at random per packet.
    pub jitter: Duration,
    /// The chance from 0 to 1 that a packet is dropped.
    pub loss: f64,
}

impl Impairment {
    /// Returns when a packet sent at `now` arrives, or `None` if it is lost.
    pub fn deliver_at(&self, random: &mut Random, now: Instant) -> Option<Instant> {
        if random.unit() < self.loss {
            return None;
        }
        Some(now + self.delay + self.jitter.mul_f64(random.unit()))
    }
}

/// `SplitMix64`: small, fast and good enough to pick drops and delays. Not for anything secret.
#[derive(Clone, Debug)]
pub struct Random(u64);

impl Random {
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number from 0 up to, but not including, 1.
    pub fn unit(&mut self) -> f64 {
        let high = u32::try_from(self.next_u64() >> 32).unwrap_or(u32::MAX);
        f64::from(high) / 4_294_967_296.0
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Destination {
    /// Through the upstream socket of the client at this address.
    Server(SocketAddr),
    Client(SocketAddr),
}

/// Ordered by delivery time, then by arrival, so equal times keep their order.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Pending {
    at: Instant,
    order: u64,
    to: Destination,
    data: Vec<u8>,
}

#[derive(Debug)]
struct Link {
    client: SocketAddr,
    upstream: UdpSocket,
    last_used: Instant,
}

#[derive(Debug)]
pub struct Proxy {
    listen: UdpSocket,
    server: SocketAddr,
    impairment: Impairment,
    random: Random,
    links: Vec<Link>,
    queue: BinaryHeap<Reverse<Pending>>,
    order: u64,
}

fn is_quiet(error: &io::Error) -> bool {
    // Windows reports an earlier send to a closed port as a failed receive. The proxy passes on
    // what it can; the bridge and server have their own timeouts.
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::ConnectionReset
    )
}

/// Reads one datagram if one is waiting.
fn try_receive(socket: &UdpSocket, buffer: &mut [u8]) -> io::Result<Option<(usize, SocketAddr)>> {
    match socket.recv_from(buffer) {
        Ok(received) => Ok(Some(received)),
        Err(error) if is_quiet(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

impl Proxy {
    /// Listens on `listen` for clients and forwards them to `server`.
    ///
    /// # Errors
    /// Fails if the listening socket cannot be set up.
    pub fn new(
        listen: UdpSocket,
        server: SocketAddr,
        impairment: Impairment,
        seed: u64,
    ) -> io::Result<Self> {
        listen.set_nonblocking(true)?;
        Ok(Self {
            listen,
            server,
            impairment,
            random: Random::new(seed),
            links: Vec::new(),
            queue: BinaryHeap::new(),
            order: 0,
        })
    }

    /// Forwards packets until `stop` is set.
    ///
    /// # Errors
    /// Fails if a socket stops working. A single failed send does not stop the proxy.
    pub fn run(&mut self, stop: &AtomicBool) -> io::Result<()> {
        let mut buffer = vec![0; BUFFER_SIZE];
        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            while let Some((length, client)) = try_receive(&self.listen, &mut buffer)? {
                let data = buffer.get(..length).unwrap_or_default().to_vec();
                self.link_for(client, now)?;
                self.schedule(Destination::Server(client), data, now);
            }
            let mut from_server = Vec::new();
            for link in &self.links {
                while let Some((length, _)) = try_receive(&link.upstream, &mut buffer)? {
                    let data = buffer.get(..length).unwrap_or_default().to_vec();
                    from_server.push((link.client, data));
                }
            }
            for (client, data) in from_server {
                self.schedule(Destination::Client(client), data, now);
            }
            self.deliver_due(now);
            self.links
                .retain(|link| now.saturating_duration_since(link.last_used) < CLIENT_IDLE_TIMEOUT);

            let next = self.queue.peek().map(|Reverse(pending)| pending.at);
            let wait = next.map_or(IDLE_WAIT, |at| at.saturating_duration_since(Instant::now()));
            thread::sleep(wait.min(IDLE_WAIT));
        }
        Ok(())
    }

    fn link_for(&mut self, client: SocketAddr, now: Instant) -> io::Result<()> {
        if let Some(link) = self.links.iter_mut().find(|link| link.client == client) {
            link.last_used = now;
            return Ok(());
        }
        let local: SocketAddr = if self.server.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        };
        let upstream = UdpSocket::bind(local)?;
        upstream.connect(self.server)?;
        upstream.set_nonblocking(true)?;
        self.links.push(Link {
            client,
            upstream,
            last_used: now,
        });
        Ok(())
    }

    fn schedule(&mut self, to: Destination, data: Vec<u8>, now: Instant) {
        if let Some(at) = self.impairment.deliver_at(&mut self.random, now) {
            self.order += 1;
            self.queue.push(Reverse(Pending {
                at,
                order: self.order,
                to,
                data,
            }));
        }
    }

    fn deliver_due(&mut self, now: Instant) {
        while self
            .queue
            .peek()
            .is_some_and(|Reverse(pending)| pending.at <= now)
        {
            let Some(Reverse(pending)) = self.queue.pop() else {
                break;
            };
            // A lost send is just more loss, which is what this proxy is for.
            let _ = match pending.to {
                Destination::Client(client) => self.listen.send_to(&pending.data, client),
                Destination::Server(client) => match self.links.iter().find(|l| l.client == client)
                {
                    Some(link) => link.upstream.send(&pending.data),
                    None => continue,
                },
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Impairment, Random};
    use std::time::{Duration, Instant};

    #[test]
    fn loss_matches_the_chosen_rate() {
        let impairment = Impairment {
            loss: 0.3,
            ..Impairment::default()
        };
        let mut random = Random::new(1);
        let now = Instant::now();
        let lost = (0..10_000)
            .filter(|_| impairment.deliver_at(&mut random, now).is_none())
            .count();
        assert!((2_800..3_200).contains(&lost), "{lost} of 10000 lost");
    }

    #[test]
    fn delays_stay_in_range_and_jitter_reorders() {
        let impairment = Impairment {
            delay: Duration::from_millis(50),
            jitter: Duration::from_millis(20),
            loss: 0.0,
        };
        let mut random = Random::new(2);
        let start = Instant::now();
        // One packet each millisecond, as a 30 Hz stream from several players would be.
        let arrivals: Vec<Instant> = (0..1_000)
            .filter_map(|ms| impairment.deliver_at(&mut random, start + Duration::from_millis(ms)))
            .collect();

        for (ms, at) in (0..).zip(&arrivals) {
            let delay = at.saturating_duration_since(start + Duration::from_millis(ms));
            assert!(delay >= Duration::from_millis(50) && delay < Duration::from_millis(70));
        }
        assert!(arrivals.windows(2).any(|pair| pair.get(1) < pair.first()));
    }

    #[test]
    fn no_impairment_delivers_everything_at_once() {
        let mut random = Random::new(3);
        let now = Instant::now();
        assert!((0..100).all(|_| Impairment::default().deliver_at(&mut random, now) == Some(now)));
    }
}
