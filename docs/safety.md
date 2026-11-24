# Safety model

[← Back to the README](../README.md)

- **Nothing is deleted permanently.** Approved items are moved to the Trash (via `NSFileManager`), so you can put them back.
- **Fixed allow-list.** Only the locations in the table below are scanned. Before anything is moved, every path is checked again. It must be an offered item or inside one, with no `..` and no symlinked folder along the way, since a link could point outside the cleanable area. The check runs once more right before each move, so an item that changed in the meantime is left alone.
- **You confirm every item.** Low-risk items are pre-selected. Anything marked **Review**, and Apple's `com.apple.*` caches, start unselected. The confirmation dialog lists the full path of everything that will be moved.

| Category | Location | Default |
|---|---|---|
| App Caches | `~/Library/Caches/*` | selected (except `com.apple.*`) |
| Log Files | `~/Library/Logs/*` | selected |
| Xcode Derived Data | `~/Library/Developer/Xcode/DerivedData/*` | selected |
| Simulator Caches | `~/Library/Developer/CoreSimulator/Caches/*` | selected |
| Package Caches | `~/.npm/_cacache`, `~/.cargo/registry/cache`, `~/.gradle/caches`, `~/Library/pnpm/store`, `~/.bun/install/cache`, `~/.yarn/berry/cache`, `~/go/pkg/mod`, `~/.m2/repository`, `~/.cocoapods/repos` | selected |
| Project Build Folders | `node_modules`, `target`, `.build`, `.next`, `Pods`, Gradle `build`/`.gradle`, `.dart_tool` anywhere in `~`, only when the matching project file (`package.json`, `Cargo.toml`, …) sits next to them | **review** – rebuilding takes time |
| Xcode Archives | `~/Library/Developer/Xcode/Archives/*` | **review** – may hold dSYMs for shipped builds |
| Device Support | `~/Library/Developer/Xcode/{iOS,watchOS} DeviceSupport/*` | **review** |
| Simulator Devices | `~/Library/Developer/CoreSimulator/Devices/*` | **review** – apps and data on the device are lost |
| Installers in Downloads | `~/Downloads/*.{dmg,pkg,mpkg,xip}` | **review** |
| Mail Downloads | `~/Library/Containers/com.apple.mail/Data/Library/Mail Downloads/*` (needs Full Disk Access) | selected |
| iOS Software Updates | `~/Library/iTunes/{iPhone,iPad,iPod} Software Updates/*.ipsw` | selected |
| Device Backups | `~/Library/Application Support/MobileSync/Backup/*` (needs Full Disk Access) | **review** – keep the latest backup of each device |
| Applications | apps in `/Applications` and `~/Applications` (and plain folders one level down) that you can remove without a password, with their files in `~/Library` | **review** |
| App Leftovers | files in `~/Library/{Application Support,Containers,Group Containers,Preferences,Saved Application State,HTTPStorages,WebKit,Cookies,Application Scripts}` named after an app that is no longer installed | **review** |
| Duplicate Files | identical files ≥ 1 MB in `~`, outside the areas Large Files skips; the oldest copy is kept and never listed | **review** |
| Large Files & Folders | files ≥ 100 MB (adjustable: 50 MB to 1 GB) anywhere in `~`, and the folders that hold them | **review**: your own files, never pre-selected |

**Large Files & Folders** offers a large file through its own folder, so a whole download (video, subtitles, extras) can go at once. Open the folder to pick single files instead. Files loose in a top-level folder like `~/Downloads`, and files in code projects (folders with `.git`), are listed on their own; those folders are never offered whole. It skips `~/Library` (app data, iCloud, Mail), hidden folders (`.git`, `.cargo`…), `node_modules`, and macOS packages (`.photoslibrary`, `.app`, `.logicx`, `.fcpbundle`…). Removing one file from inside a package corrupts it. Every file is checked again before it is moved.

**Simulator Devices** are listed by device name and OS version, read from each device's `device.plist`.

**Applications** is the one category outside your home folder. It lists only apps this account can move to the Trash without a password, and never Apple's apps or Mac Cleaner itself. An app is offered whole (its bundle is never opened, so nothing inside it can be removed on its own) and its size includes its support files: entries in `~/Library` named after its bundle identifier, or after its name in Application Support, Caches and Logs. Removing it moves the app and those files to the Trash. An app that is running is left alone.

**App Leftovers** are judged cautiously. Only reverse-DNS names count (`com.vendor.App`, with `group.` and team-ID prefixes removed). Nothing from Apple, nothing from a vendor that still has an app installed or running, and nothing changed in the last 30 days is offered. A leftover that macOS would refuse to move (it is locked, or the app can't write to it or to the folder it sits in) is still listed, so you can see it, but it is marked **Protected by macOS** and starts unticked.

**Duplicate Files** groups files by size, then by a hash of their first 64 KB, then by a hash of their contents. Hard links count once. In each group the oldest file is kept and never listed. Right before a copy is moved it is compared byte by byte with the kept file again; if they differ, or the kept file is gone, the copy is left alone.

**Project Build Folders** uses the same no-go areas as Large Files (`~/Library`, hidden folders, macOS packages). Files inside a build folder are never listed under Large Files as well.

There is no "Empty Trash" button on purpose: a mistaken clean must stay recoverable, so emptying the Trash is left to you in Finder.
