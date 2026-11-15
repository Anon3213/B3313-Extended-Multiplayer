//! Getting the user's attention while the panel is behind other windows, as while they play.

use gpui::Window;

/// Flashes the panel's taskbar button until the user brings the panel to the front. Does
/// nothing where the platform has no such thing, or if the window cannot be reached.
pub fn flash_taskbar(window: &Window) {
    #[cfg(windows)]
    windows_flash(window);
    #[cfg(not(windows))]
    let _ = window;
}

/// Plays Windows' own short sound for a notice, at the volume and with the sound the user
/// has set for it there, which may be none. Does nothing on other platforms.
pub fn sound() {
    #[cfg(windows)]
    {
        use windows::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK;

        // SAFETY: takes no pointers. A sound that cannot be played is no sound.
        let _ = unsafe { MessageBeep(MB_ICONASTERISK) };
    }
}

#[cfg(windows)]
fn windows_flash(window: &Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        FLASHW_TIMERNOFG, FLASHW_TRAY, FLASHWINFO, FlashWindowEx,
    };

    // The trait's method: GPUI's own `window_handle` is another thing of the same name.
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let Ok(size) = u32::try_from(size_of::<FLASHWINFO>()) else {
        return;
    };
    let info = FLASHWINFO {
        cbSize: size,
        hwnd: HWND(std::ptr::with_exposed_provenance_mut(
            handle.hwnd.get().cast_unsigned(),
        )),
        // Flash the taskbar button until the window comes to the front.
        dwFlags: FLASHW_TRAY | FLASHW_TIMERNOFG,
        uCount: 0,
        dwTimeout: 0,
    };
    // SAFETY: `info` is a fully initialized FLASHWINFO with its own size in `cbSize`, and it
    // outlives the call; `hwnd` is the panel's own window, which GPUI keeps open while `window`
    // is borrowed. The returned BOOL only reports the window's previous state.
    let _ = unsafe { FlashWindowEx(&raw const info) };
}
