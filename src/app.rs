//! App state and selection logic, independent of the UI.

use crate::scanner::{Category, CleanReport, Listing, Risk, SourceInfo};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub struct Node {
    pub path: String,
    pub name: String,
    /// Shown under top-level items only.
    pub location: Option<String>,
    pub size: u64,
    pub modified: Option<u64>,
    pub is_dir: bool,
    pub parent: Option<String>,
    pub cat: usize,
    pub note: Option<String>,
}

pub enum Children {
    Loading,
    Failed(String),
    Loaded { entries: Vec<String>, hidden: usize },
}

pub struct Cat {
    pub info: SourceInfo,
    pub items: Vec<String>,
    pub total: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Scanning,
    Results,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Modal {
    None,
    Confirm,
    Working,
    About,
    Update,
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Name,
    Kind,
    Modified,
    Size,
}

/// "Not modified for at least this long" filter on the top-level list.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Age {
    Any,
    Month,
    HalfYear,
    Year,
}

impl Age {
    pub const ALL: [Age; 4] = [Age::Any, Age::Month, Age::HalfYear, Age::Year];

    pub fn days(self) -> u64 {
        match self {
            Age::Any => 0,
            Age::Month => 30,
            Age::HalfYear => 182,
            Age::Year => 365,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Age::Any => "Any age",
            Age::Month => "Older than 30 days",
            Age::HalfYear => "Older than 6 months",
            Age::Year => "Older than 1 year",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Off,
    On,
    Partial,
}

pub enum NoteKind {
    Loading,
    Error,
    Info,
}

/// One visible line of the folder tree.
pub enum Row {
    Item { path: String, depth: usize },
    Note { depth: usize, text: String, kind: NoteKind },
}

pub struct Model {
    pub screen: Screen,
    pub modal: Modal,
    pub sources: Vec<SourceInfo>,
    pub cats: Vec<Cat>,
    pub nodes: HashMap<String, Node>,
    pub children: HashMap<String, Children>,
    pub expanded: HashSet<String>,
    /// Selection roots: a selected folder covers everything inside it.
    /// Never holds a path together with one of its ancestors.
    pub selected: HashSet<String>,
    pub active: usize,
    pub sort: SortKey,
    pub ascending: bool,
    pub report: Option<CleanReport>,
    /// Search text; filters the top-level items of the open category.
    pub query: String,
    pub age: Age,
    /// The row the keyboard acts on.
    pub cursor: Option<String>,
    /// What the clean in progress covers: one category, or everything selected.
    pub scope: Option<usize>,
}

impl Model {
    pub fn new(sources: Vec<SourceInfo>) -> Self {
        Self {
            screen: Screen::Welcome,
            modal: Modal::None,
            sources,
            cats: vec![],
            nodes: HashMap::new(),
            children: HashMap::new(),
            expanded: HashSet::new(),
            scope: None,
            selected: HashSet::new(),
            active: 0,
            sort: SortKey::Size,
            ascending: false,
            report: None,
            query: String::new(),
            age: Age::Any,
            cursor: None,
        }
    }

    /// Forgets the scan results (they are stale once something was moved to the Trash).
    pub fn clear_results(&mut self) {
        self.nodes.clear();
        self.children.clear();
        self.expanded.clear();
        self.selected.clear();
        self.cats.clear();
        self.cursor = None;
    }

    /// Takes in a finished scan. A path found by more than one category is shown only in the
    /// first of them, and nothing is listed inside an item an earlier category already offers.
    pub fn load_scan(&mut self, categories: Vec<Category>) {
        self.clear_results();
        for (ci, c) in categories.into_iter().enumerate() {
            let mut paths = vec![];
            let mut total = 0;
            for it in c.items {
                let listed = std::path::Path::new(&it.path)
                    .ancestors()
                    .any(|p| p.to_str().is_some_and(|p| self.nodes.get(p).is_some_and(|n| n.cat != ci)));
                if listed {
                    continue;
                }
                total += it.size;
                if it.selected {
                    self.selected.insert(it.path.clone());
                }
                paths.push(it.path.clone());
                self.nodes.insert(
                    it.path.clone(),
                    Node {
                        path: it.path,
                        name: it.name,
                        location: Some(it.location),
                        size: it.size,
                        modified: it.modified,
                        is_dir: it.is_dir,
                        parent: None,
                        cat: ci,
                        note: it.note,
                    },
                );
            }
            self.cats.push(Cat { info: c.info, items: paths, total });
        }
        self.active = self.cats.iter().position(|c| !c.items.is_empty()).unwrap_or(0);
        self.screen = Screen::Results;
    }

    pub fn load_children(&mut self, folder: &str, result: Result<Listing, String>) {
        let state = match result {
            Err(e) => Children::Failed(e),
            Ok(listing) => {
                let cat = self.nodes.get(folder).map(|n| n.cat).unwrap_or(0);
                let mut entries = vec![];
                for e in listing.entries {
                    entries.push(e.path.clone());
                    self.nodes.insert(
                        e.path.clone(),
                        Node {
                            path: e.path,
                            name: e.name,
                            location: None,
                            size: e.size,
                            modified: e.modified,
                            is_dir: e.is_dir,
                            parent: Some(folder.to_string()),
                            cat,
                            note: None,
                        },
                    );
                }
                Children::Loaded { entries, hidden: listing.hidden }
            }
        };
        self.children.insert(folder.to_string(), state);
    }

    /* ---------- Derived values ---------- */

    /// (bytes, non-empty categories) of things that are junk by nature: everything except
    /// Large Files, which are the user's own files.
    pub fn junk_found(&self) -> (u64, usize) {
        let junk = self.cats.iter().filter(|c| !is_personal(&c.info));
        junk.fold((0, 0), |(size, n), c| (size + c.total, n + usize::from(!c.items.is_empty())))
    }

    /// Bytes listed under Large Files.
    pub fn personal_found(&self) -> u64 {
        self.cats.iter().filter(|c| is_personal(&c.info)).map(|c| c.total).sum()
    }

    /// Bytes selected among the user's own files.
    pub fn personal_selected(&self) -> u64 {
        let own = |n: &&Node| is_personal(&self.cats[n.cat].info);
        self.selected.iter().filter_map(|p| self.nodes.get(p)).filter(own).map(|n| n.size).sum()
    }

    pub fn filter_active(&self) -> bool {
        !self.query.trim().is_empty() || self.age_applies(self.active)
    }

    /// The age menu filters by when items were last modified. Old Files has its own
    /// "unused for" menu instead, so there it is hidden and ignored.
    pub fn age_applies(&self, ci: usize) -> bool {
        self.age != Age::Any && self.cats.get(ci).is_none_or(|c| c.info.id != "old-files")
    }

    /// Top-level items of a category that pass the search text and the age filter.
    pub fn visible(&self, ci: usize) -> Vec<String> {
        let Some(cat) = self.cats.get(ci) else { return vec![] };
        if self.query.trim().is_empty() && !self.age_applies(ci) {
            return cat.items.clone();
        }
        let query = self.query.trim().to_lowercase();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let newest = now.saturating_sub(self.age.days() * 86_400_000);
        cat.items
            .iter()
            .filter(|p| {
                let n = &self.nodes[*p];
                let old_enough = !self.age_applies(ci) || n.modified.is_some_and(|m| m <= newest);
                let matches = query.is_empty()
                    || n.name.to_lowercase().contains(&query)
                    || n.location.as_deref().is_some_and(|l| l.to_lowercase().contains(&query))
                    || kind(&n.name, n.is_dir).to_lowercase().contains(&query);
                old_enough && matches
            })
            .cloned()
            .collect()
    }

    pub fn selection(&self) -> (usize, u64) {
        let size = self.selected.iter().filter_map(|p| self.nodes.get(p)).map(|n| n.size).sum();
        (self.selected.len(), size)
    }

    /// (selected count, selected size, state for the category checkbox)
    pub fn cat_selection(&self, ci: usize) -> (usize, u64, Check) {
        let (mut n, mut size) = (0, 0);
        for p in &self.selected {
            if let Some(node) = self.nodes.get(p) {
                if node.cat == ci {
                    n += 1;
                    size += node.size;
                }
            }
        }
        let cat = &self.cats[ci];
        let all = !cat.items.is_empty() && cat.items.iter().all(|i| self.selected.contains(i));
        let state = if all {
            Check::On
        } else if n > 0 {
            Check::Partial
        } else {
            Check::Off
        };
        (n, size, state)
    }

    pub fn check_state(&self, path: &str) -> Check {
        if self.covering(path).is_some() {
            Check::On
        } else if self.has_selected_inside(path) {
            Check::Partial
        } else {
            Check::Off
        }
    }

    /// Top-level item of the tree a node belongs to.
    pub fn top_level<'a>(&'a self, path: &'a str) -> &'a Node {
        let mut node = &self.nodes[path];
        while let Some(parent) = &node.parent {
            node = &self.nodes[parent];
        }
        node
    }

    pub fn sorted(&self, list: &[String]) -> Vec<String> {
        let mut v = list.to_vec();
        v.sort_by(|a, b| {
            let (a, b) = (&self.nodes[a], &self.nodes[b]);
            let ord = match self.sort {
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Kind => kind(&a.name, a.is_dir).cmp(&kind(&b.name, b.is_dir)).then(b.size.cmp(&a.size)),
                SortKey::Modified => a.modified.unwrap_or(0).cmp(&b.modified.unwrap_or(0)),
                SortKey::Size => a.size.cmp(&b.size),
            };
            if self.ascending {
                ord
            } else {
                ord.reverse()
            }
        });
        v
    }

    pub fn rows(&self) -> Vec<Row> {
        let mut out = vec![];
        if self.active < self.cats.len() {
            self.push_rows(&self.sorted(&self.visible(self.active)), 0, &mut out);
        }
        out
    }

    fn push_rows(&self, list: &[String], depth: usize, out: &mut Vec<Row>) {
        for path in list {
            out.push(Row::Item { path: path.clone(), depth });
            let node = &self.nodes[path];
            if !(node.is_dir && self.expanded.contains(path)) {
                continue;
            }
            let note = |text: String, kind| Row::Note { depth: depth + 1, text, kind };
            match self.children.get(path) {
                None | Some(Children::Loading) => out.push(note("Loading…".into(), NoteKind::Loading)),
                Some(Children::Failed(e)) => out.push(note(e.clone(), NoteKind::Error)),
                Some(Children::Loaded { entries, hidden }) => {
                    if entries.is_empty() {
                        out.push(note("This folder is empty".into(), NoteKind::Info));
                    } else {
                        self.push_rows(&self.sorted(entries), depth + 1, out);
                        if *hidden > 0 {
                            let text = format!("{} smaller {} not shown", hidden, if *hidden == 1 { "item" } else { "items" });
                            out.push(note(text, NoteKind::Info));
                        }
                    }
                }
            }
        }
    }

    /// Paths selected in a category, largest first (for the confirmation list).
    pub fn selected_in(&self, ci: usize) -> Vec<&Node> {
        let mut v: Vec<&Node> = self
            .selected
            .iter()
            .filter_map(|p| self.nodes.get(p))
            .filter(|n| n.cat == ci)
            .collect();
        v.sort_by_key(|n| std::cmp::Reverse(n.size));
        v
    }

    /// True when a selected path belongs to the clean being confirmed.
    fn in_scope(&self, path: &str) -> bool {
        self.scope.is_none() || self.nodes.get(path).map(|n| n.cat) == self.scope
    }

    /// The selected paths the clean being confirmed would move.
    pub fn scoped(&self) -> Vec<String> {
        self.selected.iter().filter(|p| self.in_scope(p)).cloned().collect()
    }

    /// (count, size) of `scoped`.
    pub fn scoped_selection(&self) -> (usize, u64) {
        match self.scope {
            Some(ci) => {
                let (n, size, _) = self.cat_selection(ci);
                (n, size)
            }
            None => self.selection(),
        }
    }

    pub fn selection_needs_review(&self) -> bool {
        self.selected.iter().filter(|p| self.in_scope(p)).any(|p| {
            let n = &self.nodes[p];
            self.cats[n.cat].info.risk == Risk::Review || self.top_level(p).note.is_some()
        })
    }

    /* ---------- Tree selection ---------- */

    /// The selected path itself, or the nearest selected folder above it.
    fn covering(&self, path: &str) -> Option<String> {
        let mut cur = Some(path.to_string());
        while let Some(p) = cur {
            if self.selected.contains(&p) {
                return Some(p);
            }
            cur = self.nodes.get(&p).and_then(|n| n.parent.clone());
        }
        None
    }

    fn has_selected_inside(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        self.selected.iter().any(|s| s.starts_with(&prefix))
    }

    fn drop_inside(&mut self, path: &str) {
        let prefix = format!("{path}/");
        self.selected.retain(|s| !s.starts_with(&prefix));
    }

    /// When every child of a fully listed folder is selected, select the folder instead.
    fn collapse_up(&mut self, path: &str) {
        let Some(parent) = self.nodes.get(path).and_then(|n| n.parent.clone()) else { return };
        let Some(Children::Loaded { entries, hidden: 0 }) = self.children.get(&parent) else { return };
        if entries.iter().all(|e| self.selected.contains(e)) {
            for e in entries.clone() {
                self.selected.remove(&e);
            }
            self.selected.insert(parent.clone());
            self.collapse_up(&parent);
        }
    }

    pub fn toggle(&mut self, path: &str) {
        match self.covering(path) {
            Some(cover) if cover == path => {
                self.selected.remove(path);
            }
            Some(cover) => {
                // Unchecking something inside a selected folder: keep its siblings at every level.
                self.selected.remove(&cover);
                let mut child = path.to_string();
                while child != cover {
                    let parent = self.nodes[&child].parent.clone().expect("has parent below cover");
                    if let Some(Children::Loaded { entries, .. }) = self.children.get(&parent) {
                        for sib in entries {
                            if *sib != child {
                                self.selected.insert(sib.clone());
                            }
                        }
                    }
                    child = parent;
                }
            }
            None => {
                self.drop_inside(path);
                self.selected.insert(path.to_string());
                self.collapse_up(path);
            }
        }
    }

    pub fn set_category(&mut self, ci: usize, on: bool) {
        if on {
            for item in self.cats[ci].items.clone() {
                self.drop_inside(&item);
                self.selected.insert(item);
            }
        } else {
            let nodes = &self.nodes;
            self.selected.retain(|p| nodes.get(p).map(|n| n.cat) != Some(ci));
        }
    }

    /// Selects or clears the items the current search and age filter show.
    pub fn set_visible(&mut self, on: bool) {
        for item in self.visible(self.active) {
            self.drop_inside(&item);
            if on {
                self.selected.insert(item);
            } else {
                self.selected.remove(&item);
            }
        }
    }

    /// (selected count, checkbox state) of the items the filter shows in the open category.
    pub fn visible_selection(&self) -> (usize, Check) {
        let items = self.visible(self.active);
        let on = items.iter().filter(|i| self.selected.contains(*i)).count();
        let touched = items.iter().filter(|i| self.check_state(i) != Check::Off).count();
        let state = match (on, touched) {
            (n, _) if n > 0 && n == items.len() => Check::On,
            (_, 0) => Check::Off,
            _ => Check::Partial,
        };
        (touched, state)
    }

    /// Takes a top-level item out of the results (the user asked never to see it again).
    pub fn ignore(&mut self, path: &str) {
        let Some(node) = self.nodes.get(path) else { return };
        if node.parent.is_some() {
            return;
        }
        let (ci, size) = (node.cat, node.size);
        self.drop_inside(path);
        self.selected.remove(path);
        self.expanded.remove(path);
        let cat = &mut self.cats[ci];
        cat.items.retain(|p| p != path);
        cat.total = cat.total.saturating_sub(size);
        if self.cursor.as_deref() == Some(path) {
            self.cursor = None;
        }
    }

    /// Labels an item that could not be moved, so it no longer looks like it is waiting to go,
    /// and unticks it so the next clean doesn't try it again.
    pub fn mark_failed(&mut self, path: &str) {
        if let Some(node) = self.nodes.get_mut(path) {
            node.note = Some("Couldn't be moved".to_string());
        }
        self.selected.remove(path);
    }

    /// Takes an item that was moved to the Trash out of the results, at any depth, and
    /// shrinks the folders above it by its size.
    pub fn remove_cleaned(&mut self, path: &str) {
        let Some(node) = self.nodes.get(path) else { return };
        let (size, ci) = (node.size, node.cat);
        let Some(parent) = node.parent.clone() else {
            self.ignore(path);
            return;
        };
        let prefix = format!("{path}/");
        self.drop_inside(path);
        self.selected.remove(path);
        self.expanded.remove(path);
        if self.cursor.as_deref().is_some_and(|c| c == path || c.starts_with(&prefix)) {
            self.cursor = None;
        }
        let mut above = Some(parent.clone());
        while let Some(n) = above.and_then(|p| self.nodes.get_mut(&p)) {
            n.size = n.size.saturating_sub(size);
            above = n.parent.clone();
        }
        self.cats[ci].total = self.cats[ci].total.saturating_sub(size);
        if let Some(Children::Loaded { entries, .. }) = self.children.get_mut(&parent) {
            entries.retain(|e| e != path);
        }
        self.nodes.remove(path);
    }

    pub fn set_sort(&mut self, key: SortKey) {
        if self.sort == key {
            self.ascending = !self.ascending;
        } else {
            self.sort = key;
            self.ascending = matches!(key, SortKey::Name | SortKey::Kind);
        }
    }
}

/// These list the user's own files and apps; every other category is junk by nature.
pub fn is_personal(info: &SourceInfo) -> bool {
    matches!(info.id, "large-files" | "old-files" | "duplicates" | "applications" | "ios-backups")
}

/* ---------- Formatting ---------- */

/// What Finder would roughly call this item, judged by its extension.
pub fn kind(name: &str, is_dir: bool) -> String {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    let known = match ext.as_str() {
        "app" => "Application",
        "xcarchive" => "Xcode Archive",
        "photoslibrary" => "Photo Library",
        _ if is_dir => "Folder",
        "mov" | "mp4" | "mkv" | "avi" | "m4v" | "webm" | "wmv" | "mpg" | "mpeg" => "Video",
        "mp3" | "wav" | "flac" | "aac" | "m4a" | "aiff" | "aif" | "ogg" => "Audio",
        "jpg" | "jpeg" | "png" | "heic" | "gif" | "tiff" | "tif" | "psd" | "raw" | "dng" | "webp" => "Image",
        "zip" | "tar" | "gz" | "tgz" | "bz2" | "xz" | "7z" | "rar" | "zst" => "Archive",
        "dmg" | "iso" | "img" | "sparseimage" => "Disk Image",
        "pkg" | "mpkg" | "xip" => "Installer",
        "pdf" => "PDF",
        "doc" | "docx" | "pages" | "txt" | "rtf" | "md" => "Document",
        "xls" | "xlsx" | "numbers" | "csv" => "Spreadsheet",
        "ppt" | "pptx" | "key" => "Presentation",
        "db" | "sqlite" | "sqlite3" => "Database",
        "log" => "Log",
        "ipa" | "apk" => "App Package",
        "vmdk" | "qcow2" | "vdi" => "Virtual Disk",
        "" => "File",
        _ => "",
    };
    if !known.is_empty() {
        known.to_string()
    } else if ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        format!("{} file", ext.to_ascii_uppercase())
    } else {
        "File".to_string()
    }
}

/// Decimal units, matching Finder.
pub fn bytes(n: u64) -> String {
    if n == 0 {
        return "Zero KB".into();
    }
    let units = ["bytes", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1000.0 && i < units.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} bytes")
    } else if v >= 100.0 {
        format!("{:.0} {}", v, units[i])
    } else {
        format!("{:.1} {}", v, units[i])
    }
}

