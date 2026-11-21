//! The chat as the panel shows it: lines in a box that scrolls, the newest at the bottom, with
//! the field to type in and its button in the same frame. The Play page, the Players page and
//! the Overview's Chat panel all show one.

use super::Panel;
use super::play_page::role_icon;
use crate::format;
use crate::icons::{IconName, icon};
use crate::text_input::TextInput;
use crate::theme::{MONO, Theme, scaled};
use b3313_protocol::{PlayerId, Role};
use gpui::{
    AnyElement, Context, DispatchPhase, Div, Entity, FontWeight, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, ScrollHandle, ScrollWheelEvent, SharedString, canvas,
    div, point, prelude::*, px,
};
use std::collections::VecDeque;
use std::time::Duration;

/// Lines a chat keeps; older ones are dropped.
const LINES_KEPT: usize = 200;
/// How tall the list of lines is, at 100%.
const LIST_HEIGHT: f32 = 220.;

/// A line of chat.
#[derive(Clone, Debug)]
pub(super) struct ChatEntry {
    /// The time of day it arrived.
    pub(super) at: Duration,
    /// Who said it, or nobody for a line from the server or its owner.
    pub(super) from: Option<PlayerId>,
    /// The role its speaker had as it arrived. Kept with the line: looked up when the line
    /// is drawn, it would be the role of whoever has the speaker's number by then.
    pub(super) role: Option<Role>,
    pub(super) name: String,
    pub(super) text: String,
}

/// One of the panel's two chats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Chat {
    /// What the server hears, where its owner speaks as the server.
    Server,
    /// The session joined from the Play page, where the user speaks as their player.
    Session,
}

impl Chat {
    /// Who a line typed into this chat is from, and what a slash does there.
    const fn speaker(self) -> &'static str {
        match self {
            Self::Server => {
                "You speak as the server's owner: lines show as [Server]. A slash gives the \
                 server a command."
            }
            Self::Session => "You speak as your player, as in the game. /help lists commands.",
        }
    }

    /// The mark that goes with it: the server's, or the game's pad, as the Play page has.
    /// Not the crown, which is the host's, a player.
    const fn speaker_icon(self) -> IconName {
        match self {
            Self::Server => IconName::Server,
            Self::Session => IconName::Gamepad,
        }
    }
}

/// A scrollbar's handle being dragged: where the mouse took it, and where the list was then.
#[derive(Clone, Copy, Debug)]
struct Drag {
    from: Pixels,
    offset: Pixels,
}

/// Where a list's scrollbar handle is, down from the list's top, and how tall.
struct Thumb {
    top: Pixels,
    height: Pixels,
}

/// The shortest a scrollbar's handle gets, however many lines there are.
const THUMB_MIN: Pixels = px(24.);

/// One chat's lines, oldest first, and where its list is scrolled to.
#[derive(Debug, Default)]
pub(super) struct ChatLog {
    lines: VecDeque<ChatEntry>,
    scroll: ScrollHandle,
    /// Clicked in, so the mouse wheel is its list's alone even with nothing to scroll.
    selected: bool,
    drag: Option<Drag>,
}

impl ChatLog {
    pub(super) fn push(&mut self, entry: ChatEntry) {
        if self.lines.len() == LINES_KEPT {
            self.lines.pop_front();
        }
        self.lines.push_back(entry);
    }

    /// Keeps the newest line in view if it was when the list was last drawn, so that neither a
    /// new line nor a narrower window, which wraps lines onto more rows, leaves it below the
    /// edge; a reader who has scrolled up to older lines stays there.
    fn follow_newest(&self) {
        let below = self.scroll.max_offset().height + self.scroll.offset().y;
        if below < px(1.) && self.drag.is_none() {
            self.scroll.scroll_to_bottom();
        }
    }

    /// Whether the mouse wheel over the box is kept from the page: once clicked in, or while
    /// the list had lines out of view when last drawn.
    fn holds_wheel(&self) -> bool {
        self.selected || self.scroll.max_offset().height >= px(1.)
    }

