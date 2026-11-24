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
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
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
    /// Every file of at least `large_file_min()` bytes under the root (the home folder),
    /// outside the areas `skip_for_large_files` rules out.
    LargeFiles,
    /// Build output and dependency folders of code projects under the root (the home folder).
    BuildFolders,
    /// Simulator devices: every folder in the root, named from its `device.plist`.
    Devices,
    /// Device backups made by Finder: every folder in the root, named from its `Info.plist`.
    Backups,
    /// Apps in the root (and in plain folders one level down) that the user can remove
    /// without a password. Each one is offered together with its support files in ~/Library.
    Apps,
    /// Entries in the root named after an app that is no longer installed (see `apps`).
    Leftovers,
    /// Files under the root (the home folder) that have an identical copy elsewhere (see `duplicates`).
    Duplicates,
}

/// Files at least this big (on disk) are listed under Large Files. The user can change it.
static LARGE_FILE_MIN: AtomicU64 = AtomicU64::new(100_000_000);

pub fn large_file_min() -> u64 {
    LARGE_FILE_MIN.load(Ordering::Relaxed)
}

pub fn set_large_file_min(bytes: u64) {
    LARGE_FILE_MIN.store(bytes, Ordering::Relaxed);
}

/// Folders inside a code project that its tools rebuild or download again:
/// (folder name, file that must sit next to it, what to call it).
const BUILD_KINDS: &[(&str, &str, &str)] = &[
    ("node_modules", "package.json", "Node packages"),
    (".next", "package.json", "Next.js build output"),
    ("target", "Cargo.toml", "Rust build output"),
    ("target", "pom.xml", "Maven build output"),
    (".build", "Package.swift", "Swift build output"),
    ("Pods", "Podfile", "CocoaPods packages"),
    ("build", "build.gradle", "Gradle build output"),
    ("build", "build.gradle.kts", "Gradle build output"),
    (".gradle", "build.gradle", "Gradle project cache"),
    (".gradle", "build.gradle.kts", "Gradle project cache"),
    (".dart_tool", "pubspec.yaml", "Dart build cache"),
];

/// Shared between a running scan and the UI: progress out, cancellation in.
#[derive(Default)]
pub struct ScanCtl {
    pub cancel: AtomicBool,
    /// Files and folders looked at so far.
    pub checked: AtomicU64,
    /// Ids of the categories that are done.
    pub finished: Mutex<Vec<&'static str>>,
}

impl ScanCtl {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub(crate) fn tick(&self) {
        self.checked.fetch_add(1, Ordering::Relaxed);
    }
}

const INSTALLER_EXTS: &[&str] = &["dmg", "pkg", "mpkg", "xip"];

/// Folders macOS shows as a single file. Removing one file from inside them would
/// corrupt the whole thing (a Photos library, an app, a Logic project...).
const PACKAGE_EXTS: &[&str] = &[
    "app", "photoslibrary", "photolibrary", "migratedphotolibrary", "musiclibrary", "tvlibrary",
    "imovielibrary", "aplibrary", "fcpbundle", "logicx", "band", "xcarchive", "bundle", "framework",
    "plugin", "kext", "pkg", "mpkg", "rtfd", "pages", "numbers", "key", "sparsebundle", "vmwarevm",
    "pvm", "utm", "xcodeproj", "xcworkspace", "playground", "lrlibrary", "lrcat-data",
];

struct Root {
    pub path: PathBuf,
    pub layout: Layout,
}

struct Source {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub risk: Risk,
    pub roots: Vec<Root>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub icon: &'static str,
    pub risk: Risk,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub path: String,
    pub name: String,
    pub location: String,
    pub size: u64,
    /// Last modification time, milliseconds since the Unix epoch.
    pub modified: Option<u64>,
    pub selected: bool,
    pub note: Option<String>,
    pub is_dir: bool,
}

/// One entry inside an expanded folder.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub modified: Option<u64>,
    pub is_dir: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub entries: Vec<Entry>,
    /// Entries left out because the folder has more than `MAX_ENTRIES`.
    pub hidden: usize,
}

/// Largest entries shown when a folder is expanded.
const MAX_ENTRIES: usize = 500;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    #[serde(flatten)]
    pub info: SourceInfo,
    pub total: u64,
    pub items: Vec<Item>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CleanReport {
    pub removed: usize,
    pub freed: u64,
    pub failures: Vec<Failure>,
}

/// The home folder of the account running the app.
///
/// Taken from the user database, not from `$HOME`: the whole allow-list hangs off this
/// folder, so an unset or altered variable must not be able to move it (to `/`, say).
/// Without a usable answer the app stops instead of guessing.
pub fn home_dir() -> PathBuf {
    static HOME: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| account_home().expect("could not find this account's home folder")).clone()
}

