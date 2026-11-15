//! Crash reports. A release build has no console, so a crash would otherwise leave nothing
//! behind. When the panel panics, or Windows stops it, a report of what failed, where, and what
//! the panel was doing goes in `%APPDATA%\B3313 Extended Multiplayer\crashes`, with a minidump for what
//! Windows stopped; the panel offers it on its next start.
//!
//! The panel tells this module what it is doing as it goes (`note`, `update_state`), as a crash
//! leaves no way to ask it then.

#[cfg(windows)]
mod native;
mod report;

pub use report::{DUMP_WARNING, State, chat_note, stem_time};

use crate::{preferences, privacy, system};
use report::{Report, What};
use std::backtrace::Backtrace;
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::time::Instant;

/// Lines of activity a report gives.
const ACTIVITY_LINES: usize = 300;
/// Reports kept; older ones are removed as the panel opens.
const REPORTS_KEPT: usize = 20;
/// Set to `panic`, `thread`, `native` or `overflow` to crash the panel on purpose two seconds
/// after it opens, to check that reports are written.
const TEST_VARIABLE: &str = "B3313_PANEL_CRASH_TEST";

/// Put beside the reports, saying which files are safe to share.
const README: &str = "Crash reports from the B3313 Extended Multiplayer control panel.\r\n\r\n\
    crash-*.txt  A report of what failed, where, and what the panel was doing. IP addresses,\r\n\
    \x20            server names and your user folder are hidden, paths give only file names,\r\n\
    \x20            and passwords, players' names and what was said in the chat are left out,\r\n\
    \x20            so it can be shared, as in a bug report.\r\n\r\n\
    crash-*.dmp  A minidump, for a crash only Windows could catch. It is NOT redacted: it holds\r\n\
    \x20            the panel's memory as it was, which may include IP addresses, server names\r\n\
    \x20            and passwords. Share it only with someone you trust. Open it in WinDbg or\r\n\
    \x20            Visual Studio with b3313_panel.pdb beside b3313_panel.exe.\r\n\r\n\
    The panel keeps the newest 20 reports. Any of these files can be deleted.\r\n";

static OPENED: OnceLock<Instant> = OnceLock::new();
static FOLDER: OnceLock<PathBuf> = OnceLock::new();
static ACTIVITY: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static STATE: Mutex<State> = Mutex::new(State {
    page: "",
    server: String::new(),
    play: String::new(),
    settings_file: None,
    running_settings: None,
    servers: Vec::new(),
});
/// This run's report, once written, so a later failure adds to it rather than starting another.
static WRITTEN: Mutex<Option<PathBuf>> = Mutex::new(None);
/// A report was written while the panel kept running, as when a background thread panics.
static NEW_REPORT: AtomicBool = AtomicBool::new(false);

/// Starts reporting crashes; called first thing, so that a crash while opening is caught too.
pub fn install() {
    OPENED.get_or_init(Instant::now);
    let Some(folder) = folder() else {
        return;
    };
    remove_old_reports(&folder);
    let _ = FOLDER.set(folder);
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        on_panic(info);
        // The default hook prints it too, to the console a debug build has.
        previous(info);
    }));
    #[cfg(windows)]
    native::install();
}

/// Where reports go: beside the panel's preferences.
fn folder() -> Option<PathBuf> {
    Some(preferences::path()?.with_file_name("crashes"))
}

/// Keeps something the panel did, for a report to show what led to a crash. `area` says what
/// it is about, such as `play` or a log entry's kind.
pub fn note(area: &str, text: &str) {
    let line = report::activity_line(system::local_time_of_day(), area, text);
    let mut activity = lock(&ACTIVITY);
    if activity.len() == ACTIVITY_LINES {
        activity.pop_front();
    }
    activity.push_back(line);
}

/// Changes what the panel says it is doing, for a report to give.
pub fn update_state(change: impl FnOnce(&mut State)) {
    change(&mut lock(&STATE));
}

/// Whether a report was written since last asked, while the panel kept running.
pub fn take_new_report() -> bool {
    NEW_REPORT.swap(false, Ordering::Relaxed)
}

/// Reports written after `seen`, by file name without extension, the newest first.
#[must_use]
pub fn unread_reports(seen: Option<&str>) -> Vec<PathBuf> {
    let Some(folder) = FOLDER.get() else {
        return Vec::new();
    };
    report::unread(report_stems(folder), seen)
        .into_iter()
        .map(|stem| folder.join(stem).with_extension("txt"))
        .collect()
}

/// The names, without extension, of the reports in `folder`.
fn report_stems(folder: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "txt"))
        .filter_map(|path| path.file_stem()?.to_str().map(str::to_owned))
        .filter(|stem| stem_time(stem).is_some())
        .collect()
}

/// Removes all but the newest reports, with their minidumps, which are several megabytes each.
fn remove_old_reports(folder: &Path) {
    for stem in report::unread(report_stems(folder), None)
        .iter()
        .skip(REPORTS_KEPT)
    {
        for extension in ["txt", "dmp"] {
            let _ = fs::remove_file(folder.join(stem).with_extension(extension));
        }
    }
}

fn on_panic(info: &PanicHookInfo<'_>) {
    let message = info.payload_as_str().unwrap_or("(no message)");
    // Rust's own second panic when the first reaches Windows' code, which it cannot pass
    // through: its trace is the first one's again.
    let backtrace = (message != "panic in a function that cannot unwind")
        .then(|| report::trim_backtrace(&Backtrace::force_capture().to_string()));
    let what = What::Panic {
        thread: thread_name(),
        message: message.to_owned(),
        location: info.location().map(ToString::to_string),
    };
    save(&what, backtrace.as_deref(), None, None);
}

