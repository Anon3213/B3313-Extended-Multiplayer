//! Arranging the Overview: each panel's frame, its collapsing, and Customize, where panels are
//! dragged into place, resized, removed and added. What a panel shows is `overview.rs`'s.

use super::overview::kind_chip;
use super::preferences_page::choice;
use super::{Panel, Undo};
use crate::dashboard::Kind;
use crate::icons::{IconName, icon};
use crate::overview_layout::{
    self, OverviewLayout, PanelKind, PanelState, ROW_CHOICES, Row, Width,
};
use crate::system;
use crate::theme::{Theme, scaled};
use crate::widgets::{
    Button, FocusRing as _, OneLine as _, Tip, Tone, animate, banner, card, card_header,
    dismiss_tooltips, ease_out,
};
use gpui::{
    AnyElement, Context, CursorStyle, Div, DragMoveEvent, ElementId, KeyDownEvent, Pixels,
    SharedString, Size, Stateful, Window, canvas, div, prelude::*, px,
};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How long panels take to fold, appear, go, or make room for one being dragged.
const MOTION: Duration = Duration::from_millis(160);
/// Narrower than this at 100%, half-width panels each take the whole width.
const TWO_COLUMNS_FROM: f32 = 720.;

/// What a panel shows, for its frame: a headline tile, or a card with a heading, buttons
/// beside it, and what folds away beneath.
pub(super) enum PanelContent {
    Tile(AnyElement),
    Card {
        header: AnyElement,
        actions: Vec<AnyElement>,
        body: Option<AnyElement>,
    },
}

/// The state of arranging, and of the motion that makes changes easy to follow.
#[derive(Debug, Default)]
pub(super) struct Arranging {
    /// Customize is on: panels show the strip that moves, resizes and removes them.
    pub(super) on: bool,
    drag: Option<PanelDrag>,
    /// Panels' contents' heights as last drawn, to fold them smoothly.
    body_heights: HashMap<PanelKind, Pixels>,
    /// Whole panels' sizes as last drawn, for the room a dragged one leaves.
    sizes: HashMap<PanelKind, Size<Pixels>>,
    fold: Option<Fold>,
    /// A panel just added, fading in, with its number among the changes.
    appearing: Option<(PanelKind, u64)>,
    /// A panel fading away before it is removed.
    leaving: Option<PanelKind>,
    /// Numbers each change that starts a motion, so the next one starts afresh.
    changes: u64,
}

impl Arranging {
    fn next_change(&mut self) -> u64 {
        self.changes += 1;
        self.changes
    }

    /// Has `kind` fade in, as a panel just added; returns the change's number, by which
    /// `appeared` knows it.
    fn appear(&mut self, kind: PanelKind) -> u64 {
        let change = self.next_change();
        self.appearing = Some((kind, change));
        change
    }

    /// Notes that the fading in begun as `change` is over. A panel added since is fading in
    /// still, and stays so.
    fn appeared(&mut self, change: u64) {
        if self
            .appearing
            .is_some_and(|(_, appearing)| appearing == change)
        {
            self.appearing = None;
        }
    }
}

/// A panel being dragged, and where the room for it is: `target` among the other panels.
#[derive(Clone, Copy, Debug)]
struct PanelDrag {
    kind: PanelKind,
    target: usize,
    /// Its change's number, so the room opens afresh each time it moves.
    opened: u64,
    /// Where the room was before, closing as the new room opens, and since when.
    previous: Option<(usize, Instant, u64)>,
}

/// A panel folding or unfolding.
#[derive(Clone, Copy, Debug)]
struct Fold {
    kind: PanelKind,
    started: Instant,
    opening: bool,
}

impl Fold {
    /// How far along it is, from 0 to 1 once done.
    fn progress(self) -> f32 {
        (self.started.elapsed().as_secs_f32() / MOTION.as_secs_f32()).min(1.0)
    }
}

/// What a drag carries: the panel.
#[derive(Clone, Copy, Debug)]
pub(super) struct DraggedPanel(PanelKind);

/// What follows the mouse while a panel is dragged: its name on a lifted card.
struct DragPreview(PanelKind);

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        div()
            .flex()
            .items_center()
            .gap_2()
            .w(scaled(240.))
            .px_4()
            .py_3()
            .rounded_lg()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.accent)
            .shadow_lg()
            .text_color(theme.text)
            .text_size(scaled(14.))
            .child(icon(IconName::Grip, 14., theme.text_faint))
            .child(self.0.title())
    }
}

