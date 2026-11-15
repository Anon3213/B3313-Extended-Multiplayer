//! The panel's building blocks: buttons, cards, badges and stat tiles, styled by the theme.

use crate::chart::{self, sparkline};
use crate::dashboard::Series;
use crate::format;
use crate::health::Health;
use crate::icons::{IconName, icon};
use crate::preferences::TOOLTIP_SECONDS_DEFAULT;
use crate::sort::SortOrder;
use crate::system;
use crate::theme::{FONT, Theme, scaled};
use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyView, App, ClickEvent, Context, CursorStyle,
    DispatchPhase, Div, ElementId, FontWeight, Hsla, KeyDownEvent, MouseButton, MouseDownEvent,
    Pixels, SharedString, StyleRefinement, Window, div, prelude::*, px,
};
use std::fmt;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long a tooltip takes to appear or fade, quick enough not to keep anyone waiting.
const TOOLTIP_FADE: Duration = Duration::from_millis(90);

/// Eases out: quick at first, settling gently, as things coming to rest do.
pub fn ease_out(delta: f32) -> f32 {
    1.0 - (1.0 - delta).powi(3)
}

/// `element` changed by `animator` over `duration`, from 0 to 1, starting the first time it is
/// drawn with this `id`; a new id starts it again. Drawn at the end at once if Windows'
/// animation effects are off.
pub fn animate<E: IntoElement + 'static>(
    element: E,
    id: impl Into<ElementId>,
    duration: Duration,
    animator: impl Fn(E, f32) -> E + 'static,
) -> AnyElement {
    if system::animations_on() {
        element
            .with_animation(id, Animation::new(duration).with_easing(ease_out), animator)
            .into_any_element()
    } else {
        animator(element, 1.0).into_any_element()
    }
}

/// Whether the last input was a key rather than a mouse button. Focus outlines show only then,
/// as browsers' `:focus-visible` does: someone moving with Tab needs to see where they are,
/// while a click already shows it, and an outline left on a clicked button looks stuck.
/// One per process, as there is one window, so any element can ask without a context.
static KEYBOARD_NAVIGATION: AtomicBool = AtomicBool::new(false);

/// Notes a key pressed, showing focus outlines from then on. Keys reach this before anything
/// handles them, whatever has focus: a window's own key listeners hear only keys meant for
/// the focused element and what contains it. Registered once, as the panel starts.
pub fn notice_keyboard_navigation(cx: &mut App) -> gpui::Subscription {
    cx.intercept_keystrokes(|_, window, _| {
        if !KEYBOARD_NAVIGATION.swap(true, Ordering::Relaxed) {
            window.refresh();
        }
    })
}

/// Notes a mouse button pressed, hiding focus outlines until the next key. Mouse events reach
/// the window's listeners wherever the mouse is. Called as the window paints.
pub fn track_mouse_navigation(window: &mut Window) {
    window.on_mouse_event(|_: &MouseDownEvent, phase, window, _| {
        if phase == DispatchPhase::Capture && KEYBOARD_NAVIGATION.swap(false, Ordering::Relaxed) {
            window.refresh();
        }
    });
}

/// A focus outline that shows only while keys are being used; see `KEYBOARD_NAVIGATION`.
/// Use it rather than GPUI's `focus` for anything clickable.
pub trait FocusRing: InteractiveElement + Sized {
    #[must_use]
    fn focus_ring(self, style: impl FnOnce(StyleRefinement) -> StyleRefinement) -> Self {
        if KEYBOARD_NAVIGATION.load(Ordering::Relaxed) {
            self.focus(style)
        } else {
            self
        }
    }
}

impl<E: InteractiveElement> FocusRing for E {}

/// Text kept to one line, with "…" where it is cut for want of room.
pub trait OneLine: Styled + Sized {
    /// Use this, not GPUI's `truncate`, which turns wrapping off: GPUI measures unwrapped
    /// text once, before it knows the room there is, and so never cuts it. Text that may
    /// wrap is measured again, and one line of it is all that is kept.
    #[must_use]
    fn one_line(self) -> Self {
        self.line_clamp(1).text_ellipsis()
    }
}

