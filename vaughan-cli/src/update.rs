//! `vaughan update` — refresh this install through the channel that created it.
//!
//! Design: the wallet never replaces itself by a route the user did not already
//! trust. It works out how this binary got here and hands the job back to that
//! channel — `npm -g`, `cargo install`, or the checksum-verified release tarball
//! `scripts/install.sh` uses. A build symlinked out of a checkout is refused
//! outright, because the fix there is `cargo build`, not a download.
//!
//! Deliberately NOT here: any check that runs without being asked. The periodic
//! check is opt-in and lives in [`crate::update_check`]; this module only ever
//! talks to the network because the user typed `vaughan update`.
//!
//! Trust note, stated plainly because it matters for a wallet: `SHA256SUMS` is
//! served by the same host as the tarball, so verifying it proves the download
//! was not corrupted in transit — not that the release is authentic. A funded
//! wallet should prefer a manually verified download. Release signing with a
//! pinned key is the fix and is not implemented yet.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::json;

use crate::json_out::print_json_value;

/// Upstream repository for release lookups.
const REPO: &str = "r4-ndm/Vaughan-CLI";

/// GitHub requires a User-Agent on API requests.
const USER_AGENT: &str = concat!("vaughan-cli/", env!("CARGO_PKG_VERSION"));

/// How this binary was installed, which decides how it can be refreshed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// Installed by `npm i -g vaughan` (lives under a `node_modules` tree).
    Npm,
    /// Installed by `cargo install` (lives in the cargo bin directory).
    Cargo,
    /// A plain file we can replace — release tarball or `scripts/install.sh`.
    Tarball,
    /// Resolves into a checkout's `target/` — a developer build.
    DevBuild,
}

impl InstallKind {
    /// What the user has to run to get a newer build for this kind of install.
    fn how_to_update(&self) -> &'static str {
        match self {
            Self::Npm => "npm install -g vaughan@latest",
            Self::Cargo => "cargo install --locked --git … vaughan-cli",
            Self::Tarball => "vaughan update (verified release download)",
            Self::DevBuild => "scripts/install-vaughan.sh",
        }
    }
}

/// Classify the running binary by where it resolves to.
///
/// `current_exe` resolves symlinks on Linux and macOS, so a `~/.local/bin/vaughan`
/// symlinked into a checkout reports the `target/` path — which is what lets a
/// dev build be told apart from an installed one at all.
pub fn classify(exe: &Path) -> InstallKind {
    let as_str = exe.to_string_lossy();
    if as_str.contains("/node_modules/") {
        return InstallKind::Npm;
    }
    // Anything inside a cargo target directory: `target/debug`, `target/release`,
    // or the railgun build's `target/railgun/release`. Matching on components
    // rather than a substring keeps an install directory that merely happens to
    // be named `target` from being mistaken for a checkout — the profile
    // directory has to be there too.
    let parts: Vec<_> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let in_target =
        parts.iter().any(|p| p == "target") && parts.iter().any(|p| p == "debug" || p == "release");
    if in_target {
        return InstallKind::DevBuild;
    }
    if let Some(cargo_bin) = cargo_bin_dir() {
        if exe.parent() == Some(cargo_bin.as_path()) {
            return InstallKind::Cargo;
        }
    }
    InstallKind::Tarball
}

fn cargo_bin_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CARGO_HOME") {
        return Some(PathBuf::from(home).join("bin"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo").join("bin"))
}

/// Cap on any single request. Callers on a launch path add their own, shorter
/// deadline on top (see `main::startup_update_notice`); this one exists so an
/// explicit `vaughan update` cannot hang forever either.
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared client: `user_agent` in the builder like the rest of the repo's HTTP
/// clients, plus the timeout none of them set because they are wrapped by a
/// `tokio::time::timeout` at the call site instead.
fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("could not build the HTTP client")
}

/// The release asset platform tag, matching `scripts/install.sh::detect_platform`.
///
/// Only the three combinations that releases actually publish are accepted
/// (see `npm/launcher/lib/resolve.js`, which gates the same set): building the
/// tag for, say, `macos-x86_64` would only turn into a confusing 404 later.
pub fn platform_tag() -> Result<String> {
    let tag = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux-x86_64",
        ("linux", "aarch64") => "linux-aarch64",
        ("macos", "aarch64") => "macos-aarch64",
        (os, arch) => bail!(
            "no published release for {os}-{arch} — build from source \
             (supported: linux-x86_64, linux-aarch64, macos-aarch64)"
        ),
    };
    Ok(tag.to_string())
}

