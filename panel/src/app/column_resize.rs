//! Changing a table column's width by the line at its heading's right edge: dragged, moved
//! with the keys, or set back to the usual width. The widths are the preferences' own.

use super::Panel;
use crate::dashboard::{LogColumn, PlayerColumn};
use crate::theme::{BASE_REM, Theme};
use crate::widgets::{ColumnAction, column_handle};
use gpui::{AnyElement, Context, MouseButton, MouseMoveEvent, Pixels, SharedString, Window};

/// A column of one of the panel's tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TableColumn {
    Player(PlayerColumn),
    Log(LogColumn),
}

/// A column's edge being dragged: where the mouse took hold of it, across the window, and
/// how wide the column was then.
#[derive(Clone, Copy, Debug)]
pub(super) struct ColumnDrag {
    column: TableColumn,
    from: Pixels,
    width: u16,
}

impl Panel {
    fn column_width(&self, column: TableColumn) -> Option<u16> {
        match column {
            TableColumn::Player(column) => self.preferences.player_widths.of(column),
            TableColumn::Log(column) => self.preferences.log_widths.of(column),
        }
    }

    fn set_column_width(&mut self, column: TableColumn, width: f32) {
        match column {
            TableColumn::Player(column) => self.preferences.player_widths.set(column, width),
            TableColumn::Log(column) => self.preferences.log_widths.set(column, width),
        }
    }

    /// The line at the right edge of `column`'s heading, for the heading's cell.
    pub(super) fn column_handle(
        &self,
        theme: &Theme,
        column: TableColumn,
        cx: &Context<Self>,
    ) -> AnyElement {
        let panel = cx.entity().downgrade();
        let dragged = self.column_drag.is_some_and(|drag| drag.column == column);
        let name = SharedString::from(format!("column-edge-{column:?}"));
        column_handle(theme, name, dragged, cx, move |action, _, cx| {
            let _ = panel.update(cx, |this, cx| this.act_on_column(column, action, cx));
        })
    }

    fn act_on_column(&mut self, column: TableColumn, action: ColumnAction, cx: &mut Context<Self>) {
        let Some(width) = self.column_width(column) else {
            return;
        };
        match action {
            ColumnAction::Grab(from) => {
                // The window follows the mouse from here: see `window_listeners`.
                self.column_drag = Some(ColumnDrag {
                    column,
                    from,
                    width,
                });
            }
            ColumnAction::Nudge(by) => {
                self.set_column_width(column, f32::from(width) + by);
                self.save_preferences();
            }
            ColumnAction::Reset => {
                self.column_drag = None;
                match column {
                    TableColumn::Player(column) => self.preferences.player_widths.reset(column),
                    TableColumn::Log(column) => self.preferences.log_widths.reset(column),
                }
                self.save_preferences();
            }
        }
        cx.notify();
    }

    /// Follows the mouse while a column's edge is dragged: the column is as much wider, or
    /// narrower, as the mouse has gone right or left of where it took hold.
    pub(super) fn drag_column(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.column_drag else {
            return;
        };
        // A button let go outside the window sends no release here.
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_column_drag(cx);
            return;
        }
        let moved = (event.position.x - drag.from) / window.rem_size() * BASE_REM;
        let before = self.column_width(drag.column);
        self.set_column_width(drag.column, f32::from(drag.width) + moved);
        if self.column_width(drag.column) != before {
            cx.notify();
        }
    }

    /// Ends a drag of a column's edge, remembering where it was left.
    pub(super) fn finish_column_drag(&mut self, cx: &mut Context<Self>) {
        if self.column_drag.take().is_some() {
            self.save_preferences();
            cx.notify();
        }
    }
}
