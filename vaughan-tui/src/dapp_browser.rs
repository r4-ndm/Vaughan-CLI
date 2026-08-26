//! Soft-launch the optional `vaughan-dapp-browser` binary.
//!
//! Prefer this Chromium shell when present; callers fall back to Freedom.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Env override: full command prefix; URL is appended (same shape as Freedom).
pub const DAPP_BROWSER_CMD_ENV: &str = "VAUGHAN_DAPP_BROWSER_CMD";

/// When set to a non-zero port, pass `--cdp-port` for agent control.
pub const DAPP_BROWSER_CDP_ENV: &str = "VAUGHAN_DAPP_BROWSER_CDP_PORT";

/// Try to open `url` in `vaughan-dapp-browser`. `Err` if binary missing/fails.
pub fn try_open(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("empty URL".into());
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL must be http or https".into());
    }

    let cdp_port = env::var(DAPP_BROWSER_CDP_ENV)
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p != 0);

    if let Ok(raw) = env::var(DAPP_BROWSER_CMD_ENV) {
        return spawn_cmd(&raw, url, cdp_port);
    }

    for bin in ["vaughan-dapp-browser"] {
        if spawn_bin(Path::new(bin), url, cdp_port) {
            return Ok(format!(
                "opened in Vaughan dApp browser ({bin}) — look for green inject banner; approve sign/send here"
            ));
        }
    }

    for bin in extra_bin_paths() {
        if spawn_bin(&bin, url, cdp_port) {
            return Ok(format!(
                "opened in Vaughan dApp browser ({}) — look for green inject banner; approve sign/send here",
                bin.display()
            ));
        }
    }

    Err("vaughan-dapp-browser not found on PATH".into())
}

fn spawn_cmd(raw: &str, url: &str, cdp_port: Option<u16>) -> Result<String, String> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let Some((bin, args)) = parts.split_first() else {
        return Err(format!("{DAPP_BROWSER_CMD_ENV} is empty"));
    };
    let mut cmd = Command::new(bin);
    cmd.args(args);
    cmd.arg("--url").arg(url);
    if let Some(port) = cdp_port {
        cmd.arg("--cdp-port").arg(port.to_string());
    }
    // Detach from TUI stdio so a ratatui redraw / pipe close does not kill Chromium.
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match cmd.spawn() {
        Ok(_) => Ok(format!(
            "opened in Vaughan dApp browser ({bin}) — look for green inject banner; approve sign/send here"
        )),
        Err(e) => Err(format!("{DAPP_BROWSER_CMD_ENV} (`{bin}`) failed: {e}")),
    }
}

fn spawn_bin(bin: &Path, url: &str, cdp_port: Option<u16>) -> bool {
    let mut cmd = Command::new(bin);
    cmd.arg("--url").arg(url);
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
        let err = try_open("file:///tmp/x").unwrap_err();
        assert!(err.contains("http"));
    }
}
