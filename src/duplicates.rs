//! Identical files in the user's own folders.
//!
//! Files of at least `MIN_SIZE` are grouped by size, then by a hash of their first 64 KB, then
//! by a hash of their whole contents. In each group the oldest file is the one kept: it is never
//! offered. Before a copy is moved to the Trash it is compared byte by byte with the kept file
//! again, so a hash collision or a file edited since the scan can't cost the last copy.

use crate::scanner::{build_kind, is_listed_installer, is_symlink, skip_for_large_files, tilde, ScanCtl};
use rayon::prelude::*;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::hash::Hasher;
use std::io::{BufReader, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use walkdir::WalkDir;

/// Smaller files are left out: they free little and would make the scan much slower.
pub const MIN_SIZE: u64 = 1_000_000;
const HEAD: usize = 64 * 1024;

/// Copies offered by the last scan, each with the kept file it matches.
static OFFERED: Mutex<Option<HashMap<PathBuf, PathBuf>>> = Mutex::new(None);

struct FileInfo {
    path: PathBuf,
    size: u64,
    created: std::time::SystemTime,
}

/// Regular files of at least `MIN_SIZE` under `home`, outside the areas the large-file scan skips.
fn files(home: &Path, ctl: &ScanCtl) -> Vec<FileInfo> {
    let mut seen = HashSet::new();
    WalkDir::new(home)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            e.depth() == 0
                || !e.file_type().is_dir()
                || !(skip_for_large_files(&e.file_name().to_string_lossy(), e.depth()) || build_kind(e.path()).is_some())
        })
        .take_while(|_| !ctl.cancelled())
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| !is_listed_installer(e.path(), home))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            // Hard links are one file with two names: removing one frees nothing.
            (m.len() >= MIN_SIZE && seen.insert((m.dev(), m.ino()))).then(|| FileInfo {
                path: e.path().to_path_buf(),
                size: m.len(),
                created: m.created().or_else(|_| m.modified()).unwrap_or(std::time::UNIX_EPOCH),
            })
        })
        .collect()
}

fn hash(path: &Path, limit: Option<usize>, ctl: &ScanCtl) -> Option<u64> {
    let mut reader = BufReader::with_capacity(1 << 20, File::open(path).ok()?);
    let mut hasher = DefaultHasher::new();
    let mut buf = vec![0u8; 1 << 20];
    let mut left = limit.unwrap_or(usize::MAX);
    while left > 0 {
        if ctl.cancelled() {
            return None;
        }
        let n = reader.read(&mut buf[..left.min(1 << 20)]).ok()?;
        if n == 0 {
            break;
        }
        ctl.tick();
        hasher.write(&buf[..n]);
        left -= n;
    }
    Some(hasher.finish())
}

/// Splits each group by `key`, keeping only the parts that still hold two or more files.
fn split<K: std::hash::Hash + Eq + Send>(groups: Vec<Vec<FileInfo>>, key: impl Fn(&FileInfo) -> Option<K> + Sync) -> Vec<Vec<FileInfo>> {
    groups
        .into_par_iter()
        .flat_map_iter(|group| {
            let mut parts: HashMap<K, Vec<FileInfo>> = HashMap::new();
            for f in group {
                if let Some(k) = key(&f) {
                    parts.entry(k).or_default().push(f);
                }
            }
            parts.into_values().filter(|p| p.len() > 1)
        })
        .collect()
}

/// Duplicate copies under `home`: (path, name, location). Also remembers them for `is_offered_copy`.
pub fn duplicate_items(home: &Path, ctl: &ScanCtl) -> Vec<(PathBuf, String, Option<String>)> {
    let mut by_size: HashMap<u64, Vec<FileInfo>> = HashMap::new();
    for f in files(home, ctl) {
        by_size.entry(f.size).or_default().push(f);
    }
    let groups: Vec<Vec<FileInfo>> = by_size.into_values().filter(|g| g.len() > 1).collect();
    let groups = split(groups, |f| hash(&f.path, Some(HEAD), ctl));
    let groups = split(groups, |f| hash(&f.path, None, ctl));
    if ctl.cancelled() {
        return vec![];
    }

    let mut offered = HashMap::new();
    let mut out = vec![];
    for mut group in groups {
        // Keep the oldest; on a tie, the one with the shorter path.
        group.sort_by(|a, b| a.created.cmp(&b.created).then(a.path.as_os_str().len().cmp(&b.path.as_os_str().len())));
        let kept = group.remove(0).path;
        for copy in group {
            let place = tilde(copy.path.parent().unwrap_or(&copy.path), home);
            let name = copy.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            out.push((copy.path.clone(), name, Some(format!("{place} · same as {}", tilde(&kept, home)))));
            offered.insert(copy.path, kept.clone());
        }
    }
    *OFFERED.lock().unwrap() = Some(offered);
    out
}

