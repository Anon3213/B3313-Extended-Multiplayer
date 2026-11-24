//! A crash report's text, apart from gathering what goes in it, so that its form can be tested.

use crate::system::{LocalTime, Memory};
use std::fmt::Write as _;
use std::time::Duration;

/// Settings whose values a report leaves out: the session's and the host's passwords in server
/// settings, and in the panel's preferences remembered servers' passwords, the player's name,
/// and the name invites give for the server, which leads to its owner's home.
const LEFT_OUT_KEYS: [&str; 6] = [
    "password",
    "host_password",
    "server_password",
    "server_host_password",
    "player_name",
    "invite_name",
];
/// Settings that hold a path, of which a report gives only the file name: folders can name
/// the user, as on another drive than their user folder.
const PATH_KEYS: [&str; 6] = [
    "ban_file",
    "key_file",
    "rom",
    "sm64_rom",
    "b3313_patch",
    "recent_file",
];
/// Why a minidump, unlike a report, must not be shared with just anyone.
pub const DUMP_WARNING: &str = "A minidump is not redacted: it holds the panel's memory as it \
                                was, which may include IP addresses, server names and \
                                passwords, so share it only with someone you trust.";

/// What stopped the panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum What {
    /// Rust code found something it could not go on from.
    Panic {
        thread: String,
        message: String,
        location: Option<String>,
    },
    /// Windows stopped the panel, as for an access violation in a driver, which Rust cannot
    /// catch.
    Native { thread: String, exception: String },
}

/// What the panel was doing, as it last said.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub page: &'static str,
    /// The hosted server: stopped, or where it runs and who is on it.
    pub server: String,
    /// The Play page's session.
    pub play: String,
    /// The settings file being edited, if it has one, by its file name.
    pub settings_file: Option<String>,
    /// The settings the running server started with, as a settings file writes them.
    pub running_settings: Option<String>,
    /// The servers the player joined, as typed, which a report hides wherever they appear.
    pub servers: Vec<String>,
}

/// Everything a report says, gathered at the crash.
#[derive(Debug)]
pub struct Report<'a> {
    pub what: &'a What,
    pub when: LocalTime,
    /// Since the panel opened.
    pub opened_for: Duration,
    /// The program's version and commit.
    pub build: &'a str,
    pub windows: Option<&'a str>,
    pub memory: Option<Memory>,
    /// None if the panel was changing it as it crashed.
    pub state: Option<&'a State>,
    pub backtrace: Option<&'a str>,
    /// The minidump's file name, or why it could not be written; for native crashes.
    pub dump: Option<Result<&'a str, &'a str>>,
    /// Oldest first; None if the panel was adding to it as it crashed.
    pub activity: Option<&'a [String]>,
    /// The panel's preferences file, if it could be read.
    pub preferences: Option<&'a str>,
}

