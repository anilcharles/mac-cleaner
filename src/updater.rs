//! Checks GitHub Releases for a newer version, downloads it, checks its signature and swaps it in.
//!
//! Every release DMG is signed in CI with a minisign key whose public half is built in below.
//! The trusted comment of the signature names the DMG file (and so its version), so an older
//! signed DMG can't be passed off as a newer release. Nothing is installed unless the signature
//! checks out and the app inside says it is the version the release promised.
//!
//! The swap itself happens in a small shell script that waits for this app to quit, copies the
//! new bundle next to the old one, swaps them and opens the new one.

use anyhow::{anyhow, bail, Context as _, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// minisign public key of the release signing key (key id DF4869F37C4BDD18).
const PUBLIC_KEY: &str = "RWQY3Ut882lI3/1xnXKTU/0Z74Iv8BV6izJOrYJ9F2irRv6MCb0xT5k4";
const LATEST_URL: &str = "https://api.github.com/repos/anilcharles/mac-cleaner/releases/latest";
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// One line of release notes, as shown in the update dialog.
#[derive(Clone, Debug, PartialEq)]
pub enum NoteLine {
    Heading(String),
    Bullet(String),
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: Vec<NoteLine>,
    dmg_name: String,
    dmg_url: String,
    sig_url: String,
    pub size: u64,
}

/// Shared between the download running in the background and the dialog showing it.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub cancel: AtomicBool,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

fn version_of(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.trim().trim_start_matches('v').split('.');
    let mut next = || parts.next()?.parse::<u64>().ok();
    let v = (next()?, next()?, next()?);
    parts.next().is_none().then_some(v)
}

/// True when `candidate` is a plain x.y.z version above `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (version_of(candidate), version_of(current)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// The DMG name CI gives a version; also the trusted comment its signature must carry.
fn dmg_name(version: &str) -> String {
    format!("Mac-Cleaner-{version}-arm64.dmg")
}

/// Turns release-please's markdown into headings and bullets without links or commit hashes.
fn parse_notes(body: &str) -> Vec<NoteLine> {
    let mut out = vec![];
    for line in body.lines().map(str::trim) {
        if let Some(h) = line.strip_prefix("### ") {
            out.push(NoteLine::Heading(h.trim().to_string()));
        } else if let Some(b) = line.strip_prefix("* ").or_else(|| line.strip_prefix("- ")) {
            let text = strip_markdown(b);
            if !text.is_empty() {
                out.push(NoteLine::Bullet(text));
            }
        }
    }
    out
}

/// Drops ` ([#16](url))`-style references and turns `[text](url)` into `text`; `**` goes too.
fn strip_markdown(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        let Some(mid) = rest[start..].find("](").map(|m| start + m) else { break };
        let Some(end) = rest[mid..].find(')').map(|e| mid + e) else { break };
        out.push_str(&rest[..start]);
        // A reference wrapped in parentheses, like "([#16](…))", is dropped with its brackets.
        if out.ends_with('(') && rest[end + 1..].starts_with(')') {
            out.pop();
            rest = &rest[end + 2..];
            continue;
        }
        out.push_str(&rest[start + 1..mid]);
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    let mut out = out.replace("**", "");
    // Collapse the spaces left where references were.
    while out.contains("  ") {
        out = out.replace("  ", " ");
    }
    out.trim().trim_end_matches(',').trim().to_string()
}

fn curl() -> Command {
    let mut c = Command::new("/usr/bin/curl");
    c.args(["--fail", "--location", "--silent", "--show-error", "--proto", "=https,file", "--connect-timeout", "15"]);
    c.stdin(Stdio::null());
    c
}

/// Where the latest release is read from. Debug builds can point it at a local file to try the
/// whole flow without publishing anything; a released app always asks GitHub.
fn latest_url() -> String {
    #[cfg(debug_assertions)]
    if let Ok(url) = std::env::var("MAC_CLEANER_UPDATE_FEED") {
        return url;
    }
    LATEST_URL.to_string()
}

/// Asks GitHub for the latest release. `Ok(None)` means this copy is up to date, or the newest
/// release has no DMG and signature yet (CI attaches them a few minutes after it is published).
pub fn check() -> Result<Option<Release>> {
    let out = curl()
        .args(["--max-time", "20", "-H", "Accept: application/vnd.github+json"])
        .arg(latest_url())
        .output()
        .context("couldn't run curl")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("curl: "));
    }
    let gh: GhRelease = serde_json::from_slice(&out.stdout).context("unexpected reply from GitHub")?;
    release_from(gh, CURRENT)
}