fn account_home() -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 16 * 1024];
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(libc::getuid(), &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut found) };
    if rc != 0 || found.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) }.to_bytes();
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(dir));
    (path.is_absolute() && path != Path::new("/")).then_some(path)
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
            description: "Downloaded packages from npm, Cargo, Gradle, pnpm, Bun, Yarn, Go, Maven and CocoaPods. Fetched again on demand.",
            icon: "package",
            risk: Risk::Safe,
            roots: vec![
                whole(".npm/_cacache", "npm cache"),
                whole(".cargo/registry/cache", "Cargo registry cache"),
                whole(".gradle/caches", "Gradle caches"),
                whole("Library/pnpm/store", "pnpm store"),
                whole(".bun/install/cache", "Bun cache"),
                whole(".yarn/berry/cache", "Yarn cache"),
                whole("go/pkg/mod", "Go module cache"),
                whole(".m2/repository", "Maven repository"),
                whole(".cocoapods/repos", "CocoaPods spec repos"),
            ],
        },
        Source {
            id: "build-folders",
            title: "Project Build Folders",
            description: "Build output and downloaded dependencies inside your code projects. The project's tools recreate them, which can take a while.",
            icon: "code",
            risk: Risk::Review,
            roots: vec![Root { path: home.to_path_buf(), layout: Layout::BuildFolders }],
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
            id: "simulator-devices",
            title: "Simulator Devices",
            description: "Simulators with the apps and data installed on them. Xcode can create a device again, but what was on it is gone.",
            icon: "phone",
            risk: Risk::Review,
            roots: vec![Root {
                path: h("Library/Developer/CoreSimulator/Devices"),
                layout: Layout::Devices,
            }],
        },
        Source {
            id: "installers",
            title: "Installers in Downloads",
            description: "Disk images and installer packages. Usually not needed once the app is installed.",
            icon: "download",
            risk: Risk::Review,
            roots: vec![Root {
                path: h("Downloads"),
                layout: Layout::Extensions(INSTALLER_EXTS),
            }],
        },
        Source {
            id: "mail-downloads",
            title: "Mail Downloads",
            description: "Copies of attachments you opened in Mail. The attachments stay in your mail.",
            icon: "mail",
            risk: Risk::Safe,
            roots: vec![children("Library/Containers/com.apple.mail/Data/Library/Mail Downloads")],
        },
        Source {
            id: "ios-updates",
            title: "iOS Software Updates",
            description: "Downloaded system updates for iPhone and iPad. Finder downloads them again when needed.",
            icon: "download",
            risk: Risk::Safe,
            roots: ["iPhone", "iPad", "iPod"]
                .iter()
                .map(|d| Root {
                    path: h(&format!("Library/iTunes/{d} Software Updates")),
                    layout: Layout::Extensions(&["ipsw"]),
                })
                .collect(),
        },
        Source {
            id: "ios-backups",
            title: "Device Backups",
            description: "iPhone and iPad backups made with Finder. Keep the latest one of each device unless it is backed up to iCloud.",
            icon: "phone",
            risk: Risk::Review,
            roots: vec![Root { path: h("Library/Application Support/MobileSync/Backup"), layout: Layout::Backups }],
        },
        Source {
            id: "applications",
            title: "Applications",
            description: "Apps you installed, with their settings and support files. Removing one moves the app and its files to the Trash.",
            icon: "app",
            risk: Risk::Review,
            roots: crate::apps::app_folders(home).into_iter().map(|path| Root { path, layout: Layout::Apps }).collect(),
        },
        Source {
            id: "app-leftovers",
            title: "App Leftovers",
            description: "Settings and support files of apps that are no longer installed. Anything changed in the last 30 days is left out.",
            icon: "box",
            risk: Risk::Review,
            roots: crate::apps::LEFTOVER_DIRS.iter().map(|d| Root { path: h(d), layout: Layout::Leftovers }).collect(),
        },
        Source {
            id: "duplicates",
            title: "Duplicate Files",
            description: "Files of 1 MB or more with an identical copy elsewhere in your folders. The oldest copy is kept and never listed.",
            icon: "copy",
            risk: Risk::Review,
            roots: vec![Root { path: home.to_path_buf(), layout: Layout::Duplicates }],
        },
        Source {
            id: "large-files",
            title: "Large Files & Folders",
            description: "The biggest files in your home folder, and the folders that hold them. These are your own files, not caches, so check each one.",
            icon: "drive",
            risk: Risk::Review,
            roots: vec![Root { path: home.to_path_buf(), layout: Layout::LargeFiles }],
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

/// Size on disk (allocated blocks), not following symlinks. A symlink counts as the
/// link itself, which is all that moving it to the Trash would take.
pub(crate) fn disk_size(path: &Path, ctl: Option<&ScanCtl>) -> u64 {
    WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
        .into_iter()
        .take_while(|_| match ctl {
            Some(c) => {
                c.tick();
                !c.cancelled()
            }
            None => true,
        })
        .filter_map(Result::ok)
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.blocks() * 512)
        .sum()
}

pub(crate) fn modified_ms(path: &Path) -> Option<u64> {
    fs::symlink_metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as u64)
}

pub(crate) fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

pub(crate) fn has_extension(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.iter().any(|x| x.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// Lists the candidate paths of one root together with their display name.
/// (path, display name, location text when it differs from the parent folder)
type Candidate = (PathBuf, String, Option<String>);

fn candidates(root: &Root, home: &Path, ctl: &ScanCtl) -> Vec<Candidate> {
    // A linked root would list (and later move) whatever the link points at.
    if reached_through_symlink(&root.path, home) {
        return vec![];
    }
    match root.layout {
        Layout::Whole(label) => {
            if is_real_dir(&root.path) {
                vec![(root.path.clone(), label.to_string(), None)]
            } else {
                vec![]
            }
        }
        Layout::LargeFiles => large_file_items(&root.path, large_file_min(), ctl),
        Layout::Apps => crate::apps::app_items(&root.path, home),
        Layout::Leftovers => crate::apps::leftover_items(&root.path, home),
        Layout::Duplicates => crate::duplicates::duplicate_items(&root.path, ctl),
        Layout::Backups => {
            let Ok(entries) = fs::read_dir(&root.path) else { return vec![] };
            let parent = tilde(&root.path, home);
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| is_real_dir(p))
                .map(|dir| {
                    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let plist = dir.join("Info.plist");
                    let name = plist_string(&plist, "Device Name").unwrap_or(id);
                    let location = match plist_string(&plist, "Product Name") {
                        Some(product) => format!("{product} · {parent}"),
                        None => parent.clone(),
                    };
                    (dir, name, Some(location))
                })
                .collect()
        }
        Layout::BuildFolders => build_folder_items(&root.path, ctl),
        Layout::Devices => {
            let Ok(entries) = fs::read_dir(&root.path) else { return vec![] };
            let dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| is_real_dir(p)).collect();
            let parent = tilde(&root.path, home);
            dirs.into_par_iter()
                .map(|dir| {
                    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let plist = dir.join("device.plist");
                    let name = plist_string(&plist, "name").unwrap_or(id);
                    let location = match plist_string(&plist, "runtime") {
                        Some(runtime) => format!("{} · {parent}", runtime_label(&runtime)),
                        None => parent.clone(),
                    };
                    (dir, name, Some(location))
                })
                .collect()
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
                .map(|e| (e.path(), e.file_name().to_string_lossy().into_owned(), None))
                .collect()
        }
    }
}

