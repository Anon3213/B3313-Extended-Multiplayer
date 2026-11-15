//! Messages about what the user did or what went wrong: the one showing at the corner, and the
//! recent ones, kept so that one missed while looking away can still be read.

use super::{Panel, Toast, ToastTone};
use crate::format;
use crate::icons::{IconName, icon};
use crate::system;
use crate::theme::{Theme, scaled};
use crate::widgets::{Button, Tone, animate, dismiss_tooltips};
use gpui::{AnyElement, Context, FontWeight, MouseButton, div, prelude::*};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How many recent messages are kept.
const HISTORY: usize = 20;
/// How long a message takes to appear or go.
pub(super) const FADE: Duration = Duration::from_millis(150);

/// A message as it was shown, and when.
#[derive(Debug)]
pub(super) struct Message {
    tone: ToastTone,
    text: String,
    /// The time of day it was shown.
    at: Duration,
}

/// Recent messages, newest first, and whether any came since the user last looked.
#[derive(Debug, Default)]
pub(super) struct History {
    messages: VecDeque<Message>,
    /// Messages shown so far, numbering each.
    count: u64,
    pub(super) open: bool,
    pub(super) unseen: bool,
}

impl History {
    /// Keeps a message just shown, returning its number.
    pub(super) fn remember(&mut self, tone: ToastTone, text: &str) -> u64 {
        let area = match tone {
            ToastTone::Success => "message",
            ToastTone::Error => "error",
        };
        crate::crash::note(area, text);
        self.messages.push_front(Message {
            tone,
            text: text.to_owned(),
            at: system::local_time_of_day(),
        });
        self.messages.truncate(HISTORY);
        self.unseen = !self.open;
        self.count += 1;
        self.count
    }
}

impl Panel {
    /// Takes the message at the corner away, fading it unless Windows' animations are off.
    pub(super) fn dismiss_toast(&mut self) {
        if !system::animations_on() {
            self.toast = None;
        } else if let Some(toast) = &mut self.toast {
            toast.leaving.get_or_insert_with(Instant::now);
        }
    }

    /// Whether the message at the corner is due to go, or has finished going; done as the
    /// panel polls the server.
    pub(super) fn age_toast(&mut self, lasts: impl Fn(&Toast) -> Duration) -> bool {
        let Some(toast) = &self.toast else {
            return false;
        };
        let leaving = toast.leaving.is_some();
        let due = toast.leaving.map_or_else(
            || toast.shown.elapsed() > lasts(toast),
            |leaving| leaving.elapsed() >= FADE,
        );
        if due && leaving {
            self.toast = None;
        } else if due {
            self.dismiss_toast();
        }
        due
    }

    pub(super) fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.history.open = !self.history.open;
        self.history.unseen = false;
        // The bell's tooltip would otherwise sit over the list it just opened.
        dismiss_tooltips(cx);
        cx.notify();
    }

    pub(super) fn close_history(&mut self, cx: &mut Context<Self>) {
        if self.history.open {
            self.history.open = false;
            cx.notify();
        }
    }

    pub(super) fn render_toast(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let theme = *cx.global::<Theme>();
        let toast = self.toast.as_ref()?;
        let (color, glyph) = tone_look(&theme, toast.tone);
        let message = div()
            .absolute()
            .bottom_6()
            .right_6()
            .max_w(scaled(460.))
            .child(
                div()
                    .id("toast")
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .rounded_lg()
                    .bg(theme.surface)
                    .border_1()
                    .border_color(color)
                    .shadow_lg()
                    .text_size(scaled(13.))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.dismiss_toast();
                        cx.notify();
                    }))
                    .child(icon(glyph, 16., color))
                    // May shrink, so a long message wraps inside the box and never runs past it.
                    .child(div().flex_1().min_w_0().child(self.shown_text(&toast.text)))
                    .when(toast.undo.is_some(), |toast| {
                        toast.child(
                            Button::new("undo", "Undo")
                                .icon(IconName::Undo)
                                .small()
                                .tooltip("Puts back what this changed")
                                .on_click(cx.listener(|this, _, window, cx| this.undo(window, cx))),
                        )
                    }),
            );
        // It rises a little as it fades in, and fades where it is as it goes.
        Some(if toast.leaving.is_some() {
            animate(
                message,
                ("toast-out", toast.serial),
                FADE,
                |message, delta| message.opacity(1.0 - delta),
            )
        } else {
            animate(
                message,
                ("toast-in", toast.serial),
                FADE,
                |message, delta| message.opacity(delta).bottom(scaled(16. + 8. * delta)),
            )
        })
    }

    /// The recent messages, over the page beside the sidebar's foot, while open. A click
    /// outside closes them, as Esc does.
    pub(super) fn render_history(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.history.open {
            return None;
        }
        let theme = *cx.global::<Theme>();
        let rows: Vec<AnyElement> = self
            .history
            .messages
            .iter()
            .map(|message| self.history_row(&theme, message))
            .collect();
        let empty = rows.is_empty();
        let list = div()
            .id("history")
            .absolute()
            .bottom(scaled(16.))
            .left(scaled(self.shown_sidebar_width() + 8.))
            .w(scaled(420.))
            .max_h(scaled(480.))
            .flex()
            .flex_col()
            .rounded_lg()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .text_size(scaled(13.))
            .overflow_hidden()
            // Clicks inside stay inside, not reaching the layer that closes it.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_3()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Recent messages"),
                    )
                    .child(
                        Button::new("history-close", "Close")
                            .small()
                            .tone(Tone::Quiet)
                            .tooltip("Esc closes these too")
                            .on_click(cx.listener(|this, _, _, cx| this.close_history(cx))),
                    ),
            )
            .when(empty, |list| {
                list.child(
                    div()
                        .px_4()
                        .pb_4()
                        .text_color(theme.text_muted)
                        .child("No messages yet. Confirmations and problems show here."),
                )
            })
            .child(
                div()
                    .id("history-rows")
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .children(rows),
            );
        let list = animate(list, "history-in", FADE, Styled::opacity);
        Some(
            div()
                .id("history-backdrop")
                .absolute()
                .size_full()
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.close_history(cx)),
                )
                .child(list)
                .into_any_element(),
        )
    }
}

impl Panel {
    /// The `count` newest messages, as the recent messages list shows them.
    pub(super) fn history_rows(&self, theme: &Theme, count: usize) -> Vec<AnyElement> {
        self.history
            .messages
            .iter()
            .take(count)
            .map(|message| self.history_row(theme, message))
            .collect()
    }

    /// One recent message: its icon, what it said, and when.
    fn history_row(&self, theme: &Theme, message: &Message) -> AnyElement {
        let (color, glyph) = tone_look(theme, message.tone);
        let compact = self.preferences.compact;
        div()
            .flex()
            .items_start()
            .gap_3()
            .px_4()
            .when(compact, Styled::py_1)
            .when(!compact, Styled::py_2)
            .border_t_1()
            .border_color(theme.border)
            .child(div().pt(scaled(2.)).child(icon(glyph, 14., color)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(self.shown_text(&message.text)),
            )
            .child(
                div()
                    .flex_none()
                    .text_color(theme.text_faint)
                    .child(format::time_of_day(message.at, self.hour24)),
            )
            .into_any_element()
    }
}

/// A message's color and icon: success or problem.
fn tone_look(theme: &Theme, tone: ToastTone) -> (gpui::Hsla, IconName) {
    match tone {
        ToastTone::Success => (theme.success, IconName::CircleCheck),
        ToastTone::Error => (theme.danger, IconName::CircleAlert),
    }
}