impl Report<'_> {
    /// The report, passwords left out. Addresses are hidden afterwards, over all of it.
    #[must_use]
    pub fn text(&self) -> String {
        let mut text = String::new();
        let title = "B3313 Extended Multiplayer control panel: crash report";
        let _ = writeln!(text, "{title}\n{}\n", "=".repeat(title.len()));
        heading(&mut text, "What happened");
        describe(&mut text, self.what);
        let _ = writeln!(
            text,
            "\nWhen:      {}, {} after the panel opened",
            date_time(self.when),
            elapsed(self.opened_for)
        );
        let _ = writeln!(text, "Build:     {}", self.build);
        let _ = writeln!(
            text,
            "Windows:   {} ({})",
            self.windows.unwrap_or("version unknown"),
            std::env::consts::ARCH
        );
        if let Some(memory) = self.memory {
            let _ = writeln!(
                text,
                "Memory:    the panel used {}; {} of {} free",
                bytes(memory.panel),
                bytes(memory.available),
                bytes(memory.total)
            );
        }
        match self.dump {
            Some(Ok(name)) => {
                let _ = writeln!(
                    text,
                    "Minidump:  {name}, beside this report. Open it in WinDbg or Visual Studio \
                     with b3313_panel.pdb beside b3313_panel.exe.\n           {DUMP_WARNING}"
                );
            }
            Some(Err(problem)) => {
                let _ = writeln!(text, "Minidump:  could not be written: {problem}");
            }
            None => {}
        }
        self.panel_state(&mut text);
        heading(&mut text, "Stack trace of the failing thread");
        let _ = writeln!(
            text,
            "{}",
            self.backtrace
                .unwrap_or("(none: the thread had no stack left to take one; see the minidump)")
                .trim_end()
        );
        heading(&mut text, "Recent activity, oldest first");
        match self.activity {
            Some([]) => text.push_str("(nothing yet)\n"),
            Some(lines) => {
                for line in lines {
                    let _ = writeln!(text, "{line}");
                }
            }
            None => text.push_str("(unavailable: the panel was adding to it as it crashed)\n"),
        }
        self.settings(&mut text);
        let _ = writeln!(
            text,
            "\nIP addresses, server names and your user folder are hidden in this report, paths \
             give only file names, and passwords, players' names and what was said in the chat \
             are left out, so it can be shared."
        );
        if self.dump.is_some_and(|dump| dump.is_ok()) {
            let _ = writeln!(text, "{DUMP_WARNING}");
        }
        text
    }

    fn panel_state(&self, text: &mut String) {
        heading(text, "The panel at the time");
        let Some(state) = self.state else {
            text.push_str("(unavailable: the panel was changing it as it crashed)\n");
            return;
        };
        let _ = writeln!(text, "Page:      {}", state.page);
        let _ = writeln!(text, "Server:    {}", state.server);
        let _ = writeln!(text, "Play:      {}", state.play);
        let _ = writeln!(
            text,
            "Settings:  {}",
            state
                .settings_file
                .as_deref()
                .unwrap_or("not saved to a file")
        );
    }

    fn settings(&self, text: &mut String) {
        heading(text, "Settings of the running server");
        match self
            .state
            .and_then(|state| state.running_settings.as_deref())
        {
            Some(settings) => {
                let _ = writeln!(text, "{}", for_sharing(settings).trim_end());
            }
            None => text.push_str("(the server was not running)\n"),
        }
        heading(text, "The panel's preferences");
        match self.preferences {
            Some(preferences) => {
                let _ = writeln!(text, "{}", for_sharing(preferences).trim_end());
            }
            None => text.push_str("(none saved)\n"),
        }
    }
}

/// What a later failure in the same run adds to its report, as when a panic on the panel's
/// thread is followed by Rust's own "panic in a function that cannot unwind".
#[must_use]
pub fn another(what: &What, opened_for: Duration, backtrace: Option<&str>) -> String {
    let mut text = String::new();
    heading(
        &mut text,
        &format!("Then, {} after the panel opened", elapsed(opened_for)),
    );
    describe(&mut text, what);
    if let Some(backtrace) = backtrace {
        let _ = writeln!(text, "\n{}", backtrace.trim_end());
    }
    text
}

fn heading(text: &mut String, title: &str) {
    let _ = writeln!(text, "\n{title}\n{}", "-".repeat(title.chars().count()));
}

fn describe(text: &mut String, what: &What) {
    match what {
        What::Panic {
            thread,
            message,
            location,
        } => {
            let place = location
                .as_deref()
                .map_or_else(String::new, |location| format!(" at {location}"));
            let _ = writeln!(text, "The panel panicked on thread '{thread}'{place}:");
            for line in message.lines() {
                let _ = writeln!(text, "    {line}");
            }
        }
        What::Native { thread, exception } => {
            let _ = writeln!(text, "Windows stopped the panel on thread '{thread}':");
            let _ = writeln!(text, "    {exception}");
        }
    }
}

