//! Starts the game in the `RetroArch` that Parallel Launcher installed, with the player's own
//! settings and controls, so joining needs no second program opened by hand.

use std::ffi::OsString;
use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// How many copies of the save are kept, one made at each join.
const BACKUPS_KEPT: usize = 30;

/// One thing the panel changes in the emulator for a game it starts: what, in the player's
/// words, why, and the settings that do it, each with the value it is given.
///
/// The player is shown these, as they are their emulator's settings and not the panel's: the
/// same list is what `Install::start` writes, so what is shown is what is done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub what: String,
    pub why: &'static str,
    settings: Vec<(String, &'static str)>,
}

/// A setting given the value that binds a hotkey to no key.
const NO_KEY: &str = "nul";

/// A change made whatever the player's settings are: what, why, and each setting with its
/// value.
type FixedChange = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// The changes every game the panel starts is given, as what, why and the settings. Each of
/// these would break a session, or mislead the player about one. They are laid over the
/// player's `RetroArch` settings for the one run, and `config_save_on_exit` keeps them out of
/// the player's own settings file.
const FIXED_CHANGES: [FixedChange; 6] = [
    (
        "Sound is on, and the game keeps time by it.",
        "The emulator paces a game by its sound: with sound off the game runs at over twice \
         its speed. You can still turn it down or mute it in the emulator's menu (F1).",
        &[("audio_enable", "true"), ("audio_sync", "true")],
    ),
    (
        "Run-ahead, preemptive frames and rewind are off, and no saved state is loaded as the \
         game starts.",
        "Each would put your game at a moment the other players' games are not at.",
        &[
            ("run_ahead_enabled", "false"),
            ("preemptive_frames_enable", "false"),
            ("rewind_enable", "false"),
            ("savestate_auto_load", "false"),
        ],
    ),
    (
        "The hotkeys for loading a state, rewinding, stepping a frame, fast forward, slow \
         motion, run-ahead and preemptive frames do nothing, on the keyboard, a controller or \
         the mouse.",
        "For the same reason: a game that jumps in time or runs at another speed falls out \
         of step with the others.",
        // Each hotkey has a key, and a controller's button and stick and a mouse's button,
        // which Parallel Launcher's own controller hotkeys are kept in.
        &[
            ("input_load_state", NO_KEY),
            ("input_load_state_btn", NO_KEY),
            ("input_load_state_axis", NO_KEY),
            ("input_load_state_mbtn", NO_KEY),
            ("input_rewind", NO_KEY),
            ("input_rewind_btn", NO_KEY),
            ("input_rewind_axis", NO_KEY),
            ("input_rewind_mbtn", NO_KEY),
            ("input_frame_advance", NO_KEY),
            ("input_frame_advance_btn", NO_KEY),
            ("input_frame_advance_axis", NO_KEY),
            ("input_frame_advance_mbtn", NO_KEY),
            ("input_toggle_fast_forward", NO_KEY),
            ("input_toggle_fast_forward_btn", NO_KEY),
            ("input_toggle_fast_forward_axis", NO_KEY),
            ("input_toggle_fast_forward_mbtn", NO_KEY),
            ("input_hold_fast_forward", NO_KEY),
            ("input_hold_fast_forward_btn", NO_KEY),
            ("input_hold_fast_forward_axis", NO_KEY),
            ("input_hold_fast_forward_mbtn", NO_KEY),
            ("input_toggle_slowmotion", NO_KEY),
            ("input_toggle_slowmotion_btn", NO_KEY),
            ("input_toggle_slowmotion_axis", NO_KEY),
            ("input_toggle_slowmotion_mbtn", NO_KEY),
            ("input_hold_slowmotion", NO_KEY),
            ("input_hold_slowmotion_btn", NO_KEY),
            ("input_hold_slowmotion_axis", NO_KEY),
            ("input_hold_slowmotion_mbtn", NO_KEY),
            ("input_runahead_toggle", NO_KEY),
            ("input_runahead_toggle_btn", NO_KEY),
            ("input_runahead_toggle_axis", NO_KEY),
            ("input_runahead_toggle_mbtn", NO_KEY),
            ("input_preempt_toggle", NO_KEY),
            ("input_preempt_toggle_btn", NO_KEY),
            ("input_preempt_toggle_axis", NO_KEY),
            ("input_preempt_toggle_mbtn", NO_KEY),
        ],
    ),
    (
        "The key that closes the emulator must be pressed twice.",
        "With Parallel Launcher's settings that key is Escape, which also closes a line of \
         chat you are typing in the game: one press would close the game with it.",
        &[("quit_press_twice", "true")],
    ),
    (
        "The emulator takes no commands over the network.",
        "It would take them from any PC on your network, and the panel does not need them.",
        &[("network_cmd_enable", "false")],
    ),
    (
        "The emulator does not save its settings as it closes.",
        "So none of these changes reaches your own settings.",
        &[("config_save_on_exit", "false")],
    ),
];

