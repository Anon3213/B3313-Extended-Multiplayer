//! Everything the server reported, filtered by kind, searchable, and sortable by column.

use super::column_resize::TableColumn;
use super::messages::ToastTone;
use super::overview::{hint, kind_chip, kind_icon};
use super::{Panel, Selected, before_dialog, display_name, page_frame};
use crate::columns::{self, Widths};
use crate::dashboard::{Dashboard, Kind, LogColumn, LogEntry};
use crate::icons::{IconName, icon};
use crate::sort::{Sort, SortOrder};
use crate::theme::{MONO, Theme, scaled};
use crate::widgets::{
    Button, COLUMN_HANDLE_WIDTH, FocusRing as _, OneLine as _, SCROLL_LINE, ScrollKey, Tip, Tone,
    card, sort_heading,
};
use gpui::{
    AnyElement, App, Context, FontWeight, KeyDownEvent, ScrollStrategy, SharedString,
    UniformListScrollHandle, Window, div, prelude::*, uniform_list,
};
use std::fmt::Write as _;
use std::ops::Range;
use std::rc::Rc;

/// The kind's icon, before the columns, whose widths are the user's (`columns`); the entry
/// takes the rest.
const KIND_ICON: f32 = 14.;

/// The Time and Kind columns' widths as the user has them, at 100%.
fn widths_of(widths: &Widths<LogColumn>) -> (f32, f32) {
    let width = |column| widths.of(column).map_or(0., f32::from);
    (width(LogColumn::Time), width(LogColumn::Kind))
}

/// Entries of the log picked out for one view of it, by their place in the log, and kept
/// until the log changes or the view asks for others: the log holds thousands of entries,
/// and a page is drawn many times for each change to it.
#[derive(Debug)]
pub(super) struct Picked<K> {
    /// The log's generation and what the view asked for when `entries` were picked.
    picked_for: Option<(u64, K)>,
    entries: Rc<[usize]>,
}

impl<K> Default for Picked<K> {
    fn default() -> Self {
        Self {
            picked_for: None,
            entries: Rc::default(),
        }
    }
}

impl<K: PartialEq> Picked<K> {
    /// The entries for what is `asked` of the log at `generation`: those picked last time if
    /// neither has changed since, and otherwise what `pick` picks now.
    pub(super) fn get(
        &mut self,
        generation: u64,
        asked: K,
        pick: impl FnOnce(&K) -> Vec<usize>,
    ) -> Rc<[usize]> {
        let wanted = (generation, asked);
        if self.picked_for.as_ref() != Some(&wanted) {
            self.entries = pick(&wanted.1).into();
            self.picked_for = Some(wanted);
        }
        Rc::clone(&self.entries)
    }
}

/// What decides which entries the Log shows, and in what order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Asked {
    kinds: Vec<Kind>,
    /// What is searched for, in lower case; empty for no search.
    query: String,
    sort: Sort<LogColumn>,
}

/// The Log's list, and the views of the log that are kept between draws.
#[derive(Debug, Default)]
pub(super) struct LogList {
    scroll: UniformListScrollHandle,
    shown: Picked<Asked>,
    /// What the open player's details list of what they did.
    pub(super) activity: Picked<Selected>,
}

/// The log entries the filters and search let through, by their place in the log, in the
/// order the log is sorted.
fn pick_shown(dashboard: &Dashboard, asked: &Asked) -> Vec<usize> {
    let query = asked.query.as_str();
    // Kinds whose name matches, found once rather than for every entry.
    let kinds_named: Vec<Kind> = Kind::ALL
        .into_iter()
        .filter(|kind| kind.label().to_lowercase().contains(query))
        .collect();
    let mut shown: Vec<usize> = dashboard
        .log()
        .iter()
        .enumerate()
        .filter(|(_, entry)| asked.kinds.contains(&entry.kind))
        .filter(|(_, entry)| {
            query.is_empty() || kinds_named.contains(&entry.kind) || entry.folded.contains(query)
        })
        .map(|(index, _)| index)
        .collect();
    dashboard.sort_log(&mut shown, asked.sort);
    shown
}

