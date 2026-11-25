//! Files in the user's own folders that haven't been opened or changed for a long time.
//!
//! A file's last use is the latest of its access time, its modification time, and the dates
//! Spotlight keeps of when it was last opened through Finder or an app and when it was added to its
//! folder (see `spotlight`). The access time alone isn't enough: macOS moves it forward only on the
//! first read after a change, so a file opened often but never edited keeps an old one.
//! Files of at least `MIN_SIZE` not used for `min_days()` are offered.
//! Each offered file is remembered as it was at the scan: before it is moved it must still be the
//! same ordinary file with the same contents. Looking at it in between (Quick Look, say) doesn't
//! take it off the list, but editing or replacing it does.

use crate::scanner::{build_kind, inside_build_folder, is_listed_installer, is_symlink, skip_for_large_files, tilde, ScanCtl};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

/// Smaller files are left out: they free little and would bury the ones worth looking at.
pub const MIN_SIZE: u64 = 1_000_000;

/// Choices offered for how long a file must have gone unused, in days.
pub const DAYS_CHOICES: [u64; 4] = [182, 365, 730, 1095];

static MIN_DAYS: AtomicU64 = AtomicU64::new(365);

pub fn min_days() -> u64 {
    MIN_DAYS.load(Ordering::Relaxed)
}

pub fn set_min_days(days: u64) {
    MIN_DAYS.store(days, Ordering::Relaxed);
}

const DAY_MS: u64 = 86_400_000;

/// What identifies a file as the one the scan offered: device, inode, size and modification time.
type Stamp = (u64, u64, u64, i64, i64);

/// Files offered by the last scan, as they were then.
static OFFERED: Mutex<Option<HashMap<PathBuf, Stamp>>> = Mutex::new(None);

fn stamp(m: &fs::Metadata) -> Stamp {
    (m.dev(), m.ino(), m.len(), m.mtime(), m.mtime_nsec())
}

