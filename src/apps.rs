//! Installed apps, the files they keep in ~/Library, and files left behind by apps that are gone.
//!
//! Files are tied to an app by its bundle identifier (`com.vendor.App`), which is how macOS
//! names an app's preferences, containers and saved state. Leftovers are judged cautiously:
//! only reverse-DNS names count, nothing from Apple, nothing from a vendor that still has an
//! app installed, and nothing changed in the last 30 days.

use crate::scanner::{is_real_dir, is_symlink, modified_ms, reached_through_symlink, tilde};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const OWN_ID: &str = "com.maccleaner.desktop";

/// Identifier prefixes macOS uses for its own parts ("is.workflow" is Shortcuts, from before Apple
/// bought it; "org.cups" is printing).
const APPLE_PREFIXES: &[&str] = &["com.apple.", "is.workflow.", "org.cups."];

/// Folders in ~/Library where apps keep files named after their bundle identifier. These are
/// searched for leftovers of deleted apps. (Caches and Logs have categories of their own.)
pub const LEFTOVER_DIRS: &[&str] = &[
    "Library/Application Support",
    "Library/Containers",
    "Library/Group Containers",
    "Library/Preferences",
    "Library/Saved Application State",
    "Library/HTTPStorages",
    "Library/WebKit",
    "Library/Cookies",
    "Library/Application Scripts",
];

/// Where an installed app's own files are looked for when it is removed.
const SUPPORT_DIRS: &[&str] = &[
    "Library/Application Support",
    "Library/Containers",
    "Library/Group Containers",
    "Library/Preferences",
    "Library/Saved Application State",
    "Library/HTTPStorages",
    "Library/WebKit",
    "Library/Cookies",
    "Library/Application Scripts",
    "Library/Caches",
    "Library/Logs",
    "Library/LaunchAgents",
];

/// Support folders where an app may also use its plain name ("Spotify") instead of its identifier.
const NAMED_DIRS: &[&str] = &["Library/Application Support", "Library/Caches", "Library/Logs"];

/// Leftovers touched more recently than this are probably still in use by something.
const RECENT: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// /Applications and ~/Applications, plus the plain folders directly inside them ("Setapp").
pub fn app_folders(home: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for top in [PathBuf::from("/Applications"), home.join("Applications")] {
        if !is_real_dir(&top) {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&top) {
            out.extend(entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| is_real_dir(p) && !has_app_extension(p)));
        }
        out.push(top);
    }
    out
}

fn has_app_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("app")).unwrap_or(false)
}

/// A real application: an `.app` folder (not a link to one) with an `Info.plist`. Plenty of
/// cache folders are named like `com.vendor.app`, so the extension alone proves nothing.
pub fn is_app_bundle(path: &Path) -> bool {
    has_app_extension(path) && is_real_dir(path) && path.join("Contents/Info.plist").is_file()
}

/// An app sitting directly in one of the Applications folders: the only apps Mac Cleaner offers.
pub fn is_listed_app(path: &Path, home: &Path) -> bool {
    is_app_bundle(path) && path.parent().map(|p| app_folders(home).iter().any(|f| f == p)).unwrap_or(false)
}

fn bundle_id(app: &Path) -> Option<String> {
    crate::native::bundle_identifier(app)
}

fn app_name(app: &Path) -> String {
    app.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0)
        .unwrap_or(false)
}

/// An app directly in `folder` that this account can move to the Trash without a password,
/// that is not part of macOS, and that is not Mac Cleaner itself.
pub fn is_removable_app(path: &Path, folder: &Path) -> bool {
    if path.parent() != Some(folder) || !is_app_bundle(path) || reached_through_symlink(path, Path::new("/")) {
        return false;
    }
    // The folder must be where its name says, so a swapped-in link can't redirect the move.
    if fs::canonicalize(folder).ok().as_deref() != Some(folder) {
        return false;
    }
    let Some(id) = bundle_id(path) else { return false };
    !id.starts_with("com.apple.") && id != OWN_ID && writable(path) && writable(folder)
}

/// Apps offered under Applications: (path, name, location).
pub fn app_items(folder: &Path, home: &Path) -> Vec<(PathBuf, String, Option<String>)> {
    let Ok(entries) = fs::read_dir(folder) else { return vec![] };
    let place = tilde(folder, home);
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| is_removable_app(p, folder))
        .map(|app| {
            let n = support_files(&app, home).len();
            let location = match n {
                0 => place.clone(),
                1 => format!("1 support file · {place}"),
                n => format!("{n} support files · {place}"),
            };
            let name = app_name(&app);
            (app, name, Some(location))
        })
        .collect()
}

