//! Launch **Freedom Browser** for a whitelisted dApp URL.
//!
//! Vaughan only connects dApps through Freedom (local EIP-1193 bridge). There is
//! no system-browser fallback — without Freedom the user is told to install it.
//!
//! Terminals often auto-link plain `https://…` text and open the **system**
//! browser on click. [`display_url`] inserts a zero-width space so that does not
//! happen; use **Enter** to open in Freedom.
//!
//! Set `VAUGHAN_FREEDOM_CMD` to the Freedom binary (plus optional args; URL is
//! appended). Example for a local Electron checkout:
//! `VAUGHAN_FREEDOM_CMD="$HOME/Desktop/freedom-browser/node_modules/.bin/electron $HOME/Desktop/freedom-browser"`

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Status / error when Freedom is missing or cannot be launched.
pub const FREEDOM_REQUIRED_MSG: &str =
    "Install Freedom Browser to enable dApp connect (or set VAUGHAN_FREEDOM_CMD). Press Enter — do not click the URL (terminal opens your system browser).";

/// Break terminal auto-link detectors (Kitty, VTE, …) so a click does not open
/// Chrome/Brave. The real URL is unchanged for [`open_dapp_url`].
pub fn display_url(url: &str) -> String {
    url.replacen("://", ":\u{200B}//", 1)
}

/// Open `url` in Freedom Browser only (never `xdg-open` / system default).
pub fn open_dapp_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("empty URL".into());
    }
    let parsed = url::Url::parse(url).map_err(|e| format!("invalid URL: {e}"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("URL must be http or https".into());
    }

    if let Ok(raw) = env::var("VAUGHAN_FREEDOM_CMD") {
        return spawn_freedom_cmd(&raw, url);
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
        for rel in [".local/bin/freedom", ".local/bin/freedom-browser"] {
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
        let err = open_dapp_url("file:///tmp/x").unwrap_err();
        assert!(err.contains("http"));
    }

    #[test]
    fn display_url_defangs_scheme() {
        let d = display_url("https://app.pulsex.com/");
        assert!(d.contains('\u{200B}'));
        assert!(d.contains(":\u{200B}//"));
    }

    #[test]
    fn missing_freedom_prompts_install() {
        unsafe {
            env::set_var(
                "VAUGHAN_FREEDOM_CMD",
                "vaughan-freedom-not-installed-for-tests",
            );
        }
        let err = open_dapp_url("https://app.pulsex.com/").unwrap_err();
        assert!(
            err.contains("Install Freedom"),
            "expected install prompt, got: {err}"
        );
        unsafe {
            env::remove_var("VAUGHAN_FREEDOM_CMD");
        }
    }
}