/// Compare dotted numeric versions, ignoring a leading `v` and any suffix.
///
/// Returns true when `latest` is strictly newer than `current`. A version that
/// cannot be parsed sorts as older, so a malformed tag never triggers an update.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u64> {
        s.trim()
            .trim_start_matches('v')
            // Drop any pre-release / build suffix: "0.3.0-rc1" -> "0.3.0".
            .split(['-', '+'])
            .next()
            .unwrap_or("")
            .split('.')
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (l, c) = (parse(latest), parse(current));
    if l.is_empty() {
        return false;
    }
    for i in 0..l.len().max(c.len()) {
        let (a, b) = (
            l.get(i).copied().unwrap_or(0),
            c.get(i).copied().unwrap_or(0),
        );
        if a != b {
            return a > b;
        }
    }
    false
}

/// The newest published release tag (e.g. `v0.2.12`).
pub async fn latest_tag() -> Result<String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let mut req = http_client()?
        .get(&url)
        .header("Accept", "application/vnd.github+json");
    // Private-repo installs need a token; use the same env vars install.sh reads.
    if let Some(token) = github_token() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req
        .send()
        .await
        .context("could not reach the GitHub release API")?;
    if !resp.status().is_success() {
        bail!("GitHub release lookup failed: HTTP {}", resp.status());
    }
    let body: serde_json::Value = resp.json().await.context("malformed release JSON")?;
    body.get("tag_name")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("no tag_name in the latest release"))
}

fn github_token() -> Option<String> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            if !v.trim().is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Entry point for `vaughan update`.
pub async fn run_update(json_mode: bool, check_only: bool, assume_yes: bool) -> Result<()> {
    let exe = std::env::current_exe().context("could not determine the running binary path")?;
    let kind = classify(&exe);
    let current = env!("CARGO_PKG_VERSION");
    let latest = latest_tag().await?;
    let available = is_newer(&latest, current);

    print_json_value(
        json_mode,
        &json!({
            "current": current,
            "latest": latest,
            "update_available": available,
            "install_kind": format!("{kind:?}"),
            "binary": exe.to_string_lossy(),
        }),
        || {
            println!("installed: {current} ({})", exe.to_string_lossy());
            println!("latest:    {latest}");
            if available {
                println!("an update is available — {}", kind.how_to_update());
            } else {
                println!("already up to date");
            }
        },
    );

    if check_only || !available {
        return Ok(());
    }

    // A dev build has no channel to refresh from; saying so beats downloading a
    // release over the top of someone's checkout build.
    if kind == InstallKind::DevBuild {
        bail!(
            "{} is a build from a checkout — rebuild it instead:\n  scripts/install-vaughan.sh",
            exe.to_string_lossy()
        );
    }

    if !assume_yes && !confirm(&format!("Update to {latest}?"))? {
        println!("cancelled");
        return Ok(());
    }

    match kind {
        InstallKind::Npm => run_channel(
            "npm",
            &[
                "install",
                "-g",
                &format!("vaughan@{}", latest.trim_start_matches('v')),
            ],
        ),
        InstallKind::Cargo => run_channel(
            "cargo",
            &[
                "install",
                "--locked",
                "--git",
                &format!("https://github.com/{REPO}.git"),
                "--tag",
                &latest,
                "--bin",
                "vaughan",
                "vaughan-cli",
            ],
        ),
        InstallKind::Tarball => install_tarball(&exe, &latest).await,
        InstallKind::DevBuild => unreachable!("refused above"),
    }
}

/// Hand the install back to the tool that owns it, streaming its output.
fn run_channel(program: &str, args: &[&str]) -> Result<()> {
    println!("running: {program} {}", args.join(" "));
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("could not run {program} — is it on PATH?"))?;
    if !status.success() {
        bail!("{program} exited with {status}");
    }
    println!("updated");
    Ok(())
}

