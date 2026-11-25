//! Each category is shown with the icon macOS itself uses for what it cleans: the app that makes
//! those files, or the system's own folder and device artwork. Nothing is bundled; the icons are
//! read from macOS at runtime, so they always match the installed system. When an app isn't
//! installed, the next source in the list is used.

use std::path::Path;

enum Source {
    /// An installed app, found by its bundle identifier.
    App(&'static str),
    /// A file or app at a fixed place on every Mac; the icon Finder shows for it.
    File(&'static str),
    /// The icon Finder gives files with this extension.
    FileType(&'static str),
    /// Artwork in CoreTypes.bundle, where Finder's own folder, device and badge icons live.
    System(&'static str),
}
use Source::*;

/// Used when none of a category's own sources exist.
const FALLBACK: &[Source] = &[System("GenericFolderIcon")];

const CORE_TYPES: &str = "/System/Library/CoreServices/CoreTypes.bundle/Contents/Resources";

fn sources(category: &str) -> &'static [Source] {
    match category {
        "caches" => &[System("LibraryFolderIcon")],
        "logs" => &[FileType("log")],
        "derived-data" => &[App("com.apple.dt.Xcode"), System("DeveloperFolderIcon")],
        "simulator-caches" => &[App("com.apple.iphonesimulator"), App("com.apple.dt.Xcode"), System("DeveloperFolderIcon")],
        "package-caches" => &[File("/System/Applications/Utilities/Terminal.app")],
        "build-folders" => &[System("DeveloperFolderIcon")],
        "xcode-archives" => &[File("/System/Library/CoreServices/Applications/Archive Utility.app"), App("com.apple.dt.Xcode")],
        "device-support" => &[System("com.apple.iphone")],
        "simulator-devices" => &[App("com.apple.iphonesimulator"), System("com.apple.ipad")],
        "installers" => &[File("/System/Library/CoreServices/Installer.app"), System("DownloadsFolder")],
        "mail-downloads" => &[File("/System/Applications/Mail.app")],
        // Software Update lives in System Settings; its own app has no artwork.
        "ios-updates" => &[File("/System/Applications/System Settings.app"), System("com.apple.iphone")],
        "ios-backups" => &[File("/System/Applications/Time Machine.app"), System("SidebarTimeMachine")],
        "applications" => &[System("ApplicationsFolderIcon")],
        "app-leftovers" => &[System("FullTrashIcon")],
        "duplicates" => &[System("MultipleItemsIcon")],
        "large-files" => &[File("/System/Applications/Utilities/Disk Utility.app"), System("SidebarInternalDisk")],
        "old-files" => &[File("/System/Applications/Clock.app"), System("Clock")],
        _ => &[],
    }
}

/// The icon for `category` as PNG bytes `px` pixels square, or `None` if no source is found.
pub fn png(category: &str, px: f64) -> Option<Vec<u8>> {
    sources(category).iter().chain(FALLBACK).find_map(|source| match source {
        App(id) => crate::native::app_path(id).and_then(|p| crate::native::file_icon_png(&p, px)),
        File(p) => Path::new(p).exists().then(|| crate::native::file_icon_png(Path::new(p), px)).flatten(),
        FileType(ext) => crate::native::file_type_icon_png(ext, px),
        System(name) => system_png(name, px),
    })
}

/// Finder's artwork named `name` in CoreTypes.bundle (such as `GenericFolderIcon`), as PNG bytes.
pub fn system_png(name: &str, px: f64) -> Option<Vec<u8>> {
    let file = Path::new(CORE_TYPES).join(format!("{name}.icns"));
    file.exists().then(|| crate::native::image_file_png(&file, px)).flatten()
}

#[cfg(test)]
mod tests {
    #[test]
    fn every_category_has_a_source() {
        for info in crate::scanner::source_infos() {
            assert!(!super::sources(info.id).is_empty(), "no icon source for {}", info.id);
        }
    }
}
