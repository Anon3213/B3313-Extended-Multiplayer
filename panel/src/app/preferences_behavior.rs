//! The Preferences page's cards for how the panel behaves: its messages and confirmations, its
//! alerts, what it does on opening, and putting every preference back to its default.

use super::Panel;
use super::overview::{hint, kind_chip};
use super::preferences_page::{choice, setting_row, stepper_value};
use crate::dashboard::Kind;
use crate::icons::IconName;
use crate::preferences::{
    MESSAGE_SECONDS_DEFAULT, MESSAGE_SECONDS_MAX, MESSAGE_SECONDS_MIN, StartPage,
};
use crate::theme::{Theme, scaled};
use crate::widgets::{Button, Tone, card, card_header, switch};
use b3313_bridge::text::ChatSize;
use b3313_bridge::typing::{CHAT_LINES_MIN, DEFAULT_CHAT_LINES};
use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*};

/// What Reset all puts back, and what it leaves.
const RESET_MEANING: &str = "Everything on this page back to how it came. The window's place, \
                             recent files and the Overview's layout stay; the Overview has its \
                             own Reset.";

/// What the About card says of the project, as its README does.
const ABOUT_MEANING: &str = "An unofficial project, not affiliated with Nintendo or the B3313 \
                             team. Released as it is, free to use, change and pass on, with no \
                             promise of support or further development. The panel never checks \
                             for a newer version by itself.";

impl Panel {
    /// How long messages stay, whether stopping asks first, and whether views are kept.
    pub(super) fn render_behavior(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        card(theme)
            .p_5()
            .gap_4()
            .child(card_header(
                theme,
                "Behavior",
                Some("Messages, confirmations, and what the panel keeps for next time."),
            ))
            .child(self.render_message_time(theme, cx))
            .child(switch(
                theme,
                "confirm-stop",
                "Ask before stopping with players connected",
                "Stop and Restart disconnect everyone, so they ask first while anyone is \
                 playing. Off, they act at once.",
                self.preferences.confirm_stop,
                cx.listener(|this, _, _, cx| {
                    this.preferences.confirm_stop = !this.preferences.confirm_stop;
                    this.save_preferences();
                    cx.notify();
                }),
            ))
            .child(switch(
                theme,
                "open-port",
                "Ask the router to open the server's port",
                "When the server starts on your network, the panel asks your router (UPnP) to \
                 let players over the internet reach it, and to close the port again when the \
                 server stops. Off, forward the port yourself. Applies the next time the \
                 server starts.",
                self.preferences.open_port,
                cx.listener(|this, _, _, cx| {
                    this.preferences.open_port = !this.preferences.open_port;
                    this.save_preferences();
                    cx.notify();
                }),
            ))
            .child(switch(
                theme,
                "remember-views",
                "Remember sorting and filters",
                "The Players and Bans tables' sorting, and the Log's sorting and kinds, as you \
                 left them. Off, they start as usual each time.",
                self.preferences.remember_views,
                cx.listener(|this, _, _, cx| {
                    let on = !this.preferences.remember_views;
                    this.set_remember_views(on, cx);
                }),
            ))
            .into_any_element()
    }

    /// The chat as the player's own game has it: the key that opens it and how long its lines
    /// stay. A card of its own, as it is what the Play page's Chat settings leads to.
    pub(super) fn render_game_chat(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        card(theme)
            .p_5()
            .gap_4()
            .child(card_header(
                theme,
                "Chat in the game",
                Some("For your own game, when you join a session from the Play page."),
            ))
            .child(self.render_chat_key(theme, cx))
            .child(self.render_chat_time(theme, cx))
            .child(self.render_chat_lines(theme, cx))
            .child(self.render_chat_size(theme, cx))
            .child(switch(
                theme,
                "chat-on-hover",
                "Show the chat while the mouse rests on it",
                "Resting the mouse on the chat's corner of the game's window shows its last \
                 lines, as opening the chat does. Off, the mouse is never looked at. Applies \
                 the next time you join.",
                self.preferences.chat_on_hover,
                cx.listener(|this, _, _, cx| {
                    this.preferences.chat_on_hover = !this.preferences.chat_on_hover;
                    this.save_preferences();
                    cx.notify();
                }),
            ))
            .into_any_element()
    }