fn scan_source(source: &Source, home: &Path, ignored: &HashSet<String>, ctl: &ScanCtl) -> Category {
    const OWN_ID: &str = "com.maccleaner.desktop";
    let mut items: Vec<Item> = source
        .roots
        .iter()
        .flat_map(|root| candidates(root, home, ctl))
        .collect::<Vec<_>>()
        .into_par_iter()
        .filter(|(path, name, _)| name != OWN_ID && !ignored.contains(path.to_string_lossy().as_ref()))
        .filter_map(|(path, name, location)| {
            let app = crate::apps::is_listed_app(&path, home);
            // An app is measured with its support files, which go with it.
            let support: u64 = if app {
                crate::apps::support_files(&path, home).iter().map(|p| disk_size(p, Some(ctl))).sum()
            } else {
                0
            };
            let size = disk_size(&path, Some(ctl)) + support;
            if size == 0 {
                return None;
            }
            let apple = source.id == "caches" && name.starts_with("com.apple.");
            let protected = source.id == "app-leftovers" && crate::apps::is_protected(&path);
            let note = if apple {
                Some("macOS system cache".to_string())
            } else if protected {
                Some("Protected by macOS".to_string())
            } else {
                None
            };
            Some(Item {
                // An app is offered whole: opening it would offer the files inside the bundle.
                is_dir: is_real_dir(&path) && !app,
                location: location.unwrap_or_else(|| tilde(path.parent().unwrap_or(&path), home)),
                modified: modified_ms(&path),
                selected: source.risk == Risk::Safe && !apple && !protected,
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

/// Scans every category. `ignored` holds paths of items the user hid for good.
/// When `ctl.cancel` is set the scan winds down quickly and its result must be thrown away.
pub fn scan_all(ignored: &HashSet<String>, ctl: &ScanCtl) -> Vec<Category> {
    let home = home_dir();
    sources(&home)
        .par_iter()
        .map(|s| {
            let category = scan_source(s, &home, ignored, ctl);
            ctl.finished.lock().unwrap().push(s.id);
            category
        })
        .collect()
}

/// Storage of the volume that holds the home folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskSpace {
    /// The volume's name, as Finder shows it ("Macintosh HD").
    pub name: String,
    pub total: u64,
    /// What Finder and System Settings call available, including space macOS frees on demand.
    pub free: u64,
}

impl DiskSpace {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }
}

/// Reads the home volume's storage. Asks macOS first, so the numbers match Finder; falls back to
/// `statfs`, which does not count purgeable space.
pub fn disk_space() -> Option<DiskSpace> {
    let home = home_dir();
    if let Some((name, total, free)) = crate::native::volume_space(&home) {
        if total > 0 {
            return Some(DiskSpace { name, total, free: free.min(total) });
        }
    }
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(home.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let block = stat.f_bsize as u64;
    Some(DiskSpace {
        name: "Macintosh HD".into(),
        total: stat.f_blocks as u64 * block,
        free: stat.f_bavail as u64 * block,
    })
}

/// One string value from a property list (binary or XML), read with the system's `plutil`.
pub(crate) fn plist_string(plist: &Path, key: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(plist)
        .output()
        .ok()?;
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !value.is_empty()).then_some(value)
}

/// "com.apple.CoreSimulator.SimRuntime.iOS-17-2" → "iOS 17.2".
fn runtime_label(runtime: &str) -> String {
    let short = runtime.rsplit('.').next().unwrap_or(runtime);
    match short.split_once('-') {
        Some((os, version)) => format!("{os} {}", version.replace('-', ".")),
        None => short.to_string(),
    }
}

/// A directory that is not a symlink.
pub(crate) fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

pub(crate) fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
}

/// True when `path`, or any folder between it and the home folder, is a symlink.
/// The home folder itself is not checked.
pub(crate) fn reached_through_symlink(path: &Path, home: &Path) -> bool {
    let mut current = Some(path);
    while let Some(p) = current {
        if p == home {
            return false;
        }
        if is_symlink(p) {
            return true;
        }
        current = p.parent();
    }
    false
}

/// True when `path` is exactly one of the items the scanner offers.
///
/// A root that is a symlink, or sits below one, offers nothing: its children would be
/// files from wherever the link points, even though their paths look like cache paths.
fn is_item(sources: &[Source], path: &Path, home: &Path) -> bool {
    sources.iter().flat_map(|s| &s.roots).any(|root| match root.layout {
        Layout::Whole(_) => path == root.path && !reached_through_symlink(&root.path, home),
        Layout::Children => {
            path.parent() == Some(root.path.as_path())
                && path.file_name().is_some()
                && !reached_through_symlink(&root.path, home)
        }
        Layout::Extensions(exts) => {
            path.parent() == Some(root.path.as_path())
                && has_extension(path, exts)
                && !reached_through_symlink(&root.path, home)
        }
        Layout::LargeFiles => {
            let min = large_file_min();
            is_large_file(path, &root.path, min) || is_large_file_folder(path, &root.path, min)
        }
        Layout::BuildFolders => is_build_folder(path, &root.path),
        Layout::Devices | Layout::Backups => {
            path.parent() == Some(root.path.as_path())
                && is_real_dir(path)
                && !reached_through_symlink(&root.path, home)
        }
        Layout::Apps => crate::apps::is_removable_app(path, &root.path),
        Layout::Leftovers => {
            path.parent() == Some(root.path.as_path())
                && !reached_through_symlink(&root.path, home)
                && crate::apps::is_leftover(path)
        }
        Layout::Duplicates => crate::duplicates::is_offered_copy(path, home),
    })
}

/// Items whose insides are never offered on their own: apps, whose bundle would break.
fn whole_only(path: &Path, home: &Path) -> bool {
    crate::apps::is_listed_app(path, home)
}

pub(crate) fn is_package(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| PACKAGE_EXTS.iter().any(|p| p.eq_ignore_ascii_case(e)))
        .unwrap_or(false)
}

/// Folders the large-file scan never enters.
pub(crate) fn skip_for_large_files(name: &str, depth: usize) -> bool {
    (depth == 1 && name == "Library") || name.starts_with('.') || name == "node_modules" || is_package(name)
}

/// What kind of build folder `dir` is, judged by its name and the project file next to it.
pub(crate) fn build_kind(dir: &Path) -> Option<&'static str> {
    let name = dir.file_name()?.to_str()?;
    let parent = dir.parent()?;
    BUILD_KINDS
        .iter()
        .find(|(folder, marker, _)| *folder == name && parent.join(marker).is_file())
        .map(|(_, _, label)| *label)
}