/// A panic's stack trace without the frames that are the same for every panic: Rust's own
/// panic handling at the top, before the code that panicked, and the program's start at the
/// bottom. Rust marks both ends with frames of its own; a trace without them is kept whole.
#[must_use]
pub fn trim_backtrace(trace: &str) -> String {
    // Each frame is a numbered line, followed by the lines saying where its code is.
    let mut frames: Vec<(&str, Vec<&str>)> = Vec::new();
    for line in trace.lines() {
        let numbered = line
            .trim_start()
            .split_once(": ")
            .filter(|(number, _)| number.parse::<u32>().is_ok());
        match (numbered, frames.last_mut()) {
            (Some((_, name)), _) => frames.push((name, vec![line])),
            (None, Some((_, lines))) => lines.push(line),
            (None, None) => {}
        }
    }
    let marked = |marker: &str| frames.iter().rposition(|(name, _)| name.contains(marker));
    let (Some(end_of_panicking), Some(start_of_program)) = (
        marked("__rust_end_short_backtrace"),
        marked("__rust_begin_short_backtrace"),
    ) else {
        return trace.to_owned();
    };
    let first = frames
        .iter()
        .enumerate()
        .skip(end_of_panicking + 1)
        .find(|(_, (name, _))| {
            !name.starts_with("std::panicking") && !name.starts_with("core::panicking")
        })
        .map_or(end_of_panicking + 1, |(index, _)| index);
    let mut trimmed =
        String::from("(Rust's panic handling above and the program's start below left out)\n");
    for (_, lines) in frames.get(first..start_of_program).unwrap_or_default() {
        for line in lines {
            let _ = writeln!(trimmed, "{line}");
        }
    }
    trimmed
}

/// An exception Windows raised, in words: its kind, what it touched, and where.
#[must_use]
pub fn describe_exception(code: u32, address: usize, information: &[usize]) -> String {
    let kind = match code {
        0xC000_0005 => "an access violation",
        0xC000_0006 => "a page that could not be read in",
        0xC000_00FD => "a stack overflow",
        0xC000_001D => "an illegal instruction",
        0xC000_0094 => "an integer divided by zero",
        0xC000_0374 => "heap corruption",
        0xC000_0409 => "a stack buffer overrun",
        0x8000_0003 => "a breakpoint",
        0xE06D_7363 => "a C++ exception, thrown by a library or driver",
        _ => "an exception",
    };
    let mut text = format!("{kind} (0x{code:08X}) at 0x{address:016X}");
    // For these two, the first value says what was tried, the second where.
    if matches!(code, 0xC000_0005 | 0xC000_0006)
        && let [access, target, ..] = information
    {
        let access = match access {
            0 => "reading",
            1 => "writing",
            8 => "running code at",
            _ => "touching",
        };
        let _ = write!(text, ", {access} address 0x{target:016X}");
    }
    text
}

/// What the activity keeps of a line of chat: that it was said and how long it was, which can
/// matter to a crash in drawing it, and not its words or who said them, which are the players'.
#[must_use]
pub fn chat_note(text: &str) -> String {
    format!("A line of chat, {} characters", text.chars().count())
}

/// Settings `text`, as a settings file writes them, made fit to share: each secret's and name's
/// value left out, each path cut to its file name, and each server joined hidden.
#[must_use]
pub fn for_sharing(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    for line in text.lines() {
        let setting = line
            .split_once('=')
            .map(|(key, value)| (key.trim(), value.trim()))
            .filter(|(key, value)| !key.starts_with('#') && !value.is_empty());
        match setting {
            Some((key, _)) if LEFT_OUT_KEYS.contains(&key) => {
                let _ = writeln!(kept, "{key} = (left out)");
            }
            Some((key, path)) if PATH_KEYS.contains(&key) => {
                let _ = writeln!(kept, "{key} = {}", file_name(path));
            }
            // A server the player joined, by name or IP.
            Some(("server", address)) => {
                let _ = writeln!(kept, "server = {}", crate::privacy::hide_server(address));
            }
            _ => {
                let _ = writeln!(kept, "{line}");
            }
        }
    }
    kept
}

/// The last part of a path, which names the file without the folders it is in.
#[must_use]
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// A report's file name, without its extension, from when it was written; names sort in time
/// order.
#[must_use]
pub fn file_stem(when: LocalTime) -> String {
    format!(
        "crash-{:04}-{:02}-{:02}-{:02}{:02}{:02}",
        when.year, when.month, when.day, when.hour, when.minute, when.second
    )
}

/// When a report was written, from its file name; any suffix telling apart reports from the
/// same second is ignored.
#[must_use]
pub fn stem_time(stem: &str) -> Option<LocalTime> {
    let rest = stem.strip_prefix("crash-")?;
    let number = |range: std::ops::Range<usize>| rest.get(range)?.parse::<u16>().ok();
    let separated = [4..5, 7..8, 10..11]
        .into_iter()
        .all(|range| rest.get(range) == Some("-"));
    if !separated {
        return None;
    }
    Some(LocalTime {
        year: number(0..4)?,
        month: number(5..7)?,
        day: number(8..10)?,
        hour: number(11..13)?,
        minute: number(13..15)?,
        second: number(15..17)?,
    })
}