    /// How many rows of chat the game shows at once as lines arrive.
    fn render_chat_lines(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.preferences.chat_lines;
        let choices = (CHAT_LINES_MIN..=DEFAULT_CHAT_LINES).map(|lines| {
            let tooltip = if lines == 1 {
                "The game shows the newest row of chat alone".to_owned()
            } else {
                format!("The game shows up to {lines} rows of chat at once")
            };
            choice(
                SharedString::from(format!("chat-lines-{lines}")),
                &lines.to_string(),
                lines == chosen,
                &tooltip,
                cx.listener(move |this, _, _, cx| {
                    this.preferences.chat_lines = lines;
                    this.save_preferences();
                    cx.notify();
                }),
            )
        });
        setting_row(
            theme,
            "Chat lines on screen",
            "How many rows of chat the game shows at once as lines arrive, so that fewer cover \
             the game; a long line takes more than one. Opening the chat still shows six. \
             Applies the next time you join.",
            choices,
        )
    }

    /// How large the game draws the chat.
    fn render_chat_size(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.preferences.chat_size;
        let choices = [
            (
                ChatSize::Normal,
                "Normal",
                "The chat at the size of the game's own text",
            ),
            (
                ChatSize::Large,
                "Large",
                "The chat twice the size, with fewer words to a row",
            ),
        ]
        .into_iter()
        .map(|(size, label, tooltip)| {
            choice(
                SharedString::from(format!("chat-size-{}", size.name())),
                label,
                size == chosen,
                tooltip,
                cx.listener(move |this, _, _, cx| {
                    this.preferences.chat_size = size;
                    this.save_preferences();
                    cx.notify();
                }),
            )
        });
        setting_row(
            theme,
            "Chat text size",
            "How large the game draws the chat and the line you type; names over the players \
             and the player list stay as they are. While the chat is open, Page Up and Page \
             Down scroll back through what was said, and the arrow keys bring back what you \
             sent. Applies the next time you join.",
            choices,
        )
    }

    /// Which key opens the chat in the game, or none: the usual ones, and one set in the
    /// preferences file by hand if it is none of them.
    fn render_chat_key(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.preferences.chat_key;
        let mut keys = vec![None, Some('t'), Some('y'), Some('c')];
        if !keys.contains(&chosen) {
            keys.push(chosen);
        }
        let choices = keys.into_iter().map(|key| {
            let label = key.map_or("Off".to_owned(), |key| key.to_ascii_uppercase().to_string());
            let tooltip = key.map_or_else(
                || "The keyboard is never read; chat here in the panel".to_owned(),
                |_| format!("{label} opens the chat in the game"),
            );
            choice(
                SharedString::from(format!("chat-key-{label}")),
                &label,
                key == chosen,
                &tooltip,
                cx.listener(move |this, _, _, cx| {
                    this.preferences.chat_key = key;
                    this.save_preferences();
                    cx.notify();
                }),
            )
        });
        setting_row(
            theme,
            "Chat key in the game",
            "While the game's window is in front, this key opens the chat there, and / opens it \
             for a command; Enter sends the line. The keyboard is read only then. In a game the \
             panel starts, the emulator's hotkeys on typing keys are off, so that a line does \
             not set them off. Applies the next time you join.",
            choices,
        )
    }

    /// How long the game shows a line of chat: a few usual times, and one set in the
    /// preferences file by hand if it is none of them.
    fn render_chat_time(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self.preferences.chat_seconds;
        let mut times = vec![5, 8, 15, 30, 60];
        if !times.contains(&chosen) {
            times.push(chosen);
            times.sort_unstable();
        }
        let choices = times.into_iter().map(|seconds| {
            choice(
                SharedString::from(format!("chat-time-{seconds}")),
                &format!("{seconds} s"),
                seconds == chosen,
                &format!("Chat stays on the game's screen for {seconds} seconds"),
                cx.listener(move |this, _, _, cx| {
                    this.preferences.chat_seconds = seconds;
                    this.save_preferences();
                    cx.notify();
                }),
            )
        });
        setting_row(
            theme,
            "Chat in the game stays for",
            "How long lines of chat show in the game after the newest arrives; then they go \
             until someone speaks again. Opening the chat there shows the last six whenever. \
             Applies the next time you join.",
            choices,
        )
    }

