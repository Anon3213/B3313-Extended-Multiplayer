//! The sidebar: the server's state, the pages and the settings file, in full or as icons. Its
//! edge is dragged to change its width, or far enough to collapse it.

use super::arrange::icon_button;
use super::{NOT_SAVED, Page, Panel, display_name, play_page};
use crate::format::clock;
use crate::icons::{IconName, icon};
use crate::preferences::{self, SIDEBAR_COLLAPSED_WIDTH};
use crate::system;
use crate::theme::{BASE_REM, Theme, scaled};
use crate::widgets::{FocusRing as _, OneLine as _, Tip, badge, dismiss_tooltips, dot, ease_out};
use gpui::{
    AnyElement, Context, CursorStyle, Div, FontWeight, MouseButton, MouseMoveEvent, SharedString,
    Stateful, Window, div, prelude::*, px,
};
use std::time::{Duration, Instant};

/// How long the sidebar takes to collapse or expand.
const SLIDE: Duration = Duration::from_millis(160);

/// A change between icons only and in full, under way: the sidebar's width goes from `from`
/// to whatever it is to be now, so a change of mind halfway turns back from where it is.
#[derive(Clone, Copy, Debug)]
pub(super) struct Slide {
    from: f32,
    started: Instant,
}

impl Slide {
    /// The width `progress` of the way from `from` to `to`, and whether it is still moving.
    fn width(self, to: f32) -> (f32, bool) {
        let progress = self.started.elapsed().as_secs_f32() / SLIDE.as_secs_f32();
        if progress >= 1.0 {
            (to, false)
        } else {
            (self.from + (to - self.from) * ease_out(progress), true)
        }
    }
}

impl Panel {
    /// The sidebar's width at 100%, once any slide is over.
    fn sidebar_width(&self) -> f32 {
        if self.preferences.sidebar_collapsed {
            f32::from(SIDEBAR_COLLAPSED_WIDTH)
        } else {
            f32::from(self.preferences.sidebar_width)
        }
    }

    /// The sidebar's width at 100% as drawn now, part way through a slide.
    pub(super) fn shown_sidebar_width(&self) -> f32 {
        let to = self.sidebar_width();
        self.sidebar_slide.map_or(to, |slide| slide.width(to).0)
    }

    /// Collapses the sidebar to icons, or expands it, sliding from where it is.
    fn set_collapsed_now(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if collapsed == self.preferences.sidebar_collapsed {
            return;
        }
        // Whatever moves away from under the mouse would leave its tooltip behind.
        dismiss_tooltips(cx);
        self.sidebar_slide = system::animations_on().then(|| Slide {
            from: self.shown_sidebar_width(),
            started: Instant::now(),
        });
        self.preferences.sidebar_collapsed = collapsed;
    }

