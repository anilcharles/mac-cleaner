//! Finds cleanable files and moves the ones the user approved to the Trash.
//!
//! Safety rules:
//! * Only a fixed allow-list of locations inside the user's home folder is scanned.
//! * `clean` re-checks every path against that allow-list; anything else is refused.
//! * Nothing is deleted permanently: items are moved to the Trash and can be restored.

use rayon::prelude::*;
use serde::Serialize;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;
use walkdir::WalkDir;

#[derive(Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    /// Regenerated automatically by apps and tools.
    Safe,
    /// Might hold something the user still wants; never pre-selected.
    Review,
}

/// How a scan root turns into list items.
enum Layout {
    /// Every direct child of the root is one item.
    Children,
    /// Direct children of the root with one of these extensions.
    Extensions(&'static [&'static str]),
    /// The root folder itself is one item, shown with this label.
    Whole(&'static str),
}

struct Root {
    path: PathBuf,
    layout: Layout,
}

struct Source {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    icon: &'static str,
    risk: Risk,
    roots: Vec<Root>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    id: &'static str,
    title: &'static str,
    description: &'static str,
    icon: &'static str,
    risk: Risk,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    path: String,
    name: String,
    location: String,
    size: u64,
    /// Last modification time, milliseconds since the Unix epoch.
    modified: Option<u64>,
    selected: bool,
    note: Option<String>,
    is_dir: bool,
}

/// One entry inside an expanded folder.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    path: String,
    name: String,
    size: u64,
    modified: Option<u64>,
    is_dir: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    entries: Vec<Entry>,
    /// Entries left out because the folder has more than `MAX_ENTRIES`.
    hidden: usize,
}

/// Largest entries shown when a folder is expanded.
const MAX_ENTRIES: usize = 500;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    #[serde(flatten)]
    info: SourceInfo,
    total: u64,
    items: Vec<Item>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub id: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    path: String,
    reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    removed: usize,
    freed: u64,
    failures: Vec<Failure>,
}

pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into()))
}

fn sources(home: &Path) -> Vec<Source> {
    let h = |p: &str| home.join(p);
    let children = |p: &str| Root { path: h(p), layout: Layout::Children };
    let whole = |p: &str, label| Root { path: h(p), layout: Layout::Whole(label) };

    vec![
        Source {
            id: "caches",
            title: "App Caches",
            description: "Temporary files apps keep to load faster. Apps rebuild them when needed.",
            icon: "cache",
            risk: Risk::Safe,
            roots: vec![children("Library/Caches")],
        },
        Source {
            id: "logs",
            title: "Log Files",
            description: "Activity and diagnostic logs written by apps. Only useful when troubleshooting.",
            icon: "logs",
            risk: Risk::Safe,
            roots: vec![children("Library/Logs")],
        },
        Source {
            id: "derived-data",
            title: "Xcode Derived Data",
            description: "Build products and indexes. Xcode recreates them on the next build.",
            icon: "hammer",
            risk: Risk::Safe,
            roots: vec![children("Library/Developer/Xcode/DerivedData")],
        },
        Source {
            id: "simulator-caches",
            title: "Simulator Caches",
            description: "Cached data for iOS simulators. Recreated when a simulator next boots.",
            icon: "phone",
            risk: Risk::Safe,
            roots: vec![children("Library/Developer/CoreSimulator/Caches")],
        },
        Source {
            id: "package-caches",
            title: "Package Caches",
            description: "Downloaded packages from npm, Cargo, Gradle, pnpm and Bun. Fetched again on demand.",
            icon: "package",
            risk: Risk::Safe,
            roots: vec![
                whole(".npm/_cacache", "npm cache"),
                whole(".cargo/registry/cache", "Cargo registry cache"),
                whole(".gradle/caches", "Gradle caches"),
                whole("Library/pnpm/store", "pnpm store"),
                whole(".bun/install/cache", "Bun cache"),
            ],
        },
        Source {
            id: "xcode-archives",
            title: "Xcode Archives",
            description: "Archived app builds. They may hold the only dSYMs for crash reports of shipped versions.",
            icon: "archive",
            risk: Risk::Review,
            roots: vec![children("Library/Developer/Xcode/Archives")],
        },
        Source {
            id: "device-support",
            title: "Device Support",
            description: "Debug symbols for devices you have connected. Keep versions you still debug on.",
            icon: "chip",
            risk: Risk::Review,
            roots: vec![
                children("Library/Developer/Xcode/iOS DeviceSupport"),
                children("Library/Developer/Xcode/watchOS DeviceSupport"),
            ],
        },
        Source {
            id: "installers",
            title: "Installers in Downloads",
            description: "Disk images and installer packages. Usually not needed once the app is installed.",
            icon: "download",
            risk: Risk::Review,
            roots: vec![Root {
                path: h("Downloads"),
                layout: Layout::Extensions(&["dmg", "pkg", "mpkg", "xip"]),
            }],
        },
    ]
}