/// One place in the Overview's sequence: a panel, with its place among the panels not being
/// dragged, or the room a dragged panel would land in, opening or closing.
#[derive(Clone, Copy)]
enum Slot<'a> {
    Panel(&'a PanelState, usize),
    Room { opening: bool, change: u64 },
}

impl Panel {
    fn layout_mut(&mut self) -> &mut OverviewLayout {
        &mut self.preferences.overview
    }

    /// Turns Customize on or off.
    pub(super) fn set_arranging(&mut self, on: bool, cx: &mut Context<Self>) {
        self.arranging.on = on;
        self.arranging.drag = None;
        cx.notify();
    }

    /// The Overview's header buttons: Customize, or while arranging, Reset and Done.
    pub(super) fn arrange_actions(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !self.arranging.on {
            return vec![
                Button::new("customize", "Customize")
                    .icon(IconName::Pencil)
                    .tone(Tone::Quiet)
                    .tooltip("Move, resize, collapse, remove and add the Overview's panels")
                    .on_click(cx.listener(|this, _, _, cx| this.set_arranging(true, cx)))
                    .into_any_element(),
            ];
        }
        let usual = self.preferences.overview == OverviewLayout::default();
        vec![
            Button::new("reset-layout", "Reset")
                .icon(IconName::RotateCcw)
                .tone(Tone::Quiet)
                .enabled(!usual)
                .tooltip(if usual {
                    "The Overview already has its usual panels"
                } else {
                    "Back to the usual panels; Undo brings yours back"
                })
                .on_click(cx.listener(|this, _, _, cx| this.reset_layout(cx)))
                .into_any_element(),
            Button::new("done-arranging", "Done")
                .tone(Tone::Primary)
                .tooltip("Finish arranging (Esc); changes are already saved")
                .on_click(cx.listener(|this, _, _, cx| this.set_arranging(false, cx)))
                .into_any_element(),
        ]
    }

    /// While arranging, how it is done.
    pub(super) fn render_arrange_hint(&self, theme: &Theme) -> Option<AnyElement> {
        self.arranging.on.then(|| {
            banner(
                theme.text,
                theme.accent_soft,
                IconName::Grip,
                "Drag a panel by its handle to move it, or select the handle and press Alt with \
                 the arrow keys. Changes are saved as you make them."
                    .to_owned(),
            )
            .into_any_element()
        })
    }

    fn reset_layout(&mut self, cx: &mut Context<Self>) {
        let yours = std::mem::take(self.layout_mut());
        self.save_preferences();
        self.show_undoable(
            "The Overview has its usual panels again".to_owned(),
            Undo::RestoreLayout(yours),
        );
        cx.notify();
    }

    /// Puts back a layout `reset_layout` replaced, or a panel `remove_panel` took.
    pub(super) fn restore_layout(&mut self, layout: OverviewLayout) {
        *self.layout_mut() = layout;
        self.save_preferences();
    }

    pub(super) fn restore_panel(&mut self, at: usize, panel: PanelState, cx: &mut Context<Self>) {
        let kind = panel.kind;
        self.layout_mut().restore(at, panel);
        self.fade_in(kind, cx);
        self.save_preferences();
    }

    fn add_panel(&mut self, kind: PanelKind, cx: &mut Context<Self>) {
        self.layout_mut().add(kind);
        self.fade_in(kind, cx);
        self.save_preferences();
        cx.notify();
    }

    /// Has `kind`'s panel fade in, once: the fading is forgotten when it is over, or the
    /// panel would fade in again whenever the Overview is next shown, as GPUI starts an
    /// animation anew for an element it has not drawn of late.
    fn fade_in(&mut self, kind: PanelKind, cx: &mut Context<Self>) {
        let change = self.arranging.appear(kind);
        cx.spawn(async move |this, cx| {
            // Twice as long as it takes: it starts when the panel is first drawn, a little
            // after this, and a panel that has faded in looks the same either way.
            cx.background_executor().timer(MOTION * 2).await;
            let _ = this.update(cx, |this, cx| {
                this.arranging.appeared(change);
                cx.notify();
            });
        })
        .detach();
    }