/// The reports among `stems` written after `seen`, the newest first.
#[must_use]
pub fn unread(stems: impl IntoIterator<Item = String>, seen: Option<&str>) -> Vec<String> {
    let mut unread: Vec<String> = stems
        .into_iter()
        .filter(|stem| stem_time(stem).is_some())
        .filter(|stem| seen.is_none_or(|seen| stem.as_str() > seen))
        .collect();
    unread.sort_unstable_by(|a, b| b.cmp(a));
    unread
}

/// A line of activity: its time of day, to the millisecond, what it is about, and what happened.
#[must_use]
pub fn activity_line(time_of_day: Duration, area: &str, text: &str) -> String {
    let millis = time_of_day.as_millis();
    let (hours, minutes) = (millis / 3_600_000 % 24, millis / 60_000 % 60);
    let (seconds, millis) = (millis / 1000 % 60, millis % 1000);
    format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}  {area:<10} {text}")
}

/// `2026-11-09 14:03:22`.
#[must_use]
pub fn date_time(when: LocalTime) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        when.year, when.month, when.day, when.hour, when.minute, when.second
    )
}

/// `2 h 05 min 09 s`, `5 min 09 s` or `9 s`.
fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    match (hours, minutes) {
        (0, 0) => format!("{seconds} s"),
        (0, _) => format!("{minutes} min {seconds:02} s"),
        _ => format!("{hours} h {minutes:02} min {seconds:02} s"),
    }
}