    /// Shows the sidebar with only icons, or in full, and remembers it.
    pub(super) fn set_sidebar_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        self.set_collapsed_now(collapsed, cx);
        self.save_preferences();
        cx.notify();
    }

    /// Makes the expanded sidebar `width` pixels wide at 100%, and remembers it.
    pub(super) fn set_sidebar_width(&mut self, width: u16, cx: &mut Context<Self>) {
        self.preferences.sidebar_width = width;
        self.save_preferences();
        cx.notify();
    }

    fn start_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        self.sidebar_drag = Some(self.preferences.sidebar_width);
        // The edge follows the mouse from here, not a slide.
        self.sidebar_slide = None;
        cx.notify();
    }

    /// Follows the mouse while the sidebar's edge is dragged. The sidebar starts at the
    /// window's left, so the mouse's distance from it is the width. Far enough left it
    /// collapses, keeping the width it had for when it expands; dragged back out, it expands.
    pub(super) fn drag_sidebar(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        // A button let go outside the window sends no release here.
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_sidebar_drag(cx);
            return;
        }
        let Some(width_before) = self.sidebar_drag else {
            return;
        };
        let pixels = event.position.x / window.rem_size() * BASE_REM;
        match preferences::dragged_sidebar(pixels) {
            None if !self.preferences.sidebar_collapsed => {
                self.preferences.sidebar_width = width_before;
                self.set_collapsed_now(true, cx);
            }
            None => return,
            Some(width) => {
                self.set_collapsed_now(false, cx);
                if width == self.preferences.sidebar_width && self.sidebar_slide.is_none() {
                    return;
                }
                self.preferences.sidebar_width = width;
            }
        }
        cx.notify();
    }

    /// Ends a drag of the sidebar's edge, remembering where it was left.
    pub(super) fn finish_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_drag.take().is_some() {
            self.save_preferences();
            cx.notify();
        }
    }

    pub(super) fn render_sidebar(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let collapsed = self.preferences.sidebar_collapsed;
        let (width, sliding) = self
            .sidebar_slide
            .map_or((self.sidebar_width(), false), |slide| {
                slide.width(self.sidebar_width())
            });
        if sliding {
            window.request_animation_frame();
        }
        let (status_tip, file_tip) = self.sidebar_tips();
        let nav: Vec<AnyElement> = Page::ALL
            .into_iter()
            .map(|page| self.render_nav_item(&theme, page, collapsed, cx))
            .collect();
        // Every icon keeps its place from the left in both states, so sliding between them
        // moves only the edge, uncovering or hiding the words beside the icons.
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_none()
            .w(scaled(width))
            .h_full()
            .overflow_hidden()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .p_4()
            .gap_6()
            .when(!collapsed, |sidebar| {
                sidebar.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .pt_2()
                        .child(
                            div()
                                .one_line()
                                // The size at which the whole name fits a sidebar of the
                                // usual width; at 17 its last letters were cut off.
                                .text_size(scaled(15.))
                                .font_weight(FontWeight::BOLD)
                                .child("B3313 Extended Multiplayer"),
                        )
                        .child(
                            div()
                                .one_line()
                                .text_size(scaled(13.))
                                .text_color(theme.text_muted)
                                .child("Control panel"),
                        ),
                )
            })
            .child(self.render_server_status(&theme, status_tip, cx))
            .children(self.render_session_status(&theme, cx))
            .children(self.render_streamer_badge(&theme, collapsed, cx))
            .child(div().flex().flex_col().gap_1().children(nav))
            .child(div().flex_1())
            .child(self.render_sidebar_footer(&theme, file_tip, cx))
            .child(self.render_sidebar_edge(&theme, cx))
            .into_any_element()
    }

    /// The server's state, in view on every page; a restart it waits for shows here too, since
    /// the banner that explains it is only on the overview and settings. Collapsed, the dot.
    fn render_server_status(
        &self,
        theme: &Theme,
        tip: String,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (status, color, background) = match &self.running {
            Some(_) if self.needs_restart() => (
                "Running · restart to apply".to_owned(),
                theme.warning,
                theme.warning_soft,
            ),
            Some(running) => (
                format!("Running · {}", self.shown_address(running.host.address())),
                theme.success,
                theme.success_soft,
            ),
            None => ("Stopped".to_owned(), theme.text_faint, theme.surface),
        };
        let status_box = div()
            .id("server-status")
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(scaled(38.))
            .px(scaled(15.))
            .rounded_md()
            .bg(background)
            .border_1()
            .border_color(theme.border)
            .text_size(scaled(13.))
            .child(dot(color));
        if self.preferences.sidebar_collapsed {
            status_box.tip(format!("Server: {tip}"), cx)
        } else {
            status_box
                .tip(tip, cx)
                .child(div().one_line().child(status))
        }
    }

    /// How this PC's own session stands, under the server's status and in its shape, so it
    /// is in view on every page: someone watching the Overview would otherwise not see that
    /// their own game had lost the server, or been removed. Nothing while there is no session
    /// to speak of.
    fn render_session_status(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let (status, color) = self.session_summary(theme)?;
        let status_box = div()
            .id("session-status")
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(scaled(38.))
            .px(scaled(15.))
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .text_size(scaled(13.))
            .child(dot(color));
        Some(if self.preferences.sidebar_collapsed {
            status_box.tip(format!("Your game: {status}"), cx)
        } else {
            status_box
                .tip(format!("Your game: {status}. The Play page has more."), cx)
                .child(div().one_line().child(status))
        })
    }

    /// One page's entry in the sidebar, with its count or a mark for settings to save or fix.
    /// Collapsed, it is the icon alone, its name and count in the tooltip and its mark, or a
    /// dot for someone playing, at the icon's corner.
    fn render_nav_item(
        &self,
        theme: &Theme,
        page: Page,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = page == self.page;
        let count = match page {
            Page::Players => Some(self.dashboard.player_count()),
            Page::Bans => Some(self.bans.bans().len()),
            _ => None,
        }
        .filter(|count| *count > 0);
        let marker = match page {
            Page::Play => Page::play_mark(&self.play.state, theme).map(dot),
            Page::Settings if !self.field_errors.is_empty() => Some(dot(theme.danger)),
            Page::Settings if self.form.is_dirty() => Some(dot(theme.accent)),
            _ => None,
        };
        let (hover, accent) = (theme.hover, theme.accent);
        let (text, glyph, weight) = if selected {
            (theme.text, theme.accent, FontWeight::SEMIBOLD)
        } else {
            (theme.text_muted, theme.text_faint, FontWeight::NORMAL)
        };
        // Someone playing is the one count worth a color: it says the session is live.
        let count_colors = match page {
            Page::Players => (theme.success, theme.success_soft),
            _ => (theme.text_muted, theme.hover),
        };
        // What a count or mark beside the name means; items without one need no tooltip.
        let explain = match (page, count) {
            (Page::Play, _) => match &self.play.state {
                play_page::PlayState::Playing { player, .. } => Some(format!(
                    "Green: playing as {}",
                    self.shown_text(&self.play.called(*player))
                )),
                play_page::PlayState::WaitingForGame | play_page::PlayState::Joining => {
                    Some("Amber: joining a session".to_owned())
                }
                play_page::PlayState::Reconnecting { .. } => {
                    Some("Amber: the server went silent; joining it again".to_owned())
                }
                _ => None,
            },
            (Page::Players, Some(1)) => Some("1 player in the session".to_owned()),
            (Page::Players, Some(count)) => Some(format!("{count} players in the session")),
            (Page::Bans, Some(1)) => Some("1 address banned".to_owned()),
            (Page::Bans, Some(count)) => Some(format!("{count} addresses banned")),
            (Page::Settings, _) if !self.field_errors.is_empty() => {
                Some("Red: some settings need fixing".to_owned())
            }
            (Page::Settings, _) if self.form.is_dirty() => {
                Some("Blue: changes not saved yet (Ctrl+S)".to_owned())
            }
            _ => None,
        };
        let keys = page.shortcut();
        let tip = match (collapsed, explain) {
            (true, Some(text)) => format!("{} ({keys}): {text}", page.title()),
            (true, None) => format!("{} ({keys})", page.title()),
            (false, Some(text)) => format!("{text} · {keys}"),
            (false, None) => keys.to_owned(),
        };
        let item = div()
            .id(SharedString::from(page.title()))
            .tip(tip, cx)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(scaled(36.))
            .px_3()
            .rounded_md()
            .tab_index(0)
            .cursor_pointer()
            .text_color(text)
            .font_weight(weight)
            .when(selected, |item| item.bg(theme.accent_soft))
            .when(!selected, |item| item.hover(move |style| style.bg(hover)))
            .focus_ring(move |style| style.border_1().border_color(accent))
            .on_click(cx.listener(move |this, _, _, cx| this.go_to(page, cx)))
            .child(icon(page.icon(), 16., glyph));
        if collapsed {
            let live = (page == Page::Players && count.is_some()).then(|| dot(theme.success));
            item.children(
                marker
                    .or(live)
                    .map(|mark| mark.absolute().top(scaled(5.)).left(scaled(27.))),
            )
            .into_any_element()
        } else {
            item.child(div().flex_1().one_line().child(page.title()))
                .children(marker)
                .children(
                    count.map(|count| badge(count.to_string(), count_colors.0, count_colors.1)),
                )
                .into_any_element()
        }
    }

    /// What the sidebar's server state and settings file say in full, on hover.
    fn sidebar_tips(&self) -> (String, String) {
        let status = match &self.running {
            Some(running) if self.needs_restart() => format!(
                "Running on {} with the settings it started with; the edited ones apply after \
                 a restart",
                self.shown_address(running.host.address())
            ),
            Some(running) => format!(
                "Running on {} for {}",
                self.shown_address(running.host.address()),
                clock(running.started.elapsed())
            ),
            None => "The server is stopped; start it from the Overview".to_owned(),
        };
        let file = self.form.path().map_or_else(
            || format!("{NOT_SAVED}: Save on Server settings keeps them"),
            |path| self.shown_path(path),
        );
        (status, file)
    }

    /// While streamer mode is on, a reminder in the sidebar on every page; selecting it turns
    /// streamer mode off. Collapsed, it is the icon alone.
    fn render_streamer_badge(
        &self,
        theme: &Theme,
        collapsed: bool,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.preferences.streamer_mode {
            return None;
        }
        let accent = theme.accent;
        Some(
            div()
                .id("streamer-mode")
                .tip(
                    "Streamer mode: IP addresses, server names and your user folder are hidden. \
                     Select, or \
                     press Ctrl+Shift+H, to show them again.",
                    cx,
                )
                .flex()
                .flex_none()
                .items_center()
                .gap_2()
                .h(scaled(36.))
                .px(scaled(11.))
                .rounded_md()
                .bg(theme.accent_soft)
                .border_1()
                .border_color(gpui::transparent_black())
                .text_size(scaled(13.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .tab_index(0)
                .cursor_pointer()
                .hover(|style| style.bg(theme.hover))
                .focus_ring(move |style| style.border_color(accent))
                .on_click(cx.listener(|this, _, _, cx| this.set_streamer_mode(false, cx)))
                .child(icon(IconName::EyeOff, 16., theme.accent))
                .when(!collapsed, |badge| {
                    badge.child(div().flex_1().one_line().child("Streamer mode"))
                })
                .into_any_element(),
        )
    }

    /// The settings file at the sidebar's foot, with the buttons for recent messages and for
    /// collapsing or expanding, kept in one place in both states. Collapsed, the file is an
    /// icon naming it on hover.
    fn render_sidebar_footer(
        &self,
        theme: &Theme,
        file_tip: String,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let collapsed = self.preferences.sidebar_collapsed;
        // No direction in words: a tooltip shown right after a click is built from the frame
        // before it. The icon shows which way it goes.
        let glyph = if collapsed {
            IconName::PanelLeftOpen
        } else {
            IconName::PanelLeftClose
        };
        let tip = "Shows only icons, or page names and details again (Ctrl+B)";
        let toggle = icon_button(theme, "sidebar-toggle", glyph, FOOTER_BUTTON)
            .tip(tip, cx)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_sidebar_collapsed(!collapsed, cx);
            }));
        let history_tip = if self.history.unseen {
            "Recent messages, with some you have not seen"
        } else {
            "Recent messages"
        };
        let history = icon_button(theme, "history-button", IconName::Bell, FOOTER_BUTTON)
            .relative()
            // None while the list is open: it would cover what the bell just opened.
            .when(!self.history.open, |button| button.tip(history_tip, cx))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_history(cx)))
            .when(self.history.unseen, |button| {
                button.child(
                    dot(theme.accent)
                        .absolute()
                        .top(scaled(5.))
                        .right(scaled(5.)),
                )
            });
        // The panel's version, last of all: the number alone where there is room for no more.
        let version = div()
            .id("panel-version")
            .tip(
                format!(
                    "This panel is version {}. Preferences has more.",
                    crate::VERSION
                ),
                cx,
            )
            .max_w_full()
            .text_color(theme.text_faint);
        if collapsed {
            // In a column as wide as the icons above, centered under them.
            return div()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .w(scaled(40.))
                .child(
                    div()
                        .id("sidebar-settings-file")
                        .tip(format!("Settings file: {file_tip}"), cx)
                        .py_2()
                        .child(icon(IconName::FileText, 16., theme.text_faint)),
                )
                .child(history)
                .child(toggle)
                .child(
                    version
                        .text_size(scaled(10.))
                        .child(div().one_line().child(crate::VERSION)),
                )
                .into_any_element();
        }
        let file = self
            .form
            .path()
            .map_or_else(|| NOT_SAVED.to_owned(), display_name);
        let file_and_buttons = div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("sidebar-settings-file")
                    .tip(file_tip, cx)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .text_size(scaled(12.))
                    .text_color(theme.text_faint)
                    .child("Settings file")
                    .child(div().text_color(theme.text_muted).one_line().child(file)),
            )
            .child(history)
            .child(toggle);
        let version = version.text_size(scaled(12.)).child(
            div()
                .one_line()
                .child(format!("Version {}", crate::VERSION)),
        );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(file_and_buttons)
            .child(version)
            .into_any_element()
    }

    /// The sidebar's right edge, dragged to change its width or to collapse or expand it. A
    /// line shows there while the mouse is over it or dragging; the window follows the drag
    /// from then on.
    fn render_sidebar_edge(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let accent = theme.accent;
        let tip = if self.preferences.sidebar_collapsed {
            "Drag out to show the sidebar in full"
        } else {
            "Drag to change the sidebar's width, or far left to show only icons"
        };
        div()
            .id("sidebar-edge")
            .group("sidebar-edge")
            .tip(tip, cx)
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .w(scaled(6.))
            .flex()
            .justify_end()
            .cursor(CursorStyle::ResizeLeftRight)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    // Keep the drag from selecting text or pressing what it passes over.
                    window.prevent_default();
                    cx.stop_propagation();
                    this.start_sidebar_drag(cx);
                }),
            )
            .child(
                div()
                    .w(px(2.))
                    .h_full()
                    .when(self.sidebar_drag.is_some(), |line| line.bg(accent))
                    .group_hover("sidebar-edge", move |style| style.bg(accent)),
            )
            .into_any_element()
    }
}

/// How large the buttons at the sidebar's foot are.
const FOOTER_BUTTON: f32 = 32.;