impl<E: Styled> OneLine for E {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// The one main action of a view.
    Primary,
    Secondary,
    /// Removes or refuses someone.
    Danger,
    /// Text only, for actions next to others.
    Quiet,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A button that can be reached with Tab and pressed with Enter or Space.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    tooltip: Option<SharedString>,
    tone: Tone,
    enabled: bool,
    small: bool,
    icon_only: bool,
    on_click: Option<ClickHandler>,
}

impl fmt::Debug for Button {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Button")
            .field("label", &self.label)
            .field("tone", &self.tone)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            tooltip: None,
            tone: Tone::Secondary,
            enabled: true,
            small: false,
            icon_only: false,
            on_click: None,
        }
    }

    #[must_use]
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// A glyph before the label. Every button keeps its label: an icon alone would leave the
    /// user guessing.
    #[must_use]
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Hover text, such as exactly what the button copies, or why it is disabled; it shows
    /// either way.
    #[must_use]
    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// For buttons in table rows.
    #[must_use]
    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    /// Shows the icon without the label, where there is no room for the words. The label then
    /// leads the hover text, so the user is not left guessing what the icon does.
    #[must_use]
    pub fn icon_only(mut self, icon_only: bool) -> Self {
        self.icon_only = icon_only;
        self
    }

    #[must_use]
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = *cx.global::<Theme>();
        // Hover changes only the background: GPUI does not carry a hover's text color to the
        // label, so a color the label needs to stay readable cannot change on hover.
        let (background, hover, text, border) = match self.tone {
            Tone::Primary => (
                theme.accent,
                theme.accent_hover,
                theme.on_accent,
                theme.accent,
            ),
            Tone::Secondary => (theme.surface, theme.hover, theme.text, theme.border),
            Tone::Danger => (
                theme.danger_soft,
                theme.danger_hover,
                theme.danger,
                theme.danger_soft,
            ),
            Tone::Quiet => (
                gpui::transparent_black(),
                theme.hover,
                theme.text_muted,
                gpui::transparent_black(),
            ),
        };
        let button = div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(scaled(6.))
            .h(scaled(if self.small { 28. } else { 34. }))
            .px(scaled(if self.small { 10. } else { 14. }))
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(background)
            .text_color(text)
            .text_size(scaled(if self.small { 13. } else { 14. }))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .children(
                self.icon
                    .map(|name| icon(name, if self.small { 14. } else { 16. }, text)),
            );
        // Only a button with an icon can go without its label.
        let bare = self.icon_only && self.icon.is_some();
        let tooltip = match (self.tooltip, bare) {
            (Some(text), true) => Some(format!("{}: {text}", self.label).into()),
            (None, true) => Some(self.label.clone()),
            (tooltip, false) => tooltip,
        };
        let button = if bare {
            button.px(scaled(if self.small { 7. } else { 9. }))
        } else {
            button.child(self.label)
        };
        let button = match tooltip {
            Some(text) => button.tip(text, cx),
            None => button,
        };
        if !self.enabled {
            return button.opacity(0.45);
        }
        let button = button
            .tab_index(0)
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_ring(move |style| style.border_color(theme.accent));
        match self.on_click {
            // A button's click is its own: a clickable row around it, such as a player's, must
            // not act on it too.
            Some(handler) => button.on_click(move |event, window, cx| {
                cx.stop_propagation();
                handler(event, window, cx);
            }),
            None => button,
        }
    }
}

