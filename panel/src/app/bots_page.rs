//! The Bots page: whether a bot on this PC may play the player's own game, and games beside it
//! for bots to play in the player's session (`crate::bot_games`), started and stopped here.

use super::overview::{hint, label};
use super::play_page::PlayState;
use super::preferences_page::{choice, stepper_value};
use super::{Page, Panel, page_frame};
use crate::bot_games::{self, BotGame, Status};
use crate::emulator;
use crate::icons::IconName;
use crate::join::Request;
use crate::page_layout::ArrangedPage;
use crate::theme::{Theme, scaled};
use crate::widgets::{Button, OneLine as _, Tone, card, card_header, dot, switch};
use b3313_bridge::bots;
use gpui::{AnyElement, Context, SharedString, div, prelude::*};

/// The bot games: how many to start, those running, and the player's request for the session
/// they join, while the player is in one.
#[derive(Debug)]
pub(super) struct BotGames {
    count: u8,
    /// Whether their Marios follow the player until a bot takes over, or wait for one.
    follow: bool,
    games: Vec<BotGame>,
    joined_with: Option<Request>,
}

impl Default for BotGames {
    fn default() -> Self {
        Self {
            count: 1,
            follow: false,
            games: Vec::new(),
            joined_with: None,
        }
    }
}

impl BotGames {
    /// Keeps the player's request as they join, for bot games to join the same session.
    pub(super) fn joining(&mut self, request: &Request) {
        self.joined_with = Some(request.clone());
    }

    /// Closes every bot game, as the player's session ends, and starts none until the player
    /// joins again.
    pub(super) fn end_session(&mut self) {
        self.joined_with = None;
        self.stop_games();
    }

    /// Takes every bot game out of the session and closes it. Done on a thread of its own:
    /// each waits for its session to leave and its emulator to close, which the window is not
    /// to wait for.
    fn stop_games(&mut self) {
        let games = std::mem::take(&mut self.games);
        if !games.is_empty() {
            // Should no thread start, they are closed here, the window waiting.
            let _ = std::thread::Builder::new()
                .name("b3313-bot-games".to_owned())
                .spawn(move || drop(games));
        }
    }

    /// Takes in what each bot game's session reported; whether anything changed.
    pub(super) fn poll(&mut self) -> bool {
        // Every game is polled, whatever the others report.
        let mut changed = false;
        for game in &mut self.games {
            changed |= game.poll();
        }
        changed
    }

    /// How many bot games are in the session, and how many are running at all.
    fn counts(&self) -> (usize, usize) {
        let playing = self
            .games
            .iter()
            .filter(|game| matches!(game.status, Status::Playing(_)))
            .count();
        let running = self
            .games
            .iter()
            .filter(|game| !matches!(game.status, Status::Ended(_)))
            .count();
        (playing, running)
    }
}