/// When a file was last opened or changed, in milliseconds since the Unix epoch.
fn last_used_ms(m: &fs::Metadata) -> u64 {
    let ms = |secs: i64, nanos: i64| secs.max(0) as u64 * 1000 + nanos.max(0) as u64 / 1_000_000;
    ms(m.atime(), m.atime_nsec()).max(ms(m.mtime(), m.mtime_nsec()))
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// "6 months", "1 year", "3 years".
pub fn span(days: u64) -> String {
    if days >= 365 {
        let years = days / 365;
        format!("{years} {}", if years == 1 { "year" } else { "years" })
    } else {
        let months = (days / 30).max(1);
        format!("{months} {}", if months == 1 { "month" } else { "months" })
    }
}

/// Old files under `home`: (path, name, location). Also remembers them for `is_offered`.
pub fn old_file_items(home: &Path, ctl: &ScanCtl) -> Vec<(PathBuf, String, Option<String>)> {
    let now = now_ms();
    let cutoff = now.saturating_sub(min_days() * DAY_MS);
    let mut seen = HashSet::new();
    let mut offered = HashMap::new();
    let mut out = vec![];
    let walk = WalkDir::new(home).follow_links(false).into_iter().filter_entry(|e| {
        e.depth() == 0
            || !e.file_type().is_dir()
            || !(skip_for_large_files(&e.file_name().to_string_lossy(), e.depth()) || build_kind(e.path()).is_some())
    });
    for entry in walk {
        if ctl.cancelled() {
            return vec![];
        }
        ctl.tick();
        let Ok(e) = entry else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        if !e.file_type().is_file() || name.starts_with('.') || is_listed_installer(e.path(), home) {
            continue;
        }
        let Ok(m) = e.metadata() else { continue };
        let used = last_used_ms(&m);
        // Hard links are one file with two names: removing one frees nothing.
        if m.blocks() * 512 < MIN_SIZE || used > cutoff || !seen.insert((m.dev(), m.ino())) {
            continue;
        }
        // Asked only now, for the few files the file system's dates call old: it's a lookup each.
        let used = used.max(crate::spotlight::last_activity_ms(e.path()).unwrap_or(0));
        if used > cutoff {
            continue;
        }
        let place = tilde(e.path().parent().unwrap_or(e.path()), home);
        let location = format!("Unused for {} · {place}", span((now - used) / DAY_MS));
        offered.insert(e.path().to_path_buf(), stamp(&m));
        out.push((e.path().to_path_buf(), name, Some(location)));
    }
    *OFFERED.lock().unwrap() = Some(offered);
    out
}

/// True when `path` is a file the last scan offered, still unchanged, in the scanned areas.
pub fn is_offered(path: &Path, home: &Path) -> bool {
    let Some(then) = OFFERED.lock().unwrap().as_ref().and_then(|o| o.get(path).copied()) else { return false };
    let Ok(rel) = path.strip_prefix(home) else { return false };
    let names: Vec<String> = rel.iter().map(|n| n.to_string_lossy().into_owned()).collect();
    let Some((_, folders)) = names.split_last() else { return false };
    if folders.iter().enumerate().any(|(i, n)| skip_for_large_files(n, i + 1)) || inside_build_folder(path, home) {
        return false;
    }
    if path.ancestors().skip(1).take_while(|d| *d != home).any(is_symlink) {
        return false;
    }
    fs::symlink_metadata(path).map(|m| m.is_file() && stamp(&m) == then).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, FileTimes};
    use std::time::Duration;

    fn set_times(path: &Path, accessed_days_ago: u64, modified_days_ago: u64) {
        let ago = |d: u64| SystemTime::now() - Duration::from_secs(d * 86_400);
        let times = FileTimes::new().set_accessed(ago(accessed_days_ago)).set_modified(ago(modified_days_ago));
        File::options().write(true).open(path).unwrap().set_times(times).unwrap();
    }

    #[test]
    fn finds_unused_files_and_rechecks_before_cleaning() {
        let home = std::env::temp_dir().join(format!("mc-old-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        for d in ["Documents/Taxes", "Library/Caches", ".hidden", "Downloads", "code/app/node_modules"] {
            fs::create_dir_all(home.join(d)).unwrap();
        }
        fs::write(home.join("code/app/package.json"), "{}").unwrap();
        let big = vec![7u8; MIN_SIZE as usize + 4096];
        let files = [
            ("Documents/Taxes/2021.pdf", 800, 900),  // old: listed
            ("Documents/read-last-week.pdf", 7, 900), // opened recently
            ("Documents/edited.pdf", 900, 3),         // changed recently
            ("Library/Caches/blob.bin", 900, 900),    // ~/Library is never scanned
            (".hidden/blob.bin", 900, 900),           // hidden folders aren't either
            ("Documents/.secret.bin", 900, 900),      // nor hidden files
            ("Downloads/Old.dmg", 900, 900),          // listed under Installers
            ("code/app/node_modules/x.bin", 900, 900), // part of a build folder
        ];
        for (path, accessed, modified) in files {
            fs::write(home.join(path), &big).unwrap();
            set_times(&home.join(path), accessed, modified);
        }
        fs::write(home.join("Documents/small.txt"), "tiny").unwrap();
        set_times(&home.join("Documents/small.txt"), 900, 900);

        set_min_days(365);
        let items = old_file_items(&home, &ScanCtl::default());
        let paths: Vec<&PathBuf> = items.iter().map(|(p, _, _)| p).collect();
        let old = home.join("Documents/Taxes/2021.pdf");
        assert_eq!(paths, vec![&old]);
        assert_eq!(items[0].2.as_deref(), Some("Unused for 2 years · ~/Documents/Taxes"));
        assert!(is_offered(&old, &home));
        assert!(!is_offered(&home.join("Documents/edited.pdf"), &home), "only what the scan offered");

        // Opened since the scan (Quick Look reads the file): still offered.
        let _ = fs::read(&old);
        File::options().write(true).open(&old).unwrap().set_times(FileTimes::new().set_accessed(SystemTime::now())).unwrap();
        assert!(is_offered(&old, &home));
        // Edited since the scan: refused.
        fs::write(&old, b"new contents").unwrap();
        assert!(!is_offered(&old, &home));

        set_min_days(1095);
        fs::write(&old, &big).unwrap();
        set_times(&old, 800, 900);
        assert!(old_file_items(&home, &ScanCtl::default()).is_empty(), "the age setting is honoured");
        set_min_days(365);
        let _ = fs::remove_dir_all(&home);
    }

    #[test]
    fn spans_read_naturally() {
        assert_eq!(span(182), "6 months");
        assert_eq!(span(365), "1 year");
        assert_eq!(span(800), "2 years");
        assert_eq!(span(1095), "3 years");
    }
}
