//! Streamer mode: what the panel shows in place of details its user would not want on a stream or
//! a shared screen. IP addresses are hidden entirely, keeping their port, which identifies no
//! one but tells players apart, except this PC's own and "every network", the same everywhere;
//! so are the names of servers the player joins, which lead to their owner's IP; paths lose the
//! user's folder, which names them.

use std::net::{IpAddr, SocketAddr};

/// An IPv4 address's place in text, hidden.
const HIDDEN_V4: &str = "•••.•••.•••.•••";
/// An IPv6 address's, hidden; its length gives nothing away either.
const HIDDEN_V6: &str = "••••:••••:••••:••••";
/// A server's host name, hidden; the same length whatever the name.
const HIDDEN_HOST: &str = "••••••••";

/// Whether an address identifies no one: this computer (127.0.0.1) or every network (0.0.0.0)
/// is the same on every PC, and hiding it would only make the panel's explanations useless.
fn reveals_nothing(ip: IpAddr) -> bool {
    ip.is_loopback() || ip.is_unspecified()
}

#[must_use]
pub fn hide_ip(ip: IpAddr) -> String {
    match ip {
        ip if reveals_nothing(ip) => ip.to_string(),
        IpAddr::V4(_) => HIDDEN_V4.to_owned(),
        IpAddr::V6(_) => HIDDEN_V6.to_owned(),
    }
}

/// An address with its IP hidden and its port kept, as `•••.•••.•••.•••:41313`.
#[must_use]
pub fn hide_address(address: SocketAddr) -> String {
    match address {
        address if reveals_nothing(address.ip()) => address.to_string(),
        SocketAddr::V4(_) => format!("{HIDDEN_V4}:{}", address.port()),
        SocketAddr::V6(_) => format!("[{HIDDEN_V6}]:{}", address.port()),
    }
}

/// A server's address as typed, such as `myserver.duckdns.org:41313`, hidden with its port
/// kept: an IP as `hide_address` hides it, a host name as dots. A dynamic DNS name leads
/// straight to its owner's IP, so it is hidden as an IP is. `localhost` is this PC on every PC.
#[must_use]
pub fn hide_server(address: &str) -> String {
    let address = address.trim();
    if let Ok(socket) = address.parse::<SocketAddr>() {
        return hide_address(socket);
    }
    if let Ok(ip) = address.parse::<IpAddr>() {
        return hide_ip(ip);
    }
    let (host, port) = split_port(address);
    if host.eq_ignore_ascii_case("localhost") {
        return address.to_owned();
    }
    match port {
        Some(port) => format!("{HIDDEN_HOST}:{port}"),
        None => HIDDEN_HOST.to_owned(),
    }
}

/// `text` with each of `servers`, addresses as typed, hidden wherever it appears, whole or as
/// its host name alone, as in "could not find myserver.duckdns.org". IP addresses are left to
/// `hide_in_text`, which finds any.
#[must_use]
pub fn hide_servers<'a>(text: &str, servers: impl IntoIterator<Item = &'a str>) -> String {
    let mut hidden = text.to_owned();
    for server in servers {
        let server = server.trim();
        let (host, _) = split_port(server);
        let named = !host.is_empty()
            && !host.eq_ignore_ascii_case("localhost")
            && host.trim_matches(['[', ']']).parse::<IpAddr>().is_err();
        if named && hidden.contains(host) {
            // The whole address first, so its port stays with it.
            hidden = hidden
                .replace(server, &hide_server(server))
                .replace(host, HIDDEN_HOST);
        }
    }
    hidden
}

/// An address's host and its port, if it ends in one.
fn split_port(address: &str) -> (&str, Option<u16>) {
    match address.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') || host.ends_with(']') => match port.parse() {
            Ok(port) => (host, Some(port)),
            Err(_) => (address, None),
        },
        _ => (address, None),
    }
}

/// `text` with every IP address in it hidden, alone or with a port, and the user's folder
/// (`home`, such as `C:\Users\Name`) shown as `%USERPROFILE%`.
#[must_use]
pub fn hide_in_text(text: &str, home: Option<&str>) -> String {
    let text = match home.filter(|home| !home.is_empty()) {
        Some(home) => text.replace(home, "%USERPROFILE%"),
        None => text.to_owned(),
    };
    let mut hidden = String::with_capacity(text.len());
    let mut word = String::new();
    // Whether the run of address characters so far follows a letter, digit or underscore, so
    // is part of a longer word.
    let mut glued = false;
    let mut previous = ' ';
    for character in text.chars() {
        if is_address_character(character) {
            if word.is_empty() {
                glued = is_word_character(previous);
            }
            word.push(character);
        } else {
            if glued || is_word_character(character) {
                hidden.push_str(&word);
            } else {
                hidden.push_str(&hide_word(&word));
            }
            word.clear();
            hidden.push(character);
        }
        previous = character;
    }
    if glued {
        hidden.push_str(&word);
    } else {
        hidden.push_str(&hide_word(&word));
    }
    hidden
}