/// Whether the Log follows its newest entries after `key` scrolled it: only once the key
/// has taken it to the end they arrive at, which is the bottom oldest first and the top
/// newest first. Sorted by anything but time, the newest is nowhere in particular.
fn follows_after(key: ScrollKey, sort: Sort<LogColumn>) -> bool {
    let to_newest = match sort.order {
        SortOrder::Ascending => ScrollKey::End,
        SortOrder::Descending => ScrollKey::Home,
    };
    sort.column == LogColumn::Time && key == to_newest
}

impl Panel {
    /// The log entries shown, by their place in the log, in the order the log is sorted:
    /// picked again only when the log, the kinds, the search or the sort has changed.
    fn shown_log(&mut self, cx: &Context<Self>) -> Rc<[usize]> {
        let asked = Asked {
            kinds: self.log_kinds.clone(),
            query: self.log_search.read(cx).text().trim().to_lowercase(),
            sort: self.log_sort,
        };
        let dashboard = &self.dashboard;
        self.log_list
            .shown
            .get(dashboard.log_generation(), asked, |asked| {
                pick_shown(dashboard, asked)
            })
    }

    /// Scrolls the Log's list as `key` asks, while it has the focus. As with the wheel, a
    /// reader who scrolls is reading, and is not pulled back to the newest entry; going to
    /// the newest with Home or End follows them again.
    fn scroll_log(&mut self, key: ScrollKey, window: &Window, cx: &mut Context<Self>) {
        let list = self.log_list.scroll.0.borrow().base_handle.clone();
        key.scroll_list(&list, scaled(SCROLL_LINE).to_pixels(window.rem_size()));
        self.follow_log = follows_after(key, self.log_sort);
        cx.notify();
    }