/// Download the release tarball, verify its SHA256, and swap this binary for it.
///
/// Split so the parts that can be wrong without a network are testable: see
/// [`expected_sha256`], [`verify_sha256`], and [`extract_and_replace`].
async fn install_tarball(exe: &Path, tag: &str) -> Result<()> {
    let platform = platform_tag()?;
    let archive = format!("vaughan-{platform}.tar.gz");
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");

    // A scratch dir of our own rather than the `tempfile` crate, which the
    // dependency allowlist marks dev-only.
    let tmp = ScratchDir::new()?;
    let archive_path = tmp.path().join(&archive);

    println!("downloading {archive} ({tag})");
    let bytes = fetch(&format!("{base}/{archive}")).await?;
    std::fs::write(&archive_path, &bytes).context("could not write the download")?;

    println!("verifying SHA256");
    let sums = String::from_utf8(fetch(&format!("{base}/SHA256SUMS")).await?)
        .context("SHA256SUMS was not valid UTF-8")?;
    verify_sha256(&bytes, expected_sha256(&sums, &archive)?, &archive)?;

    println!("extracting");
    extract_and_replace(&archive_path, tmp.path(), exe)?;

    println!("updated to {tag}");
    Ok(())
}

/// The digest `SHA256SUMS` lists for `archive`.
///
/// Matches on the file name as a whole field rather than a line suffix, so a
/// longer asset name that merely ends with ours (`extra-vaughan-…`) cannot be
/// mistaken for it.
fn expected_sha256<'a>(sums: &'a str, archive: &str) -> Result<&'a str> {
    sums.lines()
        .find_map(|line| {
            let mut fields = line.split_whitespace();
            let digest = fields.next()?;
            // `sha256sum` writes "<digest>  <name>"; the name may carry a
            // leading `*` in binary mode.
            let name = fields.next()?.trim_start_matches('*');
            (name == archive).then_some(digest)
        })
        .ok_or_else(|| anyhow!("no SHA256SUMS entry for {archive}"))
}

/// Fail unless `bytes` hashes to `expected`.
fn verify_sha256(bytes: &[u8], expected: &str, archive: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let actual = hex::encode(Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        bail!("checksum mismatch for {archive} — refusing to install");
    }
    Ok(())
}

/// Unpack `archive_path` into `tmp` and move the `vaughan` inside it over `exe`.
///
/// Replaces via a sibling temp file + rename: writing over a running binary
/// fails with ETXTBSY, while renaming over it is allowed and atomic. The staged
/// file is a sibling so the rename stays within one filesystem.
fn extract_and_replace(archive_path: &Path, tmp: &Path, exe: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive_path)
        .arg("-C")
        .arg(tmp)
        .status()
        .context("could not run tar — is it on PATH?")?;
    if !status.success() {
        bail!("tar exited with {status}");
    }
    let unpacked = tmp.join("vaughan");
    if !unpacked.is_file() {
        bail!("the archive did not contain a `vaughan` binary at its root");
    }

    let dir = exe
        .parent()
        .ok_or_else(|| anyhow!("the running binary has no parent directory"))?;
    let staged = dir.join(".vaughan.update.tmp");
    std::fs::copy(&unpacked, &staged).with_context(|| {
        format!(
            "could not stage the new binary in {} — check write permission",
            dir.display()
        )
    })?;
    set_executable(&staged)?;
    std::fs::rename(&staged, exe).context("could not replace the current binary")?;
    Ok(())
}

async fn fetch(url: &str) -> Result<Vec<u8>> {
    let mut req = http_client()?
        .get(url)
        .header("Accept", "application/octet-stream");
    if let Some(token) = github_token() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req
        .send()
        .await
        .with_context(|| format!("could not download {url}"))?;
    if !resp.status().is_success() {
        bail!("download failed: HTTP {} for {url}", resp.status());
    }
    Ok(resp.bytes().await.context("truncated download")?.to_vec())
}

/// A self-cleaning scratch directory under the system temp dir.
struct ScratchDir(PathBuf);

