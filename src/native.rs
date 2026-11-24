//! The few window features GPUI does not expose, done through AppKit directly.
#![allow(unexpected_cfgs)] // emitted by the objc msg_send! macro

use objc::runtime::Object;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[repr(C)]
#[derive(Clone, Copy)]
struct NSSize {
    width: f64,
    height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NSRect {
    x: f64,
    y: f64,
    size: NSSize,
}

/// The NSWindow behind a GPUI window.
pub fn ns_window(window: &gpui::Window) -> Option<*mut Object> {
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return None };
    let ns_window: *mut Object = unsafe { msg_send![appkit.ns_view.as_ptr() as *mut Object, window] };
    (!ns_window.is_null()).then_some(ns_window)
}

/// Changes the smallest size the user can drag the window to (GPUI only sets it at creation).
pub fn set_min_content_size(window: &gpui::Window, width: f32, height: f32) {
    if let Some(ns_window) = ns_window(window) {
        let size = NSSize { width: width as f64, height: height as f64 };
        let _: () = unsafe { msg_send![ns_window, setContentMinSize: size] };
    }
}

/// Puts the window in front of other apps' windows without taking keyboard focus.
/// A covered window is not redrawn, which would make the snapshot walkthrough repeat frames.
#[cfg(debug_assertions)]
pub fn order_front(window: &gpui::Window) {
    if let Some(ns_window) = ns_window(window) {
        let _: () = unsafe { msg_send![ns_window, orderFrontRegardless] };
    }
}

/// Makes this the active app and the window its key window. Only then does macOS draw the
/// title-bar buttons in colour, so the README screenshots use this to look the same in every shot.
#[cfg(debug_assertions)]
pub fn activate(window: &gpui::Window) {
    if let Some(ns_window) = ns_window(window) {
        unsafe {
            let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
            let _: () = msg_send![app, activateIgnoringOtherApps: true];
            let nil: *mut Object = std::ptr::null_mut();
            let _: () = msg_send![ns_window, makeKeyAndOrderFront: nil];
        }
    }
}

/// The part of the main screen that windows can use (without the menu bar and the Dock), in points.
pub fn screen_visible_size() -> Option<(f32, f32)> {
    unsafe {
        let screen: *mut Object = msg_send![class!(NSScreen), mainScreen];
        if screen.is_null() {
            return None;
        }
        let frame: NSRect = msg_send![screen, visibleFrame];
        Some((frame.size.width as f32, frame.size.height as f32))
    }
}

