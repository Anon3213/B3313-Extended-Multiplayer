//! The mouse pointer, as the chat watches it: while it rests over the chat's corner of the
//! game's window, the game shows the chat's rows, however long ago the last arrived, as a
//! player who wants to read back over it looks there (`People::show`).
//!
//! Only where the pointer is is asked, each time round the bridge's loop: no button and no
//! movement is read, and nothing is taken from the emulator.

use crate::text::ChatSize;

/// The game's picture, which the emulator draws as large as its window holds, in the middle.
const PICTURE_WIDTH: f64 = 320.0;
const PICTURE_HEIGHT: f64 = 240.0;
/// Where the chat is in the picture, with a little room around it: from the left edge to past
/// its box's right side, and from above its sixth row to the bottom, in each size of text, as
/// `TrollEngine/Multiplayer/people.c` lays it out.
const CHAT_RIGHT: [f64; 2] = [227.0, 275.0];
const CHAT_TOP_FROM_BOTTOM: [f64; 2] = [125.0, 215.0];

/// Whether the point (`x`, `y`) of a window's inside, `width` by `height`, falls on the chat's
/// corner of the game's picture in text of `size`.
#[must_use]
pub fn is_over_chat(width: i32, height: i32, x: i32, y: i32, size: ChatSize) -> bool {
    if width <= 0 || height <= 0 {
        return false;
    }
    let (width, height) = (f64::from(width), f64::from(height));
    let scale = (width / PICTURE_WIDTH).min(height / PICTURE_HEIGHT);
    let left = (width - PICTURE_WIDTH * scale) / 2.0;
    let top = (height - PICTURE_HEIGHT * scale) / 2.0;
    let across = (f64::from(x) - left) / scale;
    let up = PICTURE_HEIGHT - (f64::from(y) - top) / scale;
    let index = usize::from(size == ChatSize::Large);
    let right = CHAT_RIGHT.get(index).copied().unwrap_or_default();
    let chat_top = CHAT_TOP_FROM_BOTTOM.get(index).copied().unwrap_or_default();
    (0.0..=right).contains(&across) && (0.0..=chat_top).contains(&up)
}

#[cfg(windows)]
pub use system::WindowsPointer;

#[cfg(windows)]
mod system {
    use super::is_over_chat;
    use crate::text::ChatSize;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetClientRect, GetCursorPos, GetWindowThreadProcessId, IsIconic,
        WindowFromPoint,
    };

    /// How often the pointer is looked at: often enough to seem at once, and not at every turn
    /// of the bridge's loop, as it takes several calls to the system.
    const LOOK_EVERY: Duration = Duration::from_millis(100);

    /// The pointer over the windows of one emulator.
    #[derive(Debug)]
    pub struct WindowsPointer {
        /// The emulator's process: only its windows count.
        pid: u32,
        /// What was found when the pointer was last looked at, and when.
        last: Option<(bool, Instant)>,
    }

    impl WindowsPointer {
        #[must_use]
        pub const fn new(pid: u32) -> Self {
            Self { pid, last: None }
        }

        /// Whether the pointer rests on the chat's corner of the emulator's game window, as of
        /// a moment ago at most.
        pub fn is_over_chat(&mut self, size: ChatSize, now: Instant) -> bool {
            match self.last {
                Some((over, at)) if now.saturating_duration_since(at) < LOOK_EVERY => over,
                _ => {
                    let over = self.look(size);
                    self.last = Some((over, now));
                    over
                }
            }
        }

        fn look(&self, size: ChatSize) -> bool {
            let mut point = POINT { x: 0, y: 0 };
            // SAFETY: `point` is valid for one write.
            if unsafe { GetCursorPos(&raw mut point) } == 0 {
                return false;
            }
            // SAFETY: takes the point by value.
            let under = unsafe { WindowFromPoint(point) };
            if under.is_null() {
                return false;
            }
            // SAFETY: takes no pointers; a window closed since is answered with null.
            let window = unsafe { GetAncestor(under, GA_ROOT) };
            let mut pid = 0;
            // SAFETY: `pid` is valid for one write; a window that is null or closed is no
            // thread.
            let thread = unsafe { GetWindowThreadProcessId(window, &raw mut pid) };
            // SAFETY: takes no pointers.
            if thread == 0 || pid != self.pid || unsafe { IsIconic(window) } != 0 {
                return false;
            }
            let mut inside = RECT {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            // SAFETY: `inside` is valid for one write.
            if unsafe { GetClientRect(window, &raw mut inside) } == 0 {
                return false;
            }
            // SAFETY: `point` is valid for one read and one write.
            if unsafe { ScreenToClient(window, &raw mut point) } == 0 {
                return false;
            }
            is_over_chat(inside.right, inside.bottom, point.x, point.y, size)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_over_chat;
    use crate::text::ChatSize;

    #[test]
    fn the_chat_is_the_lower_left_of_the_picture_however_the_window_is_shaped() {
        let normal = ChatSize::Normal;
        // A window the picture fills: 2 window pixels to one of the game's.
        assert!(is_over_chat(640, 480, 40, 460, normal));
        assert!(is_over_chat(640, 480, 400, 300, normal));
        assert!(
            !is_over_chat(640, 480, 500, 460, normal),
            "the camera's mark"
        );
        assert!(!is_over_chat(640, 480, 40, 100, normal), "the HUD");
        // A wide window has the picture in the middle, with bars at its sides.
        assert!(!is_over_chat(1280, 480, 100, 460, normal), "the bar");
        assert!(is_over_chat(1280, 480, 360, 460, normal));
        // Large text reaches higher and further right.
        assert!(!is_over_chat(640, 480, 40, 150, normal));
        assert!(is_over_chat(640, 480, 40, 150, ChatSize::Large));
        assert!(is_over_chat(640, 480, 520, 460, ChatSize::Large));
        assert!(!is_over_chat(0, 0, 0, 0, normal));
    }
}