    /// The scrollbar's handle, if the lines passed the list's height when last drawn.
    fn thumb(&self) -> Option<Thumb> {
        let hidden = self.scroll.max_offset().height;
        let view = self.scroll.bounds().size.height;
        if hidden < px(1.) || view <= THUMB_MIN {
            return None;
        }
        let height = (view * (view / (view + hidden))).max(THUMB_MIN);
        let top = (view - height) * (-self.scroll.offset().y / hidden).clamp(0., 1.);
        Some(Thumb { top, height })
    }

    /// Scrolls the list as its scrollbar's handle follows the mouse to `y`.
    fn drag_to(&self, y: Pixels) {
        let (Some(drag), Some(thumb)) = (self.drag, self.thumb()) else {
            return;
        };
        let hidden = self.scroll.max_offset().height;
        let travel = self.scroll.bounds().size.height - thumb.height;
        if travel < px(1.) {
            return;
        }
        // The list goes up as the handle goes down, the whole of what is hidden over the
        // handle's whole travel.
        let offset = drag.offset - hidden * ((y - drag.from) / travel);
        let x = self.scroll.offset().x;
        self.scroll
            .set_offset(point(x, offset.clamp(-hidden, px(0.))));
    }

    pub(super) fn clear(&mut self) {
        self.lines.clear();
    }

    pub(super) fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub(super) fn lines(&self) -> impl Iterator<Item = &ChatEntry> {
        self.lines.iter()
    }
}

impl Panel {
    /// One line of chat: its time, who said it, with their mark if they are the host or a
    /// moderator, and what; a line from nobody is fainter.
    pub(super) fn chat_line(&self, theme: &Theme, entry: &ChatEntry, role: Option<Role>) -> Div {
        let speaker = (!entry.name.is_empty()).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap_1()
                .font_weight(FontWeight::SEMIBOLD)
                .children(role.and_then(|role| role_icon(theme, role)))
                // The server's owner, who is no player and has no role's mark.
                .when(entry.from.is_none(), |speaker| {
                    speaker.child(icon(IconName::Server, 13., theme.text_muted))
                })
                .child(self.shown_text(&entry.name))
        });
        div()
            .flex()
            .gap_3()
            .when(self.preferences.compact, Styled::py_0p5)
            .when(!self.preferences.compact, Styled::py_1)
            .text_size(scaled(13.))
            .child(
                div()
                    .w(scaled(84.))
                    .flex_none()
                    .font_family(MONO)
                    .text_size(scaled(12.))
                    .text_color(theme.text_faint)
                    .child(format::time_of_day(entry.at, self.hour24)),
            )
            .children(speaker)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(entry.from.is_none() && entry.name.is_empty(), |text| {
                        text.text_color(theme.text_muted)
                    })
                    .child(self.shown_text(&entry.text)),
            )
    }
}

/// A chat's box as one place shows it.
#[derive(Clone, Copy)]
pub(super) struct ChatBox {
    pub(super) chat: Chat,
    /// Tells this box from the same chat's on another page.
    pub(super) id: &'static str,
    /// What the list says while it has no lines.
    pub(super) empty: &'static str,
}

impl Panel {
    fn chat_log(&self, chat: Chat) -> &ChatLog {
        match chat {
            Chat::Server => &self.server_chat,
            Chat::Session => &self.play.chat,
        }
    }

    fn chat_log_mut(&mut self, chat: Chat) -> &mut ChatLog {
        match chat {
            Chat::Server => &mut self.server_chat,
            Chat::Session => &mut self.play.chat,
        }
    }