/// The part of a support file's name that is a bundle identifier: without the file
/// extension macOS adds, the `group.` prefix, or a team-ID prefix (`ABCDE12345.`).
fn identifier_in(name: &str) -> &str {
    let mut id = name;
    for ext in [".plist", ".savedState", ".binarycookies"] {
        if let Some(stripped) = id.strip_suffix(ext) {
            id = stripped;
        }
    }
    // The prefixes come in either order and sometimes twice ("group.TEAMID12345.group.com.x").
    loop {
        if let Some(stripped) = id.strip_prefix("group.").or_else(|| id.strip_prefix("groups.")) {
            id = stripped;
            continue;
        }
        match id.split_once('.') {
            Some((team, rest)) if team.len() == 10 && team.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) => id = rest,
            _ => return id,
        }
    }
}

/// `com.vendor.App` and longer, made of the characters identifiers use.
fn is_reverse_dns(id: &str) -> bool {
    let parts: Vec<&str> = id.split('.').collect();
    parts.len() >= 3
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        && (2..=6).contains(&parts[0].len())
        && parts[0].chars().all(|c| c.is_ascii_lowercase())
}

/// `id` belongs to the app `app_id`: the same, or one of its helpers and extensions.
fn same_app(id: &str, app_id: &str) -> bool {
    let (id, app_id) = (id.to_ascii_lowercase(), app_id.to_ascii_lowercase());
    id == app_id || id.starts_with(&format!("{app_id}.")) || app_id.starts_with(&format!("{id}."))
}

fn vendor(id: &str) -> String {
    id.split('.').take(2).collect::<Vec<_>>().join(".").to_ascii_lowercase()
}

/// Files in ~/Library that belong to the app at `app`, found by its bundle identifier
/// (and, in a few folders, by its name).
pub fn support_files(app: &Path, home: &Path) -> Vec<PathBuf> {
    let Some(id) = bundle_id(app) else { return vec![] };
    let name = app_name(app);
    let mut out = vec![];
    for dir in SUPPORT_DIRS {
        let root = home.join(dir);
        if reached_through_symlink(&root, home) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&root) else { continue };
        let by_name = NAMED_DIRS.contains(dir) && name.len() >= 3;
        for entry in entries.filter_map(Result::ok) {
            let file = entry.file_name().to_string_lossy().into_owned();
            if same_app(identifier_in(&file), &id) || (by_name && file == name) {
                out.push(entry.path());
            }
        }
    }
    out
}

/// True when `file` sits directly in one of the folders an app's support files come from.
pub fn is_support_dir_entry(file: &Path, home: &Path) -> bool {
    let Some(parent) = file.parent() else { return false };
    file.file_name().is_some()
        && SUPPORT_DIRS.iter().any(|d| parent == home.join(d))
        && !reached_through_symlink(parent, home)
        && !is_symlink(file)
}

/// Bundle identifiers of installed and running apps, refreshed every few seconds.
struct Installed {
    ids: Vec<String>,
    vendors: HashSet<String>,
}

fn installed() -> Arc<Installed> {
    static CACHE: Mutex<Option<(Instant, Arc<Installed>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    if let Some((at, set)) = cache.as_ref() {
        if at.elapsed() < Duration::from_secs(10) {
            return set.clone();
        }
    }
    let home = crate::scanner::home_dir();
    let mut apps: Vec<PathBuf> = vec![];
    let mut folders = app_folders(&home);
    folders.extend([PathBuf::from("/System/Applications"), PathBuf::from("/System/Applications/Utilities")]);
    for folder in folders {
        if let Ok(entries) = fs::read_dir(&folder) {
            apps.extend(entries.filter_map(Result::ok).map(|e| e.path()).filter(|p| has_app_extension(p)));
        }
    }
    // Apps running from anywhere else (a disk image, ~/Downloads) count as installed too.
    apps.extend(crate::native::running_executables().iter().filter_map(|exe| enclosing_app(exe)));
    let ids: Vec<String> = apps.iter().filter_map(|a| bundle_id(a)).collect();
    let vendors = ids.iter().map(|id| vendor(id)).collect();
    let set = Arc::new(Installed { ids, vendors });
    *cache = Some((Instant::now(), set.clone()));
    set
}

/// The outermost `.app` folder a path is in.
fn enclosing_app(path: &Path) -> Option<PathBuf> {
    path.ancestors().filter(|p| has_app_extension(p)).last().map(Path::to_path_buf)
}

/// A support-file identifier that no installed app (or app from the same vendor) claims.
fn is_orphan_id(id: &str, installed: &Installed) -> bool {
    is_reverse_dns(id)
        && !APPLE_PREFIXES.iter().any(|p| id.to_ascii_lowercase().starts_with(p))
        && !id.to_ascii_lowercase().contains("com.apple.")
        && id != OWN_ID
        && !installed.vendors.contains(&vendor(id))
        && !installed.ids.iter().any(|app| same_app(id, app))
}

/// The newest change time of `path` and, for a folder, of what is directly inside it.
fn last_touched(path: &Path) -> Option<u64> {
    let own = modified_ms(path);
    let inner = fs::read_dir(path)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| modified_ms(&e.path()))
        .max();
    own.max(inner)
}