/// True when a folder between `home` and `path` (not `path` itself) is a build folder.
/// Such files belong to Project Build Folders, not Large Files.
pub(crate) fn inside_build_folder(path: &Path, home: &Path) -> bool {
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == home || !d.starts_with(home) {
            return false;
        }
        if build_kind(d).is_some() {
            return true;
        }
        dir = d.parent();
    }
    false
}

/// Build folders of code projects under the home folder, with the same no-go areas as the
/// large-file scan. A build folder is never entered, so nested ones are not listed twice.
fn build_folders(home: &Path, ctl: &ScanCtl) -> Vec<(PathBuf, &'static str)> {
    let mut out = vec![];
    let mut walk = WalkDir::new(home).follow_links(false).into_iter();
    while let Some(entry) = walk.next() {
        if ctl.cancelled() {
            break;
        }
        ctl.tick();
        let Ok(e) = entry else { continue };
        if e.depth() == 0 || !e.file_type().is_dir() {
            continue;
        }
        if let Some(label) = build_kind(e.path()) {
            out.push((e.path().to_path_buf(), label));
            walk.skip_current_dir();
        } else if skip_for_large_files(&e.file_name().to_string_lossy(), e.depth()) {
            walk.skip_current_dir();
        }
    }
    out
}

fn build_folder_items(home: &Path, ctl: &ScanCtl) -> Vec<Candidate> {
    build_folders(home, ctl)
        .into_iter()
        .map(|(dir, label)| {
            let project = dir.parent().unwrap_or(&dir);
            let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let project_name = project.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let location = format!("{label} · {}", tilde(project.parent().unwrap_or(project), home));
            (dir, format!("{project_name}/{folder}"), Some(location))
        })
        .collect()
}

/// Re-checks a folder offered under Project Build Folders before it is moved.
fn is_build_folder(dir: &Path, home: &Path) -> bool {
    let Ok(rel) = dir.strip_prefix(home) else { return false };
    let names: Vec<String> = rel.iter().map(|n| n.to_string_lossy().into_owned()).collect();
    let Some((_, parents)) = names.split_last() else { return false };
    if parents.iter().enumerate().any(|(i, n)| skip_for_large_files(n, i + 1)) {
        return false;
    }
    // `reached_through_symlink` also refuses a build folder that is itself a link.
    build_kind(dir).is_some() && is_real_dir(dir) && !inside_build_folder(dir, home) && !reached_through_symlink(dir, home)
}

/// An installer directly in ~/Downloads is already offered by "Installers in Downloads".
pub(crate) fn is_listed_installer(path: &Path, home: &Path) -> bool {
    path.parent() == Some(home.join("Downloads").as_path()) && has_extension(path, INSTALLER_EXTS)
}

/// Large Files items: a large file inside its own folder is offered as that folder (so the
/// whole download can go), files loose in a top-level folder like ~/Downloads stay files.
/// Folders that sit side by side and are parts of one thing (the numbered chapters of a
/// course, the seasons of a show) are offered as the one folder holding them, and never
/// further up than that. Unrelated neighbours, like the downloads in a film folder, stay apart.
fn large_file_items(home: &Path, min_size: u64, ctl: &ScanCtl) -> Vec<Candidate> {
    let files = large_files(home, min_size, ctl);

    // Folders that directly contain a large file.
    let folders = outermost(files.iter().filter_map(|(p, _)| group_folder(p, home)).collect());
    // One step up: two or more of them in the same folder become that folder, when their
    // names show they belong together.
    let shared = |f: &PathBuf| {
        let parent = f.parent().filter(|p| can_group(p, home))?;
        let siblings: Vec<&PathBuf> = folders.iter().filter(|o| o.parent() == Some(parent)).collect();
        is_series(&siblings).then(|| parent.to_path_buf())
    };
    let tops = outermost(folders.iter().map(|f| shared(f).unwrap_or_else(|| f.clone())).collect());

    let mut counts = vec![0usize; tops.len()];
    let mut out = vec![];
    for (path, name) in files {
        match tops.iter().position(|t| path.starts_with(t)) {
            Some(i) => counts[i] += 1,
            None => out.push((path.clone(), name, None)),
        }
    }
    for (dir, n) in tops.into_iter().zip(counts) {
        let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let parent = tilde(dir.parent().unwrap_or(&dir), home);
        let location = format!("{n} large {} · {parent}", if n == 1 { "file" } else { "files" });
        out.push((dir, name, Some(location)));
    }
    out
}

