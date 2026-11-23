//! The GPUI view. Mirrors the original HTML/CSS design screen by screen.

use crate::app::{ago, bytes, kind, plural, Age, Check, Children, Modal, Model, NoteKind, Row, Screen, SortKey};
use crate::scanner::{self, Risk, ScanCtl};
use crate::settings::{Settings, ThemeMode, LARGE_MIN_CHOICES};
use crate::theme::*;
use gpui::{
    actions, div, linear_color_stop, linear_gradient, percentage, point, prelude::*, relative, rgb, svg,
    uniform_list, white, Animation, AnimationExt as _, AnyElement, BoxShadow, ClickEvent, ClipboardItem, Context,
    Div, ElementId, FocusHandle, FontWeight, Hsla, KeyDownEvent, MouseButton, MouseDownEvent, Pixels, Point,
    ScrollStrategy, SharedString, Size, Stateful, Subscription, Transformation, UniformListScrollHandle, Window,
};
use std::collections::HashSet;
use std::process::{Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

actions!(
    mac_cleaner,
    [
        Cancel, Quit, Confirm, Rescan, Find, SelectAll, DeselectAll, QuickLook, About, CloseWindow, Minimize,
        ThemeLight, ThemeDark, ThemeAuto, ZoomIn, ZoomOut, ZoomReset
    ]
);

/// Something a popup menu entry does.
#[derive(Clone)]
enum Cmd {
    Reveal(String),
    QuickLook(String),
    CopyPath(String),
    Expand(String),
    Ignore(String),
    Age(Age),
    LargeMin(u64),
}

struct PopItem {
    label: SharedString,
    checked: bool,
    cmd: Cmd,
}

/// A small menu shown at a point in the window: the right-click menu of a row, or a dropdown.
struct Popup {
    pos: Point<Pixels>,
    items: Vec<PopItem>,
}

pub struct CleanerApp {
    m: Model,
    focus: FocusHandle,
    list_scroll: UniformListScrollHandle,
    rows: Vec<Row>,
    /// The scan that is running, or the last one that ran.
    scan: Arc<ScanCtl>,
    toast: Option<SharedString>,
    settings: Settings,
    theme_mode: ThemeMode,
    /// Size of the whole interface: 1.0 is 100%.
    zoom: f32,
    /// Typing goes to the search box.
    search_on: bool,
    popup: Option<Popup>,
    /// (total, available) bytes of the disk.
    disk: Option<(u64, u64)>,
    /// The Kind column is dropped in a narrow window.
    show_kind: bool,
    app_icon: Arc<gpui::Image>,
    /// The walkthrough is showing made-up results (`MAC_CLEANER_SAMPLE`), for README screenshots.
    #[cfg(debug_assertions)]
    sample: bool,
    _appearance: Subscription,
}

impl CleanerApp {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let entity = cx.entity().downgrade();
        let appearance = window.observe_window_appearance(move |_, cx| {
            entity.update(cx, |_, cx| cx.notify()).ok();
        });
        let settings = Settings::load();
        scanner::set_large_file_min(settings.large_min);
        let zoom = crate::settings::zoom_override().unwrap_or(settings.zoom);
        window.set_rem_size(gpui::px(BASE_REM * zoom));
        Self {
            m: Model::new(scanner::source_infos()),
            focus,
            list_scroll: UniformListScrollHandle::new(),
            rows: vec![],
            scan: Arc::new(ScanCtl::default()),
            toast: None,
            theme_mode: crate::settings::theme_override().unwrap_or(settings.theme),
            zoom,
            settings,
            search_on: false,
            popup: None,
            disk: scanner::disk_space(),
            show_kind: true,
            app_icon: Arc::new(gpui::Image::from_bytes(
                gpui::ImageFormat::Png,
                include_bytes!("../assets/icon/AppIcon-256.png").to_vec(),
            )),
            #[cfg(debug_assertions)]
            sample: false,
            _appearance: appearance,
        }
    }

    /* ================================================================
       Actions
       ================================================================ */

    fn start_scan(&mut self, cx: &mut Context<Self>) {
        self.scan.cancel.store(true, Ordering::Relaxed); // stops a scan that is still running
        let ctl = Arc::new(ScanCtl::default());
        self.scan = ctl.clone();
        self.m.screen = Screen::Scanning;
        self.m.modal = Modal::None;
        self.popup = None;
        self.search_on = false;
        // Come back to the category that is open now (a rescan from Large Files stays there).
        let keep = self.m.cats.get(self.m.active).map(|c| c.info.id);
        cx.notify();

        // Repaint while scanning so the progress ring and the counter move.
        let ticking = ctl.clone();
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let scanning = this
                .update(cx, |this, cx| {
                    cx.notify();
                    this.m.screen == Screen::Scanning && Arc::ptr_eq(&this.scan, &ticking)
                })
                .unwrap_or(false);
            if !scanning {
                break;
            }
        })
        .detach();

        let ignored: HashSet<String> = self.settings.ignored.iter().cloned().collect();
        cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let job_ctl = ctl.clone();
            let scan = executor.spawn(async move { scanner::scan_all(&ignored, &job_ctl) });
            executor.timer(Duration::from_millis(900)).await; // keep the progress screen readable
            let cats = scan.await;
            this.update(cx, |this, cx| {
                // Cancelled, or replaced by a newer scan: this result is incomplete or stale.
                if ctl.cancelled() || !Arc::ptr_eq(&this.scan, &ctl) {
                    return;
                }
                this.m.load_scan(cats);
                if let Some(ci) = keep.and_then(|id| this.m.cats.iter().position(|c| c.info.id == id)) {
                    this.m.active = ci;
                }
                this.list_scroll = UniformListScrollHandle::new();
                this.settings.last_scan = Some(now_ms());
                this.settings.save();
                this.disk = scanner::disk_space();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn cancel_scan(&mut self, cx: &mut Context<Self>) {
        self.scan.cancel.store(true, Ordering::Relaxed);
        self.m.screen = if self.m.cats.is_empty() { Screen::Welcome } else { Screen::Results };
        cx.notify();
    }

    fn toggle_expand(&mut self, path: String, cx: &mut Context<Self>) {
        if self.m.expanded.remove(&path) {
            return cx.notify();
        }
        self.m.expanded.insert(path.clone());
        if self.m.children.contains_key(&path) {
            return cx.notify();
        }
        #[cfg(debug_assertions)]
        if self.sample {
            let (size, modified) = self.m.nodes.get(&path).map(|n| (n.size, n.modified)).unwrap_or_default();
            self.m.load_children(&path, Ok(crate::sample::listing(&path, size, modified)));
            return cx.notify();
        }
        self.m.children.insert(path.clone(), Children::Loading);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let p = path.clone();
            let listing = cx.background_executor().spawn(async move { scanner::list_children(&p) }).await;
            this.update(cx, |this, cx| {
                this.m.load_children(&path, listing);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn run_clean(&mut self, cx: &mut Context<Self>) {
        let paths = self.m.scoped();
        let scope = self.m.scope;
        let cleaned = paths.clone();
        self.m.modal = Modal::Working;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let job = executor.spawn(async move { scanner::clean(paths) });
            executor.timer(Duration::from_millis(600)).await;
            let report = job.await;
            this.update(cx, |this, cx| {
                this.settings.freed_total += report.freed;
                this.settings.cleans += u64::from(report.removed > 0);
                this.settings.save();
                this.disk = scanner::disk_space();
                this.m.report = Some(report);
                if scope.is_some() {
                    // Only one category was cleaned: keep the rest of the results to go back to.
                    for p in cleaned.iter().filter(|p| std::fs::symlink_metadata(p).is_err()) {
                        this.m.remove_cleaned(p);
                    }
                    this.m.cursor = None;
                } else {
                    this.m.clear_results();
                }
                this.m.scope = None;
                this.m.modal = Modal::None;
                this.m.screen = Screen::Done;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn reveal(&mut self, path: &str, cx: &mut Context<Self>) {
        let p = std::path::Path::new(path);
        if !scanner::is_allowed(p) || Command::new("/usr/bin/open").arg("-R").arg(p).spawn().is_err() {
            self.show_toast("Couldn't show this item in Finder.", cx);
        }
    }

    fn quick_look(&mut self, path: &str, cx: &mut Context<Self>) {
        let p = std::path::Path::new(path);
        let preview = Command::new("/usr/bin/qlmanage").arg("-p").arg(p).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        if !scanner::is_allowed(p) || preview.is_err() {
            self.show_toast("Couldn't preview this item.", cx);
        }
    }

    fn open_category(&mut self, ci: usize, cx: &mut Context<Self>) {
        if self.m.active != ci {
            self.m.active = ci;
            self.m.cursor = None;
            self.list_scroll = UniformListScrollHandle::new();
        }
        self.search_on = false;
        cx.notify();
    }

    /// The list changed under the search text or a filter: start again from the top.
    fn filter_changed(&mut self, cx: &mut Context<Self>) {
        self.m.cursor = None;
        self.list_scroll = UniformListScrollHandle::new();
        cx.notify();
    }

    fn clear_filters(&mut self, cx: &mut Context<Self>) {
        self.m.query.clear();
        self.m.age = Age::Any;
        self.search_on = false;
        self.filter_changed(cx);
    }

    fn set_theme(&mut self, mode: ThemeMode, cx: &mut Context<Self>) {
        self.theme_mode = mode;
        self.settings.theme = mode;
        self.settings.save();
        cx.notify();
    }

    /// Makes the whole interface bigger or smaller. Every size is in rems, so changing the
    /// window's rem size scales text, icons and spacing together.
    fn set_zoom(&mut self, zoom: f32, window: &mut Window, cx: &mut Context<Self>) {
        if (zoom - self.zoom).abs() < 0.001 {
            return;
        }
        let old = self.zoom;
        self.zoom = zoom;
        self.settings.zoom = zoom;
        self.settings.save();
        self.popup = None; // it was placed in pixels of the old size
        window.set_rem_size(gpui::px(BASE_REM * zoom));

        let min = window_size(WINDOW_MIN, zoom);
        crate::native::set_min_content_size(window, min.width.into(), min.height.into());
        // The window grows and shrinks with its content, so the layout stays as it was.
        if !window.is_fullscreen() && !window.is_maximized() {
            let now = window.viewport_size();
            let limit = window_size((f32::MAX, f32::MAX), 1.);
            let k = zoom / old;
            window.resize(gpui::size(
                (now.width * k).min(limit.width).max(min.width),
                (now.height * k).min(limit.height).max(min.height),
            ));
        }
        cx.notify();
    }

    /// One zoom level up (`delta > 0`) or down.
    fn step_zoom(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let next = if delta > 0 {
            ZOOM_STEPS.iter().copied().find(|s| *s > self.zoom + 0.001)
        } else {
            ZOOM_STEPS.iter().rev().copied().find(|s| *s < self.zoom - 0.001)
        };
        if let Some(zoom) = next {
            self.set_zoom(zoom, window, cx);
        }
    }

    /// Asks before cleaning one category (`scope`), or everything selected.
    fn open_confirm(&mut self, scope: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.m.scope = scope;
        if self.m.scoped_selection().0 > 0 {
            self.m.modal = Modal::Confirm;
            self.popup = None;
            window.focus(&self.focus);
            cx.notify();
        }
    }

    /// Results are on screen with nothing on top of them.
    fn browsing(&self) -> bool {
        self.m.screen == Screen::Results && self.m.modal == Modal::None && self.popup.is_none()
    }

    fn run_cmd(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        match cmd {
            Cmd::Reveal(p) => self.reveal(&p, cx),
            Cmd::QuickLook(p) => self.quick_look(&p, cx),
            Cmd::CopyPath(p) => {
                cx.write_to_clipboard(ClipboardItem::new_string(p));
                self.show_toast("Path copied.", cx);
            }
            Cmd::Expand(p) => self.toggle_expand(p, cx),
            Cmd::Ignore(p) => {
                if self.settings.ignore(&p) {
                    self.settings.save();
                    self.m.ignore(&p);
                    self.show_toast("Hidden from now on. Use Show Again in the sidebar to bring it back.", cx);
                } else {
                    self.show_toast("Couldn't hide this item.", cx);
                }
            }
            Cmd::Age(age) => {
                self.m.age = age;
                self.filter_changed(cx);
            }
            Cmd::LargeMin(bytes) => {
                if bytes != self.settings.large_min {
                    self.settings.large_min = bytes;
                    self.settings.save();
                    scanner::set_large_file_min(bytes);
                    self.start_scan(cx);
                    // The old list no longer matches the new limit.
                    self.m.clear_results();
                }
            }
        }
    }

    fn row_menu(&mut self, path: &str, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(node) = self.m.nodes.get(path) else { return };
        let item = |label: &'static str, cmd: Cmd| PopItem { label: label.into(), checked: false, cmd };
        let p = || path.to_string();
        let mut items = vec![
            item("Show in Finder", Cmd::Reveal(p())),
            item("Quick Look", Cmd::QuickLook(p())),
            item("Copy Path", Cmd::CopyPath(p())),
        ];
        if node.is_dir {
            let open = self.m.expanded.contains(path);
            items.push(item(if open { "Close Folder" } else { "Open Folder" }, Cmd::Expand(p())));
        }
        if node.parent.is_none() {
            items.push(item("Always Ignore", Cmd::Ignore(p())));
        }
        self.m.cursor = Some(p());
        self.search_on = false;
        self.popup = Some(Popup { pos, items });
        cx.notify();
    }

    /* ---------- Keyboard ---------- */

    fn move_cursor(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.m.rows();
        let items: Vec<(usize, &String)> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                Row::Item { path, .. } => Some((i, path)),
                Row::Note { .. } => None,
            })
            .collect();
        if items.is_empty() {
            return;
        }
        let last = items.len() - 1;
        let at = self.m.cursor.as_ref().and_then(|c| items.iter().position(|(_, p)| *p == c));
        let next = match at {
            Some(i) => (i as isize + delta).clamp(0, last as isize) as usize,
            None if delta > 0 => 0,
            None => last,
        };
        let (ix, path) = items[next];
        self.m.cursor = Some(path.clone());
        self.list_scroll.scroll_to_item(ix, ScrollStrategy::Top);
        cx.notify();
    }

    /// Keys that are not menu shortcuts: moving through the list and typing in the search box.
    fn key_down(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.browsing() {
            return;
        }
        let stroke = &event.keystroke;
        let key = stroke.key.as_str();
        if stroke.modifiers.platform {
            // ⌘1…⌘9 open a category.
            if let Some(ci) = key.parse::<usize>().ok().filter(|n| (1..=self.m.cats.len()).contains(n)) {
                self.open_category(ci - 1, cx);
            }
            return;
        }
        if stroke.modifiers.control {
            return;
        }
        let typed = stroke.key_char.as_deref().filter(|c| !c.is_empty() && !c.chars().any(char::is_control));

        if self.search_on {
            match (key, typed) {
                ("backspace", _) => {
                    self.m.query.pop();
                    self.filter_changed(cx);
                }
                ("down", _) => {
                    self.search_on = false;
                    self.move_cursor(1, cx);
                }
                ("up" | "tab", _) => {
                    self.search_on = false;
                    cx.notify();
                }
                (_, Some(text)) => {
                    self.m.query.push_str(text);
                    self.filter_changed(cx);
                }
                _ => {}
            }
            return;
        }

        let cursor = self.m.cursor.clone();
        match (key, cursor) {
            ("down", _) => self.move_cursor(1, cx),
            ("up", _) => self.move_cursor(-1, cx),
            ("space", Some(path)) => {
                self.m.toggle(&path);
                cx.notify();
            }
            ("right", Some(path)) => {
                if self.m.nodes[&path].is_dir && !self.m.expanded.contains(&path) {
                    self.toggle_expand(path, cx);
                }
            }
            ("left", Some(path)) => {
                if self.m.expanded.contains(&path) {
                    self.toggle_expand(path, cx);
                } else if let Some(parent) = self.m.nodes[&path].parent.clone() {
                    self.m.cursor = Some(parent);
                    self.move_cursor(0, cx);
                }
            }
            ("backspace", _) if !self.m.query.is_empty() => {
                self.m.query.pop();
                self.filter_changed(cx);
            }
            ("space" | "left" | "right" | "tab", None) => {}
            // Any other printable key starts a search.
            (_, _) => {
                if let Some(text) = typed.filter(|c| !c.trim().is_empty()) {
                    self.search_on = true;
                    self.m.query.push_str(text);
                    self.filter_changed(cx);
                }
            }
        }
    }

    fn on_cancel(&mut self, cx: &mut Context<Self>) {
        if self.popup.take().is_some() {
        } else if matches!(self.m.modal, Modal::Confirm | Modal::About) {
            self.m.modal = Modal::None;
        } else if self.m.screen == Screen::Results && (self.search_on || !self.m.query.is_empty()) {
            self.m.query.clear();
            self.search_on = false;
            return self.filter_changed(cx);
        }
        cx.notify();
    }

    fn on_confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_some() {
            return;
        }
        match (self.m.modal, self.m.screen) {
            (Modal::About, _) => {
                self.m.modal = Modal::None;
                cx.notify();
            }
            (Modal::None, Screen::Welcome) => self.start_scan(cx),
            (Modal::None, Screen::Results) if self.search_on => {
                self.search_on = false;
                cx.notify();
            }
            (Modal::None, Screen::Results) => self.open_confirm(None, window, cx),
            // Return never confirms the move to the Trash: that takes a click.
            _ => {}
        }
    }

    fn show_toast(&mut self, msg: &str, cx: &mut Context<Self>) {
        self.toast = Some(msg.to_string().into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(2600)).await;
            this.update(cx, |this, cx| {
                this.toast = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Developer-only walkthrough that saves a PNG of every screen. It never cleans anything:
    /// the "done" screen is shown with a made-up report.
    #[cfg(debug_assertions)]
    pub fn start_demo(&mut self, dir: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(wid) = crate::snapshot::window_number(window) else { return };
        self.settings.frozen = true; // the walkthrough never changes saved settings or history
        // With MAC_CLEANER_SAMPLE set, nothing is scanned: the screens show made-up results.
        let sample = std::env::var("MAC_CLEANER_SAMPLE").is_ok();
        self.sample = sample;
        if sample {
            // In-memory only (settings are frozen): nothing about this Mac appears in the pictures.
            self.settings.large_min = 100_000_000;
            self.settings.ignored.clear();
            self.settings.last_scan = None;
            self.settings.freed_total = 0;
            self.disk = Some((500_000_000_000, 212_000_000_000));
        }
        let snap = move |name: &str| {
            let path = format!("{dir}/{name}.png");
            eprintln!("snapshot {path}: {}", crate::snapshot::save(wid, &path));
        };
        cx.spawn_in(window, async move |this, cx| {
            let executor = cx.background_executor().clone();
            let wait = move |ms| executor.timer(Duration::from_millis(ms));
            // Another window on top would stop this one from redrawing, and frames would repeat.
            let forward = |this: &gpui::WeakEntity<Self>, cx: &mut gpui::AsyncWindowContext| {
                this.update_in(cx, |_, window, cx| {
                    if sample {
                        // README pictures: an inactive window has grey title-bar buttons.
                        crate::native::activate(window);
                    } else {
                        crate::native::order_front(window);
                    }
                    cx.notify();
                })
                .ok();
            };
            wait(1500).await;
            forward(&this, cx);
            wait(250).await;
            snap("1-welcome");
            this.update_in(cx, |this, window, _| window.resize(window_size(WINDOW_MIN, this.zoom))).ok();
            wait(900).await;
            forward(&this, cx);
            wait(250).await;
            snap("1b-welcome-min");
            this.update_in(cx, |this, window, _| window.resize(window_size(WINDOW_SIZE, this.zoom))).ok();
            wait(600).await;

            if sample {
                // A progress screen frozen part-way, then the sample results.
                this.update(cx, |this, cx| {
                    let ctl = ScanCtl::default();
                    ctl.finished.lock().unwrap().extend(this.m.sources.iter().take(5).map(|s| s.id));
                    ctl.checked.store(184_230, Ordering::Relaxed);
                    this.scan = Arc::new(ctl);
                    this.m.screen = Screen::Scanning;
                    cx.notify();
                })
                .ok();
                wait(450).await;
                forward(&this, cx);
                wait(250).await;
                snap("2-scanning");
                this.update(cx, |this, cx| {
                    this.m.load_scan(crate::sample::categories(now_ms()));
                    cx.notify();
                })
                .ok();
            } else {
                this.update(cx, |this, cx| this.start_scan(cx)).ok();
                wait(450).await;
                forward(&this, cx);
                wait(250).await;
                snap("2-scanning");
            }
            while this.update(cx, |this, _| this.m.screen != Screen::Results).unwrap_or(false) {
                wait(200).await;
            }
            wait(700).await;
            forward(&this, cx);
            wait(250).await;
            snap("3-results");

            // Open the biggest item and the biggest folder inside it, then untick one entry.
            let first = this.update(cx, |this, cx| {
                let cat = &this.m.cats[this.m.active];
                let first = this.m.sorted(&cat.items).into_iter().find(|p| this.m.nodes[p].is_dir)?;
                this.toggle_expand(first.clone(), cx);
                Some(first)
            });
            if let Ok(Some(first)) = first {
                while this.update(cx, |this, _| matches!(this.m.children.get(&first), Some(Children::Loading))).unwrap_or(false) {
                    wait(150).await;
                }
                let child = this.update(cx, |this, cx| {
                    let Some(Children::Loaded { entries, .. }) = this.m.children.get(&first) else { return None };
                    let sorted = this.m.sorted(entries);
                    let dir = sorted.iter().find(|p| this.m.nodes[*p].is_dir).cloned();
                    if let Some(second) = sorted.get(1) {
                        this.m.toggle(second);
                    }
                    if let Some(dir) = &dir {
                        this.toggle_expand(dir.clone(), cx);
                    }
                    dir
                });
                if let Ok(Some(dir)) = child {
                    while this.update(cx, |this, _| matches!(this.m.children.get(&dir), Some(Children::Loading))).unwrap_or(false) {
                        wait(150).await;
                    }
                }
            }
            wait(700).await;
            forward(&this, cx);
            wait(250).await;
            snap("4-tree");

            this.update(cx, |this, cx| {
                if let Some(ci) = this.m.cats.iter().position(|c| c.info.id == "large-files") {
                    this.m.active = ci;
                    this.list_scroll = UniformListScrollHandle::new();
                    cx.notify();
                }
            })
            .ok();
            wait(700).await;
            forward(&this, cx);
            wait(250).await;
            snap("4b-large-files");
            this.update(cx, |this, cx| {
                this.m.active = this.m.cats.iter().position(|c| !c.items.is_empty()).unwrap_or(0);
                this.m.query = "com".into();
                this.search_on = true;
                cx.notify();
            })
            .ok();
            wait(500).await;
            forward(&this, cx);
            wait(250).await;
            snap("4c-search");
            this.update(cx, |this, cx| {
                this.m.query.clear();
                this.search_on = false;
                let first = this.m.rows().into_iter().find_map(|r| match r {
                    Row::Item { path, .. } => Some(path),
                    Row::Note { .. } => None,
                });
                if let Some(path) = first {
                    let at = point(gpui::px(620. * this.zoom), gpui::px(250. * this.zoom));
                    this.row_menu(&path, at, cx);
                }
                cx.notify();
            })
            .ok();
            wait(500).await;
            forward(&this, cx);
            wait(250).await;
            snap("4d-row-menu");
            this.update(cx, |this, cx| {
                this.popup = None;
                this.m.modal = Modal::About;
                cx.notify();
            })
            .ok();
            wait(500).await;
            forward(&this, cx);
            wait(250).await;
            snap("4e-about");

            this.update(cx, |this, cx| {
                this.m.modal = Modal::Confirm;
                cx.notify();
            })
            .ok();
            wait(700).await;
            forward(&this, cx);
            wait(250).await;
            snap("5-confirm");
            this.update(cx, |this, cx| {
                this.m.modal = Modal::None;
                cx.notify();
            })
            .ok();

            this.update_in(cx, |this, window, _| window.resize(window_size(WINDOW_MIN, this.zoom))).ok();
            wait(900).await;
            forward(&this, cx);
            wait(250).await;
            snap("6-results-min");
            this.update_in(cx, |this, window, _| window.resize(window_size(WINDOW_SIZE, this.zoom))).ok();
            wait(600).await;

            // Change the zoom while running (settings are frozen, so nothing is saved).
            this.update_in(cx, |this, window, cx| this.step_zoom(1, window, cx)).ok();
            wait(900).await;
            forward(&this, cx);
            wait(250).await;
            snap("6b-zoom-in");
            this.update_in(cx, |this, window, cx| this.step_zoom(-1, window, cx)).ok();
            wait(900).await;
            forward(&this, cx);
            wait(250).await;
            snap("6c-zoom-back");

            this.update(cx, |this, cx| {
                let (_, freed) = this.m.selection();
                this.m.report = Some(scanner::CleanReport {
                    removed: this.m.selected.len().saturating_sub(1),
                    freed,
                    failures: vec![scanner::Failure {
                        path: format!(
                            "{}/Library/Caches/com.apple.Safari",
                            if this.sample { crate::sample::HOME.to_string() } else { scanner::home_dir().display().to_string() }
                        ),
                        reason: "macOS protects this item. Grant Full Disk Access to clean it.".into(),
                    }],
                });
                this.m.screen = Screen::Done;
                cx.notify();
            })
            .ok();
            wait(900).await;
            forward(&this, cx);
            wait(250).await;
            snap("7-done");
            this.update(cx, |_, cx| cx.quit()).ok();
        })
        .detach();
    }

    /* ================================================================
       Screens
       ================================================================ */

    fn welcome(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let scope = self.m.sources.iter().map(|s| {
            div()
                .w(relative(0.5))
                .flex()
                .items_center()
                .gap(pt(12.))
                .px(pt(10.))
                .py(pt(6.))
                .child(tile(s.id, s.icon, 26.))
                .child(div().flex_1().min_w_0().ellipsis().font_weight(FontWeight::MEDIUM).child(s.title))
                .when(s.risk == Risk::Review, |el| el.child(badge("Review", t.warning, t.warning_soft)))
        });

        // Disk space and a little history, when there is any.
        let mut facts = vec![];
        if let Some((total, free)) = self.disk {
            facts.push(format!("{} available of {}", bytes(free), bytes(total)));
        }
        if let Some(when) = self.settings.last_scan {
            facts.push(format!("Last scan: {}", ago(Some(when)).to_lowercase()));
        }
        if self.settings.freed_total > 0 {
            facts.push(format!("{} cleaned so far", bytes(self.settings.freed_total)));
        }

        div()
            .w_full()
            .min_h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .pt(TITLEBAR)
            .px(pt(48.))
            .pb(pt(24.))
            // The app icon itself; its artwork has built-in margins and shadow.
            .child(gpui::img(self.app_icon.clone()).size(pt(104.)).mb(pt(8.)))
            .child(div().text_size(pt(30.)).font_weight(FontWeight::BOLD).child("Mac Cleaner"))
            .child(
                div()
                    .max_w(pt(440.))
                    .mt(pt(8.))
                    .mb(pt(24.))
                    .text_center()
                    .text_size(pt(15.))
                    .line_height(pt(22.))
                    .text_color(t.text_2)
                    .child("Find caches, logs and developer leftovers taking up space. Nothing is removed until you review and confirm it."),
            )
            .child(
                button("start", "Start Scan", None, Btn::Large, true, t)
                    .on_click(cx.listener(|this, _, _, cx| this.start_scan(cx))),
            )
            .child(
                div()
                    .w_full()
                    .max_w(pt(600.))
                    .mt(pt(28.))
                    .p(pt(8.))
                    .rounded(pt(14.))
                    .bg(t.surface)
                    .shadow(card_shadow(t))
                    .flex()
                    .flex_wrap()
                    .children(scope),
            )
            .when(!facts.is_empty(), |el| {
                el.child(div().mt(pt(20.)).text_size(pt(12.5)).text_color(t.text_2).child(facts.join("   ·   ")))
            })
            .child(
                div()
                    .mt(pt(if facts.is_empty() { 20. } else { 8. }))
                    .flex()
                    .items_center()
                    .gap(pt(8.))
                    .text_size(pt(12.))
                    .text_color(t.text_3)
                    .child(icon("shield", 15., t.success))
                    .child("Items are moved to the Trash, so you can always put them back."),
            )
    }

    fn scanning(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let finished = self.scan.finished.lock().unwrap().clone();
        let checked = self.scan.checked.load(Ordering::Relaxed);
        let total = self.m.sources.len().max(1);
        let pct = (finished.len() * 100 / total) as u32;
        let current = self.m.sources.iter().find(|s| !finished.contains(&s.id));

        let steps = self.m.sources.iter().enumerate().map(|(i, s)| {
            let done = finished.contains(&s.id);
            div()
                .h(pt(30.))
                .px(pt(10.))
                .flex()
                .items_center()
                .gap(pt(12.))
                .text_color(if done { t.text } else { t.text_3 })
                .child(if done { tile(s.id, s.icon, 20.) } else { tile("muted", s.icon, 20.).opacity(0.45) })
                .child(div().flex_1().child(s.title))
                .child(
                    div().size(pt(18.)).flex().items_center().justify_center().child(if done {
                        icon("check", 16., t.success).into_any_element()
                    } else {
                        spinner(("step", i), 14., t).into_any_element()
                    }),
                )
        });

        div()
            .w_full()
            .min_h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .pt(TITLEBAR)
            .px(pt(48.))
            .pb(pt(24.))
            .child(
                div()
                    .relative()
                    .size(pt(132.))
                    .mb(pt(20.))
                    .child(div().absolute().inset_0().rounded_full().border(pt(8.)).border_color(t.border))
                    .when(pct > 0, |el| {
                        el.child(svg().absolute().inset_0().size(pt(132.)).path(format!("ring/{pct}.svg")).text_color(t.accent))
                    })
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(pt(28.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{pct}%")),
                    ),
            )
            .child(div().text_size(pt(20.)).font_weight(FontWeight::SEMIBOLD).child("Scanning your Mac…"))
            .child(div().mt(pt(6.)).text_color(t.text_2).child(match current {
                Some(s) => format!("Looking through {}  ·  {} items checked", s.title, grouped(checked)),
                None => "Finishing up".into(),
            }))
            .child(
                div()
                    .w(pt(360.))
                    .mt(pt(20.))
                    .p(pt(6.))
                    .rounded(pt(14.))
                    .bg(t.surface)
                    .shadow(card_shadow(t))
                    .children(steps),
            )
            .child(
                div().mt(pt(16.)).child(
                    button("cancel-scan", "Cancel", None, Btn::Secondary, true, t)
                        .on_click(cx.listener(|this, _, _, cx| this.cancel_scan(cx))),
                ),
            )
    }

    fn results(&mut self, t: &Theme, cx: &mut Context<Self>) -> Div {
        div()
            .size_full()
            .flex()
            .child(self.sidebar(t, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.detail(t, cx))
                    .child(self.action_bar(t, cx)),
            )
    }

    fn sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        // Large Files are the user's own files, so they only count once they are selected.
        let (junk, junk_cats) = self.m.junk_found();
        let personal = self.m.personal_found();
        let (_, sel_size) = self.m.selection();
        let cleanable = junk + self.m.personal_selected();
        let pct = if cleanable > 0 { (sel_size as f32 / cleanable as f32).min(1.) } else { 0. };
        let hidden = self.settings.ignored.len();

        let rows = self.m.cats.iter().enumerate().map(|(ci, c)| {
            let (n, size, state) = self.m.cat_selection(ci);
            let empty = c.items.is_empty();
            let active = ci == self.m.active;
            let sub = if empty {
                "Nothing found".to_string()
            } else if n > 0 {
                format!("{} selected", bytes(size))
            } else {
                plural(c.items.len(), "item", "items")
            };
            div()
                .id(("cat", ci))
                .min_h(pt(52.))
                .px(pt(12.))
                .rounded(pt(10.))
                .flex()
                .items_center()
                .gap(pt(12.))
                .cursor_pointer()
                .when(active, |el| el.bg(t.surface).shadow(small_shadow(t)))
                .when(!active, |el| el.hover(|s| s.bg(t.hover)))
                .on_click(cx.listener(move |this, _, _, cx| this.open_category(ci, cx)))
                .child(
                    div()
                        .id(("cat-check", ci))
                        .when(empty, |el| el.opacity(0.5))
                        .when(!empty, |el| {
                            el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                let on = this.m.cat_selection(ci).2 != Check::On;
                                this.m.set_category(ci, on);
                                cx.notify();
                            }))
                        })
                        .child(checkbox(state, t)),
                )
                .child(if empty { tile("muted", c.info.icon, 30.).opacity(0.5) } else { tile(c.info.id, c.info.icon, 30.) })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(div().ellipsis().font_weight(FontWeight::SEMIBOLD).child(c.info.title))
                        .child(div().ellipsis().text_size(pt(11.5)).text_color(t.text_3).child(sub)),
                )
                .when(!empty, |el| {
                    el.child(div().text_size(pt(12.)).font_weight(FontWeight::MEDIUM).text_color(t.text_2).child(bytes(c.total)))
                })
        });

        div()
            .w(SIDEBAR_W)
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .pt(TITLEBAR)
            .px(pt(12.))
            .pb(pt(12.))
            .bg(t.sidebar)
            .border_r_1()
            .border_color(t.border)
            .child(
                div()
                    .mx(pt(4.))
                    .mb(pt(16.))
                    .p(pt(16.))
                    .rounded(pt(14.))
                    .bg(t.surface)
                    .shadow(card_shadow(t))
                    .child(div().text_size(pt(12.)).font_weight(FontWeight::MEDIUM).text_color(t.text_2).child("Selected to clean"))
                    .child(div().mt(pt(2.)).text_size(pt(30.)).font_weight(FontWeight::BOLD).child(bytes(sel_size)))
                    .child(
                        div()
                            .h(pt(6.))
                            .mt(pt(12.))
                            .mb(pt(8.))
                            .rounded(pt(3.))
                            .bg(t.border)
                            .overflow_hidden()
                            .child(div().h_full().w(relative(pct)).rounded(pt(3.)).bg(gradient_h(0x34c7bd, t.accent))),
                    )
                    .child(div().text_size(pt(12.)).text_color(t.text_3).child(format!(
                        "{} of junk found in {}",
                        bytes(junk),
                        plural(junk_cats, "category", "categories")
                    )))
                    .when(personal > 0, |el| {
                        el.child(
                            div()
                                .mt(pt(2.))
                                .text_size(pt(12.))
                                .text_color(t.text_3)
                                .child(format!("Plus {} of large files to review", bytes(personal))),
                        )
                    }),
            )
            .child(
                div()
                    .mx(pt(16.))
                    .mb(pt(6.))
                    .text_size(pt(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_3)
                    .child("CATEGORIES"),
            )
            .child(div().id("cat-list").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(pt(2.)).children(rows))
            .when(hidden > 0, |el| {
                el.child(
                    div()
                        .mt(pt(8.))
                        .pl(pt(16.))
                        .pr(pt(4.))
                        .flex()
                        .items_center()
                        .text_size(pt(12.))
                        .text_color(t.text_3)
                        .child(div().flex_1().child(format!("{} hidden", plural(hidden, "item", "items"))))
                        .child(button("show-hidden", "Show Again", None, Btn::Ghost, true, t).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.settings.ignored.clear();
                                this.settings.save();
                                this.start_scan(cx);
                            },
                        ))),
                )
            })
    }

    fn detail(&mut self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let ci = self.m.active;
        // Select All and Deselect All act on what the search and filters show.
        let (n, state) = self.m.visible_selection();
        let shown = self.m.visible(ci).len();
        let (in_category, category_size, _) = self.m.cat_selection(ci);
        let cat = &self.m.cats[ci];
        let empty = cat.items.is_empty();
        let no_match = !empty && shown == 0;
        let (id, icon_name, title, description, risk) =
            (cat.info.id, cat.info.icon, cat.info.title, cat.info.description, cat.info.risk);

        let header = div()
            .flex()
            .items_center()
            .gap(pt(16.))
            .pb(pt(20.))
            .child(tile_lg(id, icon_name))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(pt(8.))
                            .child(div().text_size(pt(20.)).font_weight(FontWeight::BOLD).child(title))
                            .child(if risk == Risk::Review {
                                badge("Review", t.warning, t.warning_soft)
                            } else {
                                badge("Safe to remove", t.success, t.success_soft)
                            }),
                    )
                    .child(div().mt(pt(3.)).text_color(t.text_2).line_height(pt(19.)).child(description)),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .gap(pt(4.))
                    .child({
                        let enabled = shown > 0 && state != Check::On;
                        button("select-all", "Select All", None, Btn::Ghost, enabled, t).when(enabled, |b| {
                            b.on_click(cx.listener(move |this, _, _, cx| {
                                this.m.set_visible(true);
                                cx.notify();
                            }))
                        })
                    })
                    .child(button("select-none", "Deselect All", None, Btn::Ghost, n > 0, t).when(n > 0, |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| {
                            this.m.set_visible(false);
                            cx.notify();
                        }))
                    }))
                    // Cleans what is selected in this category only; the bar at the bottom cleans everything.
                    .child({
                        let label = if in_category > 0 { format!("Clean {}", bytes(category_size)) } else { "Clean".to_string() };
                        div().ml(pt(6.)).child(
                            button("review-category", label, Some("trash"), Btn::Secondary, in_category > 0, t).when(in_category > 0, |b| {
                                b.on_click(cx.listener(move |this, _, window, cx| this.open_confirm(Some(ci), window, cx)))
                            }),
                        )
                    }),
            );

        let table = if empty {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(pt(8.))
                .text_color(t.text_3)
                .child(icon("empty", 40., t.success))
                .child(div().text_size(pt(15.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child("All clean"))
                .child(format!("Nothing to remove in {title}."))
        } else if no_match {
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(pt(8.))
                .text_color(t.text_3)
                .child(icon("search", 36., t.text_3))
                .child(div().text_size(pt(15.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child("No matches"))
                .child("Nothing here fits the search and filters.")
                .child(
                    button("clear-filters", "Clear Filters", None, Btn::Ghost, true, t)
                        .on_click(cx.listener(|this, _, _, cx| this.clear_filters(cx))),
                )
        } else {
            self.rows = self.m.rows();
            let theme = *t;
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(self.table_head(t, cx))
                .child(
                    uniform_list(
                        "rows",
                        self.rows.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                            range.map(|ix| this.row(ix, &theme, cx)).collect::<Vec<_>>()
                        }),
                    )
                    .track_scroll(self.list_scroll.clone())
                    .flex_1(),
                )
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .pt(TITLEBAR)
            .px(pt(24.))
            .child(header)
            .when(!empty, |el| el.child(self.filter_bar(id == "large-files", shown, t, cx)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .mb(pt(16.))
                    .rounded(pt(14.))
                    .bg(t.surface)
                    .shadow(card_shadow(t))
                    .overflow_hidden()
                    .child(table),
            )
    }

    /// Search box and filters above the table.
    fn filter_bar(&self, large_files: bool, shown: usize, t: &Theme, cx: &mut Context<Self>) -> Div {
        let total = self.m.cats[self.m.active].items.len();
        let query = self.m.query.clone();
        let caret = || div().w(pt(1.5)).h(pt(15.)).flex_none().bg(t.accent);

        let search = div()
            .id("search")
            .flex_1()
            .min_w(pt(120.))
            .max_w(pt(300.))
            .h(pt(30.))
            .px(pt(10.))
            .rounded(pt(8.))
            .bg(t.surface)
            .border_1()
            .border_color(if self.search_on { t.accent } else { t.border_strong })
            .flex()
            .items_center()
            .gap(pt(7.))
            .overflow_hidden()
            .on_click(cx.listener(|this, _, _, cx| {
                this.search_on = true;
                cx.notify();
            }))
            .child(icon("search", 14., t.text_3))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(pt(1.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(query.is_empty(), |el| {
                        el.when(self.search_on, |el| el.child(caret())).child(div().text_color(t.text_3).child("Search"))
                    })
                    .when(!query.is_empty(), |el| {
                        // A space at the end would not be drawn; a no-break space is.
                        el.child(query.replace(' ', "\u{a0}")).when(self.search_on, |el| el.child(caret()))
                    }),
            )
            .when(!query.is_empty(), |el| {
                el.child(
                    div()
                        .id("search-clear")
                        .size(pt(18.))
                        .flex_none()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .child(icon("close", 11., t.text_3))
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.m.query.clear();
                            this.filter_changed(cx);
                        })),
                )
            });

        let age = self.m.age;
        let age_items = move || -> Vec<PopItem> {
            Age::ALL.iter().map(|a| PopItem { label: a.label().into(), checked: *a == age, cmd: Cmd::Age(*a) }).collect()
        };
        let min = self.settings.large_min;
        let min_items = move || -> Vec<PopItem> {
            LARGE_MIN_CHOICES
                .iter()
                .map(|b| PopItem { label: format!("At least {}", bytes(*b)).into(), checked: *b == min, cmd: Cmd::LargeMin(*b) })
                .collect()
        };

        div()
            .flex()
            .items_center()
            .gap(pt(8.))
            .pb(pt(12.))
            .child(search)
            .child(dropdown("age-filter", age.label().into(), age != Age::Any, t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.popup = Some(Popup { pos: event.position, items: age_items() });
                    cx.notify();
                }),
            ))
            .when(large_files, |el| {
                el.child(dropdown("size-filter", format!("At least {}", bytes(min)), false, t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        this.popup = Some(Popup { pos: event.position, items: min_items() });
                        cx.notify();
                    }),
                ))
            })
            .child(div().flex_1())
            .when(self.m.filter_active(), |el| {
                el.child(div().flex_none().text_size(pt(12.)).text_color(t.text_3).child(format!("{shown} of {total} shown")))
            })
    }

    fn table_head(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let sort = |key: SortKey, label: &'static str, right: bool, cx: &mut Context<Self>| {
            let on = self.m.sort == key;
            let arrow = if self.m.ascending { "chevron-up" } else { "chevron-down" };
            div()
                .id(label)
                .flex()
                .items_center()
                .gap(pt(3.))
                .cursor_pointer()
                .text_color(if on { t.text } else { t.text_3 })
                .hover(|s| s.text_color(t.text))
                .when(on && right, |el| el.child(icon(arrow, 10., t.text)))
                .child(label)
                .when(on && !right, |el| el.child(icon(arrow, 10., t.text)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.m.set_sort(key);
                    cx.notify();
                }))
        };
        let kind_head = self.show_kind.then(|| div().child(sort(SortKey::Kind, "KIND", false, cx)).into_any_element());
        columns(
            div().h(pt(34.)).bg(t.surface_2).border_b_1().border_color(t.border).text_size(pt(11.)).font_weight(FontWeight::SEMIBOLD),
            div(),
            div().pl(INDENT + pt(24.)).child(sort(SortKey::Name, "NAME", false, cx)),
            kind_head,
            div().child(sort(SortKey::Modified, "MODIFIED", false, cx)),
            div().flex().justify_end().child(sort(SortKey::Size, "SIZE", true, cx)),
            div(),
        )
    }

    fn row(&self, ix: usize, t: &Theme, cx: &mut Context<Self>) -> AnyElement {
        match &self.rows[ix] {
            Row::Note { depth, text, kind } => {
                let lead = match kind {
                    NoteKind::Loading => spinner(("note", ix), 12., t).into_any_element(),
                    NoteKind::Error => icon("alert", 14., t.warning).into_any_element(),
                    NoteKind::Info => div().into_any_element(),
                };
                columns(
                    div().id(("note", ix)).h(ROW_H).border_b_1().border_color(t.border).text_size(pt(12.)).text_color(t.text_3),
                    div(),
                    div()
                        .pl(INDENT * (*depth as f32) + pt(24.))
                        .flex()
                        .items_center()
                        .gap(pt(8.))
                        .child(lead)
                        .child(div().ellipsis().child(text.clone())),
                    None,
                    div(),
                    div(),
                    div(),
                )
                .into_any_element()
            }
            Row::Item { path, depth } => {
                let node = &self.m.nodes[path];
                let state = self.m.check_state(path);
                let open = self.m.expanded.contains(path);
                let (is_dir, depth) = (node.is_dir, *depth);
                let p = path.clone();

                let disclosure = if is_dir {
                    let p = path.clone();
                    div()
                        .id(("disc", ix))
                        .size(pt(18.))
                        .flex_none()
                        .rounded(pt(5.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .child(icon(if open { "chevron-down" } else { "chevron" }, 12., t.text_3))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_expand(p.clone(), cx);
                        }))
                        .into_any_element()
                } else {
                    div().size(pt(18.)).flex_none().into_any_element()
                };

                let name = div()
                    .pl(INDENT * depth as f32)
                    .flex()
                    .items_center()
                    .gap(pt(6.))
                    .min_w_0()
                    .child(disclosure)
                    .child(
                        div()
                            .mr(pt(4.))
                            .child(icon(if is_dir { "folder" } else { "file" }, 18., if is_dir { t.folder } else { t.text_3 })),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            // GPUI only truncates text with "…" correctly as a direct child of
                            // this column. A tagged item shows its tag instead of the location
                            // (the same folder for the whole category), so nothing overlaps.
                            .child(
                                div()
                                    .ellipsis()
                                    .when(depth == 0, |el| el.font_weight(FontWeight::MEDIUM))
                                    .child(node.name.clone()),
                            )
                            .when_some(node.note.clone(), |el, note| el.child(div().flex().child(pill(note, t))))
                            .when_some(node.location.clone().filter(|_| node.note.is_none()), |el, loc| {
                                el.child(div().mt(pt(1.)).ellipsis().text_size(pt(11.5)).text_color(t.text_3).child(loc))
                            }),
                    );

                let reveal_path = path.clone();
                let reveal = div()
                    .id(("reveal", ix))
                    .size(pt(28.))
                    .rounded(pt(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .opacity(0.)
                    .group_hover("row", |s| s.opacity(1.))
                    .hover(|s| s.bg(t.accent_soft))
                    .child(icon("reveal", 15., t.text_3))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.reveal(&reveal_path, cx);
                    }));

                let on = state == Check::On;
                let at_cursor = self.m.cursor.as_ref() == Some(path);
                let menu_path = path.clone();
                let kind_cell = self.show_kind.then(|| {
                    div().ellipsis().text_size(pt(12.)).text_color(t.text_2).child(kind(&node.name, is_dir)).into_any_element()
                });
                columns(
                    div()
                        .id(("row", ix))
                        .group("row")
                        .relative()
                        .h(ROW_H)
                        .border_b_1()
                        .border_color(t.border)
                        .cursor_pointer()
                        .when(on, |el| el.bg(t.selected_row))
                        .when(at_cursor && !on, |el| el.bg(t.hover))
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                            // A double-click toggles twice (no net change) and opens the folder.
                            this.m.toggle(&p);
                            this.m.cursor = Some(p.clone());
                            this.search_on = false;
                            if event.click_count() == 2 && is_dir {
                                this.toggle_expand(p.clone(), cx);
                            }
                            cx.notify();
                        }))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                this.row_menu(&menu_path, event.position, cx);
                            }),
                        ),
                    checkbox(state, t),
                    name,
                    kind_cell,
                    div().text_size(pt(12.)).text_color(t.text_2).whitespace_nowrap().child(ago(node.modified)),
                    div().flex().justify_end().font_weight(FontWeight::MEDIUM).whitespace_nowrap().child(bytes(node.size)),
                    reveal,
                )
                // The keyboard's row is marked with a bar on its left edge.
                .when(at_cursor, |el| el.child(div().absolute().left_0().top_0().bottom_0().w(pt(3.)).bg(t.accent)))
                .into_any_element()
            }
        }
    }

    fn action_bar(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let (count, size) = self.m.selection();
        div()
            .h(BAR_H)
            .flex_none()
            .flex()
            .items_center()
            .gap(pt(8.))
            .px(pt(24.))
            .border_t_1()
            .border_color(t.border)
            .bg(t.bg)
            .child(
                div().flex_1().min_w_0().flex().items_center().gap(pt(4.)).text_color(t.text_2).when_else(
                    count > 0,
                    |el| {
                        el.child(strong(plural(count, "item", "items"), t))
                            .child("selected ·")
                            .child(strong(bytes(size), t))
                    },
                    |el| el.child("Select items to clean"),
                ),
            )
            .child(
                button("rescan", "Rescan", Some("refresh"), Btn::Secondary, true, t)
                    .on_click(cx.listener(|this, _, _, cx| this.start_scan(cx))),
            )
            .child(
                button("review", if count > 0 { format!("Clean {}", bytes(size)) } else { "Clean".to_string() }, Some("trash"), Btn::Primary, count > 0, t).when(count > 0, |b| {
                    b.on_click(cx.listener(|this, _, window, cx| this.open_confirm(None, window, cx)))
                }),
            )
    }

    fn done(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let Some(r) = &self.m.report else { return div() };
        let ok = r.removed > 0;
        // After cleaning a single category the other results are still there.
        let has_results = !self.m.cats.is_empty();
        let last = r.failures.len().saturating_sub(1);
        let needs_access = r.failures.iter().any(|f| f.reason.contains("Full Disk Access"));
        let mut facts = vec![];
        if let Some((total, free)) = self.disk {
            facts.push(format!("{} available of {}", bytes(free), bytes(total)));
        }
        if self.settings.cleans > 1 {
            facts.push(format!("{} cleaned so far", bytes(self.settings.freed_total)));
        }
        let failures = r.failures.iter().enumerate().map(|(i, f)| {
            div()
                .px(pt(16.))
                .py(pt(8.))
                .when(i < last, |el| el.border_b_1().border_color(t.border))
                .child(div().ellipsis().text_size(pt(12.)).child(f.path.clone()))
                .child(div().text_size(pt(11.5)).text_color(t.text_3).child(f.reason.clone()))
        });

        div()
            .w_full()
            .min_h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .pt(TITLEBAR)
            .px(pt(48.))
            .pb(pt(48.))
            .child(
                div()
                    .size(pt(88.))
                    .mb(pt(24.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(if ok { t.success_soft } else { t.warning_soft })
                    .child(icon(if ok { "check" } else { "alert" }, 44., if ok { t.success } else { t.warning })),
            )
            .child(
                div().font_weight(FontWeight::MEDIUM).text_color(t.text_2).child(if ok { "Space recovered" } else { "Nothing was removed" }),
            )
            .child(div().mt(pt(2.)).mb(pt(6.)).text_size(pt(44.)).font_weight(FontWeight::BOLD).child(bytes(r.freed)))
            .child(
                div().max_w(pt(420.)).text_center().text_size(pt(14.)).line_height(pt(21.)).text_color(t.text_2).child(if ok {
                    format!(
                        "{} moved to the Trash. Empty the Trash to free the space for good, or put items back if you need them.",
                        plural(r.removed, "item", "items")
                    )
                } else {
                    "None of the selected items could be moved to the Trash.".into()
                }),
            )
            .when(!facts.is_empty(), |el| {
                el.child(div().mt(pt(10.)).text_size(pt(12.5)).text_color(t.text_3).child(facts.join("   ·   ")))
            })
            .child(
                div()
                    .mt(pt(28.))
                    .flex()
                    .gap(pt(8.))
                    .child(button("open-trash", "Open Trash", Some("trash"), Btn::Secondary, true, t).on_click(
                        cx.listener(|this, _, _, cx| {
                            if Command::new("/usr/bin/open").arg(scanner::home_dir().join(".Trash")).spawn().is_err() {
                                this.show_toast("Couldn't open the Trash.", cx);
                            }
                        }),
                    ))
                    .child(
                        button("scan-again", "Scan Again", Some("refresh"), if has_results { Btn::Secondary } else { Btn::Primary }, true, t)
                            .on_click(cx.listener(|this, _, _, cx| this.start_scan(cx))),
                    )
                    .when(has_results, |el| {
                        el.child(button("back-to-results", "Back to Results", None, Btn::Primary, true, t).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.m.report = None;
                                this.m.screen = Screen::Results;
                                cx.notify();
                            },
                        )))
                    }),
            )
            .when(!r.failures.is_empty(), |el| {
                el.child(
                    div()
                        .id("failures")
                        .w_full()
                        .max_w(pt(560.))
                        .max_h(pt(200.))
                        .mt(pt(28.))
                        .overflow_y_scroll()
                        .rounded(pt(14.))
                        .bg(t.surface)
                        .shadow(card_shadow(t))
                        .child(
                            div()
                                .min_h(pt(40.))
                                .pl(pt(16.))
                                .pr(pt(6.))
                                .flex()
                                .items_center()
                                .border_b_1()
                                .border_color(t.border)
                                .text_size(pt(12.))
                                .child(
                                    div()
                                        .flex_1()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text_2)
                                        .child(format!("{} couldn't be moved", plural(r.failures.len(), "item", "items"))),
                                )
                                .when(needs_access, |el| {
                                    el.child(
                                        button("open-privacy", "Open Full Disk Access Settings", None, Btn::Ghost, true, t).on_click(
                                            cx.listener(|this, _, _, cx| {
                                                let pane = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles";
                                                if Command::new("/usr/bin/open").arg(pane).spawn().is_err() {
                                                    this.show_toast("Couldn't open System Settings.", cx);
                                                }
                                            }),
                                        ),
                                    )
                                }),
                        )
                        .children(failures),
                )
            })
    }

    /// Light / Dark / Auto segmented control in the top-right corner.
    fn theme_switch(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let segment = |mode: ThemeMode, icon_name: &'static str, id: &'static str, cx: &mut Context<Self>| {
            let active = self.theme_mode == mode;
            div()
                .id(id)
                .w(pt(32.))
                .h(pt(24.))
                .rounded(pt(6.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(active, |el| el.bg(t.surface).shadow(small_shadow(t)))
                .when(!active, |el| el.hover(|s| s.bg(t.hover)))
                .child(icon(icon_name, 14., if active { t.text } else { t.text_3 }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_theme(mode, cx);
                }))
        };
        div()
            .flex()
            .gap(pt(2.))
            .p(pt(2.))
            .rounded(pt(8.))
            .bg(t.border)
            .child(segment(ThemeMode::Light, "sun", "theme-light", cx))
            .child(segment(ThemeMode::Dark, "moon", "theme-dark", cx))
            .child(segment(ThemeMode::System, "auto", "theme-auto", cx))
    }

    /// − 100% + control next to the theme switch. Clicking the percentage goes back to 100%.
    fn zoom_switch(&self, t: &Theme, cx: &mut Context<Self>) -> Div {
        let (smallest, largest) = (ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        let step = |id: &'static str, icon_name: &'static str, delta: i32, enabled: bool, cx: &mut Context<Self>| {
            div()
                .id(id)
                .w(pt(26.))
                .h(pt(24.))
                .rounded(pt(6.))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(icon_name, 13., t.text_2))
                .when(!enabled, |el| el.opacity(0.35))
                .when(enabled, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(move |this, _, window, cx| this.step_zoom(delta, window, cx)))
                })
        };
        div()
            .flex()
            .items_center()
            .gap(pt(2.))
            .p(pt(2.))
            .rounded(pt(8.))
            .bg(t.border)
            .child(step("zoom-out", "minus", -1, self.zoom > smallest + 0.001, cx))
            .child(
                div()
                    .id("zoom-reset")
                    .w(pt(44.))
                    .h(pt(24.))
                    .rounded(pt(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .text_size(pt(11.5))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_2)
                    .child(format!("{}%", (self.zoom * 100.).round()))
                    .on_click(cx.listener(|this, _, window, cx| this.set_zoom(1., window, cx))),
            )
            .child(step("zoom-in", "plus", 1, self.zoom < largest - 0.001, cx))
    }

    /* ================================================================
       Modals
       ================================================================ */

    fn confirm(&self, t: &Theme, max_h: gpui::Pixels, cx: &mut Context<Self>) -> Stateful<Div> {
        let (count, size) = self.m.scoped_selection();
        let scope = self.m.scope;
        let only = scope.and_then(|ci| self.m.cats.get(ci)).map(|c| c.info.title);
        let groups = self.m.cats.iter().enumerate().filter_map(|(ci, c)| {
            let items = self.m.selected_in(ci);
            if items.is_empty() || scope.is_some_and(|s| s != ci) {
                return None;
            }
            let total: u64 = items.iter().map(|n| n.size).sum();
            let rows = items.into_iter().map(|n| {
                div()
                    .min_h(pt(40.))
                    .px(pt(14.))
                    .py(pt(6.))
                    .flex()
                    .items_center()
                    .gap(pt(10.))
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().w(pt(22.)).flex().justify_center().child(div().size(pt(5.)).rounded_full().bg(t.text_3)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().ellipsis().text_size(pt(12.5)).font_weight(FontWeight::MEDIUM).child(n.name.clone()))
                            .child(div().ellipsis().text_size(pt(11.)).text_color(t.text_3).child(n.path.clone())),
                    )
                    .child(div().text_size(pt(12.)).text_color(t.text_2).child(bytes(n.size)))
            });
            Some(
                div()
                    .child(
                        div()
                            .h(pt(36.))
                            .px(pt(14.))
                            .flex()
                            .items_center()
                            .gap(pt(10.))
                            .bg(t.surface_2)
                            .border_b_1()
                            .border_color(t.border)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(tile(c.info.id, c.info.icon, 22.))
                            .child(div().flex_1().child(c.info.title))
                            .child(
                                div()
                                    .text_size(pt(12.))
                                    .text_color(t.text_2)
                                    .child(format!("{} · {}", plural(self.m.selected_in(ci).len(), "item", "items"), bytes(total))),
                            ),
                    )
                    .children(rows),
            )
        });

        modal_frame(t, pt(620.), max_h)
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(pt(16.))
                    .p(pt(24.))
                    .pb(pt(16.))
                    .child(
                        div()
                            .size(pt(44.))
                            .flex_none()
                            .rounded(pt(12.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .bg(t.danger_soft)
                            .child(icon("trash", 24., t.danger)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(pt(17.))
                                    .font_weight(FontWeight::BOLD)
                                    .child(format!("Move {} to the Trash?", plural(count, "item", "items"))),
                            )
                            .child(div().mt(pt(4.)).text_color(t.text_2).line_height(pt(19.)).child(format!(
                                "This frees about {}. {}Items stay in the Trash until you empty it, so you can put them back.",
                                bytes(size),
                                match only {
                                    Some(title) => format!("Only {title} is cleaned; what you selected elsewhere stays as it is. "),
                                    None => "Review the list below. ".to_string(),
                                }
                            ))),
                    ),
            )
            .child(
                div()
                    .id("confirm-list")
                    .flex_1()
                    .min_h_0()
                    .mx(pt(24.))
                    .overflow_y_scroll()
                    .rounded(pt(10.))
                    .border_1()
                    .border_color(t.border)
                    .bg(t.surface_2)
                    .children(groups),
            )
            .when(self.m.selection_needs_review(), |el| {
                el.child(
                    div()
                        .mx(pt(24.))
                        .mt(pt(16.))
                        .px(pt(12.))
                        .py(pt(10.))
                        .rounded(pt(10.))
                        .bg(t.warning_soft)
                        .flex()
                        .items_start()
                        .gap(pt(10.))
                        .child(div().mt(pt(1.)).child(icon("alert", 16., t.warning)))
                        .child(div().flex_1().min_w_0().line_height(pt(19.)).child(
                            "Your selection includes items marked Review or system caches. Make sure you no longer need them.",
                        )),
                )
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(pt(8.))
                    .p(pt(24.))
                    .pt(pt(20.))
                    .child(div().flex_1().text_size(pt(12.)).text_color(t.text_3).child("Press Esc to cancel"))
                    .child(button("cancel", "Cancel", None, Btn::Secondary, true, t).on_click(cx.listener(|this, _, _, cx| {
                        this.m.modal = Modal::None;
                        cx.notify();
                    })))
                    .child(
                        button("confirm", "Move to Trash", Some("trash"), Btn::Danger, true, t)
                            .on_click(cx.listener(|this, _, _, cx| this.run_clean(cx))),
                    ),
            )
    }

    fn about(&self, t: &Theme, cx: &mut Context<Self>) -> Stateful<Div> {
        modal_frame(t, pt(360.), pt(440.)).child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .px(pt(28.))
                .pt(pt(24.))
                .pb(pt(24.))
                .child(gpui::img(self.app_icon.clone()).size(pt(96.)))
                .child(div().mt(pt(4.)).text_size(pt(20.)).font_weight(FontWeight::BOLD).child("Mac Cleaner"))
                .child(div().mt(pt(2.)).text_color(t.text_3).child(format!("Version {}", env!("CARGO_PKG_VERSION"))))
                .child(
                    div()
                        .mt(pt(14.))
                        .mb(pt(20.))
                        .text_center()
                        .line_height(pt(19.))
                        .text_color(t.text_2)
                        .child("A careful cleaner for macOS. Everything it removes goes to the Trash, so you can put it back."),
                )
                .child(button("about-close", "Close", None, Btn::Secondary, true, t).on_click(cx.listener(|this, _, _, cx| {
                    this.m.modal = Modal::None;
                    cx.notify();
                }))),
        )
    }

    /// The open popup menu on a click-catching layer that closes it.
    fn popup_layer(&self, t: &Theme, viewport: Size<Pixels>, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let popup = self.popup.as_ref()?;
        let any_checked = popup.items.iter().any(|i| i.checked);
        // Placed at a mouse position, so these are real pixels at the current zoom.
        let width = gpui::px(if any_checked { 216. } else { 180. } * self.zoom);
        let height = gpui::px((10. + 28. * popup.items.len() as f32) * self.zoom);
        let margin = gpui::px(8. * self.zoom);
        let x = popup.pos.x.min(viewport.width - width - margin).max(margin);
        let y = popup.pos.y.min(viewport.height - height - margin).max(margin);

        let items = popup.items.iter().enumerate().map(|(i, item)| {
            let cmd = item.cmd.clone();
            div()
                .id(("popup-item", i))
                .h(pt(28.))
                .px(pt(8.))
                .rounded(pt(6.))
                .flex()
                .items_center()
                .gap(pt(6.))
                .cursor_pointer()
                .hover(|s| s.bg(t.accent_soft))
                .when(any_checked, |el| {
                    el.child(div().w(pt(14.)).flex_none().when(item.checked, |d| d.child(icon("check-bold", 11., t.accent))))
                })
                .child(div().flex_1().min_w_0().ellipsis().child(item.label.clone()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.popup = None;
                    cx.notify();
                    this.run_cmd(cmd.clone(), cx);
                }))
        });

        Some(
            div()
                .id("popup-layer")
                .absolute()
                .inset_0()
                .occlude()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.popup = None;
                    cx.notify();
                }))
                .on_mouse_down(
                    MouseButton::Right,
                    cx.listener(|this, _, _, cx| {
                        this.popup = None;
                        cx.notify();
                    }),
                )
                .child(
                    div()
                        .id("popup")
                        .absolute()
                        .left(x)
                        .top(y)
                        .w(width)
                        .p(pt(4.))
                        .rounded(pt(10.))
                        .bg(t.surface)
                        .border_1()
                        .border_color(t.border_strong)
                        .shadow(vec![shadow(rgba_hsla(0x00000030), 8., 24., t.scale)])
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .children(items),
                ),
        )
    }

    fn working(&self, t: &Theme) -> Stateful<Div> {
        modal_frame(t, pt(380.), pt(400.)).child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(pt(16.))
                .px(pt(24.))
                .py(pt(48.))
                .child(spinner("working", 28., t))
                .child(div().text_size(pt(17.)).font_weight(FontWeight::BOLD).child("Moving items to the Trash…"))
                .child(div().text_color(t.text_2).child("This usually takes a few seconds.")),
        )
    }
}

