//! A one-line text field. GPUI has none of its own; this follows its `input` example, with the
//! editing itself in `EditBuffer`.

use crate::edit_buffer::EditBuffer;
use crate::theme::{Theme, scaled};
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point,
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, UnderlineStyle, Window, actions, div,
    fill, point, prelude::*, px, relative, size,
};
use std::ops::Range;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        SelectHome,
        SelectEnd,
        Paste,
        Cut,
        Copy,
        Submit,
    ]
);

const CONTEXT: &str = "TextInput";
/// Shown in place of each character of a masked field.
const MASK: char = '•';

/// The keys a text field answers to while it has focus.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("shift-home", SelectHome, Some(CONTEXT)),
        KeyBinding::new("shift-end", SelectEnd, Some(CONTEXT)),
        KeyBinding::new("secondary-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("secondary-v", Paste, Some(CONTEXT)),
        KeyBinding::new("secondary-c", Copy, Some(CONTEXT)),
        KeyBinding::new("secondary-x", Cut, Some(CONTEXT)),
        KeyBinding::new("enter", Submit, Some(CONTEXT)),
    ]);
}

/// What a text field tells whoever subscribes to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputEvent {
    /// The text changed by the user's hand, not by `set_text`.
    Changed,
    /// The user pressed Enter.
    Submitted,
}

#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "masked, invalid, disabled and dragging vary independently; no state machine joins them"
)]
pub struct TextInput {
    focus_handle: FocusHandle,
    buffer: EditBuffer,
    placeholder: SharedString,
    /// Draws a dot for each character, as for a password. Copying is refused while masked.
    masked: bool,
    /// Shows the field as holding a value the panel refuses.
    invalid: bool,
    disabled: bool,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    /// How far the text is drawn to the left of the field's start, so that the cursor of a
    /// text longer than the field stays in view.
    scroll: Pixels,
    is_selecting: bool,
}

/// How far a text `text` wide is to be drawn to the left in a field `field` wide, given how
/// far it was (`scroll`), so that `cursor`, a place in the text, shows: as little moved as
/// will do, and never further than leaves the text's end at the field's.
fn scroll_to_show(scroll: f32, cursor: f32, text: f32, field: f32) -> f32 {
    let shown = scroll.min(cursor).max(cursor - field);
    shown.min(text - field).max(0.0)
}