fn recently_used(path: &Path) -> bool {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    last_touched(path).map(|t| now.saturating_sub(t) < RECENT.as_millis() as u64).unwrap_or(true)
}

/// A file or folder named after an app that is gone, and left alone for at least 30 days.
pub fn is_leftover(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
    !is_symlink(path) && is_orphan_id(identifier_in(&name), &installed()) && !recently_used(path)
}

/// Leftovers directly in one support folder: (path, name, location).
pub fn leftover_items(root: &Path, _home: &Path) -> Vec<(PathBuf, String, Option<String>)> {
    let Ok(entries) = fs::read_dir(root) else { return vec![] };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| is_leftover(p))
        .map(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            (p, name, None)
        })
        .collect()
}

/// True when macOS would refuse to move `path` to the Trash for this account: it is locked
/// (`chflags uchg`, or the system's own immutable / restricted flags), or the folder it sits in,
/// or the folder itself, can't be written to.
pub fn is_protected(path: &Path) -> bool {
    use std::os::macos::fs::MetadataExt;
    const LOCKED: u32 = 0x0000_0002 /* UF_IMMUTABLE */ | 0x0002_0000 /* SF_IMMUTABLE */ | 0x0008_0000 /* SF_RESTRICTED */;
    let Ok(meta) = fs::symlink_metadata(path) else { return false };
    let locked = meta.st_flags() & LOCKED != 0;
    let parent_ok = path.parent().is_some_and(writable);
    // Moving a folder rewrites its link to the parent, which needs write access to the folder.
    let self_ok = !meta.is_dir() || writable(path);
    locked || !parent_ok || !self_ok
}

/// True when a program inside the app is running.
pub fn is_running(app: &Path) -> bool {
    crate::native::running_executables().iter().any(|exe| exe.starts_with(app))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locked_items_are_protected() {
        let dir = std::env::temp_dir().join(format!("mc-protected-{}", std::process::id()));
        let item = dir.join("com.example.Gone");
        fs::create_dir_all(&item).unwrap();
        assert!(!is_protected(&item), "a plain folder can be moved");
        let lock = |flag: &str| std::process::Command::new("/usr/bin/chflags").arg(flag).arg(&item).status().unwrap();
        assert!(lock("uchg").success());
        let locked = is_protected(&item);
        assert!(lock("nouchg").success());
        fs::remove_dir_all(&dir).unwrap();
        assert!(locked, "a locked folder can't be moved");
    }

    #[test]
    fn identifiers_are_read_from_support_file_names() {
        assert_eq!(identifier_in("com.spotify.client.plist"), "com.spotify.client");
        assert_eq!(identifier_in("com.figma.Desktop.savedState"), "com.figma.Desktop");
        assert_eq!(identifier_in("group.com.docker"), "com.docker");
        assert_eq!(identifier_in("UBF8T346G9.com.microsoft.teams"), "com.microsoft.teams");
        assert_eq!(identifier_in("Spotify"), "Spotify");
        assert_eq!(identifier_in("W8F64X92K3.group.com.grammarly.ext"), "com.grammarly.ext");
        assert_eq!(identifier_in("group.6MR872QP3J.group.com.gentlemencoders.RAWPower"), "com.gentlemencoders.RAWPower");
    }

    #[test]
    fn only_reverse_dns_names_count() {
        assert!(is_reverse_dns("com.spotify.client"));
        assert!(is_reverse_dns("org.videolan.vlc"));
        assert!(!is_reverse_dns("com.spotify"));
        assert!(!is_reverse_dns("Google"));
        assert!(!is_reverse_dns("My App 2.0.1"));
        assert!(!is_reverse_dns("UBF8T346G9.Office.x"));
    }

    #[test]
    fn helpers_and_extensions_belong_to_their_app() {
        assert!(same_app("com.spotify.client", "com.spotify.client"));
        assert!(same_app("com.spotify.client.helper", "com.spotify.client"));
        assert!(same_app("com.Spotify.Client", "com.spotify.client"));
        assert!(!same_app("com.spotify.clientx", "com.spotify.client"));
    }

    #[test]
    fn orphans_spare_apple_installed_apps_and_their_vendors() {
        let installed = Installed {
            ids: vec!["com.spotify.client".into(), "com.microsoft.VSCode".into()],
            vendors: ["com.spotify", "com.microsoft"].iter().map(|s| s.to_string()).collect(),
        };
        assert!(is_orphan_id("org.videolan.vlc", &installed));
        assert!(!is_orphan_id("com.spotify.client.helper", &installed));
        assert!(!is_orphan_id("com.microsoft.autoupdate2", &installed));
        assert!(!is_orphan_id("com.apple.Safari", &installed));
        assert!(!is_orphan_id("is.workflow.my.app", &installed));
        assert!(!is_orphan_id("org.cups.PrintingPrefs", &installed));
        assert!(!is_orphan_id(identifier_in("243LU875E5.groups.com.apple.podcasts"), &installed));
        assert!(!is_orphan_id("Google", &installed));
    }
}