/// Name, total and available bytes of the volume that holds `path`, as Finder and System Settings
/// report them. "Available" includes space macOS frees on demand (purgeable files, local snapshots),
/// so it can be higher than what `statfs` reports. Safe to call from any thread.
pub fn volume_space(path: &std::path::Path) -> Option<(String, u64, u64)> {
    use objc::runtime::{BOOL, NO};
    use std::os::unix::ffi::OsStrExt;

    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    unsafe {
        // Objects below are autoreleased; this pool frees them even off the main thread.
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let ns_str = |s: &std::ffi::CStr| -> *mut Object { msg_send![class!(NSString), stringWithUTF8String: s.as_ptr()] };
        let ns_path = ns_str(&c_path);
        // A new NSURL each time, so no cached value from an earlier call is returned.
        let url: *mut Object = if ns_path.is_null() { std::ptr::null_mut() } else { msg_send![class!(NSURL), fileURLWithPath: ns_path] };
        let value = |key: &std::ffi::CStr| -> *mut Object {
            if url.is_null() {
                return std::ptr::null_mut();
            }
            let mut out: *mut Object = std::ptr::null_mut();
            let mut err: *mut Object = std::ptr::null_mut();
            let ok: BOOL = msg_send![url, getResourceValue: &mut out forKey: ns_str(key) error: &mut err];
            if ok == NO { std::ptr::null_mut() } else { out }
        };
        let number = |key: &std::ffi::CStr| -> Option<u64> {
            let n = value(key);
            if n.is_null() {
                return None;
            }
            let v: i64 = msg_send![n, longLongValue];
            u64::try_from(v).ok()
        };
        let total = number(c"NSURLVolumeTotalCapacityKey");
        let available = number(c"NSURLVolumeAvailableCapacityForImportantUsageKey");
        let name_obj = value(c"NSURLVolumeLocalizedNameKey");
        let name = if name_obj.is_null() {
            None
        } else {
            let utf8: *const std::os::raw::c_char = msg_send![name_obj, UTF8String];
            (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
        };
        let _: () = msg_send![pool, drain];
        Some((name.unwrap_or_else(|| "Macintosh HD".into()), total?, available?))
    }
}

/// The bundle identifier of the app (or other bundle) at `path`. Safe to call from any thread.
pub fn bundle_identifier(path: &std::path::Path) -> Option<String> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let ns_path: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c_path.as_ptr()];
        let bundle: *mut Object = if ns_path.is_null() { std::ptr::null_mut() } else { msg_send![class!(NSBundle), bundleWithPath: ns_path] };
        let id: *mut Object = if bundle.is_null() { std::ptr::null_mut() } else { msg_send![bundle, bundleIdentifier] };
        let out = if id.is_null() {
            None
        } else {
            let utf8: *const std::os::raw::c_char = msg_send![id, UTF8String];
            (!utf8.is_null()).then(|| std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
        };
        let _: () = msg_send![pool, drain];
        out.filter(|s| !s.is_empty())
    }
}

/// Paths of the programs this account is running.
pub fn running_executables() -> Vec<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut pids = vec![0 as libc::c_int; 8192];
    let bytes = (pids.len() * std::mem::size_of::<libc::c_int>()) as libc::c_int;
    let n = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    if n <= 0 {
        return vec![];
    }
    pids.truncate(n as usize);
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    pids.into_iter()
        .filter_map(|pid| {
            let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
            (len > 0).then(|| std::path::PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len as usize])))
        })
        .collect()
}

/// The icon Finder shows for `path`, as PNG bytes `px` pixels square.
pub fn file_icon_png(path: &std::path::Path, px: f64) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    unsafe {
        let pool: *mut Object = msg_send![class!(NSAutoreleasePool), new];
        let nil: *mut Object = std::ptr::null_mut();
        let ns_path: *mut Object = msg_send![class!(NSString), stringWithUTF8String: c_path.as_ptr()];
        let workspace: *mut Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let image: *mut Object = if ns_path.is_null() { nil } else { msg_send![workspace, iconForFile: ns_path] };
        let mut out = None;
        if !image.is_null() {
            let size = NSSize { width: px, height: px };
            let _: () = msg_send![image, setSize: size];
            let mut rect = NSRect { x: 0., y: 0., size };
            let cg: *mut std::ffi::c_void = msg_send![image, CGImageForProposedRect: &mut rect context: nil hints: nil];
            if !cg.is_null() {
                let rep: *mut Object = msg_send![class!(NSBitmapImageRep), alloc];
                let rep: *mut Object = msg_send![rep, initWithCGImage: cg];
                if !rep.is_null() {
                    let props: *mut Object = msg_send![class!(NSDictionary), dictionary];
                    // NSBitmapImageFileTypePNG
                    let data: *mut Object = msg_send![rep, representationUsingType: 4u64 properties: props];
                    if !data.is_null() {
                        let len: usize = msg_send![data, length];
                        let bytes: *const u8 = msg_send![data, bytes];
                        if !bytes.is_null() && len > 0 {
                            out = Some(std::slice::from_raw_parts(bytes, len).to_vec());
                        }
                    }
                    let _: () = msg_send![rep, release];
                }
            }
        }
        let _: () = msg_send![pool, drain];
        out
    }
}