/// Characters that make up words, which an address never runs into.
fn is_word_character(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Characters an address, with its port, can be written with.
fn is_address_character(character: char) -> bool {
    character.is_ascii_hexdigit() || matches!(character, '.' | ':' | '[' | ']')
}

/// A run of address characters, hidden if it is an address. A sentence's full stop or colon
/// after it stays as it was.
fn hide_word(word: &str) -> String {
    let trimmed = word.trim_end_matches(['.', ':']);
    let ending = word.get(trimmed.len()..).unwrap_or_default();
    let hidden = if let Ok(address) = trimmed.parse::<SocketAddr>() {
        hide_address(address)
    } else if let Ok(ip) = trimmed.parse::<IpAddr>() {
        hide_ip(ip)
    } else {
        return word.to_owned();
    };
    format!("{hidden}{ending}")
}

#[cfg(test)]
mod tests {
    use super::{hide_address, hide_in_text, hide_ip, hide_server, hide_servers};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

    #[test]
    fn addresses_lose_their_ip_and_keep_their_port() {
        let v4: SocketAddr = (Ipv4Addr::new(203, 0, 113, 9), 41313).into();
        assert_eq!(hide_address(v4), "•••.•••.•••.•••:41313");
        let v6: SocketAddr = (Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 1), 5000).into();
        assert_eq!(hide_address(v6), "[••••:••••:••••:••••]:5000");
        assert_eq!(
            hide_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))),
            "•••.•••.•••.•••"
        );
    }

    #[test]
    fn this_pc_and_every_network_stay_shown() {
        let local: SocketAddr = (Ipv4Addr::LOCALHOST, 41313).into();
        assert_eq!(hide_address(local), "127.0.0.1:41313");
        assert_eq!(hide_ip(IpAddr::V4(Ipv4Addr::UNSPECIFIED)), "0.0.0.0");
        assert_eq!(hide_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)), "::1");
        assert_eq!(
            hide_in_text("listen on 0.0.0.0, not 203.0.113.9", None),
            "listen on 0.0.0.0, not •••.•••.•••.•••"
        );
    }

    #[test]
    fn addresses_in_text_are_hidden_and_the_rest_kept() {
        let hidden = |text| hide_in_text(text, None);
        assert_eq!(
            hidden("Player 3 joined from 192.168.1.20:59455"),
            "Player 3 joined from •••.•••.•••.•••:59455"
        );
        assert_eq!(
            hidden("203.0.113.9 was banned."),
            "•••.•••.•••.••• was banned."
        );
        assert_eq!(
            hidden("from [2001:db8::1]:5000, then"),
            "from [••••:••••:••••:••••]:5000, then"
        );
        // Times, versions, sizes and plain numbers are not addresses.
        assert_eq!(
            hidden("13:25:03, v1.2.3, 2.1 KB/s, 41313"),
            "13:25:03, v1.2.3, 2.1 KB/s, 41313"
        );
        // Hex-looking words stay too: "add" and "bed" are not addresses.
        assert_eq!(hidden("add a bed"), "add a bed");
        // Nor is part of a word, such as "d::bac" in a Rust path, though it reads as IPv6.
        assert_eq!(
            hidden("std::backtrace::Backtrace, fe::cafe_x"),
            "std::backtrace::Backtrace, fe::cafe_x"
        );
    }

    #[test]
    fn known_servers_are_hidden_by_name_keeping_their_port() {
        let servers = [
            "myserver.duckdns.org:41313",
            "gamingpc",
            "203.0.113.9:41313",
        ];
        assert_eq!(
            hide_servers(
                "Reaching myserver.duckdns.org:41313… could not find myserver.duckdns.org",
                servers
            ),
            "Reaching ••••••••:41313… could not find ••••••••"
        );
        assert_eq!(hide_servers("joined gamingpc", servers), "joined ••••••••");
        // IPs are left to `hide_in_text`, which keeps this PC's own showing.
        assert_eq!(
            hide_servers("203.0.113.9:41313 and localhost:41313", servers),
            "203.0.113.9:41313 and localhost:41313"
        );
        assert_eq!(hide_server("myserver.duckdns.org:41313"), "••••••••:41313");
        assert_eq!(
            hide_server("[2001:db8::1]:5000"),
            "[••••:••••:••••:••••]:5000"
        );
        assert_eq!(hide_server("localhost:41313"), "localhost:41313");
        assert_eq!(hide_server("gamingpc"), "••••••••");
    }

    #[test]
    fn the_users_folder_is_named_by_its_variable() {
        let text = r"could not save C:\Users\Name\bans.txt";
        assert_eq!(
            hide_in_text(text, Some(r"C:\Users\Name")),
            r"could not save %USERPROFILE%\bans.txt"
        );
        assert_eq!(hide_in_text(text, None), text);
    }
}
