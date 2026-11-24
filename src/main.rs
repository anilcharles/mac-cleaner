#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod apps;
mod duplicates;
mod native;
#[cfg(debug_assertions)]
mod sample;
mod scanner;
mod settings;
#[cfg(debug_assertions)]
mod snapshot;
mod theme;
mod ui;
mod updater;

use gpui::{
    point, prelude::*, px, App, Application, AssetSource, Bounds, KeyBinding, Menu, MenuItem, SharedString,
    TitlebarOptions, WindowBounds, WindowOptions,
};
use std::borrow::Cow;

/// Icons are compiled into the binary; `ring/<percent>.svg` is drawn on demand.
struct Assets;

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$((concat!("icons/", $name, ".svg"), include_bytes!(concat!("../assets/icons/", $name, ".svg")) as &[u8])),*]
    };
}

const ICONS: &[(&str, &[u8])] = icons![
    "alert", "app", "archive", "auto", "box", "cache", "check", "check-bold", "chevron", "chevron-down", "chevron-up", "chip",
    "close", "code", "copy", "download", "drive", "empty", "file", "folder", "hammer", "logs", "mail", "minus", "moon", "package",
    "phone", "plus",
    "refresh", "reveal", "search", "shield", "sparkle", "spinner", "sun", "trash",
];

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(pct) = path.strip_prefix("ring/").and_then(|p| p.strip_suffix(".svg")) {
            let pct: f32 = pct.parse().unwrap_or(0.);
            return Ok(Some(Cow::Owned(ring_svg(pct).into_bytes())));
        }
        Ok(ICONS.iter().find(|(name, _)| *name == path).map(|(_, data)| Cow::Borrowed(*data)))
    }

    fn list(&self, _path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(ICONS.iter().map(|(name, _)| SharedString::from(*name)).collect())
    }
}

/// Progress arc on a 132×132 canvas, radius 58, 8px stroke, starting at 12 o'clock.
fn ring_svg(pct: f32) -> String {
    let pct = pct.clamp(0., 100.);
    let shape = if pct >= 100. {
        r#"<circle cx="66" cy="66" r="58"/>"#.to_string()
    } else {
        let angle = pct / 100. * std::f32::consts::TAU;
        let (x, y) = (66. + 58. * angle.sin(), 66. - 58. * angle.cos());
        let large = if pct > 50. { 1 } else { 0 };
        format!(r#"<path d="M66 8 A58 58 0 {large} 1 {x:.2} {y:.2}"/>"#)
    };
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 132 132" fill="none" stroke="black" stroke-width="8" stroke-linecap="round">{shape}</svg>"#
    )
}

fn main() {
    Application::new().with_assets(Assets).run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("escape", ui::Cancel, None),
            KeyBinding::new("enter", ui::Confirm, None),
            KeyBinding::new("cmd-q", ui::Quit, None),
            KeyBinding::new("cmd-r", ui::Rescan, None),
            KeyBinding::new("cmd-f", ui::Find, None),
            KeyBinding::new("cmd-a", ui::SelectAll, None),
            KeyBinding::new("cmd-shift-a", ui::DeselectAll, None),
            KeyBinding::new("cmd-y", ui::QuickLook, None),
            KeyBinding::new("cmd-w", ui::CloseWindow, None),
            KeyBinding::new("cmd-m", ui::Minimize, None),
            KeyBinding::new("cmd-=", ui::ZoomIn, None),
            KeyBinding::new("cmd-+", ui::ZoomIn, None),
            KeyBinding::new("cmd--", ui::ZoomOut, None),
            KeyBinding::new("cmd-0", ui::ZoomReset, None),
        ]);
        cx.on_action(|_: &ui::Quit, cx| cx.quit());
        cx.set_menus(vec![
            Menu {
                name: "Mac Cleaner".into(),
                items: vec![
                    MenuItem::action("About Mac Cleaner", ui::About),
                    MenuItem::action("Check for Updates…", ui::CheckForUpdates),
                    MenuItem::separator(),
                    MenuItem::action("Quit Mac Cleaner", ui::Quit),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("Scan Again", ui::Rescan),
                    MenuItem::action("Review & Clean…", ui::Confirm),
                    MenuItem::separator(),
                    MenuItem::action("Quick Look", ui::QuickLook),
                    MenuItem::separator(),
                    MenuItem::action("Close Window", ui::CloseWindow),
                ],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::action("Select All", ui::SelectAll),
                    MenuItem::action("Deselect All", ui::DeselectAll),
                    MenuItem::separator(),
                    MenuItem::action("Find", ui::Find),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Light", ui::ThemeLight),
                    MenuItem::action("Dark", ui::ThemeDark),
                    MenuItem::action("Match System", ui::ThemeAuto),
                    MenuItem::separator(),
                    MenuItem::action("Zoom In", ui::ZoomIn),
                    MenuItem::action("Zoom Out", ui::ZoomOut),
                    MenuItem::action("Actual Size", ui::ZoomReset),
                ],
            },
            Menu {
                name: "Window".into(),
                items: vec![MenuItem::action("Minimize", ui::Minimize)],
            },
        ]);
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        // The window opens at the saved zoom, and never larger than the screen.
        let zoom = settings::zoom_override().unwrap_or_else(|| settings::Settings::load().zoom);
        let bounds = Bounds::centered(None, ui::window_size(theme::WINDOW_SIZE, zoom), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Mac Cleaner".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(18.), px(18.))),
                }),
                window_min_size: Some(ui::window_size(theme::WINDOW_MIN, zoom)),
                app_id: Some("com.maccleaner.desktop".into()),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    #[allow(unused_mut)]
                    let mut app = ui::CleanerApp::new(window, cx);
                    // Debug builds only: a released app must not be steerable through its environment.
                    #[cfg(debug_assertions)]
                    if let Ok(dir) = std::env::var("MAC_CLEANER_SNAPSHOT_DIR") {
                        app.start_demo(dir, window, cx);
                    }
                    app
                })
            },
        )
        .expect("open window");
        cx.activate(true);
    });
}
