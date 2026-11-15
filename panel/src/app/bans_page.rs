//! Banned addresses: adding one, and letting one in again.

use super::overview::{hint, label};
use super::{Page, Panel, display_name, page_frame};
use crate::icons::IconName;
use crate::page_layout::ArrangedPage;
use crate::theme::{MONO, Theme, scaled};
use crate::widgets::{Button, Tone, card, card_header, sort_heading};
use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*};

impl Panel {
    pub(super) fn render_bans(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let count = self.bans.bans().len();
        let subtitle = match count {
            0 => "No addresses are banned".to_owned(),
            1 => "1 address is banned".to_owned(),
            count => format!("{count} addresses are banned"),
        };
        let add = self.render_ban_form(&theme, cx);
        // IP addresses in numeric order, IPv4 before IPv6.
        let mut bans = self.bans.bans().to_vec();
        bans.sort_by(|a, b| self.ban_order.apply(a.cmp(b)));
        let heading = sort_heading(
            &theme,
            "bans-heading-address",
            "ADDRESS",
            Some(self.ban_order),
            "Banned IP addresses, in numeric order",
            cx.listener(|this, _, _, cx| {
                this.ban_order = this.ban_order.flipped();
                this.remember_views();
                cx.notify();
            }),
            cx,
        );
        let rows: Vec<AnyElement> = bans
            .into_iter()
            .map(|ip| {
                div()
                    .flex()
                    .items_center()
                    .h(scaled(if self.preferences.compact { 36. } else { 48. }))
                    .px_5()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(div().flex_1().font_family(MONO).child(self.shown_ip(ip)))
                    .child(
                        Button::new(SharedString::from(format!("unban-{ip}")), "Unban")
                            .small()
                            .tooltip("Lets this address join again")
                            .on_click(cx.listener(move |this, _, _, cx| this.unban(ip, cx))),
                    )
                    .into_any_element()
            })
            .collect();
        let list = card(&theme)
            .overflow_hidden()
            .child(
                div()
                    .px_5()
                    .py_3()
                    .child(card_header(&theme, "Banned addresses", None)),
            )
            .when(rows.is_empty(), |list| {
                list.child(div().px_5().pb_4().child(hint(&theme, "Nobody is banned.")))
            })
            .when(!rows.is_empty(), |list| {
                list.child(
                    div()
                        .flex()
                        .items_center()
                        .h(scaled(34.))
                        .px_5()
                        .bg(theme.surface_sunken)
                        .text_size(scaled(12.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text_muted)
                        .child(heading),
                )
            })
            .children(rows);
        let file = self.bans.path().map(display_name);
        let subtitle = match file {
            Some(file) => format!("{subtitle} · {file}"),
            None => subtitle,
        };
        let panels = vec![("add", Some(add)), ("list", Some(list.into_any_element()))];
        let body = self.arranged(&theme, ArrangedPage::Bans, panels, cx);
        let actions = vec![self.arrange_button(ArrangedPage::Bans, cx)];
        page_frame(&theme, "Bans", subtitle, actions, body)
    }

    /// The field and button that ban an address, and where bans are kept.
    fn render_ban_form(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let storage = match self.bans.path() {
            Some(path) => format!(
                "Kept in {}, which the server reads when it starts.",
                self.shown_path(path)
            ),
            None => "No ban file is set, so bans last until the panel closes.".to_owned(),
        };
        card(theme)
            .p_5()
            .gap_4()
            .child(card_header(
                theme,
                "Ban an address",
                Some("Refuses every join from it. Anyone playing from it now is removed."),
            ))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_1p5()
                            .w(scaled(320.))
                            .child(label(theme, "IP address"))
                            .child(div().font_family(MONO).child(self.ban_input.clone())),
                    )
                    .child(
                        Button::new("ban-address", "Ban")
                            .icon(IconName::Ban)
                            .tone(Tone::Danger)
                            .tooltip(
                                "Refuses this IP address from now on (Enter in the field); asks \
                                 first if anyone is playing from it",
                            )
                            .on_click(
                                cx.listener(|this, _, window, cx| this.ban_address(window, cx)),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    // A long path wraps rather than running past the card's edge.
                    .child(hint(theme, &storage).min_w_0())
                    .when(self.bans.path().is_none(), |row| {
                        row.child(
                            Button::new("set-ban-file", "Set one")
                                .tone(Tone::Quiet)
                                .small()
                                .tooltip("Open Server settings to name a ban file")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.go_to(Page::Settings, cx)),
                                ),
                        )
                    }),
            )
            .into_any_element()
    }
}
