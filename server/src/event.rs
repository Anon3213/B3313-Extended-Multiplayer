//! What a session reports of itself: each thing worth logging, and the line the server's owner
//! reads for it.

use crate::movement::{MovementReport, Verdict};
use b3313_protocol::metrics::{LinkReport, Timings};
use b3313_protocol::{HitKind, Message, Name, PlayerId, Role};
use std::fmt;
use std::net::{IpAddr, SocketAddr};

/// Why the server refused a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitRejection {
    /// The target is the attacker, or nobody is in that slot.
    NoSuchTarget,
    /// One of the two has sent no state yet.
    NotPlaying,
    /// The target's game is paused, at a menu or between two areas: no state came from it
    /// lately.
    Paused,
    /// They are in different levels or areas, or too far apart.
    OutOfReach,
    /// The attacker hit this target less than `Config::min_hit_interval` ago.
    TooSoon,
}

/// The server's ticks over one reporting period.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TickReport {
    /// How long each took to build and send.
    pub took: Timings,
    /// How long after its scheduled time each started.
    pub late: Timings,
    /// Packets dropped by the rate limits.
    pub rate_limited: u64,
}

impl fmt::Display for TickReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let micros = |timings: &Timings| {
            let mean = timings.mean().unwrap_or_default().as_micros();
            format!("{mean} µs on average, {} at most", timings.max.as_micros())
        };
        write!(
            f,
            "{} ticks; each took {} and started {} late; {} packets over the rate limits",
            self.took.count,
            micros(&self.took),
            micros(&self.late),
            self.rate_limited,
        )
    }
}

/// Something worth logging that the session did.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Joined(PlayerId, SocketAddr),
    Left(PlayerId),
    TimedOut(PlayerId),
    WorldSet(PlayerId),
    /// A player's link over the last `REPORT_INTERVAL`. Boxed: it is far larger than the others.
    Link(PlayerId, Box<LinkReport>),
    Ticks(TickReport),
    Hit {
        attacker: PlayerId,
        target: PlayerId,
        kind: HitKind,
    },
    HitRejected {
        attacker: PlayerId,
        target: PlayerId,
        reason: HitRejection,
    },
    /// A player was removed: by the server's owner, the host or a moderator.
    Kicked(PlayerId),
    /// A player's IP address was banned, by any of those.
    Banned(PlayerId, IpAddr),
    /// The server's owner banned an IP address by itself; anyone playing from it was kicked.
    AddressBanned(IpAddr),
    Unbanned(IpAddr),
    /// Everyone in the session, as `list` asked.
    Players(Vec<(PlayerId, SocketAddr)>),
    /// A console command that could not be carried out, and why.
    CommandFailed(String),
    /// A player's movement over the last `REPORT_INTERVAL`.
    Movement(PlayerId, MovementReport),
    /// A player moved unusually; logged at most every two seconds per player.
    Flagged(PlayerId, Verdict),
    /// A player got this much progress, such as stars, which every game now adds to its save.
    ProgressAdded(PlayerId, u32),
    /// A player reported this much new progress at once, more than anyone could have got; the
    /// session did not take it.
    ProgressRefused(PlayerId, u32),
    /// A player reported collecting coins faster than play allows; those did not count for
    /// everyone. Logged at most every two seconds per player.
    CoinsRefused(PlayerId),
    /// A player has the name they are listed under.
    Named(PlayerId, Name),
    /// A player became the host or a moderator, or an ordinary player again.
    RoleChanged(PlayerId, Role),
    /// Something said in the chat, a command included: by a player, with their name then, or
    /// by the server's owner, as `SERVER_NAME`.
    Chat {
        from: Option<PlayerId>,
        name: String,
        text: Message,
    },
    /// A bridge gave a host password that is not the server's; its player is an ordinary one.
    HostPasswordWrong(PlayerId),
    /// A bridge gave the host password while the second player named was the host; its player
    /// is an ordinary one.
    HostRefused(PlayerId, PlayerId),
    /// The session was ended, by the host in the chat or the server's owner at the console;
    /// the server stops after this.
    Ended,
}