impl ScratchDir {
    fn new() -> Result<Self> {
        // Pid AND a per-process counter: the pid alone collides between two
        // instances in one process, and since `new` clears the directory first,
        // the second would delete the first's contents out from under it.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("vaughan-update-{}-{seq}", std::process::id()));
        // A leftover directory from a crashed run would otherwise poison the
        // extract step with a stale `vaughan`.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).context("could not create a scratch directory")?;
        Ok(Self(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Keep the 0755 mode a release tarball ships with.
fn set_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
            .context("could not mark the new binary executable")?;
    }
    let _ = path;
    Ok(())
}

/// Yes/no on stdin. A non-tty (piped, CI) answers no rather than hanging.
fn confirm(prompt: &str) -> Result<bool> {
    use std::io::{IsTerminal, Write};
    if !std::io::stdin().is_terminal() {
        bail!("{prompt} — not a terminal; re-run with --yes to proceed");
    }
    print!("{prompt} [y/N] ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("could not read the answer")?;
    Ok(matches!(
        line.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_versions_are_detected_and_older_ones_are_not() {
        assert!(is_newer("v0.2.12", "0.2.11"));
        assert!(is_newer("0.3.0", "0.2.11"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.2.11", "0.2.11"));
        assert!(!is_newer("0.2.10", "0.2.11"));
        // A shorter tag is not newer just by being shorter.
        assert!(!is_newer("0.2", "0.2.11"));
        assert!(is_newer("0.3", "0.2.11"));
    }

    #[test]
    fn a_malformed_tag_never_triggers_an_update() {
        assert!(!is_newer("", "0.2.11"));
        assert!(!is_newer("not-a-version", "0.2.11"));
    }

    #[test]
    fn a_prerelease_suffix_is_ignored_rather_than_mis_sorted() {
        assert!(is_newer("v0.2.12-rc1", "0.2.11"));
        assert!(!is_newer("v0.2.11-rc1", "0.2.11"));
    }

    #[test]
    fn install_kind_follows_the_resolved_path() {
        // The exact shape `npm/launcher/lib/resolve.js` spawns:
        // `<pkg>/bin/vaughan` inside the platform package's node_modules dir.
        assert_eq!(
            classify(Path::new(
                "/home/u/p/node_modules/vaughan-linux-x64/bin/vaughan"
            )),
            InstallKind::Npm
        );
        // A global npm prefix nests it one level deeper; still npm.
        assert_eq!(
            classify(Path::new(
                "/usr/lib/node_modules/vaughan/node_modules/vaughan-linux-x64/bin/vaughan"
            )),
            InstallKind::Npm
        );
        assert_eq!(
            classify(Path::new("/home/u/Vaughan-CLI/target/release/vaughan")),
            InstallKind::DevBuild
        );
        assert_eq!(
            classify(Path::new(
                "/home/u/Vaughan-CLI/target/railgun/release/vaughan"
            )),
            InstallKind::DevBuild
        );
        // A debug build is a dev build too: classifying it as a release install
        // would let `vaughan update` download a tarball over a checkout build.
        assert_eq!(
            classify(Path::new("/home/u/Vaughan-CLI/target/debug/vaughan")),
            InstallKind::DevBuild
        );
        assert_eq!(
            classify(Path::new("/home/u/.local/bin/vaughan")),
            InstallKind::Tarball
        );
        // An install directory that merely contains the word `target` is not a
        // checkout — a cargo profile directory has to be present as well.
        assert_eq!(
            classify(Path::new("/opt/target/bin/vaughan")),
            InstallKind::Tarball
        );
    }

    /// The real `SHA256SUMS` layout: `sha256sum` output, several assets.
    const SUMS: &str = concat!(
        "aaaa1111  vaughan-linux-aarch64.tar.gz\n",
        "bbbb2222  vaughan-linux-x86_64.tar.gz\n",
        "cccc3333  vaughan-macos-aarch64.tar.gz\n",
    );

    #[test]
    fn the_checksum_is_taken_from_the_matching_asset_line() {
        assert_eq!(
            expected_sha256(SUMS, "vaughan-linux-x86_64.tar.gz").unwrap(),
            "bbbb2222"
        );
        assert_eq!(
            expected_sha256(SUMS, "vaughan-macos-aarch64.tar.gz").unwrap(),
            "cccc3333"
        );
    }

    #[test]
    fn a_longer_asset_name_ending_in_ours_is_not_mistaken_for_it() {
        // This is why the match is on the whole name field and not a line
        // suffix: a suffix test would return the wrong digest here.
        let sums = "dead9999  extra-vaughan-linux-x86_64.tar.gz\n";
        assert!(expected_sha256(sums, "vaughan-linux-x86_64.tar.gz").is_err());
    }

    #[test]
    fn binary_mode_star_prefixes_still_match() {
        let sums = "beef7777 *vaughan-linux-x86_64.tar.gz\n";
        assert_eq!(
            expected_sha256(sums, "vaughan-linux-x86_64.tar.gz").unwrap(),
            "beef7777"
        );
    }

    #[test]
    fn a_missing_entry_is_an_error_rather_than_a_skipped_check() {
        assert!(expected_sha256(SUMS, "vaughan-windows-x86_64.tar.gz").is_err());
    }

    #[test]
    fn verification_accepts_the_real_digest_and_rejects_anything_else() {
        let bytes = b"vaughan release bytes";
        let digest = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(bytes))
        };
        assert!(verify_sha256(bytes, &digest, "a.tar.gz").is_ok());
        // Case-insensitive, because SHA256SUMS may be written either way.
        assert!(verify_sha256(bytes, &digest.to_uppercase(), "a.tar.gz").is_ok());
        assert!(verify_sha256(bytes, "00ff", "a.tar.gz").is_err());
        assert!(verify_sha256(b"tampered", &digest, "a.tar.gz").is_err());
    }

    /// End-to-end over the half that does not need the network: build a real
    /// tarball, unpack it, and replace a stand-in binary with its contents.
    #[test]
    fn extract_and_replace_swaps_the_target_binary() {
        let scratch = ScratchDir::new().expect("scratch");
        let stage = scratch.path().join("stage");
        let dest_dir = scratch.path().join("bin");
        std::fs::create_dir_all(&stage).expect("stage dir");
        std::fs::create_dir_all(&dest_dir).expect("dest dir");

        // The archive layout a release ships: `vaughan` at the root.
        std::fs::write(stage.join("vaughan"), b"#!/bin/sh\necho new\n").expect("payload");
        let archive = scratch.path().join("vaughan-test.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&stage)
            .arg("vaughan")
            .status()
            .expect("tar -czf");
        assert!(status.success(), "could not build the test archive");

        let exe = dest_dir.join("vaughan");
        std::fs::write(&exe, b"old binary").expect("old binary");

        let unpack = scratch.path().join("unpack");
        std::fs::create_dir_all(&unpack).expect("unpack dir");
        extract_and_replace(&archive, &unpack, &exe).expect("replace");

        assert_eq!(
            std::fs::read(&exe).expect("read back"),
            b"#!/bin/sh\necho new\n"
        );
        // The staged sibling must not be left behind.
        assert!(!dest_dir.join(".vaughan.update.tmp").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o755, "the new binary must stay executable");
        }
    }

    #[test]
    fn an_archive_without_a_vaughan_at_its_root_is_refused() {
        let scratch = ScratchDir::new().expect("scratch");
        let stage = scratch.path().join("stage");
        std::fs::create_dir_all(&stage).expect("stage dir");
        std::fs::write(stage.join("something-else"), b"x").expect("payload");
        let archive = scratch.path().join("wrong.tar.gz");
        let status = Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&stage)
            .arg("something-else")
            .status()
            .expect("tar -czf");
        assert!(status.success());

        let exe = scratch.path().join("vaughan");
        std::fs::write(&exe, b"untouched").expect("exe");
        let unpack = scratch.path().join("unpack");
        std::fs::create_dir_all(&unpack).expect("unpack dir");

        assert!(extract_and_replace(&archive, &unpack, &exe).is_err());
        // The existing binary must survive a bad archive.
        assert_eq!(std::fs::read(&exe).expect("read back"), b"untouched");
    }

    #[test]
    fn the_platform_tag_is_one_of_the_published_three_or_an_error() {
        match platform_tag() {
            Ok(tag) => assert!(
                ["linux-x86_64", "linux-aarch64", "macos-aarch64"].contains(&tag.as_str()),
                "unexpected published platform tag: {tag}"
            ),
            // An unsupported host must say so rather than inventing an asset.
            Err(e) => assert!(e.to_string().contains("no published release")),
        }
    }
}