/// What a bot's game is given besides what every game is, as what, why and the settings. A
/// bot's game runs beside the player's own, behind it, and no person plays it.
const BOT_CHANGES: [FixedChange; 3] = [
    (
        "Its sound is muted.",
        "Several games make a din, and none of a bot's is yours to hear.",
        &[("audio_mute_enable", "true")],
    ),
    (
        "It reads no controller.",
        "The emulator reads a controller with its window behind others, so yours would move \
         the bot's Mario as well as your own.",
        &[("input_joypad_driver", "null")],
    ),
    (
        "It goes on running while its window is behind others.",
        "A bot's game is always behind something, and one that paused there would stand still \
         in the session.",
        &[("pause_nonactive", "false")],
    ),
];

/// What a bot's game is changed in, for the player to be told, in the player's words.
pub fn bot_changes() -> impl Iterator<Item = &'static str> {
    BOT_CHANGES.iter().map(|&(what, _, _)| what)
}

/// The keys a line of chat is typed with, as `RetroArch`'s settings name them, besides the
/// letters, which are their own names: the marks and the space of a keyboard's main part, its
/// digits, and the number pad, and the keys that scroll the chat and bring back lines sent.
const TYPING_KEYS: [&str; 28] = [
    "space",
    "backspace",
    "enter",
    "tilde",
    "backquote",
    "minus",
    "equals",
    "leftbracket",
    "rightbracket",
    "backslash",
    "semicolon",
    "quote",
    "comma",
    "period",
    "slash",
    "add",
    "subtract",
    "multiply",
    "divide",
    "kp_plus",
    "kp_minus",
    "kp_period",
    "kp_enter",
    "kp_equals",
    "pageup",
    "pagedown",
    "up",
    "down",
];

/// Whether `key`, as `RetroArch`'s settings name one, is a key a line of chat is typed with.
fn is_typing_key(key: &str) -> bool {
    let is_letter = key.len() == 1 && key.chars().all(|letter| letter.is_ascii_lowercase());
    let is_digit = ["num", "keypad"].iter().any(|kind| {
        key.strip_prefix(kind)
            .is_some_and(|digit| digit.len() == 1 && digit.chars().all(|d| d.is_ascii_digit()))
    });
    is_letter || is_digit || TYPING_KEYS.contains(&key)
}

/// Every hotkey `settings` has on a key that a line of chat is typed with, as the setting's
/// name and the key. The player types the chat in the game's own window, and the emulator
/// sees those keys too: with its usual settings a "p" would pause it and a "~" reset the game.
/// The keys bound to the controller are not among them, which the game itself ignores while a
/// line is typed.
fn typing_hotkeys(settings: &str) -> Vec<(&str, &str)> {
    settings
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once('=')?;
            let (name, key) = (name.trim(), value.trim().trim_matches('"'));
            let is_hotkey = name.starts_with("input_")
                && !name.starts_with("input_player")
                // The key that must be held for any hotkey to act: without it, they all
                // act by themselves.
                && name != "input_enable_hotkey"
                // A gamepad's button or stick, or a mouse's button, by its number.
                && !["_btn", "_axis", "_mbtn"].iter().any(|kind| name.ends_with(kind));
            (is_hotkey && is_typing_key(key)).then_some((name, key))
        })
        .collect()
}

/// The change that turns `hotkeys` off, naming each as the player's settings do, with its
/// key: "pause toggle (p)". None if there are none to turn off.
fn typing_change(hotkeys: &[(&str, &str)]) -> Option<Change> {
    if hotkeys.is_empty() {
        return None;
    }
    let named: Vec<String> = hotkeys
        .iter()
        .map(|(name, key)| {
            let hotkey = name.trim_start_matches("input_").replace('_', " ");
            format!("{hotkey} ({key})")
        })
        .collect();
    Some(Change {
        what: format!(
            "These hotkeys of yours, on keys a line of chat is typed with, do nothing: {}.",
            named.join(", ")
        ),
        why: "You type the chat in the game's window, and the emulator would act on those keys \
              as you typed. Turn typing in the game off, under Preferences, to keep them.",
        settings: hotkeys
            .iter()
            .map(|&(name, _)| (name.to_owned(), NO_KEY))
            .collect(),
    })
}