fn release_from(gh: GhRelease, current: &str) -> Result<Option<Release>> {
    if gh.draft || gh.prerelease || !is_newer(&gh.tag_name, current) {
        return Ok(None);
    }
    let version = gh.tag_name.trim_start_matches('v').to_string();
    let name = dmg_name(&version);
    let sig_name = format!("{name}.minisig");
    let dmg = gh.assets.iter().find(|a| a.name == name);
    let sig = gh.assets.iter().find(|a| a.name == sig_name);
    let (Some(dmg), Some(sig)) = (dmg, sig) else { return Ok(None) };
    Ok(Some(Release {
        notes: parse_notes(gh.body.as_deref().unwrap_or("")),
        dmg_name: name,
        dmg_url: dmg.browser_download_url.clone(),
        sig_url: sig.browser_download_url.clone(),
        size: dmg.size,
        version,
    }))
}

#[cfg(debug_assertions)]
impl Release {
    /// A made-up release for the snapshot walkthrough.
    pub fn sample() -> Self {
        // Always one minor version ahead of this build, so the dialog reads correctly.
        let (major, minor, _) = version_of(CURRENT).unwrap_or((1, 0, 0));
        let version = format!("{major}.{}.0", minor + 1);
        Self {
            notes: parse_notes(
                "### Features\n\n* a faster scan of large folders\n\
* Quick Look for several items at once\n\n### Bug Fixes\n\n\
* the storage numbers refresh after emptying the Trash\n",
            ),
            dmg_name: dmg_name(&version),
            dmg_url: String::new(),
            sig_url: String::new(),
            size: 9_800_000,
            version,
        }
    }
}

/// Checks `data` against the built-in key, and that the signature was made for `expected_name`.
fn verify(data: &[u8], signature: &str, expected_name: &str) -> Result<()> {
    let key = minisign_verify::PublicKey::from_base64(PUBLIC_KEY).map_err(|e| anyhow!("{e}"))?;
    let sig = minisign_verify::Signature::decode(signature).map_err(|_| anyhow!("the signature is unreadable"))?;
    key.verify(data, &sig, false).map_err(|_| anyhow!("the download is not signed by Mac Cleaner's release key"))?;
    if sig.trusted_comment().trim() != expected_name {
        bail!("the signature belongs to a different release");
    }
    Ok(())
}

/// The folder downloads are staged in. Mac Cleaner never scans its own Application Support folder.
fn update_dir() -> PathBuf {
    crate::scanner::home_dir().join("Library/Application Support/Mac Cleaner/Update")
}

/// The running app bundle, if it can be replaced in place. Checked before downloading anything.
pub fn install_target() -> Result<PathBuf> {
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).context("couldn't find the running app")?;
    let bundle = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|e| e == "app"))
        .ok_or_else(|| anyhow!("Updates work only in the installed app, not in a development build."))?
        .to_path_buf();
    if bundle.to_string_lossy().contains("/AppTranslocation/") {
        bail!("Move Mac Cleaner to your Applications folder first, then open it from there.");
    }
    let parent = bundle.parent().ok_or_else(|| anyhow!("the app has no parent folder"))?;
    let c_parent = std::ffi::CString::new(parent.as_os_str().as_encoded_bytes())?;
    if unsafe { libc::access(c_parent.as_ptr(), libc::W_OK) } != 0 {
        bail!("Mac Cleaner can't write to {}. Download the new version from GitHub instead.", parent.display());
    }
    Ok(bundle)
}

