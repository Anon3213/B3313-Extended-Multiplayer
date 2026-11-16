//! Server settings, read from a file of `key = value` lines so they change without a rebuild.
//!
//! Every setting has a safe default, and every value is checked against a range, so a typo fails
//! at startup instead of weakening the server. `server/server.example.conf` lists them all.

use b3313_protocol::MAX_HOST_PASSWORD_LENGTH;
use std::fmt;
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::Duration;

/// One line of a config file, as `Config::format` writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub key: &'static str,
    /// What the setting does, with its range; `\n` breaks the comment written above it.
    pub help: &'static str,
}

/// Every setting, in the order `Config::format` writes them.
pub const SETTINGS: [Setting; 12] = [
    Setting {
        key: "max_players",
        help: "Players admitted at once (1-16).",
    },
    Setting {
        key: "player_timeout_seconds",
        help: "Seconds of silence before a player counts as gone (1-60).",
    },
    Setting {
        key: "max_hit_distance",
        help: "How far apart two players may be for a hit to count, by their latest positions \
               (50-5000).",
    },
    Setting {
        key: "min_hit_interval_ms",
        help: "Least milliseconds between two hits by one player on another (0-2000).",
    },
    Setting {
        key: "flag_speed",
        help: "Movement faster than this, in units per second within one level and area, is \
               logged as unusual\nfor the server's owner but still allowed (100-1000000). A \
               generous guess, not a measurement.",
    },
    Setting {
        key: "jump_distance",
        help: "A move longer than this between two states counts as a jump, as warps and pipes \
               make (100-1000000).",
    },
    Setting {
        key: "jumps_per_minute",
        help: "Jumps a player may make in a minute; more are logged and not passed on to others \
               (1-10000).",
    },
    Setting {
        key: "packets_per_second",
        help: "Packets per second taken from any one address; more are dropped (40-1000).",
    },
    Setting {
        key: "joins_per_second",
        help: "Join attempts per second taken from all new addresses together (1-1000).",
    },
    Setting {
        key: "password",
        help: "The session password, which players type to join, or give a bridge run by hand as\n\
               B3313_PASSWORD. Empty lets anyone join; traffic is encrypted either way. It cannot \
               contain #,\nwhich starts a comment.",
    },
    Setting {
        key: "host_password",
        help: "The host's own password: the first player whose bridge gives it is the host, who \
               may appoint\nmoderators, pass the role on and end the session from the chat. The \
               control panel makes one up\nfor whoever hosts and joins from it; a bridge run by \
               hand gives it as B3313_HOST_PASSWORD.\nEmpty, a server run without the panel has \
               no host until its owner names one. At most 64 plain\ncharacters, and it cannot \
               contain #.",
    },
    Setting {
        key: "ban_file",
        help: "A file of banned IP addresses, one a line, kept up to date as players are banned \
               and unbanned.\nEmpty keeps bans only until the server stops, or the control \
               panel that runs it closes.",
    },
];

const FILE_HEADER: &str = "# Settings for b3313-server, read with: b3313-server <address:port> \
--config <this file>\n# Every line is optional; a setting left out keeps its default. Values \
outside the ranges shown stop\n# the server.\n";

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Players admitted at once, up to the protocol's 16.
    pub max_players: u8,
    /// A player who sends nothing for this long has left without saying so.
    pub player_timeout: Duration,
    /// How far apart, by their latest states, two players may be for a hit between them to count.
    pub max_hit_distance: f32,
    /// The least time between two accepted hits by one player on another.
    pub min_hit_interval: Duration,
    /// Movement within one level and area faster than this, in units per second, is logged as
    /// unusual. Still passed on: it is a flag for the server's owner, not a limit.
    pub flag_speed: f32,
    /// A move longer than this between two states is a jump, as a warp or a pipe makes.
    pub jump_distance: f32,
    /// Jumps a player may make in a minute; more are logged and not passed on.
    pub jumps_per_minute: u32,
    /// Packets per second taken from any one address; more are dropped unread.
    pub packets_per_second: u32,
    /// Join attempts per second taken from all strangers together.
    pub joins_per_second: u32,
    /// The session password. Empty lets anyone join; traffic is encrypted either way.
    pub password: String,
    /// The password that makes the first player whose bridge gives it the host. Empty means no
    /// player is the host until the console names one, and moderators are appointed from the
    /// console only.
    pub host_password: String,
    /// A file of banned IP addresses, one a line, read at startup and kept up to date as
    /// players are banned and unbanned. None keeps bans only until the server stops.
    pub ban_file: Option<PathBuf>,
    /// The addresses banned at startup, read from `ban_file` by the program that runs the server.
    pub bans: Vec<IpAddr>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            max_players: 16,
            player_timeout: Duration::from_secs(5),
            // Each game sees the other player about 150 ms late, in which a running Mario covers
            // about 150 units; a punch reaches under 140.
            max_hit_distance: 500.0,
            // A punch, punch, kick combo lands hits about 200 ms apart.
            min_hit_interval: Duration::from_millis(150),
            // Guesses kept generous, not measurements: Mario's fastest moves, a long jump or a
            // long fall, are a few thousand units a second at this build's fast emulation.
            flag_speed: 8000.0,
            // No running Mario covers this in the 33 ms between two states.
            jump_distance: 1000.0,
            jumps_per_minute: 30,
            // A bridge in a level sends about 61 a second, its state and its objects' thirty
            // times each, and more for a moment while hits, coins or chat wait to be
            // acknowledged; the half second's burst an address is allowed covers those.
            packets_per_second: 120,
            joins_per_second: 20,
            password: String::new(),
            host_password: String::new(),
            ban_file: None,
            bans: Vec::new(),
        }
    }
}

