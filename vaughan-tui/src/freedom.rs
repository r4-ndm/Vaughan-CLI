//! Launch **VB** (`vaughan-dapp-browser`) or **Freedom Browser** (dev fallback) for a whitelisted dApp URL.
//!
//! **Product order:** Browserless Pulse (Ag / Dex / Browse / MCP) first; **VB** when
//! a page is required; Freedom only if VB is missing and the user has a local
//! checkout (`VAUGHAN_FREEDOM_CMD`). Freedom integration is **parked** until upstream
//! [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195) merges —
//! see `docs/freedom-browser-status.md`.
//!
//! Terminals often auto-link plain `https://…` text and open the **system**
//! browser on click. [`display_url`] inserts a zero-width space so that does not
//! happen; use **Enter** to open in VB or Freedom.
//!
//! Set `VAUGHAN_FREEDOM_CMD` to the Freedom binary (plus optional args; URL is
//! appended). Example for a local Electron checkout:
//! `VAUGHAN_FREEDOM_CMD="$HOME/Desktop/freedom-browser/node_modules/.bin/electron $HOME/Desktop/freedom-browser"`

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::dapp_browser;

/// Status / error when neither VB nor Freedom can be launched.
pub const FREEDOM_REQUIRED_MSG: &str =
    "Install vaughan-dapp-browser (cargo install -p vaughan-dapp-browser) for the web side door. Freedom is parked until upstream PR #195 merges (devs: VAUGHAN_FREEDOM_CMD). Unlock Vaughan, then press Enter — do not click the URL (terminal opens your system browser).";

/// Break terminal auto-link detectors (Kitty, VTE, …) so a click does not open
/// Chrome/Brave. The real URL is unchanged for [`open_dapp_url`].
pub fn display_url(url: &str) -> String {
    url.replacen("://", ":\u{200B}//", 1)
}

/// Host (+ path) for the Web list — avoids full `https://` auto-links looking broken.
pub fn display_host(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let host = u.host_str().unwrap_or("?");
            let path = u.path();
            if path.is_empty() || path == "/" {
                host.to_string()
            } else {
                format!("{host}{path}")
            }
        }
        Err(_) => display_url(url),
    }
}

/// Open `url` in Vaughan dApp browser if present, else Freedom (never `xdg-open`).
pub fn open_dapp_url(
    url: &str,
    allow_hosts: &[String],
    agent_browser_control: bool,
) -> Result<String, String> {
    open_dapp_url_with_cmds(
        url,
        allow_hosts,
        agent_browser_control,
        env::var(dapp_browser::DAPP_BROWSER_CMD_ENV).ok().as_deref(),
        env::var("VAUGHAN_FREEDOM_CMD").ok().as_deref(),
    )
}

/// [`open_dapp_url`] with explicit command overrides (tests; `None` = probe).
fn open_dapp_url_with_cmds(
    url: &str,
    allow_hosts: &[String],
    agent_browser_control: bool,
    dapp_browser_cmd: Option<&str>,
    freedom_cmd: Option<&str>,
) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("empty URL".into());
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL must be http or https".into());
    }

    match dapp_browser::try_open_with_cmd(url, allow_hosts, dapp_browser_cmd, agent_browser_control)
    {
        Ok(msg) => return Ok(msg),
        Err(e) => {
            tracing::debug!(error = %e, "vaughan-dapp-browser unavailable; trying Freedom");
        }
    }

    if let Some(raw) = freedom_cmd {
        return spawn_freedom_cmd(raw, url);
    }

    for bin in ["freedom-browser", "Freedom", "freedom"] {
        if Command::new(bin).arg(url).spawn().is_ok() {
            return Ok(format!("opened in Freedom ({bin})"));
        }
    }

    for bin in extra_bin_paths() {
        if try_spawn_bin(&bin, url) {
            return Ok(format!("opened in Freedom ({})", bin.display()));
        }
    }

    Err(FREEDOM_REQUIRED_MSG.into())
}

fn spawn_freedom_cmd(raw: &str, url: &str) -> Result<String, String> {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    let Some((bin, args)) = parts.split_first() else {
        return Err(format!(
            "VAUGHAN_FREEDOM_CMD is empty or invalid. {FREEDOM_REQUIRED_MSG}"
        ));
    };
    let mut cmd = Command::new(bin);
    cmd.args(args).arg(url);
    if let Some(dir) = args.last().map(Path::new).filter(|p| p.is_dir()) {
        cmd.current_dir(dir);
    }
    match cmd.spawn() {
        Ok(_) => Ok(format!("opened in Freedom ({bin})")),
        Err(_) => Err(format!(
            "VAUGHAN_FREEDOM_CMD (`{bin}`) failed to start. {FREEDOM_REQUIRED_MSG}"
        )),
    }
}

fn try_spawn_bin(bin: &Path, url: &str) -> bool {
    Command::new(bin).arg(url).spawn().is_ok()
}

fn extra_bin_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = dirs::home_dir() {
        for rel in [
            ".local/bin/freedom",
            ".local/bin/freedom-browser",
            ".local/bin/Freedom",
            "Applications/Freedom Browser.AppImage",
            "Desktop/freedom-browser/freedom",
            "src/freedom-browser/freedom",
            "code/freedom-browser/freedom",
        ] {
            let p = home.join(rel);
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
        let err = open_dapp_url("file:///tmp/x", &[], false).unwrap_err();
        assert!(err.contains("http"));
    }

    #[test]
    fn display_url_defangs_scheme() {
        let d = display_url("https://app.pulsex.com/");
        assert!(d.contains('\u{200B}'));
        assert!(d.contains(":\u{200B}//"));
    }

    #[test]
    fn display_host_strips_scheme() {
        assert_eq!(display_host("https://app.pulsex.com/"), "app.pulsex.com");
        assert_eq!(
            display_host("https://app.pulsex.com/swap"),
            "app.pulsex.com/swap"
        );
    }

    #[test]
    fn missing_browsers_prompts_install() {
        // Explicit bogus commands force both soft-launch paths to fail —
        // no env mutation (process-global, unsafe on edition 2024).
        let err = open_dapp_url_with_cmds(
            "https://app.pulsex.com/",
            &[],
            false,
            Some("vaughan-dapp-browser-not-installed-for-tests"),
            Some("vaughan-freedom-not-installed-for-tests"),
        )
        .unwrap_err();
        assert!(
            err.contains("Install") || err.contains("VAUGHAN_FREEDOM_CMD"),
            "expected install prompt, got: {err}"
        );
    }
}
