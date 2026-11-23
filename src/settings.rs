//! Remembers choices and a little history between launches.
//! Stored in ~/Library/Application Support/Mac Cleaner/settings.txt (never scanned or cleaned)
//! as plain `key=value` lines; `ignore=` may appear many times.

use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    System,
    Light,
    Dark,
}

/// Choices offered for the Large Files threshold, in bytes.
pub const LARGE_MIN_CHOICES: [u64; 4] = [50_000_000, 100_000_000, 500_000_000, 1_000_000_000];

pub struct Settings {
    pub theme: ThemeMode,
    /// Size of the whole interface: 1.0 is 100%.
    pub zoom: f32,
    /// Smallest file listed under Large Files.
    pub large_min: u64,
    /// Top-level items the user asked never to see again.
    pub ignored: Vec<String>,
    /// Bytes moved to the Trash over all runs.
    pub freed_total: u64,
    /// Number of cleans that moved at least one item.
    pub cleans: u64,
    /// When the last scan finished, milliseconds since the Unix epoch.
    pub last_scan: Option<u64>,
    /// Set by the snapshot walkthrough so it never changes the saved file.
    pub frozen: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            zoom: 1.,
            large_min: 100_000_000,
            ignored: vec![],
            freed_total: 0,
            cleans: 0,
            last_scan: None,
            frozen: false,
        }
    }
}

fn file() -> PathBuf {
    crate::scanner::home_dir().join("Library/Application Support/Mac Cleaner/settings.txt")
}

fn parse_theme(value: &str) -> ThemeMode {
    match value.trim() {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::System,
    }
}

/// MAC_CLEANER_THEME overrides the saved theme for one run (used by the snapshot walkthrough).
pub fn theme_override() -> Option<ThemeMode> {
    std::env::var("MAC_CLEANER_THEME").ok().map(|v| parse_theme(&v))
}

/// MAC_CLEANER_ZOOM overrides the saved zoom for one run (used by the snapshot walkthrough).
pub fn zoom_override() -> Option<f32> {
    let zoom: f32 = std::env::var("MAC_CLEANER_ZOOM").ok()?.trim().parse().ok()?;
    (0.5..=3.0).contains(&zoom).then_some(zoom)
}

/// The zoom level closest to a stored value, so a hand-edited file can't break the layout.
fn nearest_zoom(value: f32) -> f32 {
    let steps = crate::theme::ZOOM_STEPS;
    steps.into_iter().fold(1., |best, s| if (s - value).abs() < (best - value).abs() { s } else { best })
}

impl Settings {
    pub fn load() -> Self {
        Self::parse(&std::fs::read_to_string(file()).unwrap_or_default())
    }

    fn parse(text: &str) -> Self {
        let mut s = Self::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            match key {
                "theme" => s.theme = parse_theme(value),
                "zoom" => s.zoom = value.trim().parse().map(nearest_zoom).unwrap_or(1.),
                "large_min" => {
                    if let Some(v) = value.trim().parse().ok().filter(|v| LARGE_MIN_CHOICES.contains(v)) {
                        s.large_min = v;
                    }
                }
                "ignore" if value.starts_with('/') => s.ignored.push(value.to_string()),
                "freed_total" => s.freed_total = value.trim().parse().unwrap_or(0),
                "cleans" => s.cleans = value.trim().parse().unwrap_or(0),
                "last_scan" => s.last_scan = value.trim().parse().ok(),
                _ => {}
            }
        }
        s
    }

    fn serialize(&self) -> String {
        let theme = match self.theme {
            ThemeMode::System => "system",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        };
        let mut out = format!(
            "theme={theme}\nzoom={}\nlarge_min={}\nfreed_total={}\ncleans={}\n",
            self.zoom, self.large_min, self.freed_total, self.cleans
        );
        if let Some(t) = self.last_scan {
            out.push_str(&format!("last_scan={t}\n"));
        }
        for path in &self.ignored {
            out.push_str(&format!("ignore={path}\n"));
        }
        out
    }

    pub fn save(&self) {
        if self.frozen {
            return;
        }
        let path = file();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, self.serialize());
    }

    /// Hides a top-level item from future scans. Paths that can't be stored on one line are refused.
    pub fn ignore(&mut self, path: &str) -> bool {
        if path.contains('\n') || !path.starts_with('/') {
            return false;
        }
        if !self.ignored.iter().any(|p| p == path) {
            self.ignored.push(path.to_string());
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_snaps_to_an_offered_level() {
        assert_eq!(Settings::parse("zoom=1.15\n").zoom, 1.15);
        assert_eq!(Settings::parse("zoom=1.2\n").zoom, 1.15);
        assert_eq!(Settings::parse("zoom=40\n").zoom, 1.5);
        assert_eq!(Settings::parse("zoom=-3\n").zoom, 0.85);
        assert_eq!(Settings::parse("zoom=big\n").zoom, 1.);
        assert_eq!(Settings::parse("zoom=NaN\n").zoom, 1.);
    }

    #[test]
    fn settings_round_trip() {
        let mut s = Settings::default();
        s.theme = ThemeMode::Dark;
        s.zoom = 1.3;
        s.large_min = 500_000_000;
        s.freed_total = 12_345;
        s.cleans = 3;
        s.last_scan = Some(1_700_000_000_000);
        assert!(s.ignore("/Users/me/Library/Caches/a=b"));
        assert!(!s.ignore("/bad\npath"));
        let back = Settings::parse(&s.serialize());
        assert!(back.theme == ThemeMode::Dark);
        assert_eq!(back.zoom, 1.3);
        assert_eq!(back.large_min, 500_000_000);
        assert_eq!((back.freed_total, back.cleans, back.last_scan), (12_345, 3, Some(1_700_000_000_000)));
        assert_eq!(back.ignored, ["/Users/me/Library/Caches/a=b"]);
    }

    #[test]
    fn reads_the_old_theme_only_file_and_rejects_odd_thresholds() {
        let s = Settings::parse("theme=light\nlarge_min=7\n");
        assert!(s.theme == ThemeMode::Light);
        assert_eq!(s.zoom, 1., "a file without a zoom line means 100%");
        assert_eq!(s.large_min, 100_000_000);
    }
}
