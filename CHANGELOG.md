# Changelog

All notable changes to Mac Cleaner are listed here, newest first.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and version numbers follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Your Mac's storage (volume name, used-space bar, space available) at the bottom of the results sidebar.
- Storage numbers update every 3 seconds, so emptying the Trash or a big download shows up without a rescan. This also applies to the Welcome and Done screens.

### Changed
- "Available" space now matches Finder and System Settings. It includes space macOS frees on demand, so it can be higher than before.

### Development
- Tests run automatically on every pull request and push to `main`, on the self-hosted Mac runner.
- Releases run the tests before building.

## [1.0.0] - 2026-11-23

First release, published as a DMG on GitHub Releases.

### Added
- Scans 11 categories: App Caches, Log Files, Xcode Derived Data, Simulator Caches, Package Caches, Project Build Folders, Xcode Archives, Device Support, Simulator Devices, Installers in Downloads, and Large Files & Folders (adjustable from 50 MB to 1 GB).
- Moves items to the Trash, never deletes them permanently. Every path is checked against a fixed allow-list again right before it is moved, rejecting `..` and symlinked folders.
- Confirmation dialog listing every path. Low-risk items are pre-selected; anything marked **Review** and Apple's `com.apple.*` caches start unselected.
- Open folders to see what's inside, and pick single files.
- Search, age filter, sortable columns, and a right-click menu (Show in Finder, Quick Look, Copy Path, Open Folder, Always Ignore).
- Keyboard shortcuts for navigation, selection, Quick Look, zoom and rescanning.
- Light, Dark and Match System themes; zoom from 85% to 150%.
- Result screen listing anything that could not be moved, with a shortcut to Full Disk Access settings.
- Settings saved between launches: theme, zoom, Large Files size, ignore list, last scan time and total space cleaned.

### Security
- Source review of the scanning and cleaning code; the one medium and one low issue it found were fixed, with regression tests.
- `cargo audit`: no known vulnerabilities in dependencies.
- VirusTotal: no engine flagged the release DMG or the program inside it.

[Unreleased]: https://github.com/anilcharles/mac-cleaner/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/anilcharles/mac-cleaner/releases/tag/v1.0.0
