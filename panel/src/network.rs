//! Which address to give players, and the invitation that tells them how to join.

use b3313_protocol::secure::{PublicKey, join_code};
use std::fmt::Write as _;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};

/// This PC's address on its local network: the one it would reach the internet from.
/// Connecting a UDP socket sends nothing; it only has the system choose a route, and so the
/// local address on it.
#[must_use]
pub fn local_network_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    // A documentation address (TEST-NET-1): routed like any other, never answered.
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    let ip = socket.local_addr().ok()?.ip();
    (!ip.is_unspecified() && !ip.is_loopback()).then_some(ip)
}

/// Who can reach a server listening at an address, and the address to give them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    /// Listening on 127.0.0.1: only bridges on this PC can join.
    ThisPc(SocketAddr),
    /// Listening on every network, or one in particular: this is the address on it.
    Network(SocketAddr),
    /// Listening on every network, but this PC's address on them is unknown.
    Unknown { port: u16 },
}

#[must_use]
pub fn reach(listening: SocketAddr, local: Option<IpAddr>) -> Reach {
    let ip = listening.ip();
    if ip.is_loopback() {
        Reach::ThisPc(listening)
    } else if ip.is_unspecified() {
        local.map_or(
            Reach::Unknown {
                port: listening.port(),
            },
            |local| Reach::Network(SocketAddr::new(local, listening.port())),
        )
    } else {
        Reach::Network(listening)
    }
}

/// How much the server's owner copies for players, from least to most.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invite {
    /// The address alone, to paste into a command line or post where others can see.
    Address,
    /// The address and the password, for players who know how to join.
    WithPassword,
    /// Everything a new player needs: the address, the password and how to join.
    Full,
}

impl Invite {
    pub const ALL: [Self; 3] = [Self::Address, Self::WithPassword, Self::Full];

    /// What goes on the clipboard, or none when there is nothing this adds: the password
    /// alone with the address, when there is no password. `server` is the server's join
    /// code, its address and its key (`server_to_give`): with the key, a player's panel joins
    /// this server and nobody else who answers at the address.
    #[must_use]
    pub fn text(self, server: &str, password: &str) -> Option<String> {
        match self {
            Self::Address => Some(server.to_owned()),
            Self::WithPassword if password.is_empty() => None,
            Self::WithPassword => {
                Some(format!("Server address: {server}\nPassword: {password}\n"))
            }
            Self::Full => Some(full_invitation(server, password)),
        }
    }

    /// Whether what it copies holds the password, which only players should have.
    #[must_use]
    pub fn shares_password(self, password: &str) -> bool {
        !password.is_empty() && self != Self::Address
    }
}

/// How a player joins with an invite, the same in each.
const HOW_TO_JOIN: &str = "To join, copy this message, open Play in the B3313 Extended Multiplayer \
                           control panel, and select Paste invite.\n";

/// What an invite gives as the server at `address` whose key is `key`: its join code.
#[must_use]
pub fn server_to_give(address: SocketAddr, key: &PublicKey) -> String {
    join_code(&address.to_string(), key)
}

fn full_invitation(server: &str, password: &str) -> String {
    let mut text = format!("Join my B3313 multiplayer session.\nServer address: {server}\n");
    if password.is_empty() {
        text.push_str("No password is needed.\n");
        text.push_str(HOW_TO_JOIN);
    } else {
        // Writing to a String cannot fail.
        let _ = writeln!(text, "Password: {password}");
        text.push_str(HOW_TO_JOIN);
        text.push_str(
            "With b3313-bridge instead, give it the address, with the password in the \
             B3313_PASSWORD environment variable.\n",
        );
    }
    text
}

/// What an invite tells a player: the server's address, with its key if the invite had it,
/// and its password if it has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invitation {
    pub address: String,
    pub password: Option<String>,
}

/// Reads any of the three invites, or an address on its own, as pasted from a message. None if
/// it holds no address.
#[must_use]
pub fn read_invite(text: &str) -> Option<Invitation> {
    let field = |name: &str| {
        text.lines().find_map(|line| {
            let line = line.trim();
            let (label, value) = line.split_once(':')?;
            (label.trim().eq_ignore_ascii_case(name) && !value.trim().is_empty())
                .then(|| value.trim().to_owned())
        })
    };
    let address = field("Server address").or_else(|| {
        // An address alone: one word, as host:port or [v6]:port.
        let text = text.trim();
        (!text.is_empty() && !text.contains(char::is_whitespace)).then(|| text.to_owned())
    })?;
    Some(Invitation {
        address,
        password: field("Password"),
    })
}

