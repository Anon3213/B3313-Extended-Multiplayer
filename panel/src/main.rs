//! Desktop control panel for B3313 multiplayer: sets the game up and joins a session with
//! it, and hosts the session server and moderates it.
//!
//! Usage: `b3313-panel [settings file]`. Without a file it starts from the default settings.

// Shows no console window behind the panel in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alert;
mod app;
mod bot_games;
mod chart;
mod columns;
mod crash;
mod dashboard;
mod edit_buffer;
mod emulator;
mod format;
mod game_setup;
mod health;
mod host;
mod icons;
mod join;
mod network;
mod overview_layout;
mod page_layout;
mod port_map;
mod preferences;
mod privacy;
mod settings;
mod sort;
mod system;
mod text_input;
mod theme;
mod widgets;
mod zoom;

use app::Panel;
use b3313_server::Config;
use gpui::{
    App, AppContext, Application, Bounds, Pixels, TitlebarOptions, WindowBounds, WindowOptions,
    point, px, size,
};
use preferences::{Preferences, WindowPlacement};
use settings::SettingsForm;
use std::path::PathBuf;
use std::time::Duration;
use theme::Theme;
use zoom::Zoom;

/// The panel's version, as its release is named: the sidebar and the Preferences page show
/// it, and a crash report names it.
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Where the project is kept, which the Preferences page opens.
const PROJECT_PAGE: &str = "https://github.com/Anon3213/B3313-Extended-Multiplayer";

/// Where the window opens: where it was last time, as Windows apps do, if that is still on a
/// screen; otherwise centered, larger at a larger interface size but never beyond the screen.
fn starting_bounds(saved: Option<WindowPlacement>, zoom: Zoom, cx: &App) -> WindowBounds {
    let displays: Vec<Bounds<Pixels>> = cx.displays().iter().map(|d| d.bounds()).collect();
    if let Some(saved) = saved {
        let bounds = Bounds::new(
            point(px(saved.x), px(saved.y)),
            size(px(saved.width), px(saved.height)),
        );
        // Its title bar must be on some screen, or the user could not reach it, as after a
        // second monitor is unplugged.
        let title_bar = Bounds::new(bounds.origin, size(bounds.size.width, px(40.)));
        if displays
            .iter()
            .any(|display| display.intersects(&title_bar))
        {
            return if saved.maximized {
                WindowBounds::Maximized(bounds)
            } else {
                WindowBounds::Windowed(bounds)
            };
        }
    }
    let (mut width, mut height) = (zoom.scale(1240.), zoom.scale(800.));
    if let Some(display) = cx.primary_display() {
        let screen = display.bounds().size;
        width = width.min(f32::from(screen.width) * 0.9);
        height = height.min(f32::from(screen.height) * 0.9);
    }
    WindowBounds::Windowed(Bounds::centered(None, size(px(width), px(height)), cx))
}

fn main() {
    crash::install();
    // Preferences that cannot be read are left alone: the panel runs with the usual ones,
    // says so, and saves none over them.
    let (preferences, unread) = match preferences::path().map(|path| Preferences::load(&path)) {
        Some(Ok(preferences)) => (preferences, None),
        Some(Err(message)) => (Preferences::default(), Some(message)),
        None => (Preferences::default(), None),
    };
    // A file named on the command line wins over the one the panel had open last time.
    let named = std::env::args_os().nth(1).map(PathBuf::from);
    let opened = named
        .as_deref()
        .or_else(|| preferences.settings_to_reopen())
        .map(SettingsForm::open);
    let (form, problem) = match opened {
        Some(Ok(form)) => (form, None),
        Some(Err(message)) => (SettingsForm::new(&Config::default()), Some(message)),
        None => (SettingsForm::new(&Config::default()), None),
    };
    let zoom = preferences.zoom;
    let application = Application::new().with_assets(icons::Assets);
    application.run(move |cx: &mut App| {
        text_input::bind_keys(cx);
        app::bind_keys(cx);
        // For as long as the panel runs.
        widgets::notice_keyboard_navigation(cx).detach();
        let theme = Theme::for_user(
            preferences.theme,
            preferences.mode,
            cx.window_appearance(),
            system::high_contrast_on(),
        );
        cx.set_global(theme);
        cx.set_global(widgets::TooltipDuration(Duration::from_secs(u64::from(
            preferences.tooltip_seconds,
        ))));
        format::set_rate_unit(preferences.rate_unit);
        cx.on_window_closed(|cx| cx.quit()).detach();
        let options = WindowOptions {
            window_bounds: Some(starting_bounds(preferences.window, zoom, cx)),
            titlebar: Some(TitlebarOptions {
                title: Some("B3313 Extended Multiplayer Control Panel".into()),
                ..TitlebarOptions::default()
            }),
            window_min_size: Some(size(px(960.), px(600.))),
            ..WindowOptions::default()
        };
        let opened = cx.open_window(options, |window, cx| {
            cx.new(|cx| Panel::new(form, problem, (preferences, unread), window, cx))
        });
        match opened {
            Ok(_) => {
                cx.activate(true);
                if let Some(test) = crash::Test::asked() {
                    // Once the panel is running, where real crashes happen.
                    cx.spawn(async move |cx| {
                        cx.background_executor().timer(Duration::from_secs(2)).await;
                        test.run();
                    })
                    .detach();
                }
            }
            Err(error) => {
                // Said in a box: a program with a window has no terminal to say it in, and
                // the panel would seem to do nothing when started.
                alert::say_without_a_window(&format!(
                    "The panel could not open its window: {error}.\n\nA graphics driver that \
                     is out of date is the usual cause."
                ));
                cx.quit();
            }
        }
    });
}