/// `fixed` as the changes they are.
fn fixed_changes(fixed: &'static [FixedChange]) -> impl Iterator<Item = Change> {
    fixed.iter().map(|&(what, why, lines)| Change {
        what: what.to_owned(),
        why,
        settings: lines
            .iter()
            .map(|&(name, value)| (name.to_owned(), value))
            .collect(),
    })
}

/// What the panel changes in the emulator for a bot's game, given the player's own
/// `settings`: what it changes for every game, and that nobody plays this one. Nobody types
/// in it, so the player's hotkeys are left as they are.
fn bot_changes_to(settings: &str) -> Vec<Change> {
    let mut changes = changes_to(settings, false);
    changes.extend(fixed_changes(&BOT_CHANGES));
    changes
}

/// What the panel changes in the emulator for a game it starts, given the player's own
/// `settings`; `typing` says the player types the chat in the game.
fn changes_to(settings: &str, typing: bool) -> Vec<Change> {
    let fixed = fixed_changes(&FIXED_CHANGES);
    // Not those turned off whatever key they are on, which are said once already.
    let is_fixed = |name: &str| {
        let mut fixed = FIXED_CHANGES.iter().flat_map(|&(_, _, lines)| lines);
        fixed.any(|&(fixed, _)| fixed == name)
    };
    let mut hotkeys = typing_hotkeys(settings);
    hotkeys.retain(|&(name, _)| !is_fixed(name));
    let typed = typing.then(|| typing_change(&hotkeys)).flatten();
    fixed.chain(typed).collect()
}

/// `changes` as the settings file that makes them, laid over the player's own.
fn overrides(changes: &[Change]) -> String {
    let mut file = String::new();
    for (name, value) in changes.iter().flat_map(|change| &change.settings) {
        // Writing to a String cannot fail.
        let _ = writeln!(file, "{name} = \"{value}\"");
    }
    file
}

/// Parallel Launcher's `RetroArch`: the program, the settings the launcher keeps for it, and
/// its Nintendo 64 core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    program: PathBuf,
    settings: PathBuf,
    core: PathBuf,
}

impl Install {
    /// The install under Parallel Launcher's data folder, if all of it is there.
    fn under(data: &Path) -> Option<Self> {
        let install = Self {
            program: data.join("retroarch").join("retroarch.exe"),
            settings: data.join("retroarch.cfg"),
            core: data
                .join("retro-data")
                .join("cores")
                .join("parallel_n64_next_libretro.dll"),
        };
        [&install.program, &install.settings, &install.core]
            .iter()
            .all(|path| path.is_file())
            .then_some(install)
    }

    /// This PC's install, if Parallel Launcher is installed and has run a game once, which is
    /// when it writes `RetroArch`'s settings.
    #[must_use]
    pub fn find() -> Option<Self> {
        let local = std::env::var_os("LOCALAPPDATA")?;
        Self::under(&PathBuf::from(local).join("parallel-launcher").join("data"))
    }

    /// What `RetroArch` is started with to run `rom`, `overrides` laid over its settings.
    fn arguments(&self, rom: &Path, overrides: &Path) -> Vec<OsString> {
        vec![
            "-c".into(),
            self.settings.clone().into(),
            "--appendconfig".into(),
            overrides.into(),
            "-L".into(),
            self.core.clone().into(),
            rom.into(),
        ]
    }

    /// What starting a game changes in the emulator, for the player to be shown: for that
    /// run only, and never in their own settings file. `typing` says the player types the
    /// chat in the game.
    ///
    /// # Errors
    /// Fails if the player's settings cannot be read.
    pub fn changes(&self, typing: bool) -> io::Result<Vec<Change>> {
        Ok(changes_to(&self.read_settings()?, typing))
    }

    /// The player's settings as text. A byte that is no text, as in a path typed in another
    /// encoding, is no reason to refuse to join: only the lines read here matter.
    fn read_settings(&self) -> io::Result<String> {
        Ok(String::from_utf8_lossy(&std::fs::read(&self.settings)?).into_owned())
    }