/// Drops every folder that lies inside another one of the list.
fn outermost(mut folders: Vec<PathBuf>) -> Vec<PathBuf> {
    folders.sort();
    folders.dedup();
    let mut tops: Vec<PathBuf> = vec![];
    for f in folders {
        if !tops.iter().any(|t| f.starts_with(t)) {
            tops.push(f);
        }
    }
    tops
}

/// True for two or more folders named as parts of one thing: every name starts with a
/// number ("1. Introduction", "10 - Indexing"), or the names differ only in their digits
/// ("Season 1", "Season 2").
fn is_series(folders: &[&PathBuf]) -> bool {
    let names: Vec<String> = folders
        .iter()
        .filter_map(|f| f.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    if names.len() < 2 || names.len() != folders.len() {
        return false;
    }
    let numbered = names.iter().all(|n| n.starts_with(|c: char| c.is_ascii_digit()));
    let without_digits = |n: &String| n.chars().filter(|c| !c.is_ascii_digit()).collect::<String>();
    let same_but_digits = names.iter().all(|n| n.chars().any(|c| c.is_ascii_digit()) && without_digits(n) == without_digits(&names[0]));
    numbered || same_but_digits
}

/// A folder may be offered whole unless it is the home folder, a top-level folder such as
/// ~/Downloads, or a code project.
fn can_group(dir: &Path, home: &Path) -> bool {
    let depth = dir.strip_prefix(home).map(|rel| rel.components().count()).unwrap_or(0);
    depth >= 2 && !dir.join(".git").exists()
}

/// The folder a large file is offered through, if any: its own folder, when that may be
/// offered whole.
fn group_folder(file: &Path, home: &Path) -> Option<PathBuf> {
    file.parent().filter(|dir| can_group(dir, home)).map(Path::to_path_buf)
}

/// Re-checks a folder offered under Large Files before it is moved.
fn is_large_file_folder(dir: &Path, home: &Path, min_size: u64) -> bool {
    let Ok(rel) = dir.strip_prefix(home) else { return false };
    let names: Vec<String> = rel.iter().map(|n| n.to_string_lossy().into_owned()).collect();
    if names.len() < 2 || names.iter().enumerate().any(|(i, n)| skip_for_large_files(n, i + 1)) {
        return false;
    }
    if !is_real_dir(dir) || dir.join(".git").exists() {
        return false;
    }
    if build_kind(dir).is_some() || inside_build_folder(dir, home) {
        return false;
    }
    let mut parent = dir.parent();
    while let Some(d) = parent {
        if d == home {
            break;
        }
        if is_symlink(d) {
            return false;
        }
        parent = d.parent();
    }
    // Either a large file sits right in the folder, or the folders in it that hold one are
    // parts of one thing.
    let holds_large_file = |d: &Path| {
        fs::read_dir(d)
            .map(|entries| entries.filter_map(Result::ok).any(|e| is_large_file(&e.path(), home, min_size)))
            .unwrap_or(false)
    };
    holds_large_file(dir)
        || fs::read_dir(dir)
            .map(|entries| {
                let parts: Vec<PathBuf> = entries
                    .filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| is_real_dir(p) && can_group(p, home) && holds_large_file(p))
                    .collect();
                is_series(&parts.iter().collect::<Vec<_>>())
            })
            .unwrap_or(false)
}

fn large_files(home: &Path, min_size: u64, ctl: &ScanCtl) -> Vec<(PathBuf, String)> {
    WalkDir::new(home)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || !(skip_for_large_files(&e.file_name().to_string_lossy(), e.depth()) || build_kind(e.path()).is_some())
        })
        .take_while(|_| {
            ctl.tick();
            !ctl.cancelled()
        })
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.metadata().map(|m| m.blocks() * 512 >= min_size).unwrap_or(false))
        .filter(|e| !is_listed_installer(e.path(), home))
        .map(|e| (e.path().to_path_buf(), e.file_name().to_string_lossy().into_owned()))
        .collect()
}

/// The same rules as the large-file scan, checked again for one path before it is moved.
fn is_large_file(path: &Path, home: &Path, min_size: u64) -> bool {
    let Ok(rel) = path.strip_prefix(home) else { return false };
    let names: Vec<String> = rel.iter().map(|n| n.to_string_lossy().into_owned()).collect();
    let Some((file_name, folders)) = names.split_last() else { return false };
    if file_name.starts_with('.') || is_listed_installer(path, home) {
        return false;
    }
    if folders.iter().enumerate().any(|(i, n)| skip_for_large_files(n, i + 1)) {
        return false;
    }
    // Files inside a build folder go with that folder.
    if inside_build_folder(path, home) {
        return false;
    }
    // No symlinked folder between the home folder and the file.
    let mut dir = path.parent();
    while let Some(d) = dir {
        if d == home {
            break;
        }
        if is_symlink(d) {
            return false;
        }
        dir = d.parent();
    }
    fs::symlink_metadata(path)
        .map(|m| m.is_file() && m.blocks() * 512 >= min_size)
        .unwrap_or(false)
}

/// True when `path` is an offered item, or lives inside one.
///
/// For paths inside an item, every folder between the item and the path must be a
/// real folder, not a symlink: a link could point outside the cleanable area.
/// The same holds for every folder between the home folder and the item's root.
pub fn is_allowed(path: &Path) -> bool {
    is_allowed_in(path, &home_dir())
}