pub fn source_infos() -> Vec<SourceInfo> {
    sources(&home_dir()).into_iter().map(|s| info(&s)).collect()
}

fn info(s: &Source) -> SourceInfo {
    SourceInfo {
        id: s.id,
        title: s.title,
        description: s.description,
        icon: s.icon,
        risk: s.risk,
    }
}

/// Size on disk (allocated blocks), not following symlinks.
fn disk_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.blocks() * 512)
        .sum()
}

fn modified_ms(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

fn has_extension(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// Lists the candidate paths of one root together with their display name.
fn candidates(root: &Root) -> Vec<(PathBuf, String)> {
    match root.layout {
        Layout::Whole(label) => {
            if root.path.is_dir() {
                vec![(root.path.clone(), label.to_string())]
            } else {
                vec![]
            }
        }
        Layout::Children | Layout::Extensions(_) => {
            let Ok(entries) = fs::read_dir(&root.path) else { return vec![] };
            entries
                .filter_map(Result::ok)
                .filter(|e| e.file_name() != ".DS_Store" && e.file_name() != ".localized")
                .filter(|e| match root.layout {
                    Layout::Extensions(exts) => has_extension(&e.path(), exts),
                    _ => true,
                })
                .map(|e| (e.path(), e.file_name().to_string_lossy().into_owned()))
                .collect()
        }
    }
}

fn scan_source(source: &Source, home: &Path) -> Category {
    const OWN_ID: &str = "com.maccleaner.desktop";
    let mut items: Vec<Item> = source
        .roots
        .iter()
        .flat_map(candidates)
        .collect::<Vec<_>>()
        .into_par_iter()
        .filter(|(_, name)| name != OWN_ID)
        .filter_map(|(path, name)| {
            let size = disk_size(&path);
            if size == 0 {
                return None;
            }
            let apple = source.id == "caches" && name.starts_with("com.apple.");
            let note = if apple {
                Some("macOS system cache".to_string())
            } else {
                None
            };
            Some(Item {
                is_dir: is_real_dir(&path),
                location: tilde(path.parent().unwrap_or(&path), home),
                modified: modified_ms(&path),
                selected: source.risk == Risk::Safe && !apple,
                note,
                path: path.to_string_lossy().into_owned(),
                name,
                size,
            })
        })
        .collect();

    items.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    Category {
        info: info(source),
        total: items.iter().map(|i| i.size).sum(),
        items,
    }
}

pub fn scan_all(on_progress: impl Fn(Progress) + Sync) -> Vec<Category> {
    let home = home_dir();
    let sources = sources(&home);
    let total = sources.len();
    let done = AtomicUsize::new(0);

    sources
        .par_iter()
        .map(|s| {
            let category = scan_source(s, &home);
            let done = done.fetch_add(1, Ordering::SeqCst) + 1;
            on_progress(Progress { done, total, id: s.id });
            category
        })
        .collect()
}

/// A directory that is not a symlink.
fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// True when `path` is exactly one of the items the scanner offers.
fn is_item(sources: &[Source], path: &Path) -> bool {
    sources.iter().flat_map(|s| &s.roots).any(|root| match root.layout {
        Layout::Whole(_) => path == root.path,
        Layout::Children => path.parent() == Some(root.path.as_path()) && path.file_name().is_some(),
        Layout::Extensions(exts) => {
            path.parent() == Some(root.path.as_path()) && has_extension(path, exts)
        }
    })
}

/// True when `path` is an offered item, or lives inside one.
///
/// For paths inside an item, every folder between the item and the path must be a
/// real folder, not a symlink: a link could point outside the cleanable area.
pub fn is_allowed(path: &Path) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    let sources = sources(&home_dir());
    let mut current = path;
    loop {
        if is_item(&sources, current) {
            return true;
        }
        match current.parent() {
            Some(parent) if !is_symlink(parent) => current = parent,
            _ => return false,
        }
    }
}

/// Lists what is inside an offered folder, largest first.
pub fn list_children(path: &str) -> Result<Listing, String> {
    let dir = PathBuf::from(path);
    if !is_allowed(&dir) {
        return Err("This folder is outside the cleanable locations.".into());
    }
    if !is_real_dir(&dir) {
        return Err("Not a folder.".into());
    }
    let read = fs::read_dir(&dir).map_err(|e| friendly_error(&e.to_string()))?;
    let mut entries: Vec<Entry> = read
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|p| Entry {
            name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            size: disk_size(&p),
            modified: modified_ms(&p),
            is_dir: is_real_dir(&p),
            path: p.to_string_lossy().into_owned(),
        })
        .collect();
    entries.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.name.cmp(&b.name)));
    let hidden = entries.len().saturating_sub(MAX_ENTRIES);
    entries.truncate(MAX_ENTRIES);
    Ok(Listing { entries, hidden })
}

