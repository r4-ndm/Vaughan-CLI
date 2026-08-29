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
pub fn try_open(
    url: &str,
    allow_hosts: &[String],
    agent_browser_control: bool,
) -> Result<String, String> {
    try_open_with_cmd(
        url,
        allow_hosts,
        env::var(DAPP_BROWSER_CMD_ENV).ok().as_deref(),
        agent_browser_control,
    )
}

/// [`try_open`] with an explicit command override (tests; `None` = PATH probe).
pub(crate) fn try_open_with_cmd(
    url: &str,
    allow_hosts: &[String],
    cmd_override: Option<&str>,
    agent_browser_control: bool,
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
    let cdp_port = if cdp_port == 0 { None } else { Some(cdp_port) };

    let chrome = env::var(DAPP_BROWSER_CHROME_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty());

    if let Some(raw) = cmd_override {
        return spawn_cmd(raw, url, allow_hosts, cdp_port, chrome.as_deref());
    }

    for bin in ["vaughan-dapp-browser"] {
        if spawn_bin(
            Path::new(bin),
            url,
            allow_hosts,
            cdp_port,
            chrome.as_deref(),
        ) {
            return Ok(format!(
                "opened in VB ({bin}) — green inject banner; approve sign/send in Vaughan TUI"
            ));
        }
    }

    for bin in extra_bin_paths() {
        if spawn_bin(&bin, url, allow_hosts, cdp_port, chrome.as_deref()) {
            return Ok(format!(
                "opened in VB ({}) — green inject banner; approve sign/send in Vaughan TUI",
                bin.display()
            ));
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

fn append_chrome(cmd: &mut Command, chrome: Option<&str>) {
    if let Some(bin) = chrome {
        let t = bin.trim();
        if !t.is_empty() {
            cmd.arg("--chrome").arg(t);
        }
    }
}

fn spawn_cmd(
    raw: &str,
    url: &str,
    allow_hosts: &[String],
    cdp_port: Option<u16>,
    chrome: Option<&str>,
) -> Result<String, String> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let Some((bin, args)) = parts.split_first() else {
        return Err(format!("{DAPP_BROWSER_CMD_ENV} is empty"));
    };
    let mut cmd = Command::new(bin);
    cmd.args(args);
    cmd.arg("--url").arg(url);
    append_allow_hosts(&mut cmd, allow_hosts);
    append_chrome(&mut cmd, chrome);
    if let Some(port) = cdp_port {
        cmd.arg("--cdp-port").arg(port.to_string());
    }
    // Detach from TUI stdio so a ratatui redraw / pipe close does not kill Chromium.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(_) => Ok(format!(
            "opened in VB ({bin}) — green inject banner; approve sign/send in Vaughan TUI"
        )),
        Err(e) => Err(format!("{DAPP_BROWSER_CMD_ENV} (`{bin}`) failed: {e}")),
    }
}

fn spawn_bin(
    bin: &Path,
    url: &str,
    allow_hosts: &[String],
    cdp_port: Option<u16>,
    chrome: Option<&str>,
) -> bool {
    let mut cmd = Command::new(bin);
    cmd.arg("--url").arg(url);
    append_allow_hosts(&mut cmd, allow_hosts);
    append_chrome(&mut cmd, chrome);
    if let Some(port) = cdp_port {
        cmd.arg("--cdp-port").arg(port.to_string());
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.spawn().is_ok()
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
        let err = try_open("file:///tmp/x", &[], false).unwrap_err();
        assert!(err.contains("http"));
    }
}