    /// Saves the entries shown, as text, where the user picks.
    fn export_log(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut text = String::new();
        for entry in self
            .shown_log(cx)
            .iter()
            .filter_map(|&index| self.dashboard.log().get(index))
        {
            let line = entry.text.replace('\n', " · ");
            // Writing to a String cannot fail.
            let _ = writeln!(
                text,
                "{:>11}  {:<10}  {line}",
                self.entry_time(entry),
                entry.kind.label()
            );
        }
        // The project keeps run logs in logs/, so start there when the panel runs from it.
        let directory = std::env::current_dir().unwrap_or_default();
        let logs = directory.join("logs");
        let directory = if logs.is_dir() { logs } else { directory };
        before_dialog(window);
        let chosen = cx.prompt_for_new_path(&directory, Some("panel-log.txt"));
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(path))) = chosen.await {
                let saved = std::fs::write(&path, text);
                let _ = this.update(cx, |this, cx| {
                    match saved {
                        Ok(()) => this.show_toast(
                            ToastTone::Success,
                            format!("Saved the log to {}", display_name(&path)),
                        ),
                        Err(error) => {
                            this.report_problem(format!("could not save the log: {error}"));
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// A chip for each kind of entry, and the search.
    fn render_log_filters(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let chips: Vec<AnyElement> = Kind::ALL
            .into_iter()
            .map(|kind| {
                let count = self.dashboard.kind_count(kind);
                filter_chip(theme, kind, self.log_kinds.contains(&kind), count, cx)
            })
            .collect();
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .children(chips)
            .child(div().flex_1())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w(scaled(260.))
                    .child(icon(IconName::Search, 16., theme.text_faint))
                    .child(div().flex_1().child(self.log_search.clone())),
            )
            .into_any_element()
    }

    /// The log's headings, each sorting by its column.
    fn render_log_headings(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let sort = self.log_sort;
        let widths = &self.preferences.log_widths;
        let heading = |column: LogColumn, label: &str, explain: &str| {
            let cell = sort_heading(
                theme,
                SharedString::from(format!("log-heading-{label}")),
                label,
                (sort.column == column).then_some(sort.order),
                explain,
                cx.listener(move |this, _, _, cx| {
                    this.log_sort = this.log_sort.select(column);
                    this.remember_views();
                    cx.notify();
                }),
                cx,
            );
            let Some(width) = widths.of(column) else {
                return cell.flex_1().into_any_element();
            };
            // With the line at its right edge that changes a column's width.
            let line = columns::line_after(column).map(|(changed, rightward)| {
                self.column_handle(theme, (TableColumn::Log(changed), rightward), cx)
            });
            div()
                .relative()
                .flex_none()
                .w(scaled(f32::from(width)))
                .h_full()
                .flex()
                .items_center()
                .overflow_hidden()
                .pr(scaled(COLUMN_HANDLE_WIDTH))
                .child(cell)
                .children(line)
                .into_any_element()
        };
        let time = heading(
            LogColumn::Time,
            "TIME",
            "When it happened, on this PC's clock",
        );
        let kind = heading(
            LogColumn::Kind,
            "KIND",
            "What sort of entry, in the filters' order",
        );
        let entry = heading(LogColumn::Entry, "ENTRY", "What happened, A to Z");
        div()
            .flex()
            .items_center()
            .gap_3()
            .h(scaled(34.))
            .px_4()
            .border_b_1()
            .border_color(theme.border)
            .text_size(scaled(12.))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.text_muted)
            .child(div().flex_none().w(scaled(KIND_ICON)))
            .child(time)
            .child(kind)
            .child(entry)
            .into_any_element()
    }

    pub(super) fn render_log(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let shown = self.shown_log(cx);
        // Following keeps the newest in view: at the bottom oldest first, at the top newest
        // first. Sorted by anything else, the newest is nowhere in particular.
        let by_time = self.log_sort.column == LogColumn::Time;
        if self.follow_log && by_time && !shown.is_empty() {
            let (newest, strategy) = match self.log_sort.order {
                SortOrder::Ascending => (shown.len() - 1, ScrollStrategy::Bottom),
                SortOrder::Descending => (0, ScrollStrategy::Top),
            };
            self.log_list.scroll.scroll_to_item(newest, strategy);
        }
        let accent = theme.accent;
        let actions = self.log_actions(by_time, !shown.is_empty(), cx);
        let count = shown.len();
        let searching = !self.log_search.read(cx).text().trim().is_empty();
        let list = if count == 0 {
            // Blame the filters only when there is something they hide.
            let text = if self.dashboard.log().is_empty() {
                "Nothing logged yet. Joins, kicks, bans, flags and problems show here."
            } else if searching {
                "No entries of the kinds selected match the search."
            } else {
                "No entries of the kinds selected."
            };
            div().p_8().child(hint(&theme, text)).into_any_element()
        } else {
            uniform_list(
                "log",
                count,
                cx.processor(move |this, range: Range<usize>, _, cx| {
                    let theme = *cx.global::<Theme>();
                    let widths = widths_of(&this.preferences.log_widths);
                    range
                        .filter_map(|row| shown.get(row))
                        .filter_map(|&index| Some((index, this.dashboard.log().get(index)?)))
                        .map(|(index, entry)| {
                            let shown = (this.entry_time(entry), this.entry_text(entry));
                            log_row(&theme, index, entry, shown, widths, cx)
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(self.log_list.scroll.clone())
            .size_full()
            .into_any_element()
        };
        let body = div()
            .flex()
            .flex_col()
            .size_full()
            .gap_4()
            .child(self.render_log_filters(&theme, cx))
            .child(
                card(&theme)
                    .flex_1()
                    .min_h(scaled(240.))
                    .overflow_hidden()
                    .child(self.render_log_headings(&theme, cx))
                    .child(
                        div()
                            .id("log-list")
                            .flex_1()
                            .py_1()
                            .border_1()
                            .border_color(gpui::transparent_black())
                            // Reached with Tab or a click, and then scrolled with the keys.
                            .tab_index(0)
                            .focus_ring(move |style| style.border_color(accent))
                            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                                if let Some(key) = ScrollKey::pressed(event) {
                                    cx.stop_propagation();
                                    this.scroll_log(key, window, cx);
                                }
                            }))
                            // A reader who scrolls is reading: following would pull them
                            // back to the newest entry within a second.
                            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                                if this.follow_log {
                                    this.follow_log = false;
                                    cx.notify();
                                }
                            }))
                            .child(list),
                    ),
            );
        let subtitle = format!(
            "{count} of {} entries shown · Ctrl+F to search",
            self.dashboard.log().len()
        );
        page_frame(&theme, "Log", subtitle, actions, body)
    }

    /// Export, Follow and Clear.
    fn log_actions(
        &self,
        by_time: bool,
        any_shown: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let follow = self.follow_log && by_time;
        let empty = self.dashboard.log().is_empty();
        vec![
            Button::new("export-log", "Export…")
                .icon(IconName::Download)
                .enabled(any_shown)
                .tooltip(if any_shown {
                    "Saves the entries shown, as filtered, searched and sorted, to a text file"
                } else {
                    "No entries are shown to save"
                })
                .on_click(cx.listener(|this, _, window, cx| this.export_log(window, cx)))
                .into_any_element(),
            Button::new("follow-log", if follow { "Following" } else { "Follow" })
                .tone(if follow {
                    Tone::Primary
                } else {
                    Tone::Secondary
                })
                .enabled(by_time)
                .tooltip(match (by_time, follow) {
                    (false, _) => "Sort by time to follow new entries",
                    (true, true) => {
                        "Keeping the newest entries in view; select, or scroll the log, to stop"
                    }
                    (true, false) => "Keep the newest entries in view as they arrive",
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    this.follow_log = !this.follow_log;
                    cx.notify();
                }))
                .into_any_element(),
            Button::new("clear-log", "Clear")
                .tone(Tone::Quiet)
                .enabled(!empty)
                .tooltip(if empty {
                    "The log is empty"
                } else {
                    "Empties the log in the panel; Undo puts it back"
                })
                .on_click(cx.listener(|this, _, _, cx| this.clear_log(cx)))
                .into_any_element(),
        ]
    }
}

