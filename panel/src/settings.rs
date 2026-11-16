//! The server settings being edited, checked by the server's own rules, and the file they are
//! kept in.

use b3313_server::Config;
use b3313_server::config::SETTINGS;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    /// Shown with the unit it counts in.
    Number {
        unit: &'static str,
    },
    /// Masked unless the user shows it.
    Password,
    Path,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// The setting's key in a config file.
    pub key: &'static str,
    pub label: &'static str,
    pub kind: FieldKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub title: &'static str,
    /// Said under the title, where the group as a whole wants a word.
    pub detail: Option<&'static str>,
    pub fields: &'static [Field],
}

/// Under "Advanced": why its settings are there and not with the others, for a user who
/// hosts from the panel and would otherwise wonder what to type.
const ADVANCED_DETAIL: &str = "Leave these as they are unless you run a server without this \
    panel. When you host here and join on Play, the panel makes you the host by itself, with a \
    host password it makes up and never shows. To let another player be the host, pass the role \
    on with /host <player> in the chat, or name them with /host <player> in the Server chat: no \
    password is needed for either.";

const fn number(key: &'static str, label: &'static str, unit: &'static str) -> Field {
    Field {
        key,
        label,
        kind: FieldKind::Number { unit },
    }
}

/// How the editor lays the settings out; every one of `SETTINGS` appears once.
pub const GROUPS: [Group; 6] = [
    Group {
        title: "Session",
        detail: None,
        fields: &[
            number("max_players", "Player limit", "players"),
            number("player_timeout_seconds", "Timeout", "seconds"),
            Field {
                key: "password",
                label: "Password",
                kind: FieldKind::Password,
            },
        ],
    },
    Group {
        title: "Combat",
        detail: None,
        fields: &[
            number("max_hit_distance", "Hit reach", "units"),
            number("min_hit_interval_ms", "Time between hits", "ms"),
        ],
    },
    Group {
        title: "Movement checks",
        detail: None,
        fields: &[
            number("flag_speed", "Flag speed", "units/s"),
            number("jump_distance", "Jump distance", "units"),
            number("jumps_per_minute", "Jumps allowed", "per minute"),
        ],
    },
    Group {
        title: "Rate limits",
        detail: None,
        fields: &[
            number("packets_per_second", "Packets per address", "per second"),
            number("joins_per_second", "Join attempts", "per second"),
        ],
    },
    Group {
        title: "Bans",
        detail: None,
        fields: &[Field {
            key: "ban_file",
            label: "Ban file",
            kind: FieldKind::Path,
        }],
    },
    Group {
        title: "Advanced",
        detail: Some(ADVANCED_DETAIL),
        fields: &[Field {
            key: "host_password",
            label: "Host password",
            kind: FieldKind::Password,
        }],
    },
];

/// What a setting does, as one paragraph.
#[must_use]
pub fn help(key: &str) -> String {
    SETTINGS
        .iter()
        .find(|setting| setting.key == key)
        .map(|setting| setting.help.replace('\n', " "))
        .unwrap_or_default()
}

type Values = BTreeMap<&'static str, String>;