    /// Starts `rom` and returns the emulator's process ID. The game is left running when the
    /// panel closes, as one started from Parallel Launcher is. `typing` says the player will
    /// type the chat in the game, so the emulator's hotkeys on typing keys are turned off.
    ///
    /// # Errors
    /// Fails if the player's settings cannot be read, the settings laid over them cannot be
    /// written, or `RetroArch` cannot be started.
    pub fn start(&self, rom: &Path, typing: bool) -> io::Result<u32> {
        self.launch(rom, &self.changes(typing)?, "game.retroarch.cfg")
    }

    /// Starts `rom` as a bot's game (`bot_changes_to`) and returns the emulator's process ID.
    /// Each bot's game has a ROM of its own, and its settings a file named after it, as
    /// several start at once.
    ///
    /// # Errors
    /// As `start`.
    pub fn start_bot(&self, rom: &Path) -> io::Result<u32> {
        let changes = bot_changes_to(&self.read_settings()?);
        let name = rom.file_stem().unwrap_or_default().to_string_lossy();
        self.launch(rom, &changes, &format!("{name}.retroarch.cfg"))
    }

    /// Starts `rom` with `changes` laid over the player's settings, from a file of that name.
    fn launch(&self, rom: &Path, changes: &[Change], file: &str) -> io::Result<u32> {
        // Among the panel's own files, not in a folder of the player's that a ROM they chose
        // may be in; beside the ROM only where the panel has no folder.
        let overrides = crate::preferences::path()
            .and_then(|preferences| Some(preferences.parent()?.join(file)))
            .unwrap_or_else(|| rom.with_extension("retroarch.cfg"));
        if let Some(folder) = overrides.parent() {
            std::fs::create_dir_all(folder)?;
        }
        std::fs::write(&overrides, self::overrides(changes))?;
        let child = Command::new(&self.program)
            .args(self.arguments(rom, &overrides))
            .spawn()?;
        Ok(child.id())
    }

    /// Copies `rom`'s save files into a new folder under `backups`, named by the time, and
    /// returns how many there were. A session adds what anyone in it gets to the player's own
    /// save for good, so each join keeps the save as it was before.
    ///
    /// # Errors
    /// Fails if a save is there but cannot be copied.
    pub fn back_up_saves(&self, rom: &Path, backups: &Path) -> io::Result<usize> {
        let settings = self.read_settings()?;
        let (Some(saves), Some(game)) = (save_directory(&settings), rom.file_stem()) else {
            return Ok(0);
        };
        // The emulator names each save after the ROM: its name, then the kind of save.
        let of_this_game = |path: &PathBuf| path.file_stem() == Some(game) && path.is_file();
        let found: Vec<PathBuf> = match std::fs::read_dir(saves) {
            Ok(entries) => entries
                .filter_map(|entry| Some(entry.ok()?.path()))
                .filter(of_this_game)
                .collect(),
            // No save folder yet: the game has not been played.
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error),
        };
        if found.is_empty() {
            return Ok(0);
        }
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let folder = backups.join(seconds.to_string());
        std::fs::create_dir_all(&folder)?;
        for save in &found {
            if let Some(name) = save.file_name() {
                std::fs::copy(save, folder.join(name))?;
            }
        }
        forget_old_backups(backups);
        Ok(found.len())
    }
}

/// Closes the emulator with the process ID `pid`, as a bot's game is closed when its bot
/// games are stopped. Nothing is done if no emulator runs under that ID any more: the player
/// may have closed it, and another program have the ID now.
///
/// # Errors
/// Fails if the running programs cannot be listed, or the emulator cannot be closed.
pub fn close(pid: u32) -> io::Result<()> {
    use std::os::windows::process::CommandExt as _;
    /// Runs a console program without opening a window for it.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    if !b3313_bridge::process::find_processes(b3313_bridge::process::EMULATOR)?.contains(&pid) {
        return Ok(());
    }
    // Ended at once: nobody plays a bot's game, and what it saved is the session's.
    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("taskkill ended with {status}")))
    }
}

/// The folder `RetroArch` keeps saves in, from its settings: `savefile_directory = "..."`.
fn save_directory(settings: &str) -> Option<&Path> {
    settings.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        let folder = value.trim().trim_matches('"');
        (key.trim() == "savefile_directory" && !folder.is_empty()).then(|| Path::new(folder))
    })
}

