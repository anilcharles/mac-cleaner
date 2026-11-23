//! Made-up scan results for README screenshots (debug builds, `MAC_CLEANER_SAMPLE=1`).
//! Nothing here reads the disk, and none of these paths exist, so nothing can be cleaned.

use crate::scanner::{source_infos, Category, Entry, Item, Listing, Risk};

pub const HOME: &str = "/Users/you";
const DAY: u64 = 86_400_000;
const MB: u64 = 1_000_000;
const GB: u64 = 1_000_000_000;

/// (folder shown as the location, name, size, days since it changed, is a folder)
type Row = (&'static str, &'static str, u64, u64, bool);

fn rows(id: &str) -> Vec<Row> {
    let caches = "~/Library/Caches";
    let derived = "~/Library/Developer/Xcode/DerivedData";
    let devices = "~/Library/Developer/CoreSimulator/Devices";
    match id {
        "caches" => vec![
            (caches, "com.spotify.client", 2_840 * MB, 0, true),
            (caches, "Google", 1_210 * MB, 1, true),
            (caches, "Homebrew", 864 * MB, 12, true),
            (caches, "com.apple.Safari", 412 * MB, 0, true),
            (caches, "pip", 318 * MB, 40, true),
            (caches, "com.microsoft.VSCode.ShipIt", 254 * MB, 21, true),
            (caches, "Yarn", 198 * MB, 95, true),
            (caches, "org.swift.swiftpm", 96 * MB, 5, true),
            (caches, "com.figma.Desktop", 61 * MB, 3, true),
            (caches, "com.apple.helpd", 24 * MB, 0, true),
            (caches, "typescript", 18 * MB, 200, true),
        ],
        "logs" => vec![
            ("~/Library/Logs", "DiagnosticReports", 88 * MB, 2, true),
            ("~/Library/Logs", "JetBrains", 41 * MB, 14, true),
            ("~/Library/Logs", "CoreSimulator", 13 * MB, 30, true),
        ],
        "derived-data" => vec![
            (derived, "Weather-bqzxkdlrfhqmdwcjnto", 4_620 * MB, 2, true),
            (derived, "Notes-gfhdkqoeuwnzpaslbyt", 1_930 * MB, 45, true),
            (derived, "ModuleCache.noindex", 512 * MB, 2, true),
        ],
        "simulator-caches" => vec![("~/Library/Developer/CoreSimulator/Caches", "dyld", 1_480 * MB, 9, true)],
        "package-caches" => vec![
            ("~/.npm", "npm cache", 3_120 * MB, 1, true),
            ("~/.gradle", "Gradle caches", 2_400 * MB, 60, true),
            ("~/.cargo/registry", "Cargo registry cache", 1_045 * MB, 6, true),
        ],
        "build-folders" => vec![
            ("Rust · ~/Projects", "render-engine/target", 3_400 * MB, 4, true),
            ("Node.js · ~/Projects", "shop-web/node_modules", 1_250 * MB, 18, true),
            ("Node.js · ~/Projects/archive", "landing-page/node_modules", 640 * MB, 310, true),
        ],
        "xcode-archives" => vec![("~/Library/Developer/Xcode/Archives", "2026-06-02", 940 * MB, 170, true)],
        "device-support" => vec![(
            "~/Library/Developer/Xcode/iOS DeviceSupport",
            "iPhone17,1 26.1 (23B85)",
            4_480 * MB,
            15,
            true,
        )],
        "simulator-devices" => vec![
            (devices, "iPhone 17 Pro", 2_300 * MB, 7, true),
            (devices, "iPad Air 13-inch (M3)", 1_100 * MB, 120, true),
        ],
        "installers" => vec![
            ("~/Downloads", "Xcode_26.1.xip", 3_100 * MB, 20, false),
            ("~/Downloads", "Figma.dmg", 132 * MB, 64, false),
        ],
        "large-files" => vec![
            ("3 large files · ~/Movies", "Conference Talks 2025", 13_600 * MB, 210, true),
            ("~/Movies", "Family Trip 4K.mov", 6_500 * MB, 95, false),
            ("2 large files · ~/Downloads", "Design Review Recordings", 5_300 * MB, 33, true),
            ("~/Downloads", "ubuntu-26.04.1-desktop-arm64.iso", 4_200 * MB, 21, false),
            ("~/Documents", "backup-2024.zip", 2_100 * MB, 400, false),
            ("~/Downloads", "sample-dataset.parquet", 780 * MB, 8, false),
        ],
        _ => vec![],
    }
}

/// Where a row lives on the (imaginary) disk.
fn path_of(location: &str, name: &str) -> String {
    let folder = location.rsplit(" · ").next().unwrap_or(location);
    let leaf = match name {
        "npm cache" => "_cacache",
        "Gradle caches" => "caches",
        "Cargo registry cache" => "cache",
        other => other,
    };
    format!("{}/{leaf}", folder.replacen('~', HOME, 1))
}

pub fn categories(now_ms: u64) -> Vec<Category> {
    source_infos()
        .into_iter()
        .map(|info| {
            let mut items: Vec<Item> = rows(info.id)
                .into_iter()
                .map(|(location, name, size, days, is_dir)| {
                    let apple = info.id == "caches" && name.starts_with("com.apple.");
                    Item {
                        path: path_of(location, name),
                        name: name.to_string(),
                        location: location.to_string(),
                        size,
                        modified: Some(now_ms.saturating_sub(days * DAY)),
                        selected: info.risk == Risk::Safe && !apple,
                        note: apple.then(|| "macOS system cache".to_string()),
                        is_dir,
                    }
                })
                .collect();
            items.sort_by_key(|i| std::cmp::Reverse(i.size));
            Category { total: items.iter().map(|i| i.size).sum(), info, items }
        })
        .collect()
}

/// What an opened folder shows: the same handful of entries, sized as shares of the folder.
pub fn listing(folder: &str, folder_size: u64, modified: Option<u64>) -> Listing {
    // Different names one level down, so an opened folder inside a folder reads naturally.
    let nested = folder.matches('/').count() > 5;
    let names: [(&str, bool, u64); 6] = if nested {
        [
            ("blobs", true, 46),
            ("thumbnails", true, 27),
            ("sessions", true, 14),
            ("cache.sqlite", false, 8),
            ("tmp", true, 4),
            ("state.json", false, 1),
        ]
    } else {
        [
            ("Data", true, 46),
            ("Storage", true, 27),
            ("Browser", true, 14),
            ("index.db", false, 8),
            ("Code Cache", true, 4),
            ("manifest.json", false, 1),
        ]
    };
    let entries = names
        .into_iter()
        .enumerate()
        .map(|(i, (name, is_dir, share))| Entry {
            path: format!("{folder}/{name}"),
            name: name.to_string(),
            size: folder_size / 100 * share,
            modified: modified.map(|m| m.saturating_sub(i as u64 * 3 * DAY)),
            is_dir,
        })
        .collect();
    Listing { entries, hidden: if folder_size > GB { 37 } else { 0 } }
}