#[cfg(test)]
mod tests {
    use super::{Invitation, Invite, Reach, reach, read_invite, server_to_give};
    use b3313_protocol::secure::{SecureError, ServerKey, split_join_code};
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};

    #[test]
    fn the_address_to_give_depends_on_where_the_server_listens() {
        let lan = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20));
        let loopback: SocketAddr = (Ipv4Addr::LOCALHOST, 41313).into();
        let every: SocketAddr = (Ipv4Addr::UNSPECIFIED, 41313).into();
        let one: SocketAddr = (Ipv4Addr::new(10, 0, 0, 5), 41313).into();
        assert_eq!(reach(loopback, Some(lan)), Reach::ThisPc(loopback));
        assert_eq!(
            reach(every, Some(lan)),
            Reach::Network(SocketAddr::new(lan, 41313))
        );
        assert_eq!(reach(every, None), Reach::Unknown { port: 41313 });
        assert_eq!(reach(one, Some(lan)), Reach::Network(one));
    }

    #[test]
    fn each_invite_adds_to_the_one_before() {
        let address: SocketAddr = (Ipv4Addr::new(192, 168, 1, 20), 41313).into();
        let address = address.to_string();
        let text = |invite: Invite, password| invite.text(&address, password).unwrap_or_default();

        assert_eq!(text(Invite::Address, "hunter2"), "192.168.1.20:41313");
        assert_eq!(
            text(Invite::WithPassword, "hunter2"),
            "Server address: 192.168.1.20:41313\nPassword: hunter2\n"
        );
        let full = text(Invite::Full, "hunter2");
        assert!(full.contains("Password: hunter2"));
        assert!(full.contains("Paste invite"));
        assert!(full.contains("B3313_PASSWORD"));

        // Without a password, the middle one adds nothing, and the full one says so.
        assert_eq!(Invite::WithPassword.text(&address, ""), None);
        let open = text(Invite::Full, "");
        assert!(open.contains("No password"));
        assert!(!open.contains("B3313_PASSWORD"));
    }

    #[test]
    fn every_invite_reads_back_as_it_was_written() {
        let address = "192.168.1.20:41313";
        let read = |invite: Invite, password: &str| {
            invite
                .text(address, password)
                .and_then(|text| read_invite(&text))
        };
        let with = Some(Invitation {
            address: "192.168.1.20:41313".to_owned(),
            password: Some("hunter2".to_owned()),
        });
        assert_eq!(read(Invite::Full, "hunter2"), with);
        assert_eq!(read(Invite::WithPassword, "hunter2"), with);
        let without = Some(Invitation {
            address: "192.168.1.20:41313".to_owned(),
            password: None,
        });
        assert_eq!(read(Invite::Address, "hunter2"), without);
        assert_eq!(read(Invite::Full, ""), without);
        // As pasted from a chat, with spaces and odd capitals; and nothing to read.
        assert_eq!(
            read_invite("  server ADDRESS:  example.net:41313 \r\n password: a b c\r\n"),
            Some(Invitation {
                address: "example.net:41313".to_owned(),
                password: Some("a b c".to_owned()),
            })
        );
        assert_eq!(read_invite("hello there"), None);
        assert_eq!(read_invite("   "), None);
    }

    #[test]
    fn every_invite_carries_the_servers_key_to_the_player() -> Result<(), SecureError> {
        let address: SocketAddr = (Ipv4Addr::new(192, 168, 1, 20), 41313).into();
        let key = ServerKey::generate()?.public();
        let server = server_to_give(address, &key);
        for invite in Invite::ALL {
            let read = invite
                .text(&server, "hunter2")
                .and_then(|text| read_invite(&text))
                .map(|invitation| invitation.address);
            let read = read.as_deref().map(split_join_code).transpose()?;
            assert_eq!(read, Some(("192.168.1.20:41313", Some(key))), "{invite:?}");
        }
        Ok(())
    }

    #[test]
    fn only_invites_with_a_set_password_share_it() {
        assert!(!Invite::Address.shares_password("hunter2"));
        assert!(Invite::WithPassword.shares_password("hunter2"));
        assert!(Invite::Full.shares_password("hunter2"));
        assert!(!Invite::Full.shares_password(""));
    }
}