/// What the server's owner reads: one line, or one line a player for `Players`.
impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Joined(player, from) => write!(f, "Player {} joined from {from}", player.get()),
            Self::Left(player) => write!(f, "Player {} left", player.get()),
            Self::TimedOut(player) => write!(f, "Player {} timed out", player.get()),
            Self::WorldSet(player) => {
                write!(f, "The session's world comes from player {}", player.get())
            }
            Self::Link(player, report) => write!(f, "Player {}: {report}", player.get()),
            Self::Ticks(report) => write!(f, "Server: {report}"),
            Self::Hit {
                attacker,
                target,
                kind,
            } => write!(
                f,
                "Player {} hit player {} ({kind:?})",
                attacker.get(),
                target.get()
            ),
            Self::HitRejected {
                attacker,
                target,
                reason,
            } => write!(
                f,
                "Rejected a hit by player {} on player {}: {reason:?}",
                attacker.get(),
                target.get()
            ),
            Self::Kicked(player) => write!(f, "Player {} was removed", player.get()),
            Self::Banned(player, ip) => write!(f, "Player {} ({ip}) was banned", player.get()),
            Self::AddressBanned(ip) => write!(f, "{ip} was banned"),
            Self::Unbanned(ip) => write!(f, "{ip} may join again"),
            Self::Players(players) if players.is_empty() => write!(f, "Nobody is playing"),
            Self::Players(players) => {
                let lines: Vec<String> = players
                    .iter()
                    .map(|(player, address)| format!("Player {} at {address}", player.get()))
                    .collect();
                write!(f, "{}", lines.join("\n"))
            }
            Self::CommandFailed(message) => write!(f, "{message}"),
            Self::Movement(player, report) => write!(f, "Player {} {report}", player.get()),
            Self::Flagged(player, Verdict::Unusual { speed }) => write!(
                f,
                "FLAG: player {} moved at {speed:.0} units/s; `ban {}` removes them",
                player.get(),
                player.get()
            ),
            Self::Flagged(player, Verdict::Refused { distance }) => write!(
                f,
                "FLAG: player {} jumped {distance:.0} units, over their allowance, and was held \
                 back",
                player.get()
            ),
            // The session flags only unusual movement; this is here for completeness.
            Self::Flagged(player, Verdict::Normal) => {
                write!(f, "Player {} moved normally", player.get())
            }
            Self::ProgressAdded(player, count) => write!(
                f,
                "Player {} got {count} new progress (stars, keys, switches or doors); everyone \
                 shares it",
                player.get()
            ),
            Self::CoinsRefused(_)
            | Self::ProgressRefused(..)
            | Self::HostPasswordWrong(_)
            | Self::HostRefused(..) => self.fmt_refusal(f),
            Self::Named(player, name) => {
                write!(f, "Player {} is {}", player.get(), name.as_str())
            }
            Self::RoleChanged(player, Role::Host) => {
                write!(f, "Player {} is the host", player.get())
            }
            Self::RoleChanged(player, Role::Moderator) => {
                write!(f, "Player {} is now a moderator", player.get())
            }
            Self::RoleChanged(player, Role::Player) => {
                write!(f, "Player {} is an ordinary player again", player.get())
            }
            Self::Chat { name, text, .. } => write!(f, "<{name}> {}", text.as_str()),
            Self::Ended => write!(f, "The session was ended"),
        }
    }
}

impl Event {
    /// Writes the line for something a player asked or reported that the session refused; a
    /// part of `Display`, which is long enough without these.
    fn fmt_refusal(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CoinsRefused(player) => write!(
                f,
                "FLAG: player {} collected coins faster than play allows; those did not count. \
                 `ban {}` removes them",
                player.get(),
                player.get()
            ),
            Self::ProgressRefused(player, count) => write!(
                f,
                "FLAG: player {} reported {count} new progress at once, more than play allows; \
                 refused. `ban {}` removes them",
                player.get(),
                player.get()
            ),
            Self::HostPasswordWrong(player) => write!(
                f,
                "FLAG: player {} gave a wrong host password, and is an ordinary player",
                player.get()
            ),
            Self::HostRefused(player, host) => write!(
                f,
                "FLAG: player {} gave the host password while player {} is the host, and is an \
                 ordinary player; `host {}` makes them the host",
                player.get(),
                host.get(),
                player.get()
            ),
            _ => Ok(()),
        }
    }
}
