//! Commands the server's owner types into its console.

use crate::SERVER_NAME;
use b3313_protocol::{MAX_MESSAGE_LENGTH, Message, PlayerId};
use std::net::IpAddr;

/// What the console's `help` prints.
pub const HELP: &str = "Commands: list | kick <player> | ban <player or IP address> | \
                        unban <IP address> | mod <player> | unmod <player> | host <player> | \
                        say <message> | end";

/// The longest message `say` takes: a line of the chat, less the mark before it that tells
/// the players it is the server's.
const MOST_SAID: usize = MAX_MESSAGE_LENGTH - "[] ".len() - SERVER_NAME.len();

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show who is in the session, with their numbers and addresses.
    List,
    /// Remove a player; they may join again.
    Kick(PlayerId),
    /// Remove a player and refuse their IP address from now on.
    Ban(PlayerId),
    /// Refuse an IP address from now on, removing anyone playing from it.
    BanAddress(IpAddr),
    /// Let a banned IP address join again.
    Unban(IpAddr),
    /// Make a player a moderator, who may kick and ban from the chat.
    Mod(PlayerId),
    /// Make a moderator an ordinary player again.
    Unmod(PlayerId),
    /// Make a player the host; whoever was is an ordinary player again.
    Host(PlayerId),
    /// Say something to everyone in the chat, as the server.
    Say(Message),
    /// End the session: every player is told it has ended, and the server stops.
    End,
    /// Stop the server to start it again: every player's bridge is told to join again by
    /// itself. Not a console command, as a console's server has nobody to start it again; the
    /// panel gives it.
    Restart,
}

impl Command {
    /// Reads one console line, such as `kick 3`. A slash before the command, as the chat's
    /// commands have, is allowed, so nobody need remember which place wants which.
    ///
    /// # Errors
    /// Fails, with a message for whoever typed it, on anything that is not a command.
    pub fn parse(line: &str) -> Result<Self, String> {
        let line = line.trim_start();
        let line = line.strip_prefix('/').unwrap_or(line);
        let words: Vec<&str> = line.split_whitespace().collect();
        let player = |word: &str| {
            word.parse()
                .ok()
                .and_then(PlayerId::new)
                .ok_or_else(|| format!("{word} is not a player number; `list` shows them"))
        };
        match words.as_slice() {
            ["list"] => Ok(Self::List),
            ["end" | "stop"] => Ok(Self::End),
            ["kick", number] => Ok(Self::Kick(player(number)?)),
            ["mod", number] => Ok(Self::Mod(player(number)?)),
            ["unmod", number] => Ok(Self::Unmod(player(number)?)),
            ["host", number] => Ok(Self::Host(player(number)?)),
            // The message as typed, with its own spacing, after the word.
            ["say", _, ..] => {
                let said = line.trim_start().strip_prefix("say").unwrap_or_default();
                let said = said.trim();
                // Said so here, as the chat would cut the line's end off without a word.
                if said.chars().count() > MOST_SAID {
                    return Err(format!(
                        "say takes at most {MOST_SAID} characters, to fit a line of the chat"
                    ));
                }
                Message::new(said)
                    .map(Self::Say)
                    .ok_or_else(|| "say needs a message in plain letters".to_owned())
            }
            // No IP address is a bare number, so the two cannot be confused.
            ["ban", target] => match target.parse() {
                Ok(address) => Ok(Self::BanAddress(address)),
                Err(_) => player(target)
                    .map(Self::Ban)
                    .map_err(|_| format!("{target} is not a player number or an IP address")),
            },
            ["unban", address] => address
                .parse()
                .map(Self::Unban)
                .map_err(|_| format!("{address} is not an IP address")),
            _ => Err(HELP.to_owned()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, HELP};
    use b3313_protocol::{Message, PlayerId};
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    fn id(value: u8) -> PlayerId {
        PlayerId::new(value).unwrap_or_else(|| unreachable!())
    }

    #[test]
    fn commands_are_read_with_their_arguments() {
        assert_eq!(Command::parse("list"), Ok(Command::List));
        assert_eq!(Command::parse("  kick   3 "), Ok(Command::Kick(id(3))));
        assert_eq!(Command::parse("ban 15"), Ok(Command::Ban(id(15))));
        let address = IpAddr::from(Ipv4Addr::new(192, 168, 1, 20));
        let unban = Command::parse("unban 192.168.1.20");
        assert_eq!(unban, Ok(Command::Unban(address)));
        let ban = Command::parse("ban 192.168.1.20");
        assert_eq!(ban, Ok(Command::BanAddress(address)));
        let ipv6 = Command::parse("ban ::1").ok();
        assert_eq!(ipv6, Some(Command::BanAddress(Ipv6Addr::LOCALHOST.into())));
        assert_eq!(Command::parse("mod 2"), Ok(Command::Mod(id(2))));
        assert_eq!(Command::parse("unmod 2"), Ok(Command::Unmod(id(2))));
        assert_eq!(Command::parse("host 2"), Ok(Command::Host(id(2))));
        let said = Command::parse(" say  back in  five ");
        assert_eq!(
            said,
            Message::new("back in  five")
                .map(Command::Say)
                .ok_or_else(String::new)
        );
        assert!(Command::parse("say").is_err());
        assert!(Command::parse("/ say hello").is_ok());
        let fits = format!("say {}", "a".repeat(super::MOST_SAID));
        assert!(Command::parse(&fits).is_ok());
        assert!(Command::parse(&format!("{fits}a")).is_err());
        assert_eq!(Command::parse("end"), Ok(Command::End));
        assert_eq!(Command::parse("stop"), Ok(Command::End));
    }

    #[test]
    fn a_command_may_start_with_the_chats_slash() {
        assert_eq!(Command::parse("/end"), Ok(Command::End));
        assert_eq!(Command::parse(" /kick 3"), Ok(Command::Kick(id(3))));
        let said = Command::parse("/say hello");
        assert_eq!(
            said,
            Message::new("hello")
                .map(Command::Say)
                .ok_or_else(String::new)
        );
        // Only one, and only before the command: this is no command.
        assert_eq!(Command::parse("//end"), Err(HELP.to_owned()));
    }

    #[test]
    fn mistakes_explain_themselves() {
        assert!(Command::parse("kick 16").is_err_and(|e| e.contains("not a player")));
        assert!(Command::parse("kick three").is_err_and(|e| e.contains("not a player")));
        assert!(Command::parse("ban 1.2.3").is_err_and(|e| e.contains("or an IP")));
        assert!(Command::parse("unban nowhere").is_err_and(|e| e.contains("not an IP")));
        assert_eq!(Command::parse("help"), Err(HELP.to_owned()));
        assert_eq!(Command::parse(""), Err(HELP.to_owned()));
    }
}