pub fn clean(paths: Vec<String>) -> CleanReport {
    use trash::macos::{DeleteMethod, TrashContextExtMacos};

    let mut ctx = trash::TrashContext::default();
    // NSFileManager avoids Finder's AppleScript prompts and the "put back" sound per item.
    ctx.set_delete_method(DeleteMethod::NsFileManager);

    let mut report = CleanReport { removed: 0, freed: 0, failures: vec![] };
    for raw in paths {
        let path = PathBuf::from(&raw);
        let fail = |reason: &str| Failure { path: raw.clone(), reason: reason.to_string() };

        if !is_allowed(&path) {
            report.failures.push(fail("Not inside a location Mac Cleaner is allowed to clean."));
            continue;
        }
        if fs::symlink_metadata(&path).is_err() {
            report.failures.push(fail("It no longer exists."));
            continue;
        }
        let size = disk_size(&path);
        match ctx.delete(&path) {
            Ok(()) => {
                report.removed += 1;
                report.freed += size;
            }
            Err(e) => report.failures.push(fail(&friendly_error(&e.to_string()))),
        }
    }
    report
}

fn friendly_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    if lower.contains("permission") || lower.contains("not permitted") || lower.contains("513") {
        "macOS protects this item. Grant Full Disk Access to clean it.".into()
    } else if lower.contains("in use") || lower.contains("busy") {
        "An app is using it right now.".into()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_paths_outside_the_allow_list() {
        let h = home_dir();
        for bad in [
            h.clone(),
            h.join("Documents"),
            h.join("Library"),
            h.join("Library/Caches"),                         // the root itself, not a child
            h.join("Library/Caches/../Preferences"),          // traversal
            h.join("Downloads/report.pdf"),                   // wrong extension
            h.join(".npm"),                                   // parent of a Whole root
            PathBuf::from("/System/Library/Caches/x"),
            PathBuf::from("Library/Caches/relative"),
        ] {
            assert!(!is_allowed(&bad), "should refuse {}", bad.display());
        }
    }

    #[test]
    fn accepts_paths_the_scanner_offers() {
        let h = home_dir();
        for good in [
            h.join("Library/Caches/com.example.app"),
            h.join("Library/Logs/DiagnosticReports"),
            h.join("Library/Developer/Xcode/DerivedData/App-abc"),
            h.join(".npm/_cacache"),
            h.join("Downloads/Installer.DMG"),
            h.join("Library/Caches/com.example.app/nested/file.db"), // inside an item
        ] {
            assert!(is_allowed(&good), "should accept {}", good.display());
        }
    }

    #[test]
    fn refuses_paths_reached_through_a_symlink() {
        // A link inside a cache folder that points elsewhere must not expose its target.
        let caches = home_dir().join("Library/Caches");
        let item = caches.join(format!("mac-cleaner-test-{}", std::process::id()));
        let outside = std::env::temp_dir().join(format!("mac-cleaner-outside-{}", std::process::id()));
        fs::create_dir_all(item.join("real")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, item.join("link")).unwrap();

        let real_ok = is_allowed(&item.join("real/file"));
        let link_itself_ok = is_allowed(&item.join("link"));
        let through_link_ok = is_allowed(&item.join("link/secret.txt"));
        let listing_through_link = list_children(&item.join("link").to_string_lossy());

        fs::remove_dir_all(&item).unwrap();
        fs::remove_dir_all(&outside).unwrap();

        assert!(real_ok, "real nested folder should be allowed");
        assert!(link_itself_ok, "the link itself may be trashed (moves only the link)");
        assert!(!through_link_ok, "paths through a symlink must be refused");
        assert!(listing_through_link.is_err(), "symlinked folders must not be expanded");
    }

    /// Read-only: walks the real home folder and prints what would be offered.
    #[test]
    #[ignore]
    fn real_scan_summary() {
        let start = std::time::Instant::now();
        let cats = scan_all(|_| {});
        for c in &cats {
            println!("{:<26} {:>5} items {:>12} bytes", c.info.title, c.items.len(), c.total);
            for i in c.items.iter().take(3) {
                println!("    {:<44} {:>12}  selected={}", i.name, i.size, i.selected);
            }
        }
        println!("scan took {:?}", start.elapsed());

        let biggest = &cats[0].items[0];
        let listing = list_children(&biggest.path).expect("list children");
        println!("\ninside {} ({} shown, {} hidden):", biggest.name, listing.entries.len(), listing.hidden);
        for e in listing.entries.iter().take(6) {
            println!("    {:<40} {:>12}  dir={}", e.name, e.size, e.is_dir);
        }
    }
}