/// The kept file a copy was matched with, if the last scan offered it.
fn kept_for(path: &Path) -> Option<PathBuf> {
    OFFERED.lock().unwrap().as_ref()?.get(path).cloned()
}

fn plain_file(path: &Path) -> bool {
    fs::symlink_metadata(path).map(|m| m.is_file()).unwrap_or(false)
}

/// True when `path` is a copy the last scan offered, its kept file still exists, and both are
/// still ordinary files of the same size inside the home folder's scanned areas.
pub fn is_offered_copy(path: &Path, home: &Path) -> bool {
    let Some(kept) = kept_for(path) else { return false };
    let Ok(rel) = path.strip_prefix(home) else { return false };
    let names: Vec<String> = rel.iter().map(|n| n.to_string_lossy().into_owned()).collect();
    let Some((_, folders)) = names.split_last() else { return false };
    if folders.iter().enumerate().any(|(i, n)| skip_for_large_files(n, i + 1)) {
        return false;
    }
    if path.ancestors().skip(1).take_while(|d| *d != home).any(is_symlink) {
        return false;
    }
    let size = |p: &Path| fs::symlink_metadata(p).map(|m| m.len()).ok();
    plain_file(path) && plain_file(&kept) && kept != path && size(path) == size(&kept)
}

/// Compares an offered copy with its kept file, byte by byte.
pub fn still_identical(path: &Path) -> bool {
    let Some(kept) = kept_for(path) else { return false };
    let (Ok(a), Ok(b)) = (File::open(path), File::open(&kept)) else { return false };
    let (mut a, mut b) = (BufReader::with_capacity(1 << 20, a), BufReader::with_capacity(1 << 20, b));
    let (mut x, mut y) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let Ok(n) = a.read(&mut x) else { return false };
        if n == 0 {
            // Both must end at the same place.
            return matches!(b.read(&mut y[..1]), Ok(0));
        }
        if b.read_exact(&mut y[..n]).is_err() || x[..n] != y[..n] {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_copies_keeps_the_oldest_and_rechecks_before_cleaning() {
        let dir = std::env::temp_dir().join(format!("mc-dups-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a/x")).unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        let data: Vec<u8> = (0..(MIN_SIZE as usize + 10)).map(|i| (i % 251) as u8).collect();
        fs::write(dir.join("a/x/original.bin"), &data).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(dir.join("b/copy.bin"), &data).unwrap();
        let mut other = data.clone();
        *other.last_mut().unwrap() ^= 1;
        fs::write(dir.join("b/different.bin"), &other).unwrap();

        let ctl = ScanCtl::default();
        let items = duplicate_items(&dir, &ctl);
        let paths: Vec<&PathBuf> = items.iter().map(|(p, _, _)| p).collect();
        assert_eq!(paths, vec![&dir.join("b/copy.bin")]);
        assert!(is_offered_copy(&dir.join("b/copy.bin"), &dir));
        assert!(still_identical(&dir.join("b/copy.bin")));

        // Edited since the scan: no longer identical, so it must be refused.
        fs::write(dir.join("b/copy.bin"), &other).unwrap();
        assert!(!still_identical(&dir.join("b/copy.bin")));
        // The kept file gone: the copy is no longer offered at all.
        fs::write(dir.join("b/copy.bin"), &data).unwrap();
        fs::remove_file(dir.join("a/x/original.bin")).unwrap();
        assert!(!is_offered_copy(&dir.join("b/copy.bin"), &dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