fn is_allowed_in(path: &Path, home: &Path) -> bool {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return false;
    }
    let sources = sources(home);
    let mut current = path;
    loop {
        if is_item(&sources, current, home) {
            return current == path || !whole_only(current, home);
        }
        match current.parent() {
            Some(parent) if !is_symlink(parent) => current = parent,
            _ => return false,
        }
    }
}

/// True when the folder holding `path` is where its text says it is: resolving every
/// symlink below the home folder leaves it unchanged. This is what the Trash call
/// resolves too, so a folder swapped for a link since the scan is caught here.
fn parent_resolves_in_place(path: &Path, home: &Path) -> bool {
    let Some(parent) = path.parent() else { return false };
    let Ok(rel) = parent.strip_prefix(home) else { return false };
    match (fs::canonicalize(parent), fs::canonicalize(home)) {
        (Ok(real_parent), Ok(real_home)) => real_parent == real_home.join(rel),
        _ => false,
    }
}

/// The last check before a path is moved to the Trash.
fn safe_to_move(path: &Path, home: &Path) -> bool {
    is_allowed_in(path, home) && parent_resolves_in_place(path, home)
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
            size: disk_size(&p, None),
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

    let home = home_dir();
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
        let app = crate::apps::is_listed_app(&path, &home);
        if app && crate::apps::is_running(&path) {
            report.failures.push(fail("The app is open. Quit it, then clean again."));
            continue;
        }
        // Collected before the app goes, while its bundle can still name them.
        let support = if app { crate::apps::support_files(&path, &home) } else { vec![] };
        // Measuring a big folder takes a while, and its owner could swap a folder on the
        // way to `path` for a link in that time. So check again right before the move.
        let size = disk_size(&path, None);
        let moved_out_of_home = app && !path.starts_with(&home);
        if !(if moved_out_of_home { is_allowed(&path) } else { safe_to_move(&path, &home) }) {
            report.failures.push(fail("It changed while cleaning, so it was left alone."));
            continue;
        }
        if crate::duplicates::is_offered_copy(&path, &home) && !crate::duplicates::still_identical(&path) {
            report.failures.push(fail("It no longer matches its other copy, so it was left alone."));
            continue;
        }
        match ctx.delete(&path) {
            Ok(()) => {
                report.removed += 1;
                report.freed += size;
            }
            Err(e) => {
                report.failures.push(fail(&friendly_error(&e.to_string())));
                continue;
            }
        }
        // The app is gone; its files in ~/Library follow. Each one is checked like any item.
        for file in support {
            let size = disk_size(&file, None);
            if !(crate::apps::is_support_dir_entry(&file, &home) && parent_resolves_in_place(&file, &home)) {
                continue;
            }
            match ctx.delete(&file) {
                Ok(()) => report.freed += size,
                Err(e) => report.failures.push(Failure {
                    path: file.to_string_lossy().into_owned(),
                    reason: friendly_error(&e.to_string()),
                }),
            }
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
    #[test]
    fn disk_space_is_read_and_consistent() {
        let t = std::time::Instant::now();
        let d = disk_space().expect("home volume storage");
        let ms = t.elapsed().as_millis();
        assert!(ms < 2_000, "reading storage took {ms} ms");
        assert!(d.total > 0);
        assert!(d.free <= d.total);
        assert!(!d.name.is_empty());
    }

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

    #[test]
    fn refuses_roots_reached_through_a_symlink() {
        // Uses a temporary folder as the "home" so nothing real is touched.
        // ~/Library/Logs is a link to private data, and ~/.npm is a link to a folder
        // that happens to hold a _cacache: neither may be scanned or cleaned.
        let home = std::env::temp_dir().join(format!("mac-cleaner-linked-root-{}", std::process::id()));
        let write = |rel: &str| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, vec![1u8; 64 * 1024]).unwrap();
            p
        };
        write("Private/Mail/inbox.mbox");
        write("Elsewhere/_cacache/blob");
        write("Library/Caches/com.example.app/data.db");
        std::os::unix::fs::symlink(home.join("Private"), home.join("Library/Logs")).unwrap();
        std::os::unix::fs::symlink(home.join("Elsewhere"), home.join(".npm")).unwrap();
        fs::create_dir_all(home.join(".gradle")).unwrap();
        std::os::unix::fs::symlink(home.join("Private"), home.join(".gradle/caches")).unwrap();

        let offered: Vec<PathBuf> = sources(&home)
            .iter()
            .flat_map(|s| &s.roots)
            // The Applications roots are the real /Applications, outside this test's home.
            .filter(|r| r.path.starts_with(&home))
            .flat_map(|r| candidates(r, &home, &ScanCtl::default()))
            .map(|(p, _, _)| p.strip_prefix(&home).unwrap().to_path_buf())
            .collect();
        let checks = (
            is_allowed_in(&home.join("Library/Logs/Mail"), &home),
            is_allowed_in(&home.join("Library/Logs/Mail/inbox.mbox"), &home),
            is_allowed_in(&home.join(".npm/_cacache"), &home),
            is_allowed_in(&home.join(".npm/_cacache/blob"), &home),
            is_allowed_in(&home.join(".gradle/caches"), &home),
            is_allowed_in(&home.join("Library/Caches/com.example.app"), &home),
        );
        fs::remove_dir_all(&home).unwrap();

        assert_eq!(offered, [PathBuf::from("Library/Caches/com.example.app")], "linked roots offer nothing");
        assert!(!checks.0, "children of a symlinked root are refused");
        assert!(!checks.1, "paths deeper inside a symlinked root are refused");
        assert!(!checks.2, "a root below a symlinked folder is refused");
        assert!(!checks.3, "paths inside a root below a symlinked folder are refused");
        assert!(!checks.4, "a root that is itself a symlink is refused");
        assert!(checks.5, "real roots still work");
    }

    #[test]
    fn refuses_a_folder_swapped_for_a_symlink_before_the_move() {
        // The path is approved, then a folder on the way to it is replaced by a link
        // to private data holding a file of the same name.
        let home = std::env::temp_dir().join(format!("mac-cleaner-swap-{}", std::process::id()));
        let write = |rel: &str| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, vec![1u8; 64 * 1024]).unwrap();
            p
        };
        let target = write("Library/Caches/com.example.app/sub/x");
        let private = write("Private/x");
        let item = home.join("Library/Caches/com.example.app");

        let before = (safe_to_move(&target, &home), safe_to_move(&item, &home));
        fs::remove_dir_all(home.join("Library/Caches/com.example.app/sub")).unwrap();
        std::os::unix::fs::symlink(home.join("Private"), home.join("Library/Caches/com.example.app/sub")).unwrap();
        let after = (safe_to_move(&target, &home), safe_to_move(&item, &home));
        // Swapping the root itself is caught as well.
        fs::rename(home.join("Library/Caches"), home.join("Library/Caches.real")).unwrap();
        std::os::unix::fs::symlink(home.join("Private"), home.join("Library/Caches")).unwrap();
        let root_swapped = safe_to_move(&home.join("Library/Caches/x"), &home);
        let private_intact = private.exists();
        fs::remove_dir_all(&home).unwrap();

        assert!(before.0 && before.1, "untouched paths can be moved");
        assert!(!after.0, "a path behind a swapped-in link is refused");
        assert!(after.1, "the item above the link can still be moved (takes only the link)");
        assert!(!root_swapped, "a swapped root is refused");
        assert!(private_intact);
    }

    #[test]
    fn large_file_rules() {
        // Uses a temporary folder as the "home" so nothing real is touched.
        let home = std::env::temp_dir().join(format!("mac-cleaner-home-{}", std::process::id()));
        let write = |rel: &str| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, vec![1u8; 64 * 1024]).unwrap();
            p
        };
        let ok = write("Movies/film.mov");
        let in_library = write("Library/Mail/big.mbox");
        let hidden = write("code/.git/objects/pack.pack");
        let in_package = write("Pictures/Photos Library.photoslibrary/originals/a.heic");
        let in_node_modules = write("code/app/node_modules/x/blob.bin");
        let installer = write("Downloads/Setup.dmg");
        let nested_installer = write("Downloads/old/Setup.dmg");
        std::os::unix::fs::symlink(home.join("Movies"), home.join("link")).unwrap();

        let min = 1;
        let results = (
            is_large_file(&ok, &home, min),
            is_large_file(&ok, &home, u64::MAX),
            is_large_file(&in_library, &home, min),
            is_large_file(&hidden, &home, min),
            is_large_file(&in_package, &home, min),
            is_large_file(&in_node_modules, &home, min),
            is_large_file(&installer, &home, min),
            is_large_file(&nested_installer, &home, min),
            is_large_file(&home.join("link/film.mov"), &home, min),
            is_large_file(&home.join("Movies"), &home, min),
        );
        fs::remove_dir_all(&home).unwrap();

        assert!(results.0, "a big file in Movies is offered");
        assert!(!results.1, "files under the size limit are refused");
        assert!(!results.2, "~/Library is never touched");
        assert!(!results.3, "hidden folders such as .git are skipped");
        assert!(!results.4, "files inside packages like Photos libraries are skipped");
        assert!(!results.5, "node_modules is skipped");
        assert!(!results.6, "installers in Downloads belong to their own category");
        assert!(results.7, "installers deeper in Downloads are ordinary large files");
        assert!(!results.8, "paths through a symlinked folder are refused");
        assert!(!results.9, "folders are not large files");
    }

    #[test]
    fn large_files_are_offered_through_their_folder() {
        let home = std::env::temp_dir().join(format!("mac-cleaner-group-{}", std::process::id()));
        let write = |rel: &str, kb: usize| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, vec![1u8; kb * 1024]).unwrap();
            p
        };
        let min = 256 * 1024;
        write("Downloads/loose.mkv", 512);                    // loose in a top-level folder
        write("Downloads/Show S01/e01.mkv", 512);             // grouped
        write("Downloads/Show S01/e02.mkv", 512);
        write("Downloads/Show S01/subs.srt", 4);              // small sibling, comes with the folder
        write("Downloads/Show S01/Extras/making-of.mkv", 512); // nested: merged into the outer folder
        write("Documents/repo/data.bin", 512);                // code project: file only
        fs::create_dir_all(home.join("Documents/repo/.git")).unwrap();
        write("Movies/Trip/clip.mov", 64);                    // too small: nothing offered
        write("Downloads/Course/1 Intro/a.mp4", 512);         // side by side: offered as Course
        write("Downloads/Course/2 Basics/b.mp4", 512);
        write("Downloads/Course/3 More/Deep/c.mp4", 512);     // deeper: comes with Course
        write("Downloads/Wrap/Inner/Only/d.mp4", 512);        // alone: stays its own folder
        write("stuff/1 a/x.bin", 512);                        // would merge into a top-level folder
        write("stuff/2 b/y.bin", 512);
        write("Downloads/Show/Season 1/e.mkv", 512);          // differ only in digits: offered as Show
        write("Downloads/Show/Season 2/e.mkv", 512);
        write("Downloads/Films/Heat 1995/f.mkv", 512);        // unrelated neighbours stay apart
        write("Downloads/Films/Alien/f.mkv", 512);

        let mut items: Vec<(String, Option<String>)> = large_file_items(&home, min, &ScanCtl::default())
            .into_iter()
            .map(|(p, _, loc)| (p.strip_prefix(&home).unwrap().display().to_string(), loc))
            .collect();
        items.sort();

        let show = home.join("Downloads/Show S01");
        let checks = (
            is_large_file_folder(&show, &home, min),
            is_large_file_folder(&home.join("Downloads"), &home, min),
            is_large_file_folder(&home.join("Documents/repo"), &home, min),
            is_large_file_folder(&home.join("Movies/Trip"), &home, min),
            is_large_file_folder(&home.join("Downloads/Course"), &home, min),
            is_large_file_folder(&home.join("Downloads/Wrap/Inner"), &home, min),
            is_large_file_folder(&home.join("stuff"), &home, min),
            is_large_file_folder(&home.join("Downloads/Show"), &home, min),
            is_large_file_folder(&home.join("Downloads/Films"), &home, min),
        );
        fs::remove_dir_all(&home).unwrap();

        let paths: Vec<&str> = items.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(
            paths,
            [
                "Documents/repo/data.bin",
                "Downloads/Course",
                "Downloads/Films/Alien",
                "Downloads/Films/Heat 1995",
                "Downloads/Show",
                "Downloads/Show S01",
                "Downloads/Wrap/Inner/Only",
                "Downloads/loose.mkv",
                "stuff/1 a",
                "stuff/2 b",
            ]
        );
        assert!(items[1].1.as_deref().unwrap().starts_with("3 large files · "), "files of every chapter are counted");
        assert!(items[5].1.as_deref().unwrap().starts_with("3 large files · "), "nested files are counted");
        assert!(checks.7, "seasons of a show are cleaned as the show");
        assert!(!checks.8, "a folder of unrelated folders is refused");
        assert!(checks.4, "a folder of folders holding large files can be cleaned");
        assert!(!checks.5, "a folder holding just one such folder is refused");
        assert!(!checks.6, "folders are never merged into a top-level folder");
        assert!(checks.0, "a folder holding a large file can be cleaned");
        assert!(!checks.1, "top-level folders like Downloads are never offered whole");
        assert!(!checks.2, "code projects are never offered whole");
        assert!(!checks.3, "folders without a large file are refused");
    }

    #[test]
    fn build_folders_need_their_project_file() {
        let home = std::env::temp_dir().join(format!("mac-cleaner-build-{}", std::process::id()));
        let write = |rel: &str, kb: usize| {
            let p = home.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, vec![1u8; kb * 1024]).unwrap();
            p
        };
        write("code/web/package.json", 1);
        write("code/web/node_modules/left-pad/index.js", 1);
        write("code/web/node_modules/left-pad/package.json", 1);
        write("code/web/node_modules/left-pad/node_modules/dep/index.js", 1); // nested: not listed twice
        write("code/tool/Cargo.toml", 1);
        let big = write("code/tool/target/release/tool", 512);
        write("Documents/target/notes.txt", 1);               // no Cargo.toml next to it
        write("Library/x/package.json", 1);
        write("Library/x/node_modules/a/index.js", 1);        // ~/Library is never entered
        write("code/.hidden/app/package.json", 1);
        write("code/.hidden/app/node_modules/a/index.js", 1); // hidden folders are never entered
        write("code/linked/package.json", 1);
        std::os::unix::fs::symlink(home.join("Documents"), home.join("code/linked/node_modules")).unwrap();
        std::os::unix::fs::symlink(home.join("code"), home.join("link")).unwrap();

        let mut found: Vec<(String, String)> = build_folder_items(&home, &ScanCtl::default())
            .into_iter()
            .map(|(p, name, _)| (p.strip_prefix(&home).unwrap().display().to_string(), name))
            .collect();
        found.sort();
        let checks = (
            is_allowed_in(&home.join("code/web/node_modules"), &home),
            is_allowed_in(&home.join("code/tool/target/release/tool"), &home),
            is_build_folder(&home.join("Documents/target"), &home),
            is_build_folder(&home.join("Library/x/node_modules"), &home),
            is_build_folder(&home.join("code/web/node_modules/left-pad/node_modules"), &home),
            is_build_folder(&home.join("link/web/node_modules"), &home),
            is_allowed_in(&home.join("code/web"), &home),
            is_large_file(&big, &home, 1),
            is_large_file_folder(&home.join("code/tool/target/release"), &home, 1),
            is_build_folder(&home.join("code/linked/node_modules"), &home),
        );
        fs::remove_dir_all(&home).unwrap();

        assert_eq!(
            found,
            [
                ("code/tool/target".to_string(), "tool/target".to_string()),
                ("code/web/node_modules".to_string(), "web/node_modules".to_string()),
            ]
        );
        assert!(checks.0, "an offered build folder passes the re-check");
        assert!(checks.1, "so does a file inside it");
        assert!(!checks.2, "a folder named target without Cargo.toml is not a build folder");
        assert!(!checks.3, "~/Library is never touched");
        assert!(!checks.4, "folders inside a build folder are not offered on their own");
        assert!(!checks.5, "paths through a symlinked folder are refused");
        assert!(!checks.6, "the project itself is never offered");
        assert!(!checks.7, "big files inside a build folder are not Large Files");
        assert!(!checks.8, "folders inside a build folder are not Large Files folders");
        assert!(!checks.9, "a build folder that is a symlink is refused");
    }

    #[test]
    fn names_simulator_runtimes() {
        assert_eq!(runtime_label("com.apple.CoreSimulator.SimRuntime.iOS-17-2"), "iOS 17.2");
        assert_eq!(runtime_label("com.apple.CoreSimulator.SimRuntime.xrOS-2-0"), "xrOS 2.0");
        assert_eq!(runtime_label("weird"), "weird");
    }

    /// Read-only: walks the real home folder and prints what would be offered.
    #[test]
    #[ignore]
    fn real_scan_summary() {
        let start = std::time::Instant::now();
        let cats = scan_all(&HashSet::new(), &ScanCtl::default());
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

