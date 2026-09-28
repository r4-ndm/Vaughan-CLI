//! Soft-launch the optional `vaughan-dapp-browser` binary.
//!
//! Prefer this Chromium shell when present; callers fall back to Freedom.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// When set, pass `--chrome` to the dApp browser binary (e.g. `/usr/bin/brave`).
pub const DAPP_BROWSER_CHROME_ENV: &str = "VAUGHAN_DAPP_BROWSER_CHROME";

/// Env override: full command prefix; URL is appended (same shape as Freedom).
pub const DAPP_BROWSER_CMD_ENV: &str = "VAUGHAN_DAPP_BROWSER_CMD";

/// When set to a non-zero port, pass `--cdp-port` for agent control.
pub const DAPP_BROWSER_CDP_ENV: &str = "VAUGHAN_DAPP_BROWSER_CDP_PORT";

/// Try to open `url` in `vaughan-dapp-browser`. `Err` if binary missing/fails.
///
/// `keep_profile` reuses the site's saved browser profile (cookies /
/// localStorage) instead of a throwaway one.
pub fn try_open(
    url: &str,
    allow_hosts: &[String],
    agent_browser_control: bool,
    keep_profile: bool,
) -> Result<String, String> {
    try_open_with_cmd(
        url,
        allow_hosts,
        env::var(DAPP_BROWSER_CMD_ENV).ok().as_deref(),
        agent_browser_control,
        keep_profile,
    )
}

/// Optional VB flags shared by the PATH probe and the command override.
struct VbFlags {
    cdp_port: Option<u16>,
    chrome: Option<String>,
    keep_profile: bool,
}

impl VbFlags {
    fn append(&self, cmd: &mut Command) {
        if let Some(bin) = self.chrome.as_deref() {
            cmd.arg("--chrome").arg(bin);
        }
        if let Some(port) = self.cdp_port {
            cmd.arg("--cdp-port").arg(port.to_string());
        }
        if self.keep_profile {
            cmd.arg("--keep-profile");
        }
    }
}

/// [`try_open`] with an explicit command override (tests; `None` = PATH probe).
pub(crate) fn try_open_with_cmd(
    url: &str,
    allow_hosts: &[String],
    cmd_override: Option<&str>,
    agent_browser_control: bool,
    keep_profile: bool,
) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("empty URL".into());
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL must be http or https".into());
    }

    let cdp_port = vaughan_core::core::vb_browser::spawn_cdp_port(agent_browser_control);
    let flags = VbFlags {
        cdp_port: (cdp_port != 0).then_some(cdp_port),
        chrome: env::var(DAPP_BROWSER_CHROME_ENV)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        keep_profile,
    };
    let opened = |bin: &str| {
        let saved = if keep_profile {
            " · saved site data"
        } else {
            ""
        };
        format!("opened in VB ({bin}){saved} — approve sign/send in Vaughan TUI")
    };

    if let Some(raw) = cmd_override {
        return spawn_cmd(raw, url, allow_hosts, &flags).map(|bin| opened(&bin));
    }

    for bin in ["vaughan-dapp-browser"] {
        if spawn_bin(Path::new(bin), url, allow_hosts, &flags)? {
            return Ok(opened(bin));
        }
    }

    for bin in extra_bin_paths() {
        if spawn_bin(&bin, url, allow_hosts, &flags)? {
            return Ok(opened(&bin.display().to_string()));
        }
    }

    Err("vaughan-dapp-browser not found on PATH".into())
}

fn append_allow_hosts(cmd: &mut Command, allow_hosts: &[String]) {
    for h in allow_hosts {
        let t = h.trim();
        if !t.is_empty() {
            cmd.arg("--allow-host").arg(t);
        }
    }
}

/// Spawn the `VAUGHAN_DAPP_BROWSER_CMD` prefix; returns the binary name.
fn spawn_cmd(
    raw: &str,
    url: &str,
    allow_hosts: &[String],
    flags: &VbFlags,
) -> Result<String, String> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let Some((bin, args)) = parts.split_first() else {
        return Err(format!("{DAPP_BROWSER_CMD_ENV} is empty"));
    };
    let mut cmd = Command::new(bin);
    cmd.args(args);
    cmd.arg("--url").arg(url);
    append_allow_hosts(&mut cmd, allow_hosts);
    flags.append(&mut cmd);
    // Detach from TUI stdio so a ratatui redraw / pipe close does not kill Chromium.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let child = cmd
        .spawn()
        .map_err(|e| format!("{DAPP_BROWSER_CMD_ENV} (`{bin}`) failed: {e}"))?;
    survived_launch(child)?;
    Ok((*bin).to_string())
}

/// `Ok(false)` when `bin` is not runnable (try the next path); `Err` when it
/// started but exited at once (bad flags, saved site already open, …).
fn spawn_bin(
    bin: &Path,
    url: &str,
    allow_hosts: &[String],
    flags: &VbFlags,
) -> Result<bool, String> {
    let mut cmd = Command::new(bin);
    cmd.arg("--url").arg(url);
    append_allow_hosts(&mut cmd, allow_hosts);
    flags.append(&mut cmd);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let Ok(child) = cmd.spawn() else {
        return Ok(false);
    };
    survived_launch(child)?;
    Ok(true)
}

/// VB validates flags / allowlist / saved profile before Chromium starts, so a
/// failure shows up as an exit within a few hundred ms. stderr is detached, so
/// point at the likely causes instead of the (lost) message.
fn survived_launch(mut child: std::process::Child) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(600);
    while std::time::Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                return Err(
                    "VB exited at launch — if this site is saved, it may already be open in VB; \
                     otherwise update VB: cargo install --path vaughan-dapp-browser"
                        .into(),
                );
            }
            Ok(Some(_)) => return Ok(()),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(_) => return Ok(()),
        }
    }
    Ok(())
}

fn extra_bin_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("vaughan-dapp-browser");
            if sibling.is_file() {
                out.push(sibling);
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        for rel in [
            ".cargo/bin/vaughan-dapp-browser",
            ".local/bin/vaughan-dapp-browser",
        ] {
            let p = home.join(rel);
            if p.is_file() {
                out.push(p);
            }
        }
    }
    // Dev workspace target (debug then release).
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(ws) = manifest_dir.parent() {
        for rel in [
            "target/debug/vaughan-dapp-browser",
            "target/release/vaughan-dapp-browser",
        ] {
            let p = ws.join(rel);
            if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_http() {
        let err = try_open("file:///tmp/x", &[], false, false).unwrap_err();
        assert!(err.contains("http"));
    }
}