fn thread_name() -> String {
    let current = std::thread::current();
    current.name().map_or_else(
        || format!("{:?}", current.id()),
        std::borrow::ToOwned::to_owned,
    )
}

/// This run's report file: the one already written, or a new one's name, unused so far.
fn report_path(written: Option<&MutexGuard<'_, Option<PathBuf>>>) -> Option<PathBuf> {
    if let Some(path) = written.and_then(|written| written.as_ref()) {
        return Some(path.clone());
    }
    let folder = FOLDER.get()?;
    let stem = report::file_stem(system::local_now());
    // Two reports in one second, as from two runs crashing at once, each keep their own.
    (1..100).find_map(|count| {
        let name = if count == 1 {
            stem.clone()
        } else {
            format!("{stem}-{count}")
        };
        let path = folder.join(name).with_extension("txt");
        (!path.exists()).then_some(path)
    })
}

/// Writes the report of a crash, or adds it to this run's report. `dump` is the minidump
/// written for it, or why it could not be, and `chosen` the report's path, picked beside it.
fn save(
    what: &What,
    backtrace: Option<&str>,
    dump: Option<Result<&str, &str>>,
    chosen: Option<PathBuf>,
) {
    // A crash while the lock is held, as during this very function, must not wait on it.
    let mut written = try_lock(&WRITTEN);
    let Some(path) = chosen.or_else(|| report_path(written.as_ref())) else {
        return;
    };
    let opened_for = OPENED.get().map(Instant::elapsed).unwrap_or_default();
    let adding = written.as_ref().is_some_and(|written| written.is_some());
    let text = if adding {
        report::another(what, opened_for, backtrace)
    } else {
        gather(what, backtrace, dump)
    };
    let servers = try_lock(&STATE)
        .map(|state| state.servers.clone())
        .unwrap_or_default();
    let text = privacy::hide_servers(&text, servers.iter().map(String::as_str));
    let home = std::env::var("USERPROFILE").ok();
    let text = privacy::hide_in_text(&text, home.as_deref());
    if let Some(folder) = path.parent() {
        let _ = fs::create_dir_all(folder);
        let readme = folder.join("README.txt");
        if !readme.exists() {
            let _ = fs::write(readme, README);
        }
    }
    let file = OpenOptions::new().create(true).append(true).open(&path);
    if file
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .is_ok()
    {
        if let Some(written) = written.as_mut() {
            **written = Some(path);
        }
        NEW_REPORT.store(true, Ordering::Relaxed);
    }
}

/// A full report, from what the panel last said it was doing.
fn gather(what: &What, backtrace: Option<&str>, dump: Option<Result<&str, &str>>) -> String {
    let state = try_lock(&STATE);
    let activity = try_lock(&ACTIVITY).map(|activity| activity.iter().cloned().collect::<Vec<_>>());
    let preferences = preferences::path().and_then(|path| fs::read_to_string(path).ok());
    let windows = system::windows_version();
    let commit = env!("B3313_PANEL_COMMIT");
    let build = format!(
        "b3313-panel {}, commit {}, {} build",
        env!("CARGO_PKG_VERSION"),
        if commit.is_empty() { "unknown" } else { commit },
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    Report {
        what,
        when: system::local_now(),
        opened_for: OPENED.get().map(Instant::elapsed).unwrap_or_default(),
        build: &build,
        windows: windows.as_deref(),
        memory: system::memory(),
        state: state.as_deref(),
        backtrace,
        dump,
        activity: activity.as_deref(),
        preferences: preferences.as_deref(),
    }
    .text()
}

/// Waits for a lock, taking over one a panic left behind: what it guards is only ever replaced
/// whole, so it is never half-changed.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Takes a lock if it is free: during a crash, whoever holds it may be the failing thread.
fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
        Err(TryLockError::WouldBlock) => None,
    }
}

/// A crash to make on purpose, if `B3313_PANEL_CRASH_TEST` asks for one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Test {
    /// A panic on the panel's own thread, which closes it.
    Panic,
    /// A panic on another thread, after which the panel keeps running.
    Thread,
    /// An access violation, which only Windows can catch.
    Native,
    /// A stack overflow, which leaves the failing thread no stack to report with.
    Overflow,
}

impl Test {
    #[must_use]
    pub fn asked() -> Option<Self> {
        match std::env::var(TEST_VARIABLE).ok()?.as_str() {
            "panic" => Some(Self::Panic),
            "thread" => Some(Self::Thread),
            "native" => Some(Self::Native),
            "overflow" => Some(Self::Overflow),
            _ => None,
        }
    }

    #[expect(clippy::panic, reason = "crashing is this test's purpose")]
    pub fn run(self) {
        note("panel", &format!("Crashing on purpose: {self:?}"));
        match self {
            Self::Panic => panic!("a crash test, on the panel's thread"),
            Self::Thread => {
                let _ = std::thread::Builder::new()
                    .name("crash test".to_owned())
                    .spawn(|| panic!("a crash test, on a thread of its own"));
            }
            #[cfg(windows)]
            Self::Native => native::raise_access_violation(),
            #[cfg(windows)]
            Self::Overflow => {
                std::hint::black_box(overflow(0));
            }
            #[cfg(not(windows))]
            Self::Native | Self::Overflow => {}
        }
    }
}

/// Calls itself until the stack runs out; each call keeps a block of stack for itself.
#[cfg(windows)]
#[expect(unconditional_recursion, reason = "running out of stack is the point")]
fn overflow(depth: u64) -> u64 {
    let block = std::hint::black_box([depth; 512]);
    overflow(depth + 1).wrapping_add(block.iter().sum())
}