impl EventEmitter<InputEvent> for TextInput {}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, text: &str, placeholder: &str) -> Self {
        let mut buffer = EditBuffer::default();
        buffer.set_text(text);
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            buffer,
            placeholder: SharedString::from(placeholder.to_owned()),
            masked: false,
            invalid: false,
            disabled: false,
            last_layout: None,
            last_bounds: None,
            scroll: px(0.),
            is_selecting: false,
        }
    }

    #[must_use]
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    #[must_use]
    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// Replaces the text without reporting `Changed`: the caller already knows.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.buffer.set_text(text);
        cx.notify();
    }

    pub fn set_masked(&mut self, masked: bool, cx: &mut Context<Self>) {
        if self.masked != masked {
            self.masked = masked;
            cx.notify();
        }
    }

    #[must_use]
    pub fn is_masked(&self) -> bool {
        self.masked
    }

    pub fn set_invalid(&mut self, invalid: bool, cx: &mut Context<Self>) {
        if self.invalid != invalid {
            self.invalid = invalid;
            cx.notify();
        }
    }

    /// A disabled field shows its text but takes no edits, as while the server it configures runs.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            cx.notify();
        }
    }

    fn edit(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut EditBuffer)) {
        if self.disabled {
            return;
        }
        let before = self.buffer.text().to_owned();
        change(&mut self.buffer);
        if self.buffer.text() != before {
            cx.emit(InputEvent::Changed);
        }
        cx.notify();
    }

    fn navigate(&mut self, cx: &mut Context<Self>, change: impl FnOnce(&mut EditBuffer)) {
        change(&mut self.buffer);
        cx.notify();
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, EditBuffer::backspace);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        self.edit(cx, EditBuffer::delete);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, EditBuffer::left);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, EditBuffer::right);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, EditBuffer::select_left);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, EditBuffer::select_right);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, EditBuffer::select_all);
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, |buffer| buffer.move_to(0));
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, |buffer| buffer.move_to(usize::MAX));
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, |buffer| buffer.select_to(0));
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(cx, |buffer| buffer.select_to(usize::MAX));
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.edit(cx, |buffer| buffer.replace(None, &text));
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let selected = self.buffer.selected_text();
        if !self.masked && !selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let selected = self.buffer.selected_text();
        if !self.masked && !selected.is_empty() && !self.disabled {
            cx.write_to_clipboard(ClipboardItem::new_string(selected.to_owned()));
            self.edit(cx, |buffer| buffer.replace(None, ""));
        }
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let offset = self.offset_for_position(event.position);
        if event.modifiers.shift {
            self.navigate(cx, |buffer| buffer.select_to(offset));
        } else {
            self.navigate(cx, |buffer| buffer.move_to(offset));
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            let offset = self.offset_for_position(event.position);
            self.navigate(cx, |buffer| buffer.select_to(offset));
        }
    }

    fn offset_for_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if self.buffer.text().is_empty() || position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.buffer.text().len();
        }
        self.real_offset(line.closest_index_for_x(position.x - bounds.left() + self.scroll))
    }

    /// What is drawn: the text, or a dot for each of its characters.
    fn display_text(&self) -> SharedString {
        if self.masked {
            let count = self.buffer.text().chars().count();
            SharedString::from(MASK.to_string().repeat(count))
        } else {
            SharedString::from(self.buffer.text().to_owned())
        }
    }

    /// The offset in the drawn text of an offset in the real one.
    fn shown_offset(&self, offset: usize) -> usize {
        if self.masked {
            let text = self.buffer.text();
            let characters = text.char_indices().take_while(|&(i, _)| i < offset).count();
            characters * MASK.len_utf8()
        } else {
            offset
        }
    }

    fn real_offset(&self, offset: usize) -> usize {
        if self.masked {
            let character = offset / MASK.len_utf8();
            let text = self.buffer.text();
            text.char_indices()
                .nth(character)
                .map_or(text.len(), |(index, _)| index)
        } else {
            offset
        }
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.buffer.range_from_utf16(&range_utf16);
        actual_range.replace(self.buffer.range_to_utf16(&range));
        // An input method may read the text around the cursor; a masked field shows it none.
        let text = self.buffer.text().get(range)?;
        Some(if self.masked {
            MASK.to_string().repeat(text.chars().count())
        } else {
            text.to_owned()
        })
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.buffer.range_to_utf16(&self.buffer.selection()),
            reversed: self.buffer.is_reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.buffer
            .marked()
            .map(|range| self.buffer.range_to_utf16(&range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.buffer.unmark();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|range| self.buffer.range_from_utf16(&range));
        self.edit(cx, |buffer| buffer.replace(range, new_text));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16.map(|range| self.buffer.range_from_utf16(&range));
        // The selection is relative to the new text, so it is measured in the new text.
        let selected = new_selected_range_utf16.map(|selected| {
            let mut measure = EditBuffer::default();
            measure.set_text(new_text);
            measure.range_from_utf16(&selected)
        });
        self.edit(cx, |buffer| {
            buffer.replace_and_mark(range, new_text, selected);
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.buffer.range_from_utf16(&range_utf16);
        let start = bounds.left() - self.scroll;
        let x = |offset| start + line.x_for_index(self.shown_offset(offset));
        Some(Bounds::from_corners(
            point(x(range.start), bounds.top()),
            point(x(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let line = self.last_layout.as_ref()?;
        let index = line.index_for_x(position.x - bounds.left() + self.scroll)?;
        Some(self.buffer.offset_to_utf16(self.real_offset(index)))
    }
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.global::<Theme>();
        let focused = self.focus_handle.is_focused(window);
        let border = if self.invalid {
            theme.danger
        } else if focused {
            theme.accent
        } else {
            theme.border
        };
        div()
            .flex()
            .items_center()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .cursor(if self.disabled {
                CursorStyle::Arrow
            } else {
                CursorStyle::IBeam
            })
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(|_, _: &Submit, _, cx| cx.emit(InputEvent::Submitted)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .w_full()
            .h(scaled(34.))
            .px_3()
            .rounded_md()
            .border_1()
            .border_color(border)
            .bg(if self.disabled {
                theme.surface_sunken
            } else {
                theme.input
            })
            .text_color(if self.disabled {
                theme.text_muted
            } else {
                theme.text
            })
            .text_size(scaled(14.))
            .line_height(scaled(20.))
            .overflow_hidden()
            .child(TextElement { input: cx.entity() })
    }
}

/// Draws the text, the selection and the cursor, and connects the platform's input handler.
struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    /// How far left of the field's start the line is drawn.
    scroll: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        &mut (): &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = cx.global::<Theme>();
        let (accent, selection_color, placeholder_color) =
            (theme.accent, theme.selection, theme.text_faint);
        let input = self.input.read(cx);
        let style = window.text_style();
        let content = input.display_text();
        let (shown, color) = if content.is_empty() {
            (input.placeholder.clone(), placeholder_color)
        } else {
            (content, style.color)
        };
        let run = TextRun {
            len: shown.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match input
            .buffer
            .marked()
            .filter(|_| !input.buffer.text().is_empty())
        {
            Some(marked) => {
                let (start, end) = (
                    input.shown_offset(marked.start),
                    input.shown_offset(marked.end),
                );
                let underline = UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.),
                    wavy: false,
                };
                [
                    TextRun {
                        len: start,
                        ..run.clone()
                    },
                    TextRun {
                        len: end - start,
                        underline: Some(underline),
                        ..run.clone()
                    },
                    TextRun {
                        len: shown.len() - end,
                        ..run
                    },
                ]
                .into_iter()
                .filter(|run| run.len > 0)
                .collect()
            }
            None => vec![run],
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line = window
            .text_system()
            .shape_line(shown, font_size, &runs, None);

        let selected = input.buffer.selection();
        let cursor_width = scaled(1.5).to_pixels(window.rem_size());
        // A placeholder is not the field's text: it has no cursor in it to keep in view.
        let scroll = if input.buffer.text().is_empty() {
            px(0.)
        } else {
            let cursor = line.x_for_index(input.shown_offset(input.buffer.cursor()));
            px(scroll_to_show(
                f32::from(input.scroll),
                f32::from(cursor),
                f32::from(line.width + cursor_width),
                f32::from(bounds.size.width),
            ))
        };
        let start = bounds.left() - scroll;
        let x = |offset| start + line.x_for_index(input.shown_offset(offset));
        let (selection, cursor) = if selected.is_empty() {
            let cursor = Bounds::new(
                point(x(input.buffer.cursor()), bounds.top()),
                size(cursor_width, bounds.bottom() - bounds.top()),
            );
            (None, Some(fill(cursor, accent)))
        } else {
            let area = Bounds::from_corners(
                point(x(selected.start), bounds.top()),
                point(x(selected.end), bounds.bottom()),
            );
            (Some(fill(area, selection_color)), None)
        };
        PrepaintState {
            line: Some(line),
            cursor,
            selection,
            scroll,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        &mut (): &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (focus_handle, disabled) = {
            let input = self.input.read(cx);
            (input.focus_handle.clone(), input.disabled)
        };
        if !disabled {
            window.handle_input(
                &focus_handle,
                ElementInputHandler::new(bounds, self.input.clone()),
                cx,
            );
        }
        let Some(line) = prepaint.line.take() else {
            return;
        };
        let scroll = prepaint.scroll;
        let origin = point(bounds.origin.x - scroll, bounds.origin.y);
        let selection = prepaint.selection.take();
        let cursor = prepaint
            .cursor
            .take()
            .filter(|_| focus_handle.is_focused(window) && !disabled);
        // Within the text's own room: what is scrolled out of it does not show in the field's
        // padding.
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(selection) = selection {
                window.paint_quad(selection);
            }
            // A failed paint leaves this frame without the text; the next one tries again.
            let _ = line.paint(origin, window.line_height(), window, cx);
            if let Some(cursor) = cursor {
                window.paint_quad(cursor);
            }
        });
        self.input.update(cx, |input, _| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
            input.scroll = scroll;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::scroll_to_show;

    #[test]
    fn a_long_text_is_moved_only_as_far_as_shows_its_cursor() {
        // A text that fits is never moved, wherever its cursor is.
        assert_eq!(scroll_to_show(0.0, 80.0, 80.0, 100.0), 0.0);
        // Typing past the field's end moves the text to keep the cursor at that end.
        assert_eq!(scroll_to_show(0.0, 150.0, 150.0, 100.0), 50.0);
        // A cursor moved back within what shows moves nothing.
        assert_eq!(scroll_to_show(50.0, 120.0, 150.0, 100.0), 50.0);
        // One moved back past the field's start brings the text back with it.
        assert_eq!(scroll_to_show(50.0, 20.0, 150.0, 100.0), 20.0);
        // Text deleted from the end leaves no empty room there.
        assert_eq!(scroll_to_show(50.0, 110.0, 110.0, 100.0), 10.0);
        assert_eq!(scroll_to_show(50.0, 60.0, 60.0, 100.0), 0.0);
    }
}