/// A table heading that sorts by its column. The column sorted by shows its label in the
/// accent color with an arrow, up for ascending and down for descending; the tooltip says
/// what the column holds and what selecting does.
pub fn sort_heading(
    theme: &Theme,
    id: impl Into<ElementId>,
    label: &str,
    sorted: Option<SortOrder>,
    explain: &str,
    select: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> gpui::Stateful<Div> {
    // No direction in words: GPUI builds a tooltip as the hover begins, so after a click it
    // would still name the old one. The arrow shows it.
    let action = if sorted.is_some() {
        "Sorted by this; select to reverse"
    } else {
        "Select to sort by this"
    }
    .to_owned();
    let tip = if explain.is_empty() {
        action
    } else {
        format!("{explain}\n{action}")
    };
    let accent = theme.accent;
    div()
        .id(id.into())
        .tip(tip, cx)
        .flex()
        .items_center()
        .gap_1()
        .min_w_0()
        .cursor_pointer()
        .tab_index(0)
        .rounded_sm()
        .border_1()
        .border_color(gpui::transparent_black())
        .focus_ring(move |style| style.border_color(accent))
        .when(sorted.is_some(), |heading| heading.text_color(theme.accent))
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            select(event, window, cx);
        })
        // In a column made narrow the words are cut short, and the arrow stays.
        .child(div().min_w_0().one_line().child(label.to_owned()))
        .children(sorted.map(|order| {
            let arrow = match order {
                SortOrder::Ascending => IconName::ArrowUp,
                SortOrder::Descending => IconName::ArrowDown,
            };
            div().flex_none().child(icon(arrow, 12., theme.accent))
        }))
}

/// The room the line at a column's right edge takes of its heading's cell, which the heading's
/// words keep clear of.
pub const COLUMN_HANDLE_WIDTH: f32 = 11.;

/// What the user does with the line at a column's right edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColumnAction {
    /// Pressed with the mouse, here across the window, to be dragged.
    Grab(Pixels),
    /// Made this much wider, or narrower, with the keyboard.
    Nudge(f32),
    /// Given its usual width back.
    Reset,
}

/// How far a key press moves a column's edge, in the panel's units.
const COLUMN_NUDGE: f32 = 8.;

/// The line at the right edge of a column's heading, which changes the column's width: dragged,
/// or with the Left and Right keys while it has the focus, and back to the usual width on a
/// double click or Delete. It goes last in the heading's cell, which must be `relative`, and
/// shows while the mouse is over it, it has the focus, or `active`, as while it is dragged.
/// `name` tells the table's handles apart.
pub fn column_handle(
    theme: &Theme,
    name: impl Into<SharedString>,
    active: bool,
    cx: &App,
    act: impl Fn(ColumnAction, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let name: SharedString = name.into();
    let accent = theme.accent;
    let act = Rc::new(act);
    let pressed = Rc::clone(&act);
    div()
        .id(name.clone())
        .group(name.clone())
        .tip(
            "Drag to change this column's width, or select it and use the Left and Right \
             keys. Double-click it, or press Delete, for its usual width.",
            cx,
        )
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(px(COLUMN_HANDLE_WIDTH))
        // The line stands a little inside the column, clear of the next one's text.
        .pr(px(5.))
        .flex()
        .justify_end()
        .cursor(CursorStyle::ResizeLeftRight)
        .tab_index(0)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            // Keep the press from sorting by the column, and the drag from selecting text.
            window.prevent_default();
            cx.stop_propagation();
            let action = if event.click_count > 1 {
                ColumnAction::Reset
            } else {
                ColumnAction::Grab(event.position.x)
            };
            pressed(action, window, cx);
        })
        .on_click(|_, _, cx| cx.stop_propagation())
        .on_key_down(move |event: &KeyDownEvent, window, cx| {
            let action = match event.keystroke.key.as_str() {
                "left" => ColumnAction::Nudge(-COLUMN_NUDGE),
                "right" => ColumnAction::Nudge(COLUMN_NUDGE),
                "delete" | "backspace" => ColumnAction::Reset,
                _ => return,
            };
            cx.stop_propagation();
            act(action, window, cx);
        })
        .child(
            div()
                .w(px(2.))
                .h_full()
                .bg(theme.border)
                .when(active, |line| line.bg(accent))
                .group_hover(name, move |style| style.bg(accent)),
        )
        .focus_ring(move |style| style.bg(accent.opacity(0.35)))
        .into_any_element()
}

