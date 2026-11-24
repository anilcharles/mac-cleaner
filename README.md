# Mac Cleaner

A native macOS cleaner written in Rust with [GPUI](https://crates.io/crates/gpui), the GPU-drawn UI framework from the Zed editor. It finds caches, logs, developer leftovers, large files, duplicates and the files deleted apps leave behind. You can open each folder to see what's inside, and nothing is moved until you confirm every path.

Version 1.2.0. Needs macOS 12 or later. Prebuilt downloads are for Apple Silicon. Created with [Claude Code](https://claude.com/claude-code), Anthropic's AI coding tool.

![Scan results in the light theme](docs/screenshots/results.png)

![App Leftovers, with one item marked Protected by macOS](docs/screenshots/app-leftovers.png)

More screenshots, keyboard shortcuts, menus and settings are in **[Using Mac Cleaner](docs/using.md)**.

## Install

1. Download `Mac-Cleaner-<version>-arm64.dmg` from [Releases](https://github.com/anilcharles/mac-cleaner/releases). The release notes there list what changed.
2. Open the DMG and drag **Mac Cleaner** onto **Applications**.
3. Open Mac Cleaner from Applications.

**"Apple could not verify Mac Cleaner is free of malware".** macOS shows this the first time you open a copy downloaded in a browser, because the app is not notarized by Apple. Don't click **Move to Bin**. Click **Done**, then either:

- open **System Settings → Privacy & Security** and click **Open Anyway** next to *"Mac Cleaner" was blocked*, or
- run `xattr -dr com.apple.quarantine "/Applications/Mac Cleaner.app"` in Terminal and open the app again.

You only need to do this once per download.

## What it cleans

App caches, log files, Xcode derived data, archives and device support, simulator caches and devices, package-manager caches, project build folders, installers in Downloads, Mail downloads, iOS updates and backups, apps with their support files, app leftovers, duplicate files, and large files and folders.

Low-risk items are pre-selected. Anything that needs a closer look is marked **Review** and starts unticked. The full list of locations is in the [Safety model](docs/safety.md).

## Safety

- **Nothing is deleted permanently.** Everything goes to the Trash, so you can put it back. Space is freed once you empty the Trash.
- **Fixed list of locations.** Only known folders are scanned, and every path is checked again right before it is moved.
- **You confirm every item.** The confirmation dialog lists the full path of everything that will move.
- **No network, no admin rights.** The app sends nothing anywhere and never asks for a password.

What was checked, including the [VirusTotal](https://www.virustotal.com) results for each release, is in **[Is it safe to run?](docs/security.md)**. In short, the 1.2.0 DMG is [clean](https://www.virustotal.com/gui/file/cb55cb89ef9c808ab0c507194d60e77a87e77caa5a5170ef89b7d2a478de302d), and the program inside it got [one machine-learning detection from Microsoft](https://www.virustotal.com/gui/file/d3f88cd85884daad186faca347b7a507535259711b3a7d0d5e76a82a823b25d8), which that page explains.

## Build from source

```sh
cargo run --release        # run it
./scripts/install.sh       # build and install to /Applications
cargo test                 # run the tests
```

Building, releasing, tests and the project layout are in **[Development](docs/development.md)**.
