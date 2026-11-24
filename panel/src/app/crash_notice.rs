//! The panel's side of crash reports: telling the user a report was written, on the next start
//! or at once if part of the panel failed while it kept running, and keeping the crash reporter
//! told of what the panel is doing.

use super::messages::ToastTone;
use super::{Panel, display_name, play_page::PlayState};
use crate::crash::{self, stem_time};
use crate::format::{self, clock};
use crate::icons::IconName;
use crate::theme::Theme;
use crate::widgets::{Button, Tone, banner};
use gpui::{AnyElement, ClipboardItem, Context, div, prelude::*};
use std::path::{Path, PathBuf};
use std::time::Duration;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Crash reports the user has not been told of yet.
#[derive(Debug)]
pub(super) struct CrashNotice {
    /// The newest report.
    newest: PathBuf,
    count: usize,
    /// Written during this run, by part of the panel that failed while the rest kept going.
    during_run: bool,
}

impl Panel {
    /// Looks for reports written since the user last saw one; `during_run` when the crash
    /// reporter just wrote one.
    pub(super) fn find_crash_reports(&mut self, during_run: bool) {
        let mut unread = crash::unread_reports(self.preferences.seen_crash.as_deref());
        // A later failure of this run is written into the report its first one made, which
        // the user may have been told of and dismissed: it is news all the same.
        if during_run && unread.is_empty() {
            unread.extend(crash::this_runs_report());
        }
        self.crash_notice = unread.first().map(|newest| CrashNotice {
            newest: newest.clone(),
            count: unread.len(),
            during_run,
        });
    }

    /// The user has been told of the reports; the notice goes until there is a new one.
    fn crash_reports_seen(&mut self, cx: &mut Context<Self>) {
        if let Some(notice) = self.crash_notice.take() {
            self.preferences.seen_crash = file_stem(&notice.newest);
            self.save_preferences();
        }
        cx.notify();
    }

    fn copy_crash_report(&mut self, path: &Path, cx: &mut Context<Self>) {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.show_toast(ToastTone::Success, "Crash report copied".to_owned());
            }
            Err(error) => self.show_toast(
                ToastTone::Error,
                format!("Could not read the crash report: {error}"),
            ),
        }
        self.crash_reports_seen(cx);
    }

    /// Across the top of every page while there are reports the user has not seen.
    pub(super) fn render_crash_notice(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let notice = self.crash_notice.as_ref()?;
        // As `9 November at 18:45:24`.
        let when = file_stem(&notice.newest)
            .and_then(|stem| stem_time(&stem))
            .map(|time| {
                let month = MONTHS
                    .get(usize::from(time.month).saturating_sub(1))
                    .copied()
                    .unwrap_or_default();
                let of_day = Duration::from_secs(
                    u64::from(time.hour) * 3600
                        + u64::from(time.minute) * 60
                        + u64::from(time.second),
                );
                format!(
                    "{} {month} at {}",
                    time.day,
                    format::time_of_day(of_day, self.hour24)
                )
            });
        let text = match (notice.count, when) {
            _ if notice.during_run => "Part of the panel failed just now and saved a crash \
                                       report. Restart the panel if something stopped working."
                .to_owned(),
            (1, Some(when)) => {
                format!("The panel crashed on {when} and saved a report of what went wrong.")
            }
            (1, None) => "The panel crashed and saved a report of what went wrong.".to_owned(),
            (count, Some(when)) => format!(
                "The panel crashed {count} times since you last looked; the newest report is \
                 from {when}."
            ),
            (count, None) => format!("The panel crashed {count} times since you last looked."),
        };
        let open = notice.newest.clone();
        let copy = notice.newest.clone();
        let reveal = notice.newest.clone();
        let actions = div()
            .flex()
            .flex_none()
            .gap_2()
            .child(
                Button::new("crash-open", "Open report")
                    .icon(IconName::FileText)
                    .small()
                    .tooltip("Opens the newest crash report in your text editor")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.open_with_system(&open);
                        this.crash_reports_seen(cx);
                    })),
            )
            .child(
                Button::new("crash-copy", "Copy")
                    .icon(IconName::Copy)
                    .small()
                    .tooltip("Copies the newest report, to paste into a bug report or a message")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_crash_report(&copy, cx);
                    })),
            )
            .child(
                Button::new("crash-folder", "Show in folder")
                    .small()
                    .tooltip(format!(
                        "Shows the reports in File Explorer, with their minidumps (.dmp). The \
                         reports can be shared. {}",
                        crash::DUMP_WARNING
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.reveal_path(&reveal);
                        this.crash_reports_seen(cx);
                    })),
            )
            .child(
                Button::new("crash-dismiss", "")
                    .icon(IconName::X)
                    .tone(Tone::Quiet)
                    .small()
                    .tooltip("Dismiss; the reports stay in their folder")
                    .on_click(cx.listener(|this, _, _, cx| this.crash_reports_seen(cx))),
            );
        Some(
            div()
                .px_8()
                .pt_4()
                .child(
                    banner(theme.danger, theme.danger_soft, IconName::CircleAlert, text)
                        .child(actions),
                )
                .into_any_element(),
        )
    }

    /// Tells the crash reporter what the panel is doing now, for a report to give.
    pub(super) fn share_state(&self) {
        let server = self.running.as_ref().map_or_else(
            || "stopped".to_owned(),
            |running| {
                let players = self.dashboard.player_count();
                format!(
                    "running on {} for {}, {players} {}",
                    running.host.address(),
                    clock(running.started.elapsed()),
                    if players == 1 { "player" } else { "players" }
                )
            },
        );
        let play = match &self.play.state {
            PlayState::Idle => "not joined".to_owned(),
            PlayState::WaitingForGame => "waiting for the game to start".to_owned(),
            PlayState::Joining => "joining".to_owned(),
            PlayState::Reconnecting { .. } => {
                "joining again after the server went silent".to_owned()
            }
            PlayState::Playing { player, since } => format!(
                "playing as player {} for {}",
                player.get(),
                clock(since.elapsed())
            ),
            PlayState::Ended { reason, .. } => format!("ended: {reason}"),
        };
        // Its name only: the folders above it can name the user.
        let settings_file = self.form.path().map(display_name);
        let running_settings = self.running.as_ref().map(|running| running.config.format());
        let servers = self.known_servers().map(str::to_owned).collect();
        crash::update_state(|state| {
            state.page = self.page.title();
            state.server = server;
            state.play = play;
            state.settings_file = settings_file;
            state.running_settings = running_settings;
            state.servers = servers;
        });
    }
}

/// A report's file name without its extension.
fn file_stem(path: &Path) -> Option<String> {
    path.file_stem()?.to_str().map(str::to_owned)
}