    /// Fades `kind` away, then removes it, offering Undo.
    fn remove_panel(&mut self, kind: PanelKind, cx: &mut Context<Self>) {
        if !kind.removable() || self.arranging.leaving.is_some() {
            return;
        }
        if !system::animations_on() {
            self.finish_removing(kind, cx);
            return;
        }
        self.arranging.leaving = Some(kind);
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(MOTION).await;
            let _ = this.update(cx, |this, cx| this.finish_removing(kind, cx));
        })
        .detach();
    }

    fn finish_removing(&mut self, kind: PanelKind, cx: &mut Context<Self>) {
        self.arranging.leaving = None;
        if let Some((at, panel)) = self.layout_mut().remove(kind) {
            self.save_preferences();
            self.show_undoable(
                format!("{} removed from the Overview", kind.title()),
                Undo::RestorePanel(at, panel),
            );
        }
        cx.notify();
    }

    fn toggle_collapsed(&mut self, kind: PanelKind, cx: &mut Context<Self>) {
        let Some(panel) = self.layout_mut().get_mut(kind) else {
            return;
        };
        panel.collapsed = !panel.collapsed;
        let opening = !panel.collapsed;
        // The chevron's tooltip says what it did, no longer what it does.
        dismiss_tooltips(cx);
        self.arranging.fold = (system::animations_on()
            && self.arranging.body_heights.contains_key(&kind))
        .then(|| Fold {
            kind,
            started: Instant::now(),
            opening,
        });
        self.save_preferences();
        cx.notify();
    }

    /// Changes one thing about `kind`'s panel, and remembers it.
    fn set_panel(
        &mut self,
        kind: PanelKind,
        change: impl FnOnce(&mut PanelState),
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self.layout_mut().get_mut(kind) {
            change(panel);
            self.save_preferences();
            cx.notify();
        }
    }

    fn nudge_panel(&mut self, kind: PanelKind, later: bool, cx: &mut Context<Self>) {
        self.layout_mut().nudge(kind, later);
        self.save_preferences();
        cx.notify();
    }

    fn start_panel_drag(&mut self, kind: PanelKind, cx: &mut Context<Self>) {
        let Some(target) = self.preferences.overview.position(kind) else {
            return;
        };
        let opened = self.arranging.next_change();
        self.arranging.drag = Some(PanelDrag {
            kind,
            target,
            opened,
            previous: None,
        });
        cx.notify();
    }

    /// Makes room for the dragged panel at `target` among the others.
    fn drag_to(&mut self, target: usize, cx: &mut Context<Self>) {
        let change = self.arranging.changes + 1;
        let Some(drag) = &mut self.arranging.drag else {
            return;
        };
        // Wait for the room to finish moving: the panels under the mouse shift as it does,
        // and would otherwise send it straight back.
        let settling = drag
            .previous
            .is_some_and(|(_, since, _)| since.elapsed() < MOTION);
        if drag.target == target || settling {
            return;
        }
        drag.previous = Some((drag.target, Instant::now(), drag.opened));
        drag.target = target;
        drag.opened = change;
        self.arranging.changes = change;
        cx.notify();
    }

    fn drop_panel(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.arranging.drag.take() {
            self.layout_mut().move_to(drag.kind, drag.target);
            self.save_preferences();
        }
        cx.notify();
    }

    /// The Overview's panels in rows: tiles side by side, half-width panels in pairs, while
    /// arranging each with its strip, and while dragging the room the panel would land in.
    pub(super) fn render_panels(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = *cx.global::<Theme>();
        // A drag let go outside the Overview ends without moving anything.
        if self.arranging.drag.is_some() && !cx.has_active_drag() {
            self.arranging.drag = None;
        }
        let page_width = window.viewport_size().width / window.rem_size() * 16.
            - self.shown_sidebar_width()
            - 64.;
        let narrow = page_width < TWO_COLUMNS_FROM;
        let slots = self.slots(window);
        let shape: Vec<(bool, Width)> = slots
            .iter()
            .map(|slot| match slot {
                Slot::Panel(panel, _) => (panel.kind.is_tile(), panel.width),
                Slot::Room { .. } => self.dragged_shape(),
            })
            .collect();
        let rows = overview_layout::rows(&shape, narrow);
        let mut elements: Vec<Option<AnyElement>> = slots
            .into_iter()
            .map(|slot| Some(self.render_slot(&theme, slot, narrow, window, cx)))
            .collect();
        let mut take = |at: usize| elements.get_mut(at).and_then(Option::take);
        let charts_hint = self.running.is_some();
        let mut hinted = false;
        rows.into_iter()
            .map(|row| match row {
                Row::Tiles(tiles) => {
                    let row = div()
                        .flex()
                        .flex_wrap()
                        .gap_4()
                        .children(tiles.into_iter().filter_map(&mut take));
                    // Said once, under the first row of charts.
                    if charts_hint && !hinted {
                        hinted = true;
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(row)
                            .child(super::overview::hint(
                                &theme,
                                "Charts show up to the last 30 minutes, a point every 10 \
                                 seconds; each runs from zero to the value at its top right.",
                            ))
                            .into_any_element()
                    } else {
                        row.into_any_element()
                    }
                }
                Row::Halves(left, right) => div()
                    .flex()
                    .gap_5()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .children(take(left)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .children(right.and_then(&mut take)),
                    )
                    .into_any_element(),
                Row::Full(at) => div()
                    .flex()
                    .flex_col()
                    .children(take(at))
                    .into_any_element(),
            })
            .collect()
    }

    /// The panels in order, without the one being dragged and with the room for it, and the
    /// room it is leaving while that closes.
    fn slots(&self, window: &mut Window) -> Vec<Slot<'_>> {
        let drag = self.arranging.drag;
        let mut slots: Vec<Slot<'_>> = self
            .preferences
            .overview
            .panels()
            .iter()
            .filter(|panel| drag.is_none_or(|drag| drag.kind != panel.kind))
            .enumerate()
            .map(|(at, panel)| Slot::Panel(panel, at))
            .collect();
        let Some(drag) = drag else {
            return slots;
        };
        let room = Slot::Room {
            opening: true,
            change: drag.opened,
        };
        // Tiles share a row, where a closing room would only push the others about.
        let closing = drag
            .previous
            .filter(|(at, since, _)| {
                *at != drag.target && since.elapsed() < MOTION && !drag.kind.is_tile()
            })
            .map(|(at, _, change)| {
                window.request_animation_frame();
                (
                    at,
                    Slot::Room {
                        opening: false,
                        change,
                    },
                )
            });
        let target = drag.target.min(slots.len());
        match closing {
            Some((at, closing)) if at > target => {
                slots.insert(at.min(slots.len()), closing);
                slots.insert(target, room);
            }
            Some((at, closing)) => {
                slots.insert(target, room);
                slots.insert(at, closing);
            }
            None => slots.insert(target, room),
        }
        slots
    }

    /// Whether the dragged panel is a tile, and its width, for the room it leaves.
    fn dragged_shape(&self) -> (bool, Width) {
        self.arranging
            .drag
            .and_then(|drag| self.preferences.overview.get(drag.kind))
            .map_or((false, Width::Full), |panel| {
                (panel.kind.is_tile(), panel.width)
            })
    }

    fn render_slot(
        &self,
        theme: &Theme,
        slot: Slot<'_>,
        narrow: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (panel, at) = match slot {
            Slot::Room { opening, change } => return self.render_room(theme, opening, change),
            Slot::Panel(panel, at) => (panel, at),
        };
        let kind = panel.kind;
        let content = self.panel_content(panel, cx);
        let frame = match content {
            PanelContent::Tile(tile) => tile,
            PanelContent::Card {
                header,
                actions,
                body,
            } => self.card_frame(theme, panel, header, actions, body, window, cx),
        };
        let sideways = kind.is_tile() || (panel.width == Width::Half && !narrow);
        let element = div()
            .id(panel_id("panel", kind))
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .gap_1p5()
            .when(kind.is_tile(), |slot| slot.min_w(scaled(170.)))
            .when(self.arranging.on, |slot| {
                slot.child(Self::edit_strip(theme, panel, cx))
            })
            .child(div().flex().flex_col().flex_1().child(frame))
            .child(measure(kind, false, cx))
            // While a panel is dragged, the one under the mouse decides where its room goes:
            // before it or after, across for panels side by side, down for the rest.
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<DraggedPanel>, _, cx| {
                    let position = event.event.position;
                    if !event.bounds.contains(&position) {
                        return;
                    }
                    let center = event.bounds.center();
                    let before = if sideways {
                        position.x < center.x
                    } else {
                        position.y < center.y
                    };
                    this.drag_to(if before { at } else { at + 1 }, cx);
                }),
            );
        // A panel just added fades in; one being removed fades away.
        match (self.arranging.appearing, self.arranging.leaving) {
            (_, Some(leaving)) if leaving == kind => animate(
                element,
                panel_id("leaving", kind),
                MOTION,
                |element, delta| element.opacity(1. - delta),
            ),
            (Some((appearing, change)), _) if appearing == kind => {
                animate(element, ("appearing", change), MOTION, Styled::opacity)
            }
            _ => element.into_any_element(),
        }
    }

    /// The room a dragged panel would land in: as large as the panel, opening where it would
    /// go and closing where it was, so the panels around it slide rather than jump.
    fn render_room(&self, theme: &Theme, opening: bool, change: u64) -> AnyElement {
        let (tile, _) = self.dragged_shape();
        let height = self
            .arranging
            .drag
            .and_then(|drag| self.arranging.sizes.get(&drag.kind))
            .map_or(px(120.), |size| size.height);
        let room = div()
            .flex_1()
            .when(tile, |room| room.min_w(scaled(170.)))
            .h(height)
            .rounded_lg()
            .border_2()
            .border_color(theme.accent.opacity(0.5))
            .bg(theme.accent_soft.opacity(0.5));
        if opening {
            animate(
                room,
                ("room-opening", change),
                MOTION,
                move |room, delta| room.h(height * delta).opacity(delta),
            )
        } else {
            animate(
                room,
                ("room-closing", change),
                MOTION,
                move |room, delta| room.h(height * (1. - delta)).opacity(1. - delta),
            )
        }
    }

    /// A card panel: its heading with its buttons and the chevron that folds it, and beneath,
    /// what folds away.
    #[expect(
        clippy::too_many_arguments,
        reason = "a frame's parts as the panel gives them, and the contexts drawing needs"
    )]
    fn card_frame(
        &self,
        theme: &Theme,
        panel: &PanelState,
        header: AnyElement,
        actions: Vec<AnyElement>,
        body: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let kind = panel.kind;
        // No direction in words: GPUI builds a tooltip from the frame before a click, so right
        // after one it would name the old state. The chevron's direction shows it.
        let glyph = if panel.collapsed {
            IconName::ChevronRight
        } else {
            IconName::ChevronDown
        };
        let tip = format!("Folds {} to its heading, or unfolds it", kind.title());
        let chevron = icon_button(theme, panel_id("fold", kind), glyph, PANEL_BUTTON)
            .tip(tip, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_collapsed(kind, cx)));
        card(theme)
            .flex_1()
            .p_5()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(div().flex_1().min_w(scaled(160.)).child(header))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap_2()
                            .children(actions)
                            .child(chevron),
                    ),
            )
            .children(self.fold_body(kind, panel.collapsed, body, window, cx))
            .into_any_element()
    }

    /// What folds away beneath a card's heading: shown, hidden, or part way while folding,
    /// clipped to a height that follows its own.
    fn fold_body(
        &self,
        kind: PanelKind,
        collapsed: bool,
        body: Option<AnyElement>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let body = body?;
        let fold = self.arranging.fold.filter(|fold| fold.kind == kind);
        let progress = fold.map(Fold::progress).filter(|progress| *progress < 1.);
        if collapsed && progress.is_none() {
            return None;
        }
        // The gap above it folds with it, so nothing jumps when it finishes.
        let inner = div()
            .relative()
            .pt_4()
            .child(body)
            .child(measure(kind, true, cx));
        let shown = fold.zip(progress).and_then(|(fold, progress)| {
            window.request_animation_frame();
            let eased = ease_out(progress);
            let part = if fold.opening { eased } else { 1. - eased };
            self.arranging
                .body_heights
                .get(&kind)
                .map(|height| (*height * part, part))
        });
        Some(
            div()
                .overflow_hidden()
                .when_some(shown, |clip, (height, part)| clip.h(height).opacity(part))
                .child(inner)
                .into_any_element(),
        )
    }

    /// Above each panel while arranging: its handle, its name, its width, and Remove; for a
    /// panel with choices, those beneath.
    fn edit_strip(theme: &Theme, panel: &PanelState, cx: &mut Context<Self>) -> AnyElement {
        let kind = panel.kind;
        let remove = icon_button(theme, panel_id("remove", kind), IconName::X, PANEL_BUTTON)
            .when(!kind.removable(), |button| button.opacity(0.45))
            .tip(
                if kind.removable() {
                    "Remove from the Overview; Undo brings it back"
                } else {
                    "The server's controls stay on the Overview; collapse it instead"
                },
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.remove_panel(kind, cx)));
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(Self::drag_handle(theme, kind, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .one_line()
                            .text_size(scaled(12.))
                            .text_color(theme.text_muted)
                            .child(kind.title()),
                    )
                    .children(Self::size_choices(panel, cx))
                    .child(remove),
            )
            .children(Self::panel_choices(theme, panel, cx))
            .into_any_element()
    }

    /// What a panel is dragged by, and moved by with Alt and the arrow keys once selected.
    fn drag_handle(theme: &Theme, kind: PanelKind, cx: &mut Context<Self>) -> Stateful<Div> {
        let accent = theme.accent;
        let preview_panel = cx.entity().downgrade();
        div()
            .id(panel_id("handle", kind))
            .tip(
                "Drag to move this panel; or select it and press Alt with an arrow key",
                cx,
            )
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(scaled(24.))
            .rounded_sm()
            .border_1()
            .border_color(gpui::transparent_black())
            .tab_index(0)
            .cursor(CursorStyle::OpenHand)
            .hover(|style| style.bg(theme.hover))
            .focus_ring(move |style| style.border_color(accent))
            .on_drag(DraggedPanel(kind), move |dragged, _, _, cx| {
                let _ = preview_panel.update(cx, |this, cx| this.start_panel_drag(dragged.0, cx));
                cx.new(|_| DragPreview(dragged.0))
            })
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                if !event.keystroke.modifiers.alt {
                    return;
                }
                match event.keystroke.key.as_str() {
                    "up" | "left" => this.nudge_panel(kind, false, cx),
                    "down" | "right" => this.nudge_panel(kind, true, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(icon(IconName::Grip, 14., theme.text_faint))
    }

    /// How much room a panel takes: a card's width, or whether a tile shows its chart.
    fn size_choices(panel: &PanelState, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let kind = panel.kind;
        if kind.is_tile() {
            vec![choice(
                panel_id("chart", kind),
                "Chart",
                !panel.collapsed,
                "Shows or hides its chart; without one, it is the number alone",
                cx.listener(move |this, _, _, cx| this.toggle_collapsed(kind, cx)),
            )]
        } else {
            [
                (Width::Full, "Full", "As wide as the page"),
                (Width::Half, "Half", "Half as wide, beside another half"),
            ]
            .into_iter()
            .map(|(width, label, tip)| {
                choice(
                    panel_id(label, kind),
                    label,
                    panel.width == width,
                    tip,
                    cx.listener(move |this, _, _, cx| {
                        this.set_panel(kind, |panel| panel.width = width, cx);
                    }),
                )
            })
            .collect()
        }
    }

    /// A panel's own choices while arranging: how many entries a list shows, and the kinds
    /// the Log panel shows.
    fn panel_choices(
        theme: &Theme,
        panel: &PanelState,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let kind = panel.kind;
        if !kind.has_rows() {
            return None;
        }
        let rows = ROW_CHOICES.into_iter().map(|rows| {
            choice(
                SharedString::from(format!("rows-{}-{rows}", kind.key())),
                &rows.to_string(),
                panel.rows == rows,
                "How many entries it shows",
                cx.listener(move |this, _, _, cx| {
                    this.set_panel(kind, |panel| panel.rows = rows, cx);
                }),
            )
        });
        let kinds: Vec<AnyElement> = if kind == PanelKind::Log {
            Kind::ALL
                .into_iter()
                .map(|entry_kind| {
                    let on = panel.kinds.contains(&entry_kind);
                    // As the Log page's own filters look.
                    kind_chip(
                        theme,
                        SharedString::from(format!("kind-{}", entry_kind.key())),
                        entry_kind,
                        (on, None),
                        cx,
                        cx.listener(move |this, _, _, cx| {
                            this.set_panel(
                                kind,
                                |panel| {
                                    if let Some(at) =
                                        panel.kinds.iter().position(|k| *k == entry_kind)
                                    {
                                        panel.kinds.remove(at);
                                    } else {
                                        // In the log's order, as saved and compared.
                                        panel.kinds.push(entry_kind);
                                        panel.kinds.sort_by_key(|k| {
                                            Kind::ALL.iter().position(|all| all == k)
                                        });
                                    }
                                },
                                cx,
                            );
                        }),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let label = |text: &str| {
            div()
                .text_size(scaled(12.))
                .text_color(theme.text_muted)
                .child(text.to_owned())
        };
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_1()
                .pl(scaled(32.))
                .child(label("Entries"))
                .children(rows)
                .when(!kinds.is_empty(), |choices| {
                    choices
                        .child(div().w(scaled(8.)))
                        .child(label("Kinds"))
                        .children(kinds)
                })
                .into_any_element(),
        )
    }

    /// While arranging, after the panels: the ones not shown, each with Add.
    pub(super) fn render_add_panels(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.arranging.on {
            return None;
        }
        let available = self.preferences.overview.available();
        let rows: Vec<AnyElement> = available
            .into_iter()
            .map(|kind| {
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(kind.title())
                                    .when(kind.is_tile(), |title| {
                                        title.child(
                                            div()
                                                .text_size(scaled(11.))
                                                .text_color(theme.text_faint)
                                                .child("TILE"),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(scaled(12.))
                                    .text_color(theme.text_muted)
                                    .child(kind.description()),
                            ),
                    )
                    .child(
                        Button::new(panel_id("add", kind), "Add")
                            .icon(IconName::Plus)
                            .small()
                            .tooltip(format!(
                                "Adds {} at the end; drag it from there",
                                kind.title()
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| this.add_panel(kind, cx))),
                    )
                    .into_any_element()
            })
            .collect();
        let empty = rows.is_empty();
        Some(
            card(theme)
                .p_5()
                .gap_3()
                .child(card_header(
                    theme,
                    "Add a panel",
                    Some("Tiles sit side by side with the tiles next to them."),
                ))
                .when(empty, |card| {
                    card.child(super::overview::hint(
                        theme,
                        "Every panel is on the Overview.",
                    ))
                })
                .children(rows)
                .into_any_element(),
        )
    }
}

/// Lets a dragged panel be dropped anywhere on the Overview, where its room is.
pub(super) fn accept_panel_drops(body: Div, cx: &mut Context<Panel>) -> Div {
    body.on_drop(cx.listener(|this, _: &DraggedPanel, _, cx| this.drop_panel(cx)))
}

/// Notes a panel's size as it is drawn, or with `body` its contents' height, for folding and
/// for the room it leaves when dragged.
fn measure(kind: PanelKind, body: bool, cx: &mut Context<Panel>) -> impl IntoElement {
    let panel = cx.entity().downgrade();
    canvas(
        move |bounds, _, cx| {
            let _ = panel.update(cx, |this, _| {
                if body {
                    this.arranging.body_heights.insert(kind, bounds.size.height);
                } else {
                    this.arranging.sizes.insert(kind, bounds.size);
                }
            });
        },
        |_, (), _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// An element's id for one panel, as `fold-log`.
fn panel_id(what: &str, kind: PanelKind) -> ElementId {
    SharedString::from(format!("{what}-{}", kind.key())).into()
}

/// How large a panel's chevron and Remove are.
const PANEL_BUTTON: f32 = 28.;

/// A square icon button `size` across: a panel's chevron and Remove, and the buttons at the
/// sidebar's foot.
pub(super) fn icon_button(
    theme: &Theme,
    id: impl Into<gpui::ElementId>,
    glyph: IconName,
    size: f32,
) -> Stateful<Div> {
    let accent = theme.accent;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(scaled(size))
        .rounded_md()
        .border_1()
        .border_color(gpui::transparent_black())
        .tab_index(0)
        .cursor_pointer()
        .hover(|style| style.bg(theme.hover))
        .focus_ring(move |style| style.border_color(accent))
        .child(icon(glyph, 16., theme.text_muted))
}

#[cfg(test)]
mod tests {
    use super::Arranging;
    use crate::overview_layout::PanelKind;

    #[test]
    fn a_panel_added_stops_fading_in_once_and_only_its_own_fading_is_ended() {
        let mut arranging = Arranging::default();
        let first = arranging.appear(PanelKind::Log);
        assert_eq!(arranging.appearing, Some((PanelKind::Log, first)));
        arranging.appeared(first);
        assert_eq!(arranging.appearing, None);

        // Another panel added before the first has faded in: the first's end leaves the
        // second fading in, until its own.
        let first = arranging.appear(PanelKind::Log);
        let second = arranging.appear(PanelKind::Chat);
        arranging.appeared(first);
        assert_eq!(arranging.appearing, Some((PanelKind::Chat, second)));
        arranging.appeared(second);
        assert_eq!(arranging.appearing, None);
    }
}