/// `512 MB`, or `9.3 GB` from a gigabyte up.
fn bytes(count: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if count >= GB {
        // Tenths of a gigabyte, in whole numbers, which stay exact.
        let tenths = count * 10 / GB;
        format!("{}.{} GB", tenths / 10, tenths % 10)
    } else {
        format!("{} MB", count / MB)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DUMP_WARNING, Report, State, What, activity_line, another, describe_exception, file_stem,
        for_sharing, stem_time, trim_backtrace, unread,
    };
    use crate::preferences::{Passwords, Preferences};
    use crate::system::{LocalTime, Memory};
    use std::path::PathBuf;
    use std::time::Duration;

    const WHEN: LocalTime = LocalTime {
        year: 2026,
        month: 11,
        day: 9,
        hour: 14,
        minute: 3,
        second: 22,
    };

    fn panic() -> What {
        What::Panic {
            thread: "main".to_owned(),
            message: "attempt to subtract with overflow".to_owned(),
            location: Some("src\\app.rs:599:5".to_owned()),
        }
    }

    fn report<'a>(what: &'a What, state: Option<&'a State>, activity: &'a [String]) -> Report<'a> {
        Report {
            what,
            when: WHEN,
            opened_for: Duration::from_secs(761),
            build: "b3313-panel 0.1.0, commit 97f43fe",
            windows: Some("10.0.26100"),
            memory: Some(Memory {
                panel: 142 * 1024 * 1024,
                available: 10 * 1024 * 1024 * 1024,
                total: 32 * 1024 * 1024 * 1024,
            }),
            state,
            backtrace: Some("   0: b3313_panel::app::Panel::poll\n"),
            dump: None,
            activity: Some(activity),
            preferences: Some(
                "theme = dark\nserver = 203.0.113.7:41313\nserver_password = hunter2\n",
            ),
        }
    }

    #[test]
    fn a_report_says_what_failed_where_and_what_led_to_it() {
        let state = State {
            page: "Overview",
            server: "running on 0.0.0.0:41313 for 10:02, 3 players".to_owned(),
            play: "not joined".to_owned(),
            settings_file: Some("C:\\Hosting\\server.conf".to_owned()),
            running_settings: Some("max_players = 16\npassword = open sesame\n".to_owned()),
            servers: Vec::new(),
        };
        let activity = ["14:01:02.123  session    Player 2 joined".to_owned()];
        let text = report(&panic(), Some(&state), &activity).text();
        for expected in [
            "thread 'main' at src\\app.rs:599:5",
            "    attempt to subtract with overflow",
            "2026-11-09 14:03:22, 12 min 41 s after the panel opened",
            "commit 97f43fe",
            "the panel used 142 MB; 10.0 GB of 32.0 GB free",
            "Page:      Overview",
            "b3313_panel::app::Panel::poll",
            "Player 2 joined",
            "max_players = 16",
            "theme = dark",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
        assert!(!text.contains("open sesame"), "{text}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(
            !text.contains("Minidump"),
            "only native crashes have one: {text}"
        );
        // With one, the report warns, beside it and at the end, that it is not redacted.
        let what = panic();
        let mut with_dump = report(&what, Some(&state), &activity);
        with_dump.dump = Some(Ok("crash-2026-11-09-140322.dmp"));
        let text = with_dump.text();
        assert_eq!(text.matches(DUMP_WARNING).count(), 2, "{text}");
    }

    #[test]
    fn a_report_says_what_it_could_not_gather() {
        let native = What::Native {
            thread: "4812".to_owned(),
            exception: "a stack overflow (0xC00000FD) at 0x00007FF6DEAD0000".to_owned(),
        };
        let mut gathered = report(&native, None, &[]);
        gathered.activity = None;
        gathered.backtrace = None;
        gathered.dump = Some(Err("access is denied"));
        let text = gathered.text();
        for expected in [
            "Windows stopped the panel on thread '4812'",
            "a stack overflow",
            "could not be written: access is denied",
            "the panel was changing it as it crashed",
            "the panel was adding to it as it crashed",
            "the thread had no stack left",
            "the server was not running",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
    }

    #[test]
    fn a_later_failure_adds_its_own_section() {
        let text = another(&panic(), Duration::from_secs(3725), Some("   0: frame\n"));
        assert!(text.contains("Then, 1 h 02 min 05 s after the panel opened"));
        assert!(text.contains("attempt to subtract with overflow"));
        assert!(text.contains("0: frame"));
    }

    #[test]
    fn panic_traces_keep_only_the_program_s_own_frames() {
        let trace = "   0: std::backtrace::Backtrace::force_capture\n\
                     \x20            at /rustc/library/std/src/backtrace.rs:312\n\
                     \x20  1: b3313_panel::crash::on_panic\n\
                     \x20  2: std::sys::backtrace::__rust_end_short_backtrace<closure>\n\
                     \x20  3: std::panicking::panic_handler\n\
                     \x20  4: core::panicking::panic_fmt\n\
                     \x20  5: b3313_panel::app::Panel::poll\n\
                     \x20            at .\\panel\\src\\app.rs:599\n\
                     \x20  6: b3313_panel::main\n\
                     \x20  7: std::sys::backtrace::__rust_begin_short_backtrace<fn>\n\
                     \x20  8: main\n";
        let trimmed = trim_backtrace(trace);
        assert!(
            trimmed.contains("   5: b3313_panel::app::Panel::poll\n             at .\\panel\\src\\app.rs:599\n   6: b3313_panel::main\n"),
            "{trimmed}"
        );
        for gone in [
            "force_capture",
            "on_panic",
            "panic_fmt",
            "__rust",
            "   8: main",
        ] {
            assert!(!trimmed.contains(gone), "{gone:?} kept in:\n{trimmed}");
        }
        // Without Rust's markers, as in a trace taken for a native crash, nothing is left out.
        let unmarked = "   0: b3313_panel::main\n   1: main\n";
        assert_eq!(trim_backtrace(unmarked), unmarked);
    }

    #[test]
    fn exceptions_name_what_was_touched() {
        assert_eq!(
            describe_exception(0xC000_0005, 0x7FF6_0000_1234, &[1, 0x10]),
            "an access violation (0xC0000005) at 0x00007FF600001234, writing address \
             0x0000000000000010"
        );
        assert_eq!(
            describe_exception(0xC000_00FD, 0x10, &[0, 0x20]),
            "a stack overflow (0xC00000FD) at 0x0000000000000010"
        );
        assert!(describe_exception(0x1234_5678, 0, &[]).starts_with("an exception (0x12345678)"));
        // An access violation without its details still says what it was.
        assert!(describe_exception(0xC000_0005, 0, &[]).ends_with("at 0x0000000000000000"));
    }

    #[test]
    fn secrets_are_left_out_and_paths_cut_to_file_names() {
        let text = "# password = in a comment stays\npassword = open sesame\npassword =\n\
                    server_password=hunter2\nmax_players = 16\n\
                    rom = D:\\Games\\Someone\\b3313 multiplayer.z64\nban_file = bans.txt\n\
                    recent_file = E:/hosting/server.conf\nlisten_address = 0.0.0.0:41313\n\
                    server = myserver.duckdns.org:41313\n";
        assert_eq!(
            for_sharing(text),
            "# password = in a comment stays\npassword = (left out)\npassword =\n\
             server_password = (left out)\nmax_players = 16\n\
             rom = b3313 multiplayer.z64\nban_file = bans.txt\n\
             recent_file = server.conf\nlisten_address = 0.0.0.0:41313\n\
             server = ••••••••:41313\n"
        );
    }

    /// The files as the server and the panel really write them, so that a setting added to
    /// either with a secret, a folder or a name in it fails here until the report hides it.
    #[test]
    fn the_settings_and_preferences_as_written_share_nothing_private() {
        let settings = b3313_server::Config {
            password: "open sesame".to_owned(),
            host_password: "crown jewels".to_owned(),
            ban_file: Some(PathBuf::from(r"D:\Private\bans.txt")),
            ..b3313_server::Config::default()
        };
        let mut preferences = Preferences {
            rom: Some(PathBuf::from(r"D:\Private\multiplayer.z64")),
            sm64_rom: Some(PathBuf::from(r"D:\Private\Super Mario 64 (USA).z64")),
            b3313_patch: Some(PathBuf::from(r"D:\Private\b3313 a2ae.bps")),
            recent_files: vec![PathBuf::from(r"D:\Private\server.conf")],
            player_name: "Weegee".to_owned(),
            ..Preferences::default()
        };
        let passwords = Passwords {
            session: "hunter2",
            host: "paper crown",
        };
        preferences.remember_server("myserver.duckdns.org:41313", Some(passwords));
        for written in [settings.format(), preferences.format()] {
            let shared = for_sharing(&written);
            for private in [
                "open sesame",
                "crown jewels",
                "hunter2",
                "paper crown",
                "Private",
                "Weegee",
                "duckdns",
            ] {
                assert!(!shared.contains(private), "{private:?} kept in:\n{shared}");
            }
        }
        // What is safe and useful stays: which files, by name.
        let shared = for_sharing(&preferences.format());
        assert!(
            shared.contains("sm64_rom = Super Mario 64 (USA).z64"),
            "{shared}"
        );
        assert!(shared.contains("b3313_patch = b3313 a2ae.bps"), "{shared}");
    }

    #[test]
    fn report_names_sort_by_time_and_give_it_back() {
        let stem = file_stem(WHEN);
        assert_eq!(stem_time(&stem), Some(WHEN));
        assert_eq!(stem_time(&format!("{stem}-2")), Some(WHEN));
        let later = file_stem(LocalTime {
            day: 10,
            hour: 1,
            ..WHEN
        });
        assert!(later > stem);
        assert_eq!(stem_time("crash-notes"), None);
        assert_eq!(stem_time("other-2026-11-09-140322"), None);
    }

    #[test]
    fn unread_reports_are_those_after_the_last_seen_newest_first() {
        let stems = [
            "crash-2026-11-01-090000",
            "crash-2026-11-09-140322",
            "crash-2026-11-09-140322-2",
            "notes",
        ]
        .map(str::to_owned);
        assert_eq!(
            unread(stems.clone(), Some("crash-2026-11-01-090000")),
            ["crash-2026-11-09-140322-2", "crash-2026-11-09-140322"]
        );
        assert_eq!(unread(stems.clone(), None).len(), 3);
        assert_eq!(
            unread(stems, Some("crash-2026-11-09-140322-2")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn activity_lines_give_the_time_to_the_millisecond() {
        let time = Duration::from_millis(14 * 3_600_000 + 3 * 60_000 + 22_512);
        assert_eq!(
            activity_line(time, "play", "Joined as player 2"),
            "14:03:22.512  play       Joined as player 2"
        );
    }
}