/// What was wrong with a config file, and on which line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "config line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ConfigError {}

/// Parses `value` as a number from `low` to `high`.
fn number<T>(value: &str, low: T, high: T) -> Result<T, String>
where
    T: std::str::FromStr + PartialOrd + fmt::Display + Copy,
{
    let parsed: T = value
        .parse()
        .map_err(|_| format!("{value} is not a number"))?;
    // Not `parsed < low || parsed > high`, which lets through `nan`, that is neither.
    if !(low <= parsed && parsed <= high) {
        return Err(format!("{value} is outside {low} to {high}"));
    }
    Ok(parsed)
}

impl Config {
    /// Reads settings from `text`; anything not set keeps its default.
    ///
    /// # Errors
    /// Fails on an unknown or repeated key, a line without `=`, or a value out of range.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let mut config = Self::default();
        let mut seen: Vec<&str> = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let error = |message: String| ConfigError { line, message };
            let content = raw.split('#').next().unwrap_or_default().trim();
            if content.is_empty() {
                continue;
            }
            let (key, value) = content
                .split_once('=')
                .ok_or_else(|| error("expected key = value".to_owned()))?;
            let (key, value) = (key.trim(), value.trim());
            if seen.contains(&key) {
                return Err(error(format!("{key} is set twice")));
            }
            seen.push(key);
            config.set(key, value).map_err(error)?;
        }
        Ok(config)
    }

    /// Writes every setting with its help as comments, in a file `parse` reads back as `self`.
    /// `bans` is not written: it comes from the ban file.
    #[must_use]
    pub fn format(&self) -> String {
        let mut text = FILE_HEADER.to_owned();
        for setting in SETTINGS {
            text.push('\n');
            for line in setting.help.lines() {
                text.push_str("# ");
                text.push_str(line);
                text.push('\n');
            }
            let value = self.value(setting.key).unwrap_or_default();
            let line = format!("{} = {value}", setting.key);
            text.push_str(line.trim_end());
            text.push('\n');
        }
        text
    }

    /// A setting's value as a config file writes it, or None for an unknown key.
    #[must_use]
    pub fn value(&self, key: &str) -> Option<String> {
        Some(match key {
            "max_players" => self.max_players.to_string(),
            "player_timeout_seconds" => self.player_timeout.as_secs().to_string(),
            "max_hit_distance" => self.max_hit_distance.to_string(),
            "min_hit_interval_ms" => self.min_hit_interval.as_millis().to_string(),
            "flag_speed" => self.flag_speed.to_string(),
            "jump_distance" => self.jump_distance.to_string(),
            "jumps_per_minute" => self.jumps_per_minute.to_string(),
            "packets_per_second" => self.packets_per_second.to_string(),
            "joins_per_second" => self.joins_per_second.to_string(),
            "password" => self.password.clone(),
            "host_password" => self.host_password.clone(),
            "ban_file" => self
                .ban_file
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            _ => return None,
        })
    }

    /// Sets one setting from its text, as a config file line would.
    ///
    /// # Errors
    /// Fails, with a message for the server's owner, on an unknown key or a value that is out
    /// of range or that a config file could not hold: one with `#` or with spaces around it.
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        // A file cuts a value at `#` and trims it, so such a value would not read back the same.
        if value.contains('#') {
            return Err(format!("{key} cannot contain #, which starts a comment"));
        }
        if value.trim() != value {
            return Err(format!("{key} cannot start or end with a space"));
        }
        match key {
            "max_players" => self.max_players = number(value, 1, 16)?,
            "player_timeout_seconds" => {
                self.player_timeout = Duration::from_secs(number(value, 1, 60)?);
            }
            "max_hit_distance" => self.max_hit_distance = number(value, 50.0, 5000.0)?,
            "min_hit_interval_ms" => {
                self.min_hit_interval = Duration::from_millis(number(value, 0, 2000)?);
            }
            "flag_speed" => self.flag_speed = number(value, 100.0, 1_000_000.0)?,
            "jump_distance" => self.jump_distance = number(value, 100.0, 1_000_000.0)?,
            "jumps_per_minute" => self.jumps_per_minute = number(value, 1, 10_000)?,
            "packets_per_second" => self.packets_per_second = number(value, 40, 1000)?,
            "joins_per_second" => self.joins_per_second = number(value, 1, 1000)?,
            // Everything after `=`, trimmed; it cannot contain `#`, which starts a comment.
            "password" => value.clone_into(&mut self.password),
            "host_password" => {
                // What a bridge can send: anything else would never match what one gives.
                let fits = value.len() <= MAX_HOST_PASSWORD_LENGTH
                    && value.bytes().all(|byte| (0x20..=0x7E).contains(&byte));
                if !fits {
                    return Err(format!(
                        "the host password must be at most {MAX_HOST_PASSWORD_LENGTH} plain \
                         characters (letters, digits, punctuation and spaces)"
                    ));
                }
                value.clone_into(&mut self.host_password);
            }
            "ban_file" if !value.is_empty() => self.ban_file = Some(PathBuf::from(value)),
            "ban_file" => self.ban_file = None,
            _ => return Err(format!("unknown setting {key}")),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::time::Duration;

    #[test]
    fn settings_override_defaults_and_comments_are_ignored() {
        let config = Config::parse(
            "# Small private game\n\
             max_players = 4\n\
             \n\
             player_timeout_seconds=10   # slow links\n\
             min_hit_interval_ms = 300\n",
        );
        let expected = Config {
            max_players: 4,
            player_timeout: Duration::from_secs(10),
            min_hit_interval: Duration::from_millis(300),
            ..Config::default()
        };
        assert_eq!(config, Ok(expected));
        assert_eq!(Config::parse(""), Ok(Config::default()));
    }

    #[test]
    fn the_example_file_is_valid_and_shows_the_defaults() {
        let example = include_str!("../server.example.conf");
        assert_eq!(Config::parse(example), Ok(Config::default()));
        // Kept identical, so the example documents every setting `format` knows.
        assert_eq!(example.replace("\r\n", "\n"), Config::default().format());
    }

    #[test]
    fn a_formatted_config_reads_back_with_every_setting_changed() {
        let changed = Config {
            max_players: 3,
            player_timeout: Duration::from_secs(9),
            max_hit_distance: 612.5,
            min_hit_interval: Duration::from_millis(40),
            flag_speed: 9100.0,
            jump_distance: 1500.0,
            jumps_per_minute: 7,
            packets_per_second: 200,
            joins_per_second: 2,
            password: "open sesame".to_owned(),
            host_password: "my own".to_owned(),
            ban_file: Some("bans here.txt".into()),
            bans: Vec::new(),
        };
        assert_eq!(Config::parse(&changed.format()), Ok(changed));
    }

    #[test]
    fn set_refuses_values_a_file_could_not_hold() {
        let mut config = Config::default();
        for (key, value, fragment) in [
            ("password", "a#b", "#"),
            ("password", " padded", "space"),
            ("ban_file", "bans.txt ", "space"),
            ("colour", "red", "unknown"),
        ] {
            let message = config.set(key, value).err().unwrap_or_default();
            assert!(message.contains(fragment), "{key} = {value:?}: {message}");
        }
        assert_eq!(config, Config::default(), "refused values change nothing");
        assert_eq!(config.set("password", "fine"), Ok(()));
        assert_eq!(config.password, "fine");
    }

    #[test]
    fn mistakes_are_reported_with_their_line() {
        for (text, line, fragment) in [
            ("max_players = 17", 1, "outside"),
            ("\nmax_players = many", 2, "not a number"),
            ("max_player = 4", 1, "unknown setting"),
            ("max_players 4", 1, "expected"),
            ("max_players = 4\nmax_players = 5", 2, "twice"),
            ("max_hit_distance = 1e9", 1, "outside"),
            ("flag_speed = nan", 1, "outside"),
            ("host_password = pässword", 1, "plain characters"),
        ] {
            let error = Config::parse(text).err();
            assert_eq!(error.as_ref().map(|e| e.line), Some(line), "{text}");
            let message = error.map(|e| e.message).unwrap_or_default();
            assert!(message.contains(fragment), "{text}: {message}");
        }
    }
}
