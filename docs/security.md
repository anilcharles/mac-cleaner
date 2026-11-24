# Is it safe to run?

[← Back to the README](../README.md)

For anyone asking whether this contains a virus: all of the source is in this repository, and you can read it and build the app from it yourself. Below is what the app does, what was checked, and what was not.

**What the app does** (each point can be confirmed in `src/`)

- **Network use is limited to updates.** The app talks to GitHub only, and only to look for and download a new version (see [Updates](using.md#updates)). It sends nothing about your Mac or your files. Turn off "Check for updates automatically" in About Mac Cleaner and it connects only when you choose Check for Updates.
- **Updates are signed.** A downloaded update is installed only if its signature matches the release key built into the app, and the signature names that exact version. Anyone who could change the files on the GitHub release page still couldn't get the app to install something else.
- **No admin rights.** It never asks for a password, and installs no helper, background process or login item.
- **It runs Apple tools, by full path:** `/usr/bin/open` (Finder, the Trash, System Settings), `/usr/bin/qlmanage` (Quick Look) and `/usr/bin/plutil` (to read simulator names and an update's version). For updates it also runs `/usr/bin/curl` (download), `/usr/bin/hdiutil` (open the DMG), `/usr/bin/ditto` (copy the app) and a short `/bin/sh` script that swaps the app once it has quit.
- **It writes its own settings,** and while updating, a staging folder next to them. Everything it removes is moved to the Trash, never deleted.

**Checks run on 2026-11-23**

- **Source review.** Claude read the scanning and cleaning code (about 3,080 lines as it stood that afternoon) looking for ways the app could remove the wrong files. It followed the method in Cloudflare's open-source [security-audit skill](https://github.com/cloudflare/security-audit-skill), in its focused-review mode, not its full six-phase audit. It found no critical or high issues, one medium and one low. Both were fixed the same day, each with a regression test:
  - *Medium:* a scanned folder that was a symlink, or sat below one, could make the app offer and trash files from the link's target. Such folders are now refused.
  - *Low:* a folder on the way to an approved item could be swapped for a link between the check and the move. Every path is now checked again immediately before it is moved.
  - Hardening done at the same time: the home folder is read from the user database, not from `$HOME`; `open` is called by its full path; the snapshot walkthrough exists only in debug builds.
- **Dependency audit.** `cargo audit` 0.23.0 checked all 724 packages in `Cargo.lock` against the RustSec advisory database: **no known vulnerabilities**. It listed 14 "unmaintained" notices, all for packages pulled in by GPUI.

- **Antivirus scan.** The version 1.2.0 release download was uploaded to [VirusTotal](https://www.virustotal.com) on 24 November 2026. VirusTotal runs a file past the engines of many antivirus vendors.
  - the DMG from the [v1.2.0 release](https://github.com/anilcharles/mac-cleaner/releases/tag/v1.2.0): **no engine flagged it**. 0 malicious, 0 suspicious, 61 engines found nothing, 12 do not handle this file type ([report](https://www.virustotal.com/gui/file/cb55cb89ef9c808ab0c507194d60e77a87e77caa5a5170ef89b7d2a478de302d));
  - the program file inside the app: **1 engine flagged it**. Microsoft reported `Trojan:Script/Wacatac.B!ml`; 0 suspicious, 60 engines found nothing, 1 failed, 11 do not handle this file type ([report](https://www.virustotal.com/gui/file/d3f88cd85884daad186faca347b7a507535259711b3a7d0d5e76a82a823b25d8)). The `!ml` ending means Microsoft's machine-learning model guessed from the file's traits, not that it matched known malware. This kind of guess often hits new programs that are not signed with an Apple Developer ID, like this one. It has not been reported to Microsoft as a false positive yet. The source is all in this repository if you want to check for yourself.
  - The earlier 1.0.0 release was not flagged by any engine ([program](https://www.virustotal.com/gui/file/39d98649fdb27db33bda7b1880feb487e2f446df88a4a711aef5f1b1e0f9c8b4), [DMG](https://www.virustotal.com/gui/file/736c031ff62050b6b27649d4e1b9a25e81f91eb1e0fc762f604166c05f5b7a2c)).

**What this does not cover**

- This is not an independent audit or a penetration test. The review was done by Claude, the same AI tool that wrote the code, by reading the source; nobody outside the project has reviewed it.
- A VirusTotal result covers that one build on that day. It is not proof that the app is harmless, and it does not carry over to a later version or to a copy you build yourself, which will have a different fingerprint.
- Cloudflare did not audit, scan, test or endorse this app. The security-audit skill is a published set of review instructions that Claude followed; no Cloudflare service was run.
- Code added after that review has not been reviewed for security: zoom, search and filters, popup menus, the About dialog, the ignore list, Project Build Folders, Simulator Devices, Quick Look and the updater.
- A very small timing window remains between the last check and the move to the Trash, and the home folder itself is not checked for being a symlink.
- The app is ad-hoc signed, not notarized by Apple, so macOS warns when a downloaded copy is opened. A prebuilt copy from someone else cannot be shown to match this source; build it yourself with `./scripts/bundle.sh`.

To repeat the checks: `cargo test` and `cargo install cargo-audit && cargo audit`.