    /// A chat's box: `lines` in a list that scrolls, with a scrollbar once they pass its
    /// height, over the field to type in and its button.
    ///
    /// The mouse wheel over the box scrolls only its list, not the page as well, while the
    /// list has lines out of view; with none, the wheel is the page's, so the box is no dead
    /// spot in it. A click in the box selects it, shown by its border, and then it keeps the
    /// wheel even so. A click or a turn of the wheel anywhere else lets go.
    pub(super) fn chat_box(
        &self,
        theme: &Theme,
        ChatBox { chat, id, empty }: ChatBox,
        lines: Vec<Div>,
        (input, send): (&Entity<TextInput>, AnyElement),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let log = self.chat_log(chat);
        log.follow_newest();
        let list = div()
            .id(id)
            .overflow_y_scroll()
            .track_scroll(&log.scroll)
            .h(scaled(LIST_HEIGHT))
            .pl_3()
            // Clear of the scrollbar.
            .pr_4()
            .py_2()
            // Not a flex column: that would squeeze the lines to fit, not scroll them.
            .when(lines.is_empty(), |list| {
                list.child(
                    div()
                        .text_size(scaled(13.))
                        .text_color(theme.text_faint)
                        .child(empty),
                )
            })
            .children(lines);
        let select = cx.listener(move |this, _: &MouseDownEvent, _, cx| {
            let log = this.chat_log_mut(chat);
            if !log.selected {
                log.selected = true;
                cx.notify();
            }
        });
        div()
            .relative()
            .flex()
            .flex_col()
            .rounded_md()
            .border_1()
            .border_color(if log.selected {
                theme.accent
            } else {
                theme.border
            })
            .bg(theme.surface_sunken)
            .on_mouse_down(MouseButton::Left, select)
            .on_mouse_down_out(cx.listener(move |this, _, _, cx| this.deselect_chat(chat, cx)))
            // The list has scrolled by now, and the page would too.
            .when(log.holds_wheel(), |chat_box| {
                chat_box.on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            })
            .child(self.chat_watcher(chat, cx))
            .child(
                div()
                    .relative()
                    .child(list)
                    .children(self.chat_scrollbar(theme, chat, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .p_2()
                    .border_t_1()
                    .border_color(theme.border)
                    // Who the line will be from, said where it is typed: the panel has two
                    // chats that look alike, and they speak as different people.
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .text_size(scaled(12.))
                            .text_color(theme.text_muted)
                            .child(icon(chat.speaker_icon(), 13., theme.text_muted))
                            .child(chat.speaker()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().flex_1().child(input.clone()))
                            .child(send),
                    ),
            )
            .into_any_element()
    }

    fn deselect_chat(&mut self, chat: Chat, cx: &mut Context<Self>) {
        let log = self.chat_log_mut(chat);
        if log.selected {
            log.selected = false;
            cx.notify();
        }
    }

    /// Hears the mouse wherever it is in the window, which the box's own listeners do not: the
    /// wheel turned outside a selected box lets go of it, and a scrollbar being dragged
    /// follows the mouse past the box's edges, until its button comes up.
    fn chat_watcher(&self, chat: Chat, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let log = self.chat_log(chat);
        let (selected, dragging) = (log.selected, log.drag.is_some());
        let panel = cx.entity().downgrade();
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                if selected {
                    let panel = panel.clone();
                    window.on_mouse_event(move |event: &ScrollWheelEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture && !bounds.contains(&event.position) {
                            // The panel is only gone as the window closes.
                            let _ = panel.update(cx, |this, cx| this.deselect_chat(chat, cx));
                        }
                    });
                }
                if dragging {
                    let moved = panel.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            // A button let go outside the window sends no release: without
                            // this the list would follow the mouse until the next click.
                            let held = event.pressed_button == Some(MouseButton::Left);
                            let _ = moved.update(cx, |this, cx| {
                                if held {
                                    this.chat_log(chat).drag_to(event.position.y);
                                } else {
                                    this.chat_log_mut(chat).drag = None;
                                }
                                cx.notify();
                            });
                        }
                    });
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = panel.update(cx, |this, cx| {
                                this.chat_log_mut(chat).drag = None;
                                cx.notify();
                            });
                        }
                    });
                }
            },
        )
        .absolute()
        .size_full()
    }

    /// The list's scrollbar, once its lines pass its height: where the list is among them, and
    /// a handle to drag there.
    fn chat_scrollbar(
        &self,
        theme: &Theme,
        chat: Chat,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let log = self.chat_log(chat);
        let thumb = log.thumb()?;
        let held = log.drag.is_some();
        Some(
            div()
                .id(SharedString::from(format!("{chat:?}-scrollbar")))
                .absolute()
                .top(thumb.top)
                .right(scaled(3.))
                .w(scaled(6.))
                .h(thumb.height)
                .rounded_full()
                .bg(if held {
                    theme.text_muted
                } else {
                    theme.text_faint
                })
                .hover(|thumb| thumb.bg(theme.text_muted))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        let log = this.chat_log_mut(chat);
                        log.drag = Some(Drag {
                            from: event.position.y,
                            offset: log.scroll.offset().y,
                        });
                        cx.notify();
                    }),
                ),
        )
    }
}