impl Panel {
    pub(super) fn render_bots_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = *cx.global::<Theme>();
        let subtitle = match self.play.bots.counts() {
            (_, 0) => "Programs on this PC that play B3313 in your session".to_owned(),
            (playing, running) => format!("{playing} of {running} bot games in the session"),
        };
        let panels = vec![
            ("yours", Some(self.render_own_bot(&theme, cx))),
            ("games", Some(self.render_bot_games(&theme, cx))),
        ];
        let body = self.arranged(&theme, ArrangedPage::Bots, panels, cx);
        let actions = vec![self.arrange_button(ArrangedPage::Bots, cx)];
        page_frame(&theme, "Bots", subtitle, actions, body)
    }

    /// Whether a bot on this PC may play the player's own game.
    fn render_own_bot(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let port = self.preferences.bot_port.unwrap_or(bots::DEFAULT_PORT);
        let detail = format!(
            "While you are in a session, a program on this PC alone can connect to \
             127.0.0.1:{port} and play your game: see where Mario and everyone else are, hold \
             its controller, chat, and warp, go to a player or follow one. BOTS.md, beside \
             this program, says how to write one. Applies the next time you join."
        );
        card(theme)
            .p_5()
            .gap_4()
            .child(card_header(
                theme,
                "Your game",
                Some("For a bot that plays your own game, with you watching or taking turns."),
            ))
            .child(switch(
                theme,
                "bot-port",
                "Let a bot play your game",
                &detail,
                self.preferences.bot_port.is_some(),
                cx.listener(move |this, _, _, cx| {
                    this.preferences.bot_port = match this.preferences.bot_port {
                        Some(_) => None,
                        None => Some(port),
                    };
                    this.save_preferences();
                    cx.notify();
                }),
            ))
            .into_any_element()
    }

    /// Games beside the player's for bots to play: how many to start, or those running.
    fn render_bot_games(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let changes: Vec<&str> = emulator::bot_changes().collect();
        let about = format!(
            "Games beside yours for bots on this PC to play, each a player of its own in your \
             session. Each is your game in an emulator of its own. {} Each takes about a tenth \
             of a processor core and 600 MB of memory, and a player's place on the server. \
             They close when you stop them or leave.",
            changes.join(" ")
        );
        let body = if !self.play.bots.games.is_empty() {
            self.bot_game_rows(theme, cx)
        } else if self.play.bridge.is_some() && self.play.bots.joined_with.is_some() {
            self.bot_game_starter(theme, cx)
        } else {
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_3()
                .child(hint(
                    theme,
                    "Join a session on the Play page first: bot games join the one you are in.",
                ))
                .child(
                    Button::new("bot-games-play", "Go to Play")
                        .small()
                        .tone(Tone::Quiet)
                        .icon(IconName::Gamepad)
                        .on_click(cx.listener(|this, _, _, cx| this.go_to(Page::Play, cx))),
                )
        };
        card(theme)
            .p_5()
            .gap_3()
            .child(card_header(theme, "Bot games", Some(&about)))
            .child(body)
            .into_any_element()
    }

    /// Starts as many bot games as chosen, in the session the player is in.
    fn start_bot_games(&mut self, cx: &mut Context<Self>) {
        let Some(joined) = self.play.bots.joined_with.clone() else {
            return;
        };
        // By the name the server gave the player, which may not be the one typed.
        let follow = match self.play.state {
            PlayState::Playing { player, .. } if self.play.bots.follow => {
                Some(self.play.called(player))
            }
            _ => None,
        };
        for number in 1..=self.play.bots.count {
            let started = bot_games::port(self.preferences.bot_port, number)
                .ok_or_else(|| "No port is left after yours for a bot game.".to_owned())
                .and_then(|port| BotGame::start(&joined, number, port, follow.clone()));
            match started {
                Ok(game) => self.play.bots.games.push(game),
                Err(message) => {
                    self.report_problem(message);
                    break;
                }
            }
        }
        cx.notify();
    }

    /// How many to start, and the button that starts them.
    fn bot_game_starter(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let count = self.play.bots.count;
        let step = |id: &'static str, text: &'static str, to: u8, tip: &str| {
            let enabled = (1..=bot_games::MOST).contains(&to) && to != count;
            Button::new(id, text)
                .small()
                .enabled(enabled)
                .tooltip(tip.to_owned())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.play.bots.count = to;
                    cx.notify();
                }))
                .into_any_element()
        };
        let games = if count == 1 { "game" } else { "games" };
        let follow = self.play.bots.follow;
        let what = [
            (
                false,
                "Wait for a bot",
                "They stay at the title screen until a bot connects and plays them",
            ),
            (
                true,
                "Follow you",
                "Their Marios get into the game and follow yours, through paintings and doors, \
                 until a bot connects and takes over",
            ),
        ]
        .into_iter()
        .map(|(on, text, tip)| {
            choice(
                SharedString::from(format!("bot-games-follow-{on}")),
                text,
                on == follow,
                tip,
                cx.listener(move |this, _, _, cx| {
                    this.play.bots.follow = on;
                    cx.notify();
                }),
            )
        });
        let starter = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(step(
                "bot-games-fewer",
                "−",
                count.saturating_sub(1),
                "One fewer",
            ))
            .child(stepper_value(
                Button::new("bot-games-count", count.to_string())
                    .small()
                    .tone(Tone::Quiet)
                    .enabled(false),
            ))
            .child(step(
                "bot-games-more",
                "+",
                count.saturating_add(1),
                &format!("One more, up to {}", bot_games::MOST),
            ))
            .child(
                Button::new("bot-games-start", format!("Start {count} bot {games}"))
                    .small()
                    .icon(IconName::Play)
                    .tooltip(
                        "Starts them, each in an emulator of its own, and joins them to your \
                         session",
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.start_bot_games(cx))),
            );
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(label(theme, "When they join"))
                    .children(what),
            )
            .child(starter)
    }

    /// Each bot game: its name, where its bot connects, and where its session is; and the
    /// button that stops them all.
    fn bot_game_rows(&self, theme: &Theme, cx: &mut Context<Self>) -> gpui::Div {
        let rows = self.play.bots.games.iter().map(|game| {
            let color = match game.status {
                Status::Playing(_) if game.has_bot => theme.success,
                Status::Playing(_) => theme.accent,
                Status::Ended(_) => theme.danger,
                _ => theme.warning,
            };
            let detail = match (&game.status, game.address) {
                (Status::Ended(_), _) => self.shown_text(&game.status.describe()),
                (status, Some(address)) => format!(
                    "{}; a bot connects to {}; {}",
                    status.describe(),
                    self.shown_address(address),
                    if game.has_bot {
                        "one is playing"
                    } else {
                        "none is connected"
                    }
                ),
                (status, None) => status.describe(),
            };
            div()
                .flex()
                .items_center()
                .gap_3()
                .min_w_0()
                .child(dot(color))
                .child(
                    div()
                        .flex_none()
                        .w(scaled(56.))
                        .child(label(theme, &game.name)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(hint(theme, &detail).one_line()),
                )
        });
        div().flex().flex_col().gap_2().children(rows).child(
            div().pt_1().child(
                Button::new("bot-games-stop", "Stop bot games")
                    .small()
                    .icon(IconName::Square)
                    .tone(Tone::Danger)
                    .tooltip("Takes them out of the session and closes their emulators")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.play.bots.stop_games();
                        cx.notify();
                    })),
            ),
        )
    }
}