/// Shows or hides one kind of entry.
fn filter_chip(
    theme: &Theme,
    kind: Kind,
    on: bool,
    count: usize,
    cx: &mut Context<Panel>,
) -> AnyElement {
    let select = cx.listener(move |this, _, _, cx| {
        if let Some(at) = this.log_kinds.iter().position(|k| *k == kind) {
            this.log_kinds.remove(at);
        } else {
            this.log_kinds.push(kind);
        }
        this.remember_views();
        cx.notify();
    });
    kind_chip(
        theme,
        SharedString::from(format!("filter-{}", kind.label())),
        kind,
        (on, Some(count)),
        cx,
        select,
    )
}

/// One line of the log; rows are all one height, as the list requires, so a long entry is cut
/// short and shown in full on hover.
fn log_row(
    theme: &Theme,
    index: usize,
    entry: &LogEntry,
    (time, text): (String, String),
    (time_width, kind_width): (f32, f32),
    cx: &App,
) -> AnyElement {
    div()
        .id(("log-row", index))
        .tip(text.clone(), cx)
        .flex()
        .items_center()
        .gap_3()
        // No wider than the list, or the entry runs under the card's edge and is cut there
        // without the "…" that says there is more.
        .w_full()
        .overflow_hidden()
        .h(scaled(SCROLL_LINE))
        .px_4()
        .child(kind_icon(theme, entry.kind))
        .child(
            div()
                .flex_none()
                .w(scaled(time_width))
                .one_line()
                .font_family(MONO)
                .text_size(scaled(12.))
                .text_color(theme.text_faint)
                .child(time),
        )
        .child(
            div()
                .flex_none()
                .w(scaled(kind_width))
                .one_line()
                .text_size(scaled(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(entry.kind.label()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .one_line()
                .text_size(scaled(13.))
                .child(text),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::{Asked, Picked, follows_after, pick_shown};
    use crate::dashboard::{Dashboard, Kind, LogColumn};
    use crate::sort::Sort;
    use crate::widgets::ScrollKey;
    use std::time::Instant;

    fn asked(kinds: &[Kind], query: &str, sort: Sort<LogColumn>) -> Asked {
        Asked {
            kinds: kinds.to_vec(),
            query: query.to_owned(),
            sort,
        }
    }

    /// What the Log shows of `dashboard`'s log for `asked`, through `picked`, and whether it
    /// was picked anew for this call.
    fn shown(
        picked: &mut Picked<Asked>,
        dashboard: &Dashboard,
        asked: Asked,
    ) -> (Vec<usize>, bool) {
        let mut anew = false;
        let entries = picked.get(dashboard.log_generation(), asked, |asked| {
            anew = true;
            pick_shown(dashboard, asked)
        });
        (entries.to_vec(), anew)
    }

    #[test]
    fn the_log_is_filtered_again_only_when_it_or_what_is_asked_of_it_changes() {
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        dashboard.note(Kind::Session, "Server started".to_owned(), now);
        dashboard.note(Kind::Problem, "could not save".to_owned(), now);
        dashboard.note(Kind::Session, "Luigi joined".to_owned(), now);
        let by_time = Sort::by(LogColumn::Time);
        let sessions = || asked(&[Kind::Session], "", by_time);
        let mut picked = Picked::default();

        assert_eq!(
            shown(&mut picked, &dashboard, sessions()),
            (vec![0, 2], true)
        );
        // Drawn again with nothing changed: what was picked is kept.
        assert_eq!(
            shown(&mut picked, &dashboard, sessions()),
            (vec![0, 2], false)
        );
        // A new entry, of a kind shown or not, and the list is picked again.
        dashboard.note(Kind::Session, "Luigi left".to_owned(), now);
        assert_eq!(
            shown(&mut picked, &dashboard, sessions()),
            (vec![0, 2, 3], true)
        );
        dashboard.note(Kind::Problem, "again".to_owned(), now);
        assert_eq!(
            shown(&mut picked, &dashboard, sessions()),
            (vec![0, 2, 3], true)
        );
        // Each of the kinds, the search and the sort changing picks it again.
        let both = [Kind::Session, Kind::Problem];
        assert_eq!(
            shown(&mut picked, &dashboard, asked(&both, "", by_time)),
            (vec![0, 1, 2, 3, 4], true)
        );
        assert_eq!(
            shown(&mut picked, &dashboard, asked(&both, "luigi", by_time)),
            (vec![2, 3], true)
        );
        let newest_first = by_time.select(LogColumn::Time);
        let searched = || asked(&both, "luigi", newest_first);
        assert_eq!(
            shown(&mut picked, &dashboard, searched()),
            (vec![3, 2], true)
        );
        assert_eq!(
            shown(&mut picked, &dashboard, searched()),
            (vec![3, 2], false)
        );
        // Cleared, and put back by Undo.
        let taken = dashboard.take_log();
        assert_eq!(shown(&mut picked, &dashboard, searched()), (vec![], true));
        dashboard.restore_log(taken);
        assert_eq!(
            shown(&mut picked, &dashboard, searched()),
            (vec![3, 2], true)
        );
    }

    #[test]
    fn a_search_finds_entries_by_their_kinds_name_too() {
        let now = Instant::now();
        let mut dashboard = Dashboard::new(now);
        dashboard.note(Kind::Session, "Server started".to_owned(), now);
        dashboard.note(Kind::Problem, "could not save".to_owned(), now);
        dashboard.note(Kind::Session, "a problem with the port".to_owned(), now);
        let all = asked(&Kind::ALL, "problem", Sort::by(LogColumn::Time));
        assert_eq!(pick_shown(&dashboard, &all), [1, 2]);
        // A kind that is filtered out stays out, whatever is searched for.
        let sessions = asked(&[Kind::Session], "problem", Sort::by(LogColumn::Time));
        assert_eq!(pick_shown(&dashboard, &sessions), [2]);
    }

    #[test]
    fn the_log_follows_again_only_from_the_end_its_newest_entries_are_at() {
        let oldest_first = Sort::by(LogColumn::Time);
        let newest_first = oldest_first.select(LogColumn::Time);
        assert!(follows_after(ScrollKey::End, oldest_first));
        assert!(follows_after(ScrollKey::Home, newest_first));
        // Scrolling away from them stops it, as the wheel does.
        for key in [
            ScrollKey::Up,
            ScrollKey::Down,
            ScrollKey::PageUp,
            ScrollKey::PageDown,
            ScrollKey::Home,
        ] {
            assert!(!follows_after(key, oldest_first), "{key:?}");
        }
        assert!(!follows_after(ScrollKey::End, newest_first));
        // Sorted by kind, the newest entries are at neither end.
        assert!(!follows_after(ScrollKey::End, Sort::by(LogColumn::Kind)));
    }
}