pub fn ago(ms: Option<u64>) -> String {
    let Some(ms) = ms else { return "—".into() };
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(ms);
    let days = now.saturating_sub(ms) / 86_400_000;
    let unit = |n: u64, one: &str, last: &str| {
        if n == 1 {
            last.to_string()
        } else {
            format!("{n} {one}s ago")
        }
    };
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=6 => format!("{days} days ago"),
        7..=29 => unit(days / 7, "week", "Last week"),
        30..=364 => unit(days / 30, "month", "Last month"),
        _ => unit(days / 365, "year", "Last year"),
    }
}

pub fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", n, if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(path: &str, parent: Option<&str>) -> Node {
        Node {
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            location: None,
            size: 10,
            modified: None,
            is_dir: true,
            parent: parent.map(Into::into),
            cat: 0,
            note: None,
        }
    }

    /// /a contains /a/x and /a/y; /a/x contains /a/x/1 and /a/x/2.
    fn model() -> Model {
        let mut m = Model::new(vec![]);
        for (p, parent) in [
            ("/a", None),
            ("/a/x", Some("/a")),
            ("/a/y", Some("/a")),
            ("/a/x/1", Some("/a/x")),
            ("/a/x/2", Some("/a/x")),
        ] {
            m.nodes.insert(p.into(), node(p, parent));
        }
        m.children.insert("/a".into(), Children::Loaded { entries: vec!["/a/x".into(), "/a/y".into()], hidden: 0 });
        m.children.insert("/a/x".into(), Children::Loaded { entries: vec!["/a/x/1".into(), "/a/x/2".into()], hidden: 0 });
        m
    }

    fn sel(m: &Model) -> Vec<&str> {
        let mut v: Vec<&str> = m.selected.iter().map(String::as_str).collect();
        v.sort();
        v
    }

    #[test]
    fn unchecking_a_child_keeps_siblings_at_every_level() {
        let mut m = model();
        m.toggle("/a");
        m.toggle("/a/x/1");
        assert_eq!(sel(&m), ["/a/x/2", "/a/y"]);
        assert!(m.check_state("/a") == Check::Partial);
        assert!(m.check_state("/a/x/2") == Check::On);
    }

    #[test]
    fn selecting_every_child_selects_the_folder() {
        let mut m = model();
        m.toggle("/a/x/1");
        m.toggle("/a/x/2");
        assert_eq!(sel(&m), ["/a/x"]);
        m.toggle("/a/y");
        assert_eq!(sel(&m), ["/a"]);
    }

    #[test]
    fn selecting_a_folder_drops_selected_descendants() {
        let mut m = model();
        m.toggle("/a/x/1");
        m.toggle("/a");
        assert_eq!(sel(&m), ["/a"]);
        let (count, size) = m.selection();
        assert_eq!((count, size), (1, 10), "no double counting");
    }

    #[test]
    fn search_and_age_filter_the_top_level_list() {
        let mut m = Model::new(vec![]);
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        let day = 86_400_000;
        for (path, age_days) in [("/c/Chrome", 1), ("/c/Slack", 40), ("/c/old.dmg", 400)] {
            let mut n = node(path, None);
            n.is_dir = !path.ends_with(".dmg");
            n.modified = Some(now - age_days * day);
            m.nodes.insert(path.into(), n);
        }
        let info = SourceInfo { id: "caches", title: "", description: "", icon: "", risk: Risk::Safe };
        m.cats.push(Cat { info, items: vec!["/c/Chrome".into(), "/c/Slack".into(), "/c/old.dmg".into()], total: 30 });

        m.query = "sla".into();
        assert_eq!(m.visible(0), ["/c/Slack"]);
        m.query = "disk image".into();
        assert_eq!(m.visible(0), ["/c/old.dmg"], "the kind is searched too");
        m.query.clear();
        m.age = Age::Month;
        assert_eq!(m.visible(0), ["/c/Slack", "/c/old.dmg"]);

        m.set_visible(true);
        assert_eq!(sel(&m), ["/c/Slack", "/c/old.dmg"], "Select All only takes what the filter shows");
        m.ignore("/c/Slack");
        assert_eq!(sel(&m), ["/c/old.dmg"]);
        assert_eq!((m.cats[0].items.len(), m.cats[0].total), (2, 20));
    }

    #[test]
    fn cleaning_one_category_leaves_the_others() {
        let mut m = model();
        let cat = |id| Cat {
            info: SourceInfo { id, title: "", description: "", icon: "", risk: Risk::Safe },
            items: vec![],
            total: 0,
        };
        m.cats = vec![cat("caches"), cat("logs")];
        m.cats[0].items = vec!["/a".into()];
        m.cats[0].total = 10;
        m.nodes.insert("/b".into(), node("/b", None));
        m.nodes.get_mut("/b").unwrap().cat = 1;
        m.cats[1].items = vec!["/b".into()];
        m.cats[1].total = 10;
        m.toggle("/a/x/1");
        m.toggle("/b");

        m.scope = Some(0);
        assert_eq!(m.scoped(), ["/a/x/1"]);
        assert_eq!(m.scoped_selection().0, 1);
        m.scope = None;
        assert_eq!(m.scoped().len(), 2);

        let part = m.nodes["/a/x/1"].size;
        let before = m.nodes["/a"].size;
        m.remove_cleaned("/a/x/1");
        assert_eq!(sel(&m), ["/b"], "the other category keeps its selection");
        assert!(!m.nodes.contains_key("/a/x/1"));
        assert_eq!(m.nodes["/a"].size, before.saturating_sub(part), "folders above shrink");
        assert_eq!(m.cats[0].total, 10u64.saturating_sub(part));
        m.remove_cleaned("/b");
        assert!(m.cats[1].items.is_empty());
    }

    #[test]
    fn large_files_are_not_counted_as_junk() {
        let mut m = Model::new(vec![]);
        let cat = |id, total| Cat {
            info: SourceInfo { id, title: "", description: "", icon: "", risk: Risk::Review },
            items: vec![format!("/{id}")],
            total,
        };
        m.cats = vec![cat("caches", 5), cat("build-folders", 7), cat("large-files", 100)];
        assert_eq!(m.junk_found(), (12, 2));
        assert_eq!(m.personal_found(), 100);
    }

    #[test]
    fn a_path_is_listed_in_the_first_category_that_finds_it() {
        let info = |id| SourceInfo { id, title: "", description: "", icon: "", risk: Risk::Review };
        let item = |path: &str, size| crate::scanner::Item {
            path: path.into(),
            name: path.rsplit('/').next().unwrap().into(),
            location: String::new(),
            size,
            modified: None,
            selected: false,
            note: None,
            is_dir: !path.contains('.'),
        };
        let cat = |id, items: Vec<crate::scanner::Item>| Category { info: info(id), total: items.iter().map(|i| i.size).sum(), items };
        let mut m = Model::new(vec![]);
        m.load_scan(vec![
            cat("duplicates", vec![item("/h/Desktop/copy.mov", 300)]),
            cat("large-files", vec![item("/h/Desktop/copy.mov", 300), item("/h/Movies/Trip", 900)]),
            cat("old-files", vec![item("/h/Movies/Trip/notes.pdf", 5), item("/h/Documents/2021.pdf", 7)]),
        ]);
        assert_eq!(m.cats[1].items, ["/h/Movies/Trip"]);
        assert_eq!(m.cats[1].total, 900, "the total counts only what is shown");
        assert_eq!(m.cats[2].items, ["/h/Documents/2021.pdf"], "nothing inside another category's folder");
        assert_eq!(m.nodes["/h/Desktop/copy.mov"].cat, 0);
        assert_eq!(m.personal_found(), 300 + 900 + 7, "nothing is counted twice");
    }

    #[test]
    fn old_files_ignore_the_age_menu() {
        let mut m = Model::new(vec![]);
        let mut n = node("/h/old.pdf", None);
        n.modified = Some(0);
        m.nodes.insert("/h/new.pdf".into(), node("/h/new.pdf", None));
        m.nodes.insert("/h/old.pdf".into(), n);
        let info = SourceInfo { id: "old-files", title: "", description: "", icon: "", risk: Risk::Review };
        m.cats.push(Cat { info, items: vec!["/h/new.pdf".into(), "/h/old.pdf".into()], total: 20 });
        m.age = Age::Year;
        assert_eq!(m.visible(0).len(), 2);
        assert!(!m.filter_active());
    }

    #[test]
    fn names_kinds() {
        assert_eq!(kind("Movie.MKV", false), "Video");
        assert_eq!(kind("Xcode.app", true), "Application");
        assert_eq!(kind("com.apple.Safari", true), "Folder");
        assert_eq!(kind("data.bin", false), "BIN file");
        assert_eq!(kind("README", false), "File");
    }

    #[test]
    fn formats_sizes_like_finder() {
        assert_eq!(bytes(0), "Zero KB");
        assert_eq!(bytes(1_234_000), "1.2 MB");
        assert_eq!(bytes(245_000_000), "245 MB");
        assert_eq!(bytes(21_100_000_000), "21.1 GB");
    }
}
