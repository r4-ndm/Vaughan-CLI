//! Launch Freedom Browser (or a system opener) for a whitelisted dApp URL.
//!
//! Freedom does not yet expose a stable `--url` flag; we try
//! `VAUGHAN_FREEDOM_CMD` (binary + optional args, URL appended), then common
//! binary names, then `xdg-open` / `open` as a fallback.

use std::env;
use std::process::Command;

/// Open `url` in Freedom when possible; otherwise the OS default browser.
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
        let parts: Vec<&str> = raw.split_whitespace().collect();
        if let Some((bin, args)) = parts.split_first() {
            let mut cmd = Command::new(bin);
            cmd.args(args).arg(url);
            match cmd.spawn() {
                Ok(_) => {
                    return Ok(format!("launched via VAUGHAN_FREEDOM_CMD ({bin})"));
                }
                Err(e) => {
                    return Err(format!("VAUGHAN_FREEDOM_CMD failed: {e}"));
                }
            }
        }
    }

    for bin in ["freedom-browser", "Freedom", "freedom"] {
        if Command::new(bin).arg(url).spawn().is_ok() {
            return Ok(format!("launched {bin}"));
        }
    }

    // Fallback: system URL opener (default browser — not Freedom-specific).
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "xdg-open"
    };
    let result = if opener == "cmd" {
        Command::new("cmd").args(["/C", "start", "", url]).spawn()
    } else {
        Command::new(opener).arg(url).spawn()
    };
    match result {
        Ok(_) => Ok(format!(
            "opened with {opener} (set VAUGHAN_FREEDOM_CMD to prefer Freedom)"
        )),
        Err(e) => Err(format!("could not open URL: {e}")),
    }
}