fn values_of(config: &Config) -> Values {
    SETTINGS
        .iter()
        .map(|setting| (setting.key, config.value(setting.key).unwrap_or_default()))
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsForm {
    values: Values,
    /// As last opened or saved, to tell whether there is anything to save.
    saved: Values,
    path: Option<PathBuf>,
}

impl SettingsForm {
    /// A form showing `config`, not yet saved anywhere.
    #[must_use]
    pub fn new(config: &Config) -> Self {
        let values = values_of(config);
        Self {
            saved: values.clone(),
            values,
            path: None,
        }
    }

    /// Reads a config file.
    ///
    /// # Errors
    /// Fails, naming the file and the line, if it cannot be read or holds an invalid setting.
    pub fn open(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let config =
            Config::parse(&text).map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(Self {
            path: Some(path.to_owned()),
            ..Self::new(&config)
        })
    }

    #[must_use]
    pub fn value(&self, key: &str) -> &str {
        self.values.get(key).map_or("", String::as_str)
    }

    pub fn set(&mut self, key: &str, text: &str) {
        if let Some(value) = self.values.get_mut(key) {
            text.clone_into(value);
        }
    }

    /// The fields whose values the server would refuse, with why.
    #[must_use]
    pub fn errors(&self) -> BTreeMap<&'static str, String> {
        let mut config = Config::default();
        self.values
            .iter()
            .filter_map(|(&key, value)| Some((key, config.set(key, value).err()?)))
            .collect()
    }

    /// The settings as the server would take them.
    ///
    /// # Errors
    /// Fails, with the first refused field's message, if any value is invalid.
    pub fn config(&self) -> Result<Config, String> {
        let mut config = Config::default();
        for (key, value) in &self.values {
            config.set(key, value)?;
        }
        Ok(config)
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.values != self.saved
    }

    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Writes the settings to `path`, which becomes the form's file.
    ///
    /// # Errors
    /// Fails if a value is invalid, so no file is written that the server would refuse, or if
    /// the file cannot be written.
    pub fn save_as(&mut self, path: &Path) -> Result<(), String> {
        let config = self.config()?;
        crate::preferences::replace_file(path, &config.format())
            .map_err(|error| format!("cannot save {}: {error}", path.display()))?;
        self.path = Some(path.to_owned());
        self.saved = self.values.clone();
        Ok(())
    }

    /// Puts back the values last opened or saved.
    pub fn revert(&mut self) {
        self.values = self.saved.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::{GROUPS, SettingsForm, help};
    use b3313_server::Config;
    use b3313_server::config::SETTINGS;
    use std::path::PathBuf;

    #[test]
    fn the_editor_shows_every_setting_once() {
        let mut shown: Vec<&str> = GROUPS
            .iter()
            .flat_map(|group| group.fields.iter().map(|field| field.key))
            .collect();
        shown.sort_unstable();
        let mut known: Vec<&str> = SETTINGS.iter().map(|setting| setting.key).collect();
        known.sort_unstable();
        assert_eq!(shown, known);
        assert!(help("flag_speed").contains("per second within one level"));
        assert!(!help("password").contains('\n'));
    }

    #[test]
    fn refused_values_are_reported_by_field_and_block_the_config() {
        let mut form = SettingsForm::new(&Config::default());
        form.set("max_players", "17");
        form.set("password", "has#hash");
        let errors = form.errors();
        let keys: Vec<&str> = errors.keys().copied().collect();
        assert_eq!(keys, ["max_players", "password"]);
        assert!(form.config().is_err());

        form.set("max_players", "4");
        form.set("password", "ok");
        let config = form.config();
        assert_eq!(config.map(|c| c.max_players), Ok(4));
        assert!(form.errors().is_empty());
    }

    /// A file path no other test uses, removed when dropped.
    struct ScratchFile(PathBuf);

    impl ScratchFile {
        fn new(name: &str) -> Self {
            let file = format!("b3313-panel-{name}-{}.conf", std::process::id());
            Self(std::env::temp_dir().join(file))
        }
    }

    impl Drop for ScratchFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn saved_settings_open_again_and_edits_mark_the_form_unsaved() -> Result<(), String> {
        let file = ScratchFile::new("round-trip");
        let mut form = SettingsForm::new(&Config::default());
        assert!(!form.is_dirty());
        form.set("max_players", "6");
        form.set("ban_file", "bans.txt");
        assert!(form.is_dirty());
        form.save_as(&file.0)?;
        assert!(!form.is_dirty());
        assert_eq!(form.path(), Some(file.0.as_path()));

        let opened = SettingsForm::open(&file.0)?;
        assert_eq!(opened, form);
        let config = opened.config()?;
        assert_eq!(config.max_players, 6);
        assert_eq!(config.ban_file, Some("bans.txt".into()));

        form.set("max_players", "7");
        form.revert();
        assert_eq!(form.value("max_players"), "6");
        Ok(())
    }

    #[test]
    fn invalid_settings_are_neither_saved_nor_opened() {
        let file = ScratchFile::new("invalid");
        let mut form = SettingsForm::new(&Config::default());
        form.set("joins_per_second", "0");
        assert!(form.save_as(&file.0).is_err());
        assert!(!file.0.exists(), "nothing written");

        let written = std::fs::write(&file.0, "max_players = 4\nmax_players = 5\n");
        assert!(written.is_ok());
        let error = SettingsForm::open(&file.0).err().unwrap_or_default();
        assert!(error.contains("line 2"), "{error}");
    }
}