/// An on/off switch with its label and an explanation, the whole row pressable.
pub fn switch(
    theme: &Theme,
    id: impl Into<ElementId>,
    label: &str,
    detail: &str,
    on: bool,
    toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (track, knob_side) = if on {
        (theme.accent, true)
    } else {
        (theme.border, false)
    };
    let knob = div().size(scaled(14.)).rounded_full().bg(if on {
        theme.on_accent
    } else {
        theme.text_muted
    });
    let accent = theme.accent;
    div()
        .id(id.into())
        .flex()
        .items_center()
        .gap_4()
        .py_2()
        .px_2()
        // Lined up with the card's heading; the padding is only for the hover background.
        .mx(scaled(-9.))
        .rounded_md()
        .border_1()
        .border_color(gpui::transparent_black())
        .tab_index(0)
        .cursor_pointer()
        .hover(|style| style.bg(theme.surface_sunken))
        .focus_ring(move |style| style.border_color(accent))
        .on_click(toggle)
        .child(
            // The text wraps rather than pushing the switch past the card's edge.
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_size(scaled(14.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label.to_owned()),
                )
                .child(
                    div()
                        .text_size(scaled(12.))
                        .text_color(theme.text_muted)
                        .child(detail.to_owned()),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .when(knob_side, Div::justify_end)
                .w(scaled(36.))
                .h(scaled(20.))
                .px(scaled(3.))
                .rounded_full()
                .bg(track)
                .child(knob),
        )
        .into_any_element()
}

/// A raised area that groups related content.
pub fn card(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border)
        .rounded_lg()
}

/// A card's heading, with what it is for beneath.
pub fn card_header(theme: &Theme, title: &str, detail: Option<&str>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .text_size(scaled(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child(title.to_owned()),
        )
        .children(detail.map(|detail| {
            div()
                .text_size(scaled(13.))
                .text_color(theme.text_muted)
                .child(detail.to_owned())
        }))
}

/// A small rounded label, such as a count or a state.
pub fn badge(text: impl Into<SharedString>, color: Hsla, background: Hsla) -> Div {
    div()
        .flex_none()
        .px_2()
        .py(scaled(1.))
        .rounded_full()
        .bg(background)
        .text_color(color)
        .text_size(scaled(12.))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

/// A round dot, as for a server's state.
pub fn dot(color: Hsla) -> Div {
    div().flex_none().size(scaled(8.)).rounded_full().bg(color)
}

/// One headline number: what it measures, the number, and an explanation.
#[derive(Debug)]
pub struct Stat<'a> {
    pub icon: IconName,
    pub label: &'static str,
    pub value: String,
    pub detail: String,
    /// Colors the number, and says the state in a word, when it needs attention.
    pub health: Health,
    /// Its recent values, charted under the number.
    pub trend: Option<Trend<'a>>,
    /// What it measures and, for one that can need attention, where its limits are; shown on
    /// hover over the tile.
    pub explain: String,
}

/// A measurement's recent values, and how to write one, as its chart's scale does.
#[derive(Clone, Copy)]
pub struct Trend<'a> {
    pub series: &'a Series,
    pub unit: fn(f64) -> String,
    /// What the series' values are multiplied by for the scale, as bytes are to make bits, so
    /// its top is a round number in the unit shown.
    pub factor: f64,
}

impl<'a> Trend<'a> {
    #[must_use]
    pub fn new(series: &'a Series, unit: fn(f64) -> String) -> Self {
        Self {
            series,
            unit,
            factor: 1.0,
        }
    }

    /// A series of bytes per second, its scale in the user's unit for rates.
    #[must_use]
    pub fn rate(series: &'a Series) -> Self {
        Self {
            series,
            unit: format::scale_rate,
            factor: format::rate_unit().per_byte(),
        }
    }
}

impl fmt::Debug for Trend<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Trend")
            .field("series", self.series)
            .finish_non_exhaustive()
    }
}