/// Downloads, verifies and unpacks `release`. Returns the new app bundle, ready for [`install`].
/// Nothing is left behind when it fails.
pub fn download(release: &Release, progress: &Progress) -> Result<PathBuf> {
    let dir = update_dir();
    let result = fetch_and_unpack(release, progress, &dir);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

fn fetch_and_unpack(release: &Release, progress: &Progress, dir: &Path) -> Result<PathBuf> {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).context("couldn't create the download folder")?;
    let dmg = dir.join(&release.dmg_name);

    let mut child = curl()
        .args(["--speed-limit", "1024", "--speed-time", "30", "-o"])
        .arg(&dmg)
        .arg(&release.dmg_url)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("couldn't run curl")?;
    let status = loop {
        if progress.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("cancelled");
        }
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if let Ok(meta) = std::fs::metadata(&dmg) {
            progress.done.store(meta.len(), Ordering::Relaxed);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = std::io::Read::read_to_string(&mut stderr, &mut err);
        }
        bail!("download failed: {}", err.trim().trim_start_matches("curl: "));
    }
    progress.done.store(release.size, Ordering::Relaxed);

    let sig = curl().args(["--max-time", "30"]).arg(&release.sig_url).output().context("couldn't run curl")?;
    if !sig.status.success() {
        bail!("couldn't download the signature");
    }
    let data = std::fs::read(&dmg).context("couldn't read the download")?;
    verify(&data, &String::from_utf8_lossy(&sig.stdout), &release.dmg_name)?;
    drop(data);

    let staged = unpack(&dmg, dir)?;
    let _ = std::fs::remove_file(&dmg);
    let version = bundle_version(&staged)?;
    if version != release.version {
        bail!("the downloaded app says it is version {version}, not {}", release.version);
    }
    Ok(staged)
}

/// Mounts the DMG, copies the app out of it and unmounts it again.
fn unpack(dmg: &Path, dir: &Path) -> Result<PathBuf> {
    let mount = dir.join("mount");
    std::fs::create_dir_all(&mount)?;
    let attach = Command::new("/usr/bin/hdiutil")
        .args(["attach", "-nobrowse", "-readonly", "-noautoopen", "-quiet", "-mountpoint"])
        .arg(&mount)
        .arg(dmg)
        .stdin(Stdio::null())
        .output()
        .context("couldn't run hdiutil")?;
    if !attach.status.success() {
        bail!("couldn't open the disk image");
    }
    let staged = dir.join("Mac Cleaner.app");
    let copied = Command::new("/usr/bin/ditto").arg(mount.join("Mac Cleaner.app")).arg(&staged).status();
    let detached = Command::new("/usr/bin/hdiutil").args(["detach", "-quiet"]).arg(&mount).status();
    if !detached.is_ok_and(|s| s.success()) {
        let _ = Command::new("/usr/bin/hdiutil").args(["detach", "-force", "-quiet"]).arg(&mount).status();
    }
    let _ = std::fs::remove_dir(&mount);
    if !copied.is_ok_and(|s| s.success()) {
        bail!("couldn't copy the app out of the disk image");
    }
    Ok(staged)
}

