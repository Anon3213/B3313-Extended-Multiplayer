//! Every command of the chat in one table: its words, what follows them, and who may give it.
//!
//! The chat reads a line by the table, a session checks a player's role against it, and the
//! console and the panel's Server chat take from it the words they share with the chat. So a
//! new command is a row here, with what carries it out in `Session` and, if the server's
//! owner has it too, in `serve`; the help that names it is checked against the table by a
//! test.

use crate::console::Command;
use crate::people::{ChatCommand, Target};
use b3313_protocol::{PlayerId, Role};

/// Who may give a command in the chat.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum May {
    Anyone,
    /// The host and the moderators.
    Moderators,
    Host,
}

impl May {
    #[must_use]
    pub fn allows(self, role: Role) -> bool {
        match self {
            Self::Anyone => true,
            Self::Moderators => role.moderates(),
            Self::Host => role == Role::Host,
        }
    }
}

/// Who may give a command, and what a player who may not is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    pub may: May,
    pub refusal: &'static str,
}

const fn only(may: May, refusal: &'static str) -> Rule {
    Rule { may, refusal }
}

const ANYONE: Rule = only(May::Anyone, "");
const REMOVERS: Rule = only(
    May::Moderators,
    "Only the host and moderators can remove a player.",
);
const APPOINTER: Rule = only(May::Host, "Only the host can appoint or remove moderators.");

/// What follows a command's word, and the command the line then is: in the chat, and at the
/// console, where the server's owner types the same word if the console has the command.
#[derive(Debug)]
enum Form {
    /// Nothing; anything after the word is ignored.
    Alone {
        chat: fn() -> ChatCommand,
        console: Option<fn() -> Command>,
    },
    /// A player: by name or number in the chat, by number at the console.
    Player {
        chat: fn(Target) -> ChatCommand,
        console: fn(PlayerId) -> Command,
    },
    /// Words of the player's own, or none.
    Text(fn(String) -> ChatCommand),
}

#[derive(Debug)]
struct Word {
    /// The command's word first, then any others that mean the same.
    names: &'static [&'static str],
    rule: Rule,
    form: Form,
}

const fn of_a_player(
    name: &'static [&'static str],
    rule: Rule,
    chat: fn(Target) -> ChatCommand,
    console: fn(PlayerId) -> Command,
) -> Word {
    Word {
        names: name,
        rule,
        form: Form::Player { chat, console },
    }
}

static WORDS: [Word; 10] = [
    Word {
        names: &["help"],
        rule: ANYONE,
        // The console answers anything it does not know with its own help.
        form: Form::Alone {
            chat: || ChatCommand::Help,
            console: None,
        },
    },
    Word {
        names: &["list"],
        rule: ANYONE,
        form: Form::Alone {
            chat: || ChatCommand::List,
            console: Some(|| Command::List),
        },
    },
    of_a_player(&["kick"], REMOVERS, ChatCommand::Kick, Command::Kick),
    of_a_player(&["ban"], REMOVERS, ChatCommand::Ban, Command::Ban),
    of_a_player(&["mod"], APPOINTER, ChatCommand::Mod, Command::Mod),
    of_a_player(&["unmod"], APPOINTER, ChatCommand::Unmod, Command::Unmod),
    of_a_player(
        &["host"],
        only(May::Host, "Only the host can pass the host's role on."),
        ChatCommand::Host,
        Command::Host,
    ),
    Word {
        names: &["color", "colour"],
        rule: ANYONE,
        form: Form::Text(ChatCommand::Color),
    },
    Word {
        names: &["name"],
        rule: ANYONE,
        form: Form::Text(ChatCommand::Name),
    },
    Word {
        names: &["end", "stop"],
        rule: only(May::Host, "Only the host can end the session."),
        form: Form::Alone {
            chat: || ChatCommand::End,
            console: Some(|| Command::End),
        },
    },
];

fn word(name: &str) -> Option<&'static Word> {
    WORDS.iter().find(|word| word.names.contains(&name))
}

/// Reads a chat message that starts with `/`: the command it is, and who may give it.
///
/// # Errors
/// Fails, with a line for the player who typed it, on anything that is not a command.
pub fn read(text: &str) -> Result<(ChatCommand, Rule), String> {
    let text = text.strip_prefix('/').unwrap_or(text);
    let (name, rest) = text.split_once(' ').unwrap_or((text, ""));
    let rest = rest.trim();
    let Some(word) = word(name) else {
        return Err(format!("/{name} is not a command; /help lists them."));
    };
    let command = match word.form {
        Form::Alone { chat, .. } => chat(),
        Form::Player { .. } if rest.is_empty() => {
            return Err(format!(
                "/{name} needs a player: a number from /list, or a name."
            ));
        }
        Form::Player { chat, .. } => chat(
            rest.parse()
                .map_or_else(|_| Target::Name(rest.to_owned()), Target::Number),
        ),
        Form::Text(chat) => chat(rest.to_owned()),
    };
    Ok((command, word.rule))
}