impl Render for CleanerApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut t = match self.theme_mode {
            ThemeMode::Light => Theme::light(),
            ThemeMode::Dark => Theme::dark(),
            ThemeMode::System => Theme::for_appearance(window.appearance()),
        };
        t.scale = self.zoom;
        // Keyboard shortcuts and menu items reach this view through its focus.
        if !self.focus.is_focused(window) {
            window.focus(&self.focus);
        }
        let viewport = window.viewport_size();
        self.show_kind = viewport.width >= gpui::px(1040. * self.zoom);
        let screen: Div = match self.m.screen {
            Screen::Welcome => scrolling(self.welcome(&t, cx)),
            Screen::Scanning => scrolling(self.scanning(&t, cx)),
            Screen::Results => self.results(&t, cx),
            Screen::Done => scrolling(self.done(&t, cx)),
        };
        let max_h = (viewport.height - gpui::px(64. * self.zoom)).min(gpui::px(640. * self.zoom));
        let modal = match self.m.modal {
            Modal::None => None,
            Modal::Confirm => Some(self.confirm(&t, max_h, cx)),
            Modal::Working => Some(self.working(&t)),
            Modal::About => Some(self.about(&t, cx)),
        };
        let popup = self.popup_layer(&t, viewport, cx);

        div()
            .key_context("MacCleaner")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| this.key_down(event, cx)))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.on_cancel(cx)))
            .on_action(cx.listener(|this, _: &Confirm, window, cx| this.on_confirm(window, cx)))
            .on_action(cx.listener(|this, _: &Rescan, _, cx| {
                if this.m.modal == Modal::None && this.m.screen != Screen::Scanning {
                    this.start_scan(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Find, _, cx| {
                if this.browsing() {
                    this.search_on = true;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if this.browsing() {
                    this.m.set_visible(true);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &DeselectAll, _, cx| {
                if this.browsing() {
                    this.m.set_visible(false);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &QuickLook, _, cx| {
                if let Some(path) = this.m.cursor.clone().filter(|_| this.browsing()) {
                    this.quick_look(&path, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &About, _, cx| {
                if this.m.modal == Modal::None {
                    this.popup = None;
                    this.m.modal = Modal::About;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .on_action(cx.listener(|_, _: &Minimize, window, _| window.minimize_window()))
            .on_action(cx.listener(|this, _: &ThemeLight, _, cx| this.set_theme(ThemeMode::Light, cx)))
            .on_action(cx.listener(|this, _: &ThemeDark, _, cx| this.set_theme(ThemeMode::Dark, cx)))
            .on_action(cx.listener(|this, _: &ThemeAuto, _, cx| this.set_theme(ThemeMode::System, cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, window, cx| this.step_zoom(1, window, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, window, cx| this.step_zoom(-1, window, cx)))
            .on_action(cx.listener(|this, _: &ZoomReset, window, cx| this.set_zoom(1., window, cx)))
            .relative()
            .size_full()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(".SystemUIFont")
            .text_size(pt(13.))
            .line_height(relative(1.35))
            .child(screen)
            .child(
                div()
                    .absolute()
                    .top(pt(12.))
                    .right(pt(16.))
                    .flex()
                    .items_center()
                    .gap(pt(8.))
                    .child(self.zoom_switch(&t, cx))
                    .child(self.theme_switch(&t, cx)),
            )
            .when_some(modal, |el, card| {
                el.child(
                    div()
                        .id("backdrop")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .bg(t.backdrop)
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(pt(32.))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if matches!(this.m.modal, Modal::Confirm | Modal::About) {
                                this.m.modal = Modal::None;
                                cx.notify();
                            }
                        }))
                        .child(card),
                )
            })
            .when_some(popup, |el, layer| el.child(layer))
            .when_some(self.toast.clone(), |el, msg| {
                el.child(
                    div().absolute().bottom(pt(84.)).left_0().right_0().flex().justify_center().child(
                        div()
                            .px(pt(14.))
                            .py(pt(8.))
                            .rounded(pt(10.))
                            .bg(rgba_hsla(0x1e1e20eb))
                            .text_color(white())
                            .text_size(pt(12.5))
                            .child(msg),
                    ),
                )
            })
    }
}

/* ================================================================
   Building blocks
   ================================================================ */

/// Single-line text that ends in "…" when it doesn't fit.
/// (`truncate()` measures before flex sizing and never truncates inside flex rows;
/// a one-line clamp is re-measured at the final width.)
trait Ellipsis: Styled + Sized {
    fn ellipsis(self) -> Self {
        self.overflow_hidden().line_clamp(1).text_ellipsis()
    }
}
impl<T: Styled> Ellipsis for T {}

/// A centred screen that scrolls when the window is too short for it
/// (a small display at a high zoom). The content is at least as tall as the window.
fn scrolling(content: Div) -> Div {
    div().size_full().child(div().id("screen").size_full().overflow_y_scroll().child(content))
}

/// One table line: the same fixed columns for the header and every row.
/// `kind` is `None` when the window is too narrow for that column.
fn columns<E: ParentElement + Styled>(
    base: E,
    check: impl IntoElement,
    name: impl IntoElement,
    kind: Option<AnyElement>,
    date: impl IntoElement,
    size: impl IntoElement,
    action: impl IntoElement,
) -> E {
    base.w_full()
        .flex()
        .items_center()
        .px(ROW_PAD_X)
        .gap(COL_GAP)
        .child(div().w(COL_CHECK).flex_none().flex().items_center().child(check))
        .child(div().flex_1().min_w_0().child(name))
        .children(kind.map(|kind| div().w(COL_KIND).flex_none().min_w_0().child(kind)))
        .child(div().w(COL_DATE).flex_none().child(date))
        .child(div().w(COL_SIZE).flex_none().child(size))
        .child(div().w(COL_ACTION).flex_none().child(action))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// 1234567 → "1,234,567".
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A button that opens a popup menu. Highlighted while its filter is narrowing the list.
fn dropdown(id: &'static str, label: String, active: bool, t: &Theme) -> Stateful<Div> {
    let (hover, fg) = (t.surface_2, if active { t.accent } else { t.text });
    div()
        .id(id)
        .flex_none()
        .h(pt(30.))
        .px(pt(10.))
        .rounded(pt(8.))
        .bg(t.surface)
        .border_1()
        .border_color(if active { t.accent } else { t.border_strong })
        .flex()
        .items_center()
        .gap(pt(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .whitespace_nowrap()
        .text_size(pt(12.5))
        .text_color(fg)
        .child(label)
        .child(icon("chevron-down", 10., t.text_3))
}

fn icon(name: &str, size: f32, color: impl Into<Hsla>) -> gpui::Svg {
    svg().path(format!("icons/{name}.svg")).size(pt(size)).flex_none().text_color(color.into())
}

fn spinner(id: impl Into<ElementId>, size: f32, t: &Theme) -> Div {
    div()
        .relative()
        .size(pt(size))
        .flex_none()
        .child(div().absolute().inset_0().rounded_full().border(pt(2.)).border_color(t.border_strong))
        .child(
            svg()
                .absolute()
                .inset_0()
                .size(pt(size))
                .path("icons/spinner.svg")
                .text_color(t.accent)
                .with_animation(id, Animation::new(Duration::from_millis(800)).repeat(), |s, delta| {
                    s.with_transformation(Transformation::rotate(percentage(delta)))
                }),
        )
}

fn gradient(top: u32, bottom: u32) -> gpui::Background {
    linear_gradient(180., linear_color_stop(rgb(top), 0.), linear_color_stop(rgb(bottom), 1.))
}

fn gradient_h(left: u32, right: Hsla) -> gpui::Background {
    linear_gradient(90., linear_color_stop(rgb(left), 0.), linear_color_stop(right, 1.))
}

fn tile(cat_id: &str, icon_name: &str, size: f32) -> Div {
    let (top, bottom) = if cat_id == "muted" { (0xb5b5bb, 0x8e8e93) } else { tile_colors(cat_id) };
    div()
        .size(pt(size))
        .flex_none()
        .rounded(pt(size * 0.27))
        .flex()
        .items_center()
        .justify_center()
        .bg(gradient(top, bottom))
        .shadow(vec![shadow(rgba_hsla(0x00000014), 1., 2., 1.)])
        .child(icon(icon_name, size * 0.57, white()))
}

fn tile_lg(cat_id: &str, icon_name: &str) -> Div {
    tile(cat_id, icon_name, 44.).rounded(pt(11.))
}

fn checkbox(state: Check, t: &Theme) -> Div {
    let base = div().size(pt(18.)).flex_none().rounded(pt(5.)).flex().items_center().justify_center();
    match state {
        Check::Off => base.bg(t.surface).border_1().border_color(t.border_strong),
        Check::On => base.bg(t.accent).child(icon("check-bold", 11., white())),
        Check::Partial => base.bg(t.accent).child(div().w(pt(9.)).h(pt(2.)).rounded(pt(1.)).bg(white())),
    }
}

fn badge(label: &'static str, fg: Hsla, bg: Hsla) -> Div {
    div()
        .flex_none()
        .h(pt(20.))
        .px(pt(8.))
        .rounded(pt(10.))
        .flex()
        .items_center()
        .bg(bg)
        .text_color(fg)
        .text_size(pt(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

fn pill(text: String, t: &Theme) -> Div {
    div()
        .flex_none()
        .h(pt(18.))
        .px(pt(6.))
        .rounded(pt(5.))
        .flex()
        .items_center()
        .bg(t.warning_soft)
        .text_color(t.warning)
        .text_size(pt(10.5))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text)
}

fn strong(text: String, t: &Theme) -> Div {
    div().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(text)
}

#[derive(Clone, Copy, PartialEq)]
enum Btn {
    Primary,
    Danger,
    Secondary,
    Ghost,
    Large,
}

fn button(id: &'static str, label: impl Into<SharedString>, icon_name: Option<&str>, kind: Btn, enabled: bool, t: &Theme) -> Stateful<Div> {
    let (bg, hover_bg, fg) = match kind {
        Btn::Primary | Btn::Large => (t.accent, t.accent_hover, white()),
        Btn::Danger => (t.danger, t.danger_hover, white()),
        Btn::Secondary => (t.surface, t.surface_2, t.text),
        Btn::Ghost => (gpui::transparent_black(), t.accent_soft, t.accent),
    };
    let el = div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(pt(6.))
        .whitespace_nowrap()
        .bg(bg)
        .text_color(fg)
        .font_weight(FontWeight::MEDIUM)
        .when(kind == Btn::Large, |b| {
            b.h(pt(44.)).px(pt(28.)).rounded(pt(12.)).text_size(pt(15.)).font_weight(FontWeight::SEMIBOLD)
        })
        .when(kind == Btn::Ghost, |b| b.h(pt(28.)).px(pt(10.)).rounded(pt(8.)))
        .when(kind != Btn::Large && kind != Btn::Ghost, |b| b.h(pt(32.)).px(pt(14.)).rounded(pt(8.)))
        .when(kind == Btn::Secondary, |b| b.border_1().border_color(t.border_strong))
        .when(kind != Btn::Ghost, |b| b.shadow(small_shadow(t)))
        .when_some(icon_name, |b, name| b.child(icon(name, 15., fg)))
        .child(label.into());
    if enabled {
        el.cursor_pointer().hover(move |s| s.bg(hover_bg))
    } else {
        el.opacity(0.4)
    }
}

/// The dialog card. `render` places it on a dimmed backdrop.
fn modal_frame(t: &Theme, width: gpui::Rems, max_h: impl Clone + Into<gpui::Length>) -> Stateful<Div> {
    div()
        .id("modal-card")
        .w(width)
        .max_h(max_h)
        .flex()
        .flex_col()
        .rounded(pt(16.))
        .bg(t.surface)
        .shadow(vec![shadow(rgba_hsla(0x00000038), 24., 64., t.scale)])
        .border_1()
        .border_color(t.border)
        .overflow_hidden()
        // Clicks inside the card must not reach the backdrop (which cancels).
        .on_click(|_, _, cx| cx.stop_propagation())
}

/// Shadows are the one thing GPUI wants in real pixels, so they are scaled by hand.
fn shadow(color: Hsla, y: f32, blur: f32, scale: f32) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(gpui::px(0.), gpui::px(y * scale)),
        blur_radius: gpui::px(blur * scale),
        spread_radius: gpui::px(0.),
    }
}

fn card_shadow(t: &Theme) -> Vec<BoxShadow> {
    vec![shadow(t.shadow.opacity(0.5), 1., 2., t.scale), shadow(t.shadow.opacity(0.6), 4., 14., t.scale)]
}

fn small_shadow(t: &Theme) -> Vec<BoxShadow> {
    vec![shadow(t.shadow.opacity(0.6), 1., 2., t.scale)]
}

/// The window's rem size at 100% zoom; `pt()` in theme.rs divides by the same number.
const BASE_REM: f32 = 16.;

/// A window size given in points at 100% zoom, scaled and kept inside the screen.
pub fn window_size(points: (f32, f32), zoom: f32) -> Size<Pixels> {
    let (max_w, max_h) = crate::native::screen_visible_size().unwrap_or((f32::MAX, f32::MAX));
    gpui::size(gpui::px((points.0 * zoom).min(max_w)), gpui::px((points.1 * zoom).min(max_h)))
}

fn rgba_hsla(hex: u32) -> Hsla {
    gpui::rgba(hex).into()
}