    /// How long a message stays at the corner, a second at a time; the value goes back to the
    /// default.
    fn render_message_time(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let seconds = self.preferences.message_seconds;
        let step = |id: &'static str, label: &'static str, to: u8| {
            let enabled = to != seconds;
            let tooltip = match (id, enabled) {
                ("messages-shorter", true) => format!("Shorter: {to} s"),
                ("messages-shorter", false) => "Already the shortest".to_owned(),
                (_, true) => format!("Longer: {to} s"),
                (_, false) => "Already the longest".to_owned(),
            };
            Button::new(id, label)
                .small()
                .enabled(enabled)
                .tooltip(tooltip)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.preferences.message_seconds = to;
                    this.save_preferences();
                    cx.notify();
                }))
                .into_any_element()
        };
        let is_default = seconds == MESSAGE_SECONDS_DEFAULT;
        let controls = vec![
            step(
                "messages-shorter",
                "−",
                seconds.saturating_sub(1).max(MESSAGE_SECONDS_MIN),
            ),
            stepper_value(
                Button::new("messages-default", format!("{seconds} s"))
                    .small()
                    .tone(Tone::Quiet)
                    .enabled(!is_default)
                    .tooltip(if is_default {
                        "The usual time".to_owned()
                    } else {
                        format!("Back to {MESSAGE_SECONDS_DEFAULT} s")
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.preferences.message_seconds = MESSAGE_SECONDS_DEFAULT;
                        this.save_preferences();
                        cx.notify();
                    })),
            ),
            step(
                "messages-longer",
                "+",
                (seconds + 1).min(MESSAGE_SECONDS_MAX),
            ),
        ];
        setting_row(
            theme,
            "Messages",
            "How long confirmations and problems stay at the corner; with Undo, 6 s longer. The \
             bell keeps the last 20.",
            controls,
        )
    }

    /// Flashing the taskbar while the panel is behind, and for which kinds of entry.
    pub(super) fn render_alerts(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let on = self.preferences.alerts;
        // Reports come every few seconds; alerting for them would never stop.
        let chips: Vec<AnyElement> = Kind::ALL
            .into_iter()
            .filter(|kind| *kind != Kind::Report)
            .map(|kind| {
                let chosen = self.preferences.alert_kinds.contains(&kind);
                kind_chip(
                    theme,
                    SharedString::from(format!("alert-{}", kind.key())),
                    kind,
                    (chosen, None),
                    cx,
                    cx.listener(move |this, _, _, cx| {
                        let kinds = &mut this.preferences.alert_kinds;
                        if let Some(at) = kinds.iter().position(|k| *k == kind) {
                            kinds.remove(at);
                        } else {
                            kinds.push(kind);
                        }
                        this.save_preferences();
                        cx.notify();
                    }),
                )
            })
            .collect();
        card(theme)
            .p_5()
            .gap_3()
            .child(card_header(
                theme,
                "Alerts",
                Some("For when the panel is behind the game or other windows."),
            ))
            .child(switch(
                theme,
                "alerts",
                "Flash the taskbar button",
                "For the kinds of entry chosen below. It flashes until you bring the panel to \
                 the front.",
                on,
                cx.listener(|this, _, _, cx| {
                    this.preferences.alerts = !this.preferences.alerts;
                    this.save_preferences();
                    cx.notify();
                }),
            ))
            .child(switch(
                theme,
                "alert-sound",
                "Play a sound",
                "For the same kinds of entry, and when your own game loses its server or is \
                 removed from a session: heard while the game is in front, where the taskbar \
                 is not seen. It is Windows' own notice sound, at the volume set there.",
                self.preferences.alert_sound,
                cx.listener(|this, _, _, cx| {
                    this.preferences.alert_sound = !this.preferences.alert_sound;
                    this.save_preferences();
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    // Still choosable while alerts are off, but faint, as they wait.
                    .when(!on, |kinds| kinds.opacity(0.55))
                    .child(div().font_weight(FontWeight::MEDIUM).child("Alert for"))
                    .child(div().flex().flex_wrap().gap_2().children(chips))
                    .child(hint(
                        theme,
                        "Problems include the server stopping on its own. Reports, every few \
                         seconds, are never alerted.",
                    )),
            )
            .into_any_element()
    }

    /// The settings file and server to bring back, and the page to show first.
    pub(super) fn render_startup(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let address = self.shown_text(
            self.preferences
                .listen_address
                .as_deref()
                .unwrap_or(super::DEFAULT_ADDRESS),
        );
        let pages: Vec<AnyElement> = StartPage::ALL
            .into_iter()
            .map(|page| {
                let label = super::Page::from(page).title();
                choice(
                    SharedString::from(format!("start-page-{}", page.key())),
                    label,
                    page == self.preferences.start_page,
                    &format!("Open on {label}"),
                    cx.listener(move |this, _, _, cx| {
                        this.preferences.start_page = page;
                        this.save_preferences();
                        cx.notify();
                    }),
                )
            })
            .collect();
        card(theme)
            .p_5()
            .gap_3()
            .child(card_header(
                theme,
                "When the panel opens",
                Some("A settings file named when starting the panel always comes first."),
            ))
            .child(switch(
                theme,
                "reopen-settings",
                "Open the last settings file",
                "The file last opened or saved, from the recent files on Server settings.",
                self.preferences.reopen_settings,
                cx.listener(|this, _, _, cx| {
                    this.preferences.reopen_settings = !this.preferences.reopen_settings;
                    this.save_preferences();
                    cx.notify();
                }),
            ))
            .child(switch(
                theme,
                "start-server",
                "Start the server",
                &format!(
                    "On {address}, the address it last started on, unless the settings file \
                     cannot be opened."
                ),
                self.preferences.start_server,
                cx.listener(|this, _, _, cx| {
                    this.preferences.start_server = !this.preferences.start_server;
                    this.save_preferences();
                    cx.notify();
                }),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_1()
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Page shown first"),
                    )
                    .child(div().flex().flex_wrap().gap_2().children(pages)),
            )
            .into_any_element()
    }

    /// What this program is and which version of it, with the way to the project's page.
    pub(super) fn render_about(theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let version = format!("B3313 Extended Multiplayer, version {}", crate::VERSION);
        card(theme)
            .p_5()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_4()
                    .child(div().flex_1().min_w(scaled(240.)).child(card_header(
                        theme,
                        "About",
                        Some(&version),
                    )))
                    .child(
                        Button::new("project-page", "Project page")
                            .icon(IconName::Globe)
                            .tooltip(format!(
                                "Opens {} in your browser: the newest version, what each \
                                 changed, and the source",
                                crate::PROJECT_PAGE
                            ))
                            .on_click(cx.listener(|_, _, _, cx| cx.open_url(crate::PROJECT_PAGE))),
                    ),
            )
            .child(hint(theme, ABOUT_MEANING))
            .into_any_element()
    }

    /// Every preference on this page back to its default, with Undo.
    pub(super) fn render_reset(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let usual = self.preferences.reset() == self.preferences;
        card(theme)
            .p_5()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_4()
                    .child(div().flex_1().min_w(scaled(240.)).child(card_header(
                        theme,
                        "Reset preferences",
                        Some(RESET_MEANING),
                    )))
                    .child(
                        Button::new("reset-preferences", "Reset all")
                            .icon(IconName::RotateCcw)
                            .enabled(!usual)
                            .tooltip(if usual {
                                "Every preference is already as it came"
                            } else {
                                "Back to the defaults; Undo brings yours back"
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.reset_preferences(window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}