/// A chart with its scale: the value at its top and zero at the right, and how far back it
/// reaches beneath, so a line can be read, not only its shape.
fn chart_with_scale(theme: &Theme, trend: Trend<'_>, health: Health) -> Div {
    const HEIGHT: f32 = 36.;
    // The scale's labels are a line this tall, centered on the lines they name.
    const LABEL: f32 = 12.;
    let (line, fill) = chart_colors(theme, health);
    let values = trend.series.values();
    // Rounded in the unit shown, then back in the series' own for drawing.
    let shown_top = chart::top(values.iter().map(|value| value * trend.factor));
    let top = shown_top / trend.factor;
    let minutes = (chart::span(values.len()).as_secs() + 30) / 60;
    let small = |text: String| {
        div()
            .text_size(scaled(11.))
            .line_height(scaled(LABEL))
            .text_color(theme.text_faint)
            .child(text)
    };
    div()
        .pt_2()
        .flex()
        .items_start()
        .gap_2()
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    // Faint lines at the top and bottom, where the scale's values are.
                    div()
                        .border_t_1()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(sparkline(trend.series, top, line, fill, HEIGHT)),
                )
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .child(small(format!("{minutes} min ago")))
                        .child(small("now".to_owned())),
                ),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .justify_between()
                // From half a label above the top line to half one below the bottom line, the
                // chart's height and its two one-pixel lines apart.
                .h(scaled(HEIGHT + 2. + LABEL))
                .mt(scaled(-LABEL / 2.))
                .child(small((trend.unit)(shown_top)))
                .child(small("0".to_owned())),
        )
}

/// The colors of a chart: the accent, or the state's color when it needs attention, so the
/// line agrees with the number above it.
pub fn chart_colors(theme: &Theme, health: Health) -> (Hsla, Hsla) {
    let line = theme.health(health).unwrap_or(theme.accent);
    (line, line.opacity(0.16))
}

