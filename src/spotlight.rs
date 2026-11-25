//! Dates Spotlight keeps about a file that the file system doesn't.
//!
//! `kMDItemLastUsedDate` is set each time the file is opened through Launch Services (Finder,
//! the Dock, an app's Open or Open Recent menu, `open`), and is what Finder shows as "Last opened".
//! `kMDItemDateAdded` is when the file arrived in its folder (downloaded, copied or moved there).

use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

type CFRef = *const c_void;

#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn MDItemCreate(alloc: CFRef, path: CFRef) -> CFRef;
    fn MDItemCopyAttribute(item: CFRef, name: CFRef) -> CFRef;
    static kMDItemLastUsedDate: CFRef;
    static kMDItemDateAdded: CFRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithBytes(alloc: CFRef, bytes: *const u8, len: isize, encoding: u32, external: bool) -> CFRef;
    fn CFGetTypeID(obj: CFRef) -> usize;
    fn CFDateGetTypeID() -> usize;
    fn CFDateGetAbsoluteTime(date: CFRef) -> f64;
    fn CFRelease(obj: CFRef);
}

/// Seconds from the Unix epoch to Core Foundation's (2001-01-01).
const CF_EPOCH: f64 = 978_307_200.;

/// The later of when `path` was last opened through Finder or an app and when it was added to its
/// folder, in milliseconds since the Unix epoch. `None` when Spotlight knows neither.
pub fn last_activity_ms(path: &Path) -> Option<u64> {
    const UTF8: u32 = 0x0800_0100;
    let bytes = path.as_os_str().as_bytes();
    unsafe {
        let cf_path = CFStringCreateWithBytes(std::ptr::null(), bytes.as_ptr(), bytes.len() as isize, UTF8, false);
        if cf_path.is_null() {
            return None;
        }
        let item = MDItemCreate(std::ptr::null(), cf_path);
        CFRelease(cf_path);
        if item.is_null() {
            return None;
        }
        let date_ms = |name: CFRef| -> Option<u64> {
            let value = MDItemCopyAttribute(item, name);
            if value.is_null() {
                return None;
            }
            let ms = (CFGetTypeID(value) == CFDateGetTypeID())
                .then(|| ((CFDateGetAbsoluteTime(value) + CF_EPOCH) * 1000.).max(0.) as u64);
            CFRelease(value);
            ms
        };
        let latest = date_ms(kMDItemLastUsedDate).max(date_ms(kMDItemDateAdded));
        CFRelease(item);
        latest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_spotlight_dates_without_failing() {
        let file = std::env::temp_dir().join(format!("mc-spotlight-{}", std::process::id()));
        std::fs::write(&file, b"x").unwrap();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        // An unindexed file may have no dates; any date given must be a sane one.
        if let Some(ms) = last_activity_ms(&file) {
            assert!(ms > 978_307_200_000 && ms <= now + 60_000, "{ms}");
        }
        assert_eq!(last_activity_ms(Path::new("/no/such/file")), None);
        let _ = std::fs::remove_file(&file);
    }
}
