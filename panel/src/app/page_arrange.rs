//! Putting a page's panels in the user's order: the Arrange button at the top of a page, and
//! the strip over each panel that moves it while arranging. The Overview's Customize, which
//! also adds, removes and resizes panels, is `arrange`.

use super::Panel;
use crate::icons::{IconName, icon};
use crate::page_layout::{ArrangedPage, PagePanel};
use crate::theme::{Theme, scaled};
use crate::widgets::{Button, Tone};
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, prelude::*};

impl Panel {
    /// Arrange, or Done while arranging: whether `page`'s panels show the strip that moves
    /// them.
    pub(super) fn arrange_button(&self, page: ArrangedPage, cx: &mut Context<Self>) -> AnyElement {
        let arranging = self.page_arranging == Some(page);
        let button = if arranging {
            Button::new("page-arranged", "Done")
                .tone(Tone::Primary)
                .tooltip("Finish arranging; the order is already saved")
        } else {
            Button::new("page-arrange", "Arrange")
                .tone(Tone::Quiet)
                .icon(IconName::Pencil)
                .tooltip("Choose the order of this page's panels")
        };
        button
            .on_click(cx.listener(move |this, _, _, cx| {
                this.page_arranging = (!arranging).then_some(page);
                cx.notify();
            }))
            .into_any_element()
    }

    /// `page`'s body: its panels in the user's order, each under the strip that moves it while
    /// arranging. `panels` are the page's by their keys, with nothing for one the page leaves
    /// out for now.
    pub(super) fn arranged(
        &self,
        theme: &Theme,
        page: ArrangedPage,
        panels: Vec<(&'static str, Option<AnyElement>)>,
        cx: &mut Context<Self>,
    ) -> Div {
        let mut panels: Vec<(&'static str, AnyElement)> = panels
            .into_iter()
            .filter_map(|(key, content)| Some((key, content?)))
            .collect();
        let shown: Vec<&'static str> = panels.iter().map(|(key, _)| *key).collect();
        let order = self.preferences.pages.of(page).order();
        let place = |key: &str| order.iter().position(|panel| panel.key == key);
        panels.sort_by_key(|(key, _)| place(key));
        let arranging = self.page_arranging == Some(page);
        let framed = panels.into_iter().map(|(key, content)| {
            let named = order.iter().find(|panel| panel.key == key);
            match named {
                Some(panel) if arranging => {
                    self.movable_panel(theme, (page, *panel), &shown, content, cx)
                }
                _ => content,
            }
        });
        div()
            .flex()
            .flex_col()
            .gap_5()
            .children(framed.collect::<Vec<_>>())
    }

    /// A panel under the strip that names it and moves it up or down among those `shown`.
    fn movable_panel(
        &self,
        theme: &Theme,
        (page, panel): (ArrangedPage, PagePanel),
        shown: &[&'static str],
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = self.preferences.pages.of(page);
        let mover = |later: bool, cx: &mut Context<Self>| {
            let (what, glyph) = if later {
                ("down", IconName::ArrowDown)
            } else {
                ("up", IconName::ArrowUp)
            };
            let shown = shown.to_vec();
            Button::new(
                SharedString::from(format!("move-{}-{what}", panel.key)),
                if later { "Move down" } else { "Move up" },
            )
            .icon(glyph)
            .small()
            .enabled(layout.can_move(panel.key, later, &shown))
            .tooltip(if later {
                "Puts this panel below the next one"
            } else {
                "Puts this panel above the one before it"
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.preferences
                    .pages
                    .of_mut(page)
                    .nudge(panel.key, later, &shown);
                this.save_preferences();
                cx.notify();
            }))
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(theme.accent_soft)
                    .child(icon(IconName::Grip, 14., theme.text_muted))
                    .child(
                        div()
                            .flex_1()
                            .text_size(scaled(13.))
                            .font_weight(FontWeight::MEDIUM)
                            .child(panel.title),
                    )
                    .child(mover(false, cx))
                    .child(mover(true, cx)),
            )
            .child(content)
            .into_any_element()
    }
}