/// A word of the chat as the console takes it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum AtConsole {
    Alone(fn() -> Command),
    Player(fn(PlayerId) -> Command),
}

/// What the console makes of a word it shares with the chat, if it has that command.
pub(crate) fn at_console(name: &str) -> Option<AtConsole> {
    match word(name)?.form {
        Form::Alone { console, .. } => console.map(AtConsole::Alone),
        Form::Player { console, .. } => Some(AtConsole::Player(console)),
        Form::Text(_) => None,
    }
}

/// Whether the command of this word, given without its slash, is followed by a player: one
/// the panel's Server chat may name by name, as the players' chat does.
#[must_use]
pub fn names_a_player(name: &str) -> bool {
    word(name).is_some_and(|word| matches!(word.form, Form::Player { .. }))
}

#[cfg(test)]
mod tests {
    use super::{Form, May, WORDS, names_a_player, read};
    use crate::console::{self, Command};
    use crate::people::{ChatCommand, HOST_HELP, Target};
    use b3313_protocol::Role;

    #[test]
    fn a_commands_other_words_mean_the_same() {
        assert_eq!(read("/colour red"), read("/color red"));
        assert_eq!(read("/stop"), read("/end"));
        let color = read("/color  Dark Green ").map(|(command, _)| command);
        assert_eq!(color, Ok(ChatCommand::Color("Dark Green".to_owned())));
    }

    #[test]
    fn each_command_says_who_may_give_it() {
        let may = |line: &str, role: Role| read(line).is_ok_and(|(_, rule)| rule.may.allows(role));
        for line in ["/help", "/list", "/color"] {
            assert!(may(line, Role::Player), "{line}");
        }
        for line in ["/kick 1", "/ban 1"] {
            assert!(!may(line, Role::Player), "{line}");
            assert!(may(line, Role::Moderator), "{line}");
            assert!(may(line, Role::Host), "{line}");
        }
        for line in ["/mod 1", "/unmod 1", "/host 1", "/end"] {
            assert!(!may(line, Role::Moderator), "{line}");
            assert!(may(line, Role::Host), "{line}");
        }
        // Whoever may not is told why; nobody is refused what anyone may do.
        for word in &WORDS {
            let is_open = word.rule.may == May::Anyone;
            assert_eq!(word.rule.refusal.is_empty(), is_open, "{:?}", word.names);
        }
    }

    #[test]
    fn a_command_that_takes_a_player_takes_one_everywhere() {
        for name in ["kick", "ban", "mod", "unmod", "host"] {
            assert!(names_a_player(name), "{name}");
            // In the chat by name or number, and never without one.
            let named = read(&format!("/{name} Luigi the Second")).map(|(command, _)| command);
            assert!(format!("{named:?}").contains("Luigi the Second"), "{name}");
            let numbered = format!("{:?}", read(&format!("/{name} 3")));
            assert!(numbered.contains(&format!("{:?}", Target::Number(3))));
            assert!(read(&format!("/{name}")).is_err_and(|why| why.contains("needs a player")));
            // At the console by number.
            assert!(
                console::Command::parse(&format!("{name} 3")).is_ok(),
                "{name}"
            );
        }
        for name in ["list", "end", "color", "help", "say", "dance", ""] {
            assert!(!names_a_player(name), "{name}");
        }
    }

    #[test]
    fn the_console_has_the_chats_words_but_for_help_and_color() {
        assert_eq!(Command::parse("list"), Ok(Command::List));
        assert_eq!(Command::parse("stop"), Ok(Command::End));
        // Its own help answers these, as anything it does not know.
        for line in ["help", "color red", "list 3", "kick", "kick 1 2"] {
            assert_eq!(
                Command::parse(line),
                Err(console::HELP.to_owned()),
                "{line}"
            );
        }
    }

    #[test]
    fn the_help_names_every_command_there_is() {
        let host_help = HOST_HELP.join(" ");
        for word in &WORDS {
            let Some(&name) = word.names.first() else {
                unreachable!("every command has a word");
            };
            // The help is not listed in itself.
            if name != "help" {
                assert!(host_help.contains(&format!("/{name}")), "/{name}");
            }
            let at_console = match word.form {
                Form::Alone { console, .. } => console.is_some(),
                Form::Player { .. } => true,
                Form::Text(_) => false,
            };
            if at_console {
                assert!(console::HELP.contains(name), "{name}");
            }
        }
    }
}
