//! Colours and metrics. Same palette as the original CSS design, for light and dark mode.

use gpui::{rgb, rgba, Hsla, Rems, WindowAppearance};

#[derive(Clone, Copy)]
pub struct Theme {
    /// The zoom factor, for the few sizes that must be given in real pixels (shadows).
    pub scale: f32,
    pub bg: Hsla,
    pub sidebar: Hsla,
    pub surface: Hsla,
    pub surface_2: Hsla,
    pub hover: Hsla,
    pub selected_row: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub text: Hsla,
    pub text_2: Hsla,
    pub text_3: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_soft: Hsla,
    pub danger: Hsla,
    pub danger_hover: Hsla,
    pub danger_soft: Hsla,
    pub success: Hsla,
    pub success_soft: Hsla,
    pub warning: Hsla,
    pub warning_soft: Hsla,
    pub folder: Hsla,
    pub shadow: Hsla,
    pub backdrop: Hsla,
}

impl Theme {
    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            _ => Self::light(),
        }
    }

    pub fn light() -> Self {
        Self {
            scale: 1.,
            bg: rgb(0xf5f5f7).into(),
            sidebar: rgb(0xececef).into(),
            surface: rgb(0xffffff).into(),
            surface_2: rgb(0xf7f7f9).into(),
            hover: rgba(0x00000009).into(),
            selected_row: rgba(0x007aff10).into(),
            border: rgba(0x00000014).into(),
            border_strong: rgba(0x00000024).into(),
            text: rgb(0x1d1d1f).into(),
            text_2: rgb(0x6e6e73).into(),
            text_3: rgb(0x8e8e93).into(),
            accent: rgb(0x007aff).into(),
            accent_hover: rgb(0x0068d9).into(),
            accent_soft: rgba(0x007aff1a).into(),
            danger: rgb(0xff3b30).into(),
            danger_hover: rgb(0xe0332a).into(),
            danger_soft: rgba(0xff3b301f).into(),
            success: rgb(0x34c759).into(),
            success_soft: rgba(0x34c7591f).into(),
            warning: rgb(0xf09000).into(),
            warning_soft: rgba(0xff9f0a1f).into(),
            folder: rgb(0x3a9cf5).into(),
            shadow: rgba(0x0000001a).into(),
            backdrop: rgba(0x00000047).into(),
        }
    }

    pub fn dark() -> Self {
        Self {
            scale: 1.,
            bg: rgb(0x1c1c1e).into(),
            sidebar: rgb(0x232325).into(),
            surface: rgb(0x2a2a2d).into(),
            surface_2: rgb(0x303033).into(),
            hover: rgba(0xffffff0a).into(),
            selected_row: rgba(0x0a84ff1a).into(),
            border: rgba(0xffffff14).into(),
            border_strong: rgba(0xffffff29).into(),
            text: rgb(0xf5f5f7).into(),
            text_2: rgb(0xa1a1a6).into(),
            text_3: rgb(0x7c7c80).into(),
            accent: rgb(0x0a84ff).into(),
            accent_hover: rgb(0x3395ff).into(),
            accent_soft: rgba(0x0a84ff2e).into(),
            danger: rgb(0xff453a).into(),
            danger_hover: rgb(0xff5e55).into(),
            danger_soft: rgba(0xff453a29).into(),
            success: rgb(0x30d158).into(),
            success_soft: rgba(0x30d15826).into(),
            warning: rgb(0xffa526).into(),
            warning_soft: rgba(0xffa52626).into(),
            folder: rgb(0x4aa8ff).into(),
            shadow: rgba(0x00000066).into(),
            backdrop: rgba(0x00000073).into(),
        }
    }
}

/// Gradient colours for each category tile (top, bottom).
pub fn tile_colors(category_id: &str) -> (u32, u32) {
    match category_id {
        "caches" => (0x5ac8fa, 0x007aff),
        "logs" => (0xa7a7b0, 0x74747c),
        "derived-data" => (0x8e8cff, 0x5856d6),
        "simulator-caches" => (0x64d8cb, 0x2ba9bf),
        "package-caches" => (0xffbb4d, 0xff8c00),
        "build-folders" => (0x7d8ba1, 0x48566a),
        "simulator-devices" => (0x6fb1ff, 0x3a6fe0),
        "xcode-archives" => (0xcfab87, 0xa2845e),
        "device-support" => (0xd98af5, 0xa64bd6),
        "installers" => (0x6ade86, 0x28b14c),
        "large-files" => (0xff8a9e, 0xff375f),
        "mail-downloads" => (0x6cc4ff, 0x1f8ef1),
        "ios-updates" => (0xa3d977, 0x5fae2e),
        "ios-backups" => (0x9fabbd, 0x5f6d82),
        "applications" => (0x7f9cff, 0x4a5cf0),
        "app-leftovers" => (0xe9a4d6, 0xc25aa8),
        "duplicates" => (0xffd56b, 0xf0a500),
        _ => (0x5ac8fa, 0x007aff),
    }
}

/// Zoom levels offered by View > Zoom In / Zoom Out.
pub const ZOOM_STEPS: [f32; 5] = [0.85, 1.0, 1.15, 1.3, 1.5];

/// Window size at 100% zoom, in points: (default, minimum).
pub const WINDOW_SIZE: (f32, f32) = (1120., 740.);
pub const WINDOW_MIN: (f32, f32) = (940., 700.);

/// A size in design points (what it measures at 100% zoom).
///
/// Every size in the UI goes through this, so it is stored in rems and the whole
/// interface scales together when the zoom changes the window's rem size.
pub const fn pt(points: f32) -> Rems {
    Rems(points / 16.)
}

// Layout metrics shared by the table header and rows so columns line up exactly.
pub const TITLEBAR: Rems = pt(52.);
pub const SIDEBAR_W: Rems = pt(324.);
pub const BAR_H: Rems = pt(64.);
pub const ROW_H: Rems = pt(46.);
pub const ROW_PAD_X: Rems = pt(16.);
pub const COL_GAP: Rems = pt(16.);
pub const COL_CHECK: Rems = pt(20.);
pub const COL_KIND: Rems = pt(84.);
pub const COL_DATE: Rems = pt(112.);
pub const COL_SIZE: Rems = pt(88.);
pub const COL_ACTION: Rems = pt(28.);
pub const INDENT: Rems = pt(22.);