/// Removes all but the newest `BACKUPS_KEPT` backups, so joining daily for years does not fill
/// the disk. Only folders this module named, by the second they were made, are touched.
fn forget_old_backups(backups: &Path) {
    let Ok(entries) = std::fs::read_dir(backups) else {
        return;
    };
    let mut made: Vec<(u64, PathBuf)> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let seconds = path.file_name()?.to_str()?.parse().ok()?;
            Some((seconds, path))
        })
        .collect();
    made.sort_unstable();
    let extra = made.len().saturating_sub(BACKUPS_KEPT);
    for (_, folder) in made.into_iter().take(extra) {
        // One that cannot be removed now is tried again at the next join.
        let _ = std::fs::remove_dir_all(folder);
    }
}

#[cfg(test)]
mod tests {
    use super::{BACKUPS_KEPT, Install, bot_changes_to, changes_to, overrides};
    use std::path::{Path, PathBuf};

    #[test]
    fn what_the_player_is_shown_is_what_the_emulator_is_given() {
        let theirs = "input_pause_toggle = \"p\"\ninput_menu_toggle = \"f1\"\n\
                      input_enable_hotkey = \"space\"\n";
        let changes = changes_to(theirs, true);
        let written = overrides(&changes);

        // Every change says what it is and why, and its settings are all that is written.
        assert!(
            changes
                .iter()
                .all(|change| !change.what.is_empty() && !change.why.is_empty())
        );
        let settings: usize = changes.iter().map(|change| change.settings.len()).sum();
        assert_eq!(written.lines().count(), settings);
        // What keeps a session in step, and what keeps it all out of their own settings.
        for needed in [
            "audio_enable = \"true\"",
            "rewind_enable = \"false\"",
            "input_load_state = \"nul\"",
            "input_toggle_fast_forward_btn = \"nul\"",
            "config_save_on_exit = \"false\"",
        ] {
            assert!(written.contains(needed), "{needed} is missing");
        }
        // Their own hotkey on a typing key is named to them with its key, and turned off.
        let named = changes.iter().any(|c| c.what.contains("pause toggle (p)"));
        assert!(named);
        assert!(written.contains("input_pause_toggle = \"nul\""));
        // But not the key they hold for a hotkey to act, without which all would act alone.
        assert!(!written.contains("input_enable_hotkey"));

        // One the panel turns off on any key is said, and written, once.
        let both = changes_to("input_rewind = \"backspace\"\n", true);
        assert_eq!(overrides(&both).matches("input_rewind = ").count(), 1);
        assert!(both.iter().all(|c| !c.what.contains("rewind (backspace)")));

        // With the chat typed in the panel, their hotkeys are left as they are.
        let untyped = changes_to(theirs, false);
        assert_eq!(untyped.len() + 1, changes.len());
        assert!(!overrides(&untyped).contains("input_pause_toggle"));
    }

    #[test]
    fn a_bots_game_is_muted_reads_no_controller_and_runs_behind_other_windows() {
        let theirs = "input_pause_toggle = \"p\"\n";
        let written = overrides(&bot_changes_to(theirs));
        for needed in [
            "audio_mute_enable = \"true\"",
            "input_joypad_driver = \"null\"",
            "pause_nonactive = \"false\"",
            // What every game is given, so the bot's stays in step too.
            "audio_sync = \"true\"",
            "config_save_on_exit = \"false\"",
        ] {
            assert!(written.contains(needed), "{needed} is missing");
        }
        // Nobody types in it, so the player's hotkeys stay as they are.
        assert!(!written.contains("input_pause_toggle"));
        assert_eq!(super::bot_changes().count(), super::BOT_CHANGES.len());
    }

    /// A sample of an emulator's settings with the hotkeys it is to name, kept as files so
    /// that another program holding the same rule can be tested against the same two.
    #[test]
    fn the_typing_hotkeys_of_a_sample_of_settings_are_the_ones_expected() {
        let settings = include_str!("../../tests/data/typing_hotkeys.cfg");
        let expected = include_str!("../../tests/data/typing_hotkeys.expected");

        let found: Vec<&str> = super::typing_hotkeys(settings)
            .into_iter()
            .map(|(name, _)| name)
            .collect();

        assert_eq!(found, expected.lines().collect::<Vec<_>>());
    }