fn bundle_version(app: &Path) -> Result<String> {
    let out = Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(app.join("Contents/Info.plist"))
        .output()?;
    if !out.status.success() {
        bail!("the downloaded app has no version");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Waits for this process to exit, puts the new bundle in place of the old one (keeping the old
/// one until the new one is fully copied) and opens it.
const SWAP_SCRIPT: &str = r#"
pid="$1"; new="$2"; dest="$3"
i=0
while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 300 ]; do sleep 0.1; i=$((i + 1)); done
rm -rf "$dest.new" "$dest.old"
if ! /usr/bin/ditto "$new" "$dest.new"; then
  rm -rf "$dest.new"; /usr/bin/open "$dest"; exit 1
fi
if ! mv "$dest" "$dest.old"; then
  rm -rf "$dest.new"; /usr/bin/open "$dest"; exit 1
fi
if ! mv "$dest.new" "$dest"; then
  mv "$dest.old" "$dest"; /usr/bin/open "$dest"; exit 1
fi
rm -rf "$dest.old" "$(dirname "$new")"
touch "$dest"
/usr/bin/open "$dest"
"#;

/// Starts the swap script for `staged` → `target`. The caller quits the app right after.
pub fn install(staged: &Path, target: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let log = std::fs::File::create(update_dir().join("install.log")).ok();
    let mut cmd = Command::new("/bin/sh");
    cmd.arg("-c")
        .arg(SWAP_SCRIPT)
        .arg("mac-cleaner-update")
        .arg(std::process::id().to_string())
        .arg(staged)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // Its own process group, so it outlives this app.
        .process_group(0);
    match log {
        Some(f) => cmd.stderr(f),
        None => cmd.stderr(Stdio::null()),
    };
    cmd.spawn().context("couldn't start the installer")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &[u8] = b"mac cleaner update test\n";
    const FIXTURE_SIG: &str = "untrusted comment: signature from minisign secret key
RUQY3Ut882lI32HBds+TwQ+Y8ry9VrqV8D//+ClLM1RCs7jc39T3VWIPgyYZHx0wKHOjb/oqiX4gK7OeIts/KgqQuykiPve6eQY=
trusted comment: Mac-Cleaner-9.9.9-arm64.dmg
JOKeaDIi5NIEuwZ3siA0PgGxJVMjeOvK0e/rjBcSRJxVsQQVDiYNcUvF+A/qyEQgUB3j7kWlmJ+VHxLdkAVYCg==
";

    #[test]
    fn compares_versions() {
        assert!(is_newer("v1.3.0", "1.2.0"));
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "1.99.99"));
        assert!(!is_newer("v1.2.0", "1.2.0"));
        assert!(!is_newer("1.1.9", "1.2.0"));
        assert!(!is_newer("1.3.0-beta", "1.2.0"));
        assert!(!is_newer("1.3", "1.2.0"));
        assert!(!is_newer("garbage", "1.2.0"));
    }

    #[test]
    fn accepts_a_signed_file_and_refuses_anything_else() {
        verify(FIXTURE, FIXTURE_SIG, "Mac-Cleaner-9.9.9-arm64.dmg").unwrap();
        let tampered = b"mac cleaner update test!\n";
        assert!(verify(tampered, FIXTURE_SIG, "Mac-Cleaner-9.9.9-arm64.dmg").is_err());
        // A genuine signature of another release can't stand in for this one.
        assert!(verify(FIXTURE, FIXTURE_SIG, "Mac-Cleaner-10.0.0-arm64.dmg").is_err());
        // Changing the trusted comment breaks the signature.
        let relabeled = FIXTURE_SIG.replace("9.9.9", "10.0.0");
        assert!(verify(FIXTURE, &relabeled, "Mac-Cleaner-10.0.0-arm64.dmg").is_err());
        assert!(verify(FIXTURE, "not a signature", "Mac-Cleaner-9.9.9-arm64.dmg").is_err());
    }

    #[test]
    fn reads_release_please_notes() {
        let body = "## [1.2.0](https://github.com/a/b/compare/v1.1.0...v1.2.0) (2026-11-24)\n\n\n### Features\n\n\
* app uninstaller, app leftovers ([#16](https://github.com/a/b/issues/16)) ([450c51c](https://github.com/a/b/commit/450c51c))\n\
* see the [docs](https://example.com) for **more**\n\n### Bug Fixes\n\n* a fix\n";
        assert_eq!(
            parse_notes(body),
            vec![
                NoteLine::Heading("Features".into()),
                NoteLine::Bullet("app uninstaller, app leftovers".into()),
                NoteLine::Bullet("see the docs for more".into()),
                NoteLine::Heading("Bug Fixes".into()),
                NoteLine::Bullet("a fix".into()),
            ]
        );
    }

    fn gh(tag: &str, assets: &[&str]) -> GhRelease {
        GhRelease {
            tag_name: tag.into(),
            body: None,
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|n| GhAsset { name: n.to_string(), browser_download_url: format!("https://x/{n}"), size: 7 })
                .collect(),
        }
    }

    #[test]
    fn offers_only_newer_releases_with_a_signed_dmg() {
        let full = ["Mac-Cleaner-1.3.0-arm64.dmg", "Mac-Cleaner-1.3.0-arm64.dmg.minisig"];
        let r = release_from(gh("v1.3.0", &full), "1.2.0").unwrap().unwrap();
        assert_eq!(r.version, "1.3.0");
        assert_eq!(r.dmg_url, "https://x/Mac-Cleaner-1.3.0-arm64.dmg");
        assert_eq!(r.size, 7);
        assert!(release_from(gh("v1.3.0", &full), "1.3.0").unwrap().is_none());
        // CI is still building: no DMG, or no signature yet.
        assert!(release_from(gh("v1.3.0", &[]), "1.2.0").unwrap().is_none());
        assert!(release_from(gh("v1.3.0", &full[..1]), "1.2.0").unwrap().is_none());
    }
}

