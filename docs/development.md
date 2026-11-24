# Development

[← Back to the README](../README.md)

## Run

```sh
cargo run --release
```

## Build the app

```sh
./scripts/bundle.sh        # → target/release/Mac Cleaner.app (ad-hoc signed)
./scripts/install.sh       # build, quit the running app, replace /Applications/Mac Cleaner.app, open it
./scripts/screenshots.sh   # regenerate docs/screenshots/ from sample data
```

## Releasing

Releasing is automated with [release-please](https://github.com/googleapis/release-please). Pull requests are squash-merged and their titles follow [Conventional Commits](https://www.conventionalcommits.org/): `feat:` for a new feature, `fix:` for a bug fix, and `docs:`, `ci:`, `chore:`, `refactor:` or `test:` for everything else (a CI check enforces this). After `feat:` or `fix:` changes are merged, release-please opens a `chore: release x.y.z` pull request that bumps the version and lists those changes as the release notes. Merging it tags `vx.y.z`, publishes the GitHub Release with those notes, and the self-hosted runner builds the DMG and attaches it.

## Tests

```sh
cargo test                                              # allow-list, symlink, large-file, build-folder, filter, selection and settings tests
cargo test real_scan_summary -- --ignored --nocapture   # read-only scan of your Mac
```

## UI snapshots

The app can capture its own window, which needs no Screen Recording permission. This walks through every screen, saves PNGs, and quits. It **never cleans anything** and never changes the saved settings; the result screen uses a made-up report.

```sh
MAC_CLEANER_SNAPSHOT_DIR=/tmp/shots MAC_CLEANER_THEME=dark MAC_CLEANER_ZOOM=1.3 cargo run   # debug builds only; release builds ignore this variable
```

`MAC_CLEANER_THEME` (`light` or `dark`) and `MAC_CLEANER_ZOOM` (0.5 to 3.0) override the saved choices for that run. The walkthrough covers the welcome screen (also at the smallest window size), scanning, results, an opened folder, Large Files & Folders, search, the right-click menu, About, the confirm dialog, the smallest window size, a zoom step in and back, and the result screen. The screenshots show real file names from your Mac, so check them before sharing. The walkthrough keeps its window in front of other apps, since a covered window is not redrawn and frames would repeat.

**Sample data.** Add `MAC_CLEANER_SAMPLE=1` to skip the scan and show made-up results from `src/sample.rs` instead (paths under `/Users/you`, invented sizes, a fixed disk size). `./scripts/screenshots.sh` uses this to regenerate `docs/screenshots/` for the README and [Using Mac Cleaner](using.md) in light and dark.

## How it is checked

- **Unit tests** cover the allow-list, symlink handling, the large-file and build-folder rules, app-leftover matching, duplicate detection, tree selection, filters and saved settings. They use temporary folders, never real files.
- **Layout** is checked from the snapshots above, at 85%, 100% and 150% zoom, in light and dark, including the smallest window size.
- **Not covered automatically:** pressing keyboard shortcuts, clicking menu items and dragging the window by its top edge. Try these by hand after changing that code.
- **Moving files to the Trash** is only ever done by a person using the app. No test or walkthrough cleans anything.

## Zoom and the `pt()` unit

Every size in the UI is written as `pt(n)` (defined in `theme.rs`), which stores it in rem units. Changing the zoom changes the window's rem size, and everything follows. New UI code must use `pt()`; real pixels (`gpui::px`) are only for values tied to the mouse or the window, multiplied by the zoom.

## Project layout

```
src/
  main.rs       app start-up, window, menu, embedded icons, progress-ring SVG
  ui.rs         GPUI view: all screens, table, dialog, building blocks
  app.rs        state and tree-selection logic (UI-independent, unit tested)
  scanner.rs    scanning, allow-list, folder listing, move-to-Trash
  apps.rs       installed apps, their support files, leftovers of deleted apps
  duplicates.rs identical-file search and the byte-by-byte recheck before a move
  theme.rs      light/dark palette, layout metrics, zoom levels and the pt() size unit
  settings.rs   saved choices: theme, zoom, size limit, ignore list, history
  category_icons.rs  which macOS icon each category shows (app, folder or device artwork, read at runtime)
  native.rs     AppKit calls GPUI does not expose (minimum window size, usable screen size)
  snapshot.rs   developer-only self-capture for UI checks (debug builds)
  sample.rs     made-up scan results for README screenshots (debug builds)
assets/icons/   small UI glyphs from Lucide (lucide.dev, ISC license: LICENSE-lucide.txt); category
                and file icons come from macOS itself
assets/icon/    app icon source: make_icon.py → AppIcon.svg → render_icon.sh → AppIcon.icns
```

## Notes

- GPUI is pre-1.0; this project pins `gpui = "0.2.2"`.