pub fn stat_tile(theme: &Theme, stat: Stat<'_>, cx: &App) -> AnyElement {
    let alert = theme.health(stat.health);
    let chart = stat
        .trend
        .map(|trend| chart_with_scale(theme, trend, stat.health));
    card(theme)
        .id(SharedString::from(format!("tile-{}", stat.label)))
        .tip(stat.explain, cx)
        .flex_1()
        .min_w(scaled(170.))
        .p_4()
        .gap_1()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(scaled(12.))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text_muted)
                .child(icon(stat.icon, 14., theme.text_faint))
                .child(div().flex_1().child(stat.label.to_uppercase()))
                .children(alert.map(|color| {
                    let soft = color.opacity(0.14);
                    badge(stat.health.word(), color, soft)
                })),
        )
        .child(
            div()
                .text_size(scaled(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(alert.unwrap_or(theme.text))
                .child(stat.value),
        )
        .child(
            div()
                .text_size(scaled(12.))
                .text_color(theme.text_faint)
                .child(stat.detail),
        )
        .children(chart)
        .into_any_element()
}

/// How long a tooltip stays, a preference; zero means none at all. Every tooltip goes through
/// `Tip::tip` or a button's `tooltip`, which both ask this.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooltipDuration(pub Duration);

impl gpui::Global for TooltipDuration {}

fn tooltip_duration(cx: &App) -> Duration {
    cx.try_global::<TooltipDuration>().map_or(
        Duration::from_secs(u64::from(TOOLTIP_SECONDS_DEFAULT)),
        |duration| duration.0,
    )
}

/// Adds hover text to an element, unless the user turned tooltips off.
pub trait Tip: StatefulInteractiveElement + Sized {
    #[must_use]
    fn tip(self, text: impl Into<SharedString>, cx: &App) -> Self {
        if tooltip_duration(cx).is_zero() {
            self
        } else {
            let text: SharedString = text.into();
            self.tooltip(Tooltip::text(text))
        }
    }
}

impl<E: StatefulInteractiveElement> Tip for E {}

/// The tooltips that exist: one while it shows or waits to be taken down.
#[derive(Clone, Debug, Default)]
struct OpenTooltips(Vec<gpui::WeakEntity<Tooltip>>);

impl gpui::Global for OpenTooltips {}

/// Makes tooltips follow the mouse: a tooltip shows over the element under it and goes as soon
/// as the mouse leaves. GPUI decides whether a tooltip still belongs only while drawing a frame,
/// and moving the mouse from one element to another draws none by itself, so a tooltip would
/// linger until something else changed, and its element could not show another meanwhile.
/// While a tooltip is open, each mouse movement draws a frame; otherwise nothing changes.
/// Called as the window paints.
pub fn follow_tooltips(window: &mut Window) {
    window.on_mouse_event(|_: &gpui::MouseMoveEvent, phase, window, cx| {
        let open = cx
            .try_global::<OpenTooltips>()
            .is_some_and(|open| !open.0.is_empty());
        if phase == gpui::DispatchPhase::Capture && open {
            window.refresh();
        }
    });
}

/// Counts calls to `dismiss_tooltips`; a tooltip written before the latest shows nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TooltipGeneration(u64);

impl gpui::Global for TooltipGeneration {}

fn tooltip_generation(cx: &App) -> TooltipGeneration {
    cx.try_global::<TooltipGeneration>()
        .copied()
        .unwrap_or_default()
}

/// Empties every tooltip open now, as when what they say may no longer be shown.
pub fn dismiss_tooltips(cx: &mut App) {
    cx.update_default_global::<TooltipGeneration, _>(|generation, _| {
        generation.0 = generation.0.wrapping_add(1);
    });
    // GPUI draws a tooltip again only when it is told to, as it draws any view; without this
    // an open one keeps showing what it said until the mouse moves.
    let open = cx
        .try_global::<OpenTooltips>()
        .map(|open| open.0.clone())
        .unwrap_or_default();
    for tooltip in open {
        let _ = tooltip.update(cx, |_, cx| cx.notify());
    }
}

/// Hover text, as for what an element may have cut short or what a number means. It goes
/// away after the user's chosen time, even if the mouse stays.
#[derive(Debug)]
pub struct Tooltip {
    text: SharedString,
    expired: bool,
    /// When it was written, against `dismiss_tooltips`.
    generation: TooltipGeneration,
}

impl Tooltip {
    /// What `tooltip` takes to show `text`.
    pub fn text(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView {
        let text = text.into();
        move |_, cx| {
            let lasts = tooltip_duration(cx);
            cx.new(|cx: &mut Context<Self>| {
                // Known while it exists, for `follow_tooltips` and `dismiss_tooltips`.
                let this = cx.entity().downgrade();
                let id = this.entity_id();
                cx.update_default_global::<OpenTooltips, _>(|open, _| open.0.push(this));
                cx.on_release(move |_, cx| {
                    cx.update_default_global::<OpenTooltips, _>(|open, _| {
                        open.0.retain(|tooltip| tooltip.entity_id() != id);
                    });
                })
                .detach();
                // GPUI keeps a tooltip while the mouse stays; this one empties itself instead.
                cx.spawn(async move |tooltip, cx| {
                    cx.background_executor().timer(lasts).await;
                    let _ = tooltip.update(cx, |tooltip, cx| {
                        tooltip.expired = true;
                        cx.notify();
                    });
                })
                .detach();
                Self {
                    text: text.clone(),
                    expired: false,
                    generation: tooltip_generation(cx),
                }
            })
            .into()
        }
    }
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Emptied at once when what it says may no longer be shown.
        if self.generation != tooltip_generation(cx) {
            return div().into_any_element();
        }
        let theme = cx.global::<Theme>();
        let tooltip = div()
            .max_w(scaled(640.))
            .px_3()
            .py_2()
            .rounded_md()
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .font_family(FONT)
            .text_size(scaled(13.))
            .text_color(theme.text)
            .child(self.text.clone());
        // It fades in rather than popping up, and out when its time is up. Leaving its element
        // takes it down at once: GPUI removes it then, and the next one is already wanted.
        if self.expired {
            animate(tooltip, "tooltip-out", TOOLTIP_FADE, |tooltip, delta| {
                tooltip.opacity(1.0 - delta)
            })
        } else {
            animate(tooltip, "tooltip-in", TOOLTIP_FADE, Styled::opacity)
        }
    }
}

/// A notice across a view, such as settings that wait for a restart.
pub fn banner(color: Hsla, background: Hsla, glyph: IconName, text: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .py_3()
        .rounded_lg()
        .bg(background)
        .text_color(color)
        .text_size(scaled(13.))
        .child(icon(glyph, 16., color))
        // Wraps rather than pushing buttons beside it out of the banner.
        .child(div().flex_1().min_w_0().child(text))
}