    #[test]
    fn hotkeys_on_typing_keys_are_turned_off_and_nothing_else_is() {
        // As Parallel Launcher writes them, and a few a player might add.
        let theirs = "input_pause_toggle = \"p\"\n\
                      input_reset = \"tilde\"\n\
                      input_volume_up = \"add\"\n\
                      input_state_slot_increase = \"num7\"\n\
                      input_menu_toggle = \"f1\"\n\
                      input_exit_emulator = \"escape\"\n\
                      input_toggle_fullscreen = \"f11\"\n\
                      input_screenshot = \"nul\"\n\
                      input_player1_a = \"x\"\n\
                      input_player1_start = \"enter\"\n\
                      input_exit_emulator_btn = \"5\"\n\
                      input_max_users = \"8\"\n\
                      input_driver = \"dinput\"\n\
                      video_driver = \"d\"\n";
        let off: String = overrides(&changes_to(theirs, true))
            .lines()
            .filter(|line| !overrides(&changes_to("", true)).contains(line))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(
            off,
            "input_pause_toggle = \"nul\"\n\
             input_reset = \"nul\"\n\
             input_volume_up = \"nul\"\n\
             input_state_slot_increase = \"nul\""
        );
    }

    #[test]
    fn the_game_starts_with_the_players_settings_and_ours_over_them() {
        let install = Install {
            program: PathBuf::from("retroarch.exe"),
            settings: PathBuf::from("their.cfg"),
            core: PathBuf::from("core.dll"),
        };
        let arguments = install.arguments(Path::new("game.z64"), Path::new("ours.cfg"));
        let arguments: Vec<&str> = arguments.iter().filter_map(|a| a.to_str()).collect();
        // `RetroArch` reads `--appendconfig` after `-c`, so ours win; the ROM comes last.
        assert_eq!(
            arguments,
            [
                "-c",
                "their.cfg",
                "--appendconfig",
                "ours.cfg",
                "-L",
                "core.dll",
                "game.z64"
            ]
        );
    }

    /// A folder of this test's own, empty, under the system's temporary one.
    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("b3313-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let _ = std::fs::create_dir_all(&folder);
        folder
    }

    #[test]
    fn joining_copies_this_games_saves_aside_and_no_others() -> std::io::Result<()> {
        let work = scratch("saves");
        let saves = work.join("saves");
        std::fs::create_dir_all(&saves)?;
        std::fs::write(saves.join("game.srm"), b"progress")?;
        std::fs::write(saves.join("game.eep"), b"more")?;
        std::fs::write(saves.join("another game.srm"), b"not ours")?;
        let settings = work.join("their.cfg");
        std::fs::write(
            &settings,
            format!(
                "audio_enable = \"true\"\nsavefile_directory = \"{}\"\n",
                saves.display()
            ),
        )?;
        let install = Install {
            program: PathBuf::from("retroarch.exe"),
            settings,
            core: PathBuf::from("core.dll"),
        };
        let backups = work.join("backups");
        // Older backups than are kept: only the newest stay, and nothing else there is touched.
        for old in 1..=BACKUPS_KEPT {
            std::fs::create_dir_all(backups.join(old.to_string()))?;
        }
        std::fs::create_dir_all(backups.join("the player's own folder"))?;

        assert_eq!(
            install.back_up_saves(Path::new("C:/games/game.z64"), &backups)?,
            2
        );

        let made: Vec<PathBuf> = std::fs::read_dir(&backups)?
            .filter_map(|entry| Some(entry.ok()?.path()))
            .collect();
        assert_eq!(made.len(), BACKUPS_KEPT + 1);
        assert!(!backups.join("1").exists());
        assert!(backups.join("2").exists());
        assert!(backups.join("the player's own folder").exists());
        let copied = made.iter().find(|folder| folder.join("game.srm").exists());
        let Some(copied) = copied else {
            unreachable!("the save was not copied");
        };
        assert_eq!(std::fs::read(copied.join("game.srm"))?, b"progress");
        assert!(copied.join("game.eep").exists());
        assert!(!copied.join("another game.srm").exists());
        // A game never played has nothing to copy, and that is no failure.
        assert_eq!(
            install.back_up_saves(Path::new("unplayed.z64"), &backups)?,
            0
        );
        std::fs::remove_dir_all(work)
    }

    #[test]
    fn an_install_missing_a_part_is_no_install() {
        // A folder that holds none of `RetroArch`.
        assert_eq!(
            Install::under(&std::env::temp_dir().join("b3313-no-such-launcher")),
            None
        );
    }
}
