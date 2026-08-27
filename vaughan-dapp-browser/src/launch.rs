//! Chromium launch with an unpacked extension that injects EIP-1193.
//!
//! Avoids chromiumoxide page automation (flaky on heavy dApps). The window
//! stays open until the user closes it.
//!
//! **Phase 1 gap:** the host allowlist is enforced for the *initial* `--url`
//! only. In-tab navigation is not yet gated (CDP / extension watcher later).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

use crate::allowlist::Allowlist;
use crate::extension_assets;
use crate::provider_inject;

pub struct LaunchOpts {
    pub url: String,
    pub provider_ws: String,
    pub allow: Allowlist,
    pub cdp_port: u16,
    pub chrome: Option<String>,
}

/// Reject non-loopback provider WebSocket URLs (extension must not phone home).
pub fn validate_provider_ws(raw: &str) -> Result<(), String> {
    let u = url::Url::parse(raw).map_err(|e| format!("invalid --provider-ws: {e}"))?;
    if !matches!(u.scheme(), "ws" | "wss") {
        return Err("--provider-ws must be ws:// or wss://".into());
    }
    let host = u.host_str().unwrap_or("").to_ascii_lowercase();
    if host != "127.0.0.1" && host != "localhost" {
        return Err("--provider-ws must target 127.0.0.1 or localhost".into());
    }
    Ok(())
}

/// Resolve provider WebSocket URL, attaching `access_token` when available.
///
/// Token sources (first wins): `--provider-ws` already containing `access_token`,
/// `VAUGHAN_PROVIDER_SESSION_TOKEN`, or `provider.session` under the profile dir.
pub fn resolve_provider_ws(raw: &str) -> Result<String, String> {
    validate_provider_ws(raw)?;
    if raw.contains("access_token=") {
        return Ok(raw.to_string());
    }
    let token = std::env::var("VAUGHAN_PROVIDER_SESSION_TOKEN")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(read_default_provider_session);
    let Some(token) = token else {
        return Ok(raw.to_string());
    };
    let sep = if raw.contains('?') { '&' } else { '?' };
    Ok(format!("{raw}{sep}access_token={token}"))
}

fn read_default_provider_session() -> Option<String> {
    let base = dirs::data_dir()?;
    let mut paths = vec![
        base.join("vaughan-cli/provider.session"),
        base.join("vaughan-cli").join("provider.session"),
    ];
    if let Ok(rd) = std::fs::read_dir(base.join("vaughan-cli/profiles")) {
        for e in rd.flatten() {
            paths.push(e.path().join("provider.session"));
        }
    }
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for p in paths {
        let Ok(meta) = std::fs::metadata(&p) else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(tok) = std::fs::read_to_string(&p) else {
            continue;
        };
        let tok = tok.trim().to_string();
        if tok.is_empty() {
            continue;
        }
        if best.as_ref().map(|(t, _)| modified > *t).unwrap_or(true) {
            best = Some((modified, tok));
        }
    }
    best.map(|(_, t)| t)
}

/// Redact the session token from a provider WS URL before printing/logging.
pub fn redact_access_token(raw: &str) -> String {
    match raw.split_once("access_token=") {
        Some((head, _)) => format!("{head}access_token=<redacted>"),
        None => raw.to_string(),
    }
}

fn pick_chrome(explicit: &Option<String>) -> Result<String, String> {
    if let Some(p) = explicit {
        let path = resolve_executable(p).ok_or_else(|| format!("chrome binary not found: {p}"))?;
        return Ok(path);
    }
    for cand in [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
        "chrome",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
    ] {
        if let Some(path) = resolve_executable(cand) {
            return Ok(path);
        }
    }
    Err(
        "no Chromium/Chrome binary found (install chromium or pass --chrome /usr/bin/chromium)"
            .into(),
    )
}

fn resolve_executable(bin: &str) -> Option<String> {
    let candidate = if bin.contains('/') {
        PathBuf::from(bin)
    } else {
        // PATH lookup in-process: no `sh -c`, so no quoting/injection surface
        // for the `--chrome` flag value.
        let paths = std::env::var_os("PATH")?;
        let mut found = None;
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join(bin);
            if is_executable_file(&candidate) {
                found = Some(candidate);
                break;
            }
        }
        found?
    };
    let meta = std::fs::metadata(&candidate).ok()?;
    if !meta.is_file() {
        return None;
    }
    std::fs::canonicalize(&candidate)
        .ok()
        .map(|p| p.display().to_string())
        .or_else(|| Some(candidate.display().to_string()))
}

/// Regular file with an exec bit (mirrors what `command -v` would accept).
fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
        .unwrap_or(0)
}

fn write_inject_extension(dir: &Path, provider_ws: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("ext dir: {e}"))?;
    std::fs::write(dir.join("manifest.json"), extension_assets::manifest_json())
        .map_err(|e| format!("write manifest: {e}"))?;
    std::fs::write(
        dir.join("background.js"),
        extension_assets::background_js(provider_ws),
    )
    .map_err(|e| format!("write background.js: {e}"))?;
    std::fs::write(
        dir.join("content_bridge.js"),
        extension_assets::content_bridge_js(),
    )
    .map_err(|e| format!("write content_bridge.js: {e}"))?;
    std::fs::write(dir.join("inject.js"), provider_inject::script())
        .map_err(|e| format!("write inject.js: {e}"))?;
    Ok(())
}

/// Serve the inject self-check page on loopback; returns base URL.
fn spawn_self_check_server() -> Result<(std::thread::JoinHandle<()>, String), String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    listener
        .set_nonblocking(false)
        .map_err(|e| format!("blocking: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let body = provider_inject::self_check_html();
    let handle = thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = handle_http(&mut stream, body);
        }
    });
    thread::sleep(Duration::from_millis(50));
    Ok((handle, format!("http://127.0.0.1:{port}/")))
}

fn handle_http(stream: &mut TcpStream, body: &str) -> std::io::Result<()> {
    let mut buf = [0u8; 1024];
    let _ = stream.read(&mut buf)?;
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(resp.as_bytes())?;
    Ok(())
}

/// Run headed Chromium until the user closes the window.
pub fn run_browser(opts: LaunchOpts) -> Result<(), String> {
    validate_provider_ws(&opts.provider_ws)?;
    opts.allow
        .check_url(&opts.url)
        .map_err(|e| format!("url not allowlisted: {e}"))?;

    let chrome = pick_chrome(&opts.chrome)?;
    let export_cdp = opts.cdp_port != 0;
    // Unique temp dir even when CDP is off (no debugging port allocated).
    let session = if export_cdp {
        opts.cdp_port
    } else {
        free_port().max(1)
    };

    let base = std::env::temp_dir().join(format!("vaughan-dapp-browser-{session}"));
    let profile = base.join("profile");
    let ext = base.join("ext");
    let _ = std::fs::remove_dir_all(&base);
    // Create the session dir atomically and owner-only. The extension bundle
    // embeds the provider session token and $TMPDIR is shared: a plain
    // create_dir_all between the remove and create would follow a symlink
    // planted by another local user, leaking the token into their directory.
    // `create` fails if anything appears in between, so races fail closed.
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&base)
            .map_err(|e| format!("session dir: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir(&base).map_err(|e| format!("session dir: {e}"))?;
    }
    // Defense in depth: never write the token-bearing bundle through a link.
    let meta = std::fs::symlink_metadata(&base).map_err(|e| format!("session dir stat: {e}"))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err("session dir is not a real directory".into());
    }
    std::fs::create_dir_all(&profile).map_err(|e| format!("profile: {e}"))?;
    write_inject_extension(&ext, &opts.provider_ws)?;

    eprintln!("vaughan-dapp-browser: chrome={chrome}");
    eprintln!(
        "vaughan-dapp-browser: CSP-safe extension → {} (provider {}, origin {}, id {})",
        opts.url,
        redact_access_token(&opts.provider_ws),
        extension_assets::EXTENSION_ORIGIN,
        extension_assets::EXTENSION_ID
    );
    eprintln!(
        "vaughan-dapp-browser: note — initial URL allowlisted; in-tab navigation not yet gated"
    );
    if export_cdp {
        println!(
            "{}",
            serde_json::json!({
                "cdp": format!("http://127.0.0.1:{}", opts.cdp_port),
                "agentControl": true,
            })
        );
        eprintln!(
            "vaughan-dapp-browser: CDP exported on 127.0.0.1:{}",
            opts.cdp_port
        );
    } else {
        eprintln!("vaughan-dapp-browser: agent CDP off (pass --cdp-port N to enable)");
    }
    eprintln!("Look for a green top banner: “Vaughan injected …”");
    eprintln!("dApps may say “Injected” — approve sign/send in the Vaughan TUI.");
    eprintln!("Close the Chromium window when finished.");

    let mut cmd = Command::new(&chrome);
    cmd.arg(format!("--user-data-dir={}", profile.display()))
        .arg(format!("--disable-extensions-except={}", ext.display()))
        .arg(format!("--load-extension={}", ext.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-dev-shm-usage")
        .arg("--disable-sync")
        .arg("--disable-features=DisableLoadExtensionCommandLineSwitch")
        .arg(format!("--app={}", opts.url))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    if export_cdp {
        cmd.arg(format!("--remote-debugging-port={}", opts.cdp_port))
            .arg("--remote-debugging-address=127.0.0.1");
    }

    let status = cmd
        .status()
        .map_err(|e| format!("spawn chromium ({chrome}): {e}"))?;

    let _ = std::fs::remove_dir_all(&base);

    if !status.success() {
        eprintln!("vaughan-dapp-browser: chromium exited ({status})");
    }
    Ok(())
}

/// Open the local inject self-check page (PASS/FAIL + request accounts).
pub fn run_self_check(provider_ws: &str, chrome: Option<String>) -> Result<(), String> {
    let provider_ws = resolve_provider_ws(provider_ws)?;
    let (_server, url) = spawn_self_check_server()?;
    let allow = Allowlist::from_url_and_hosts(&url, &[])?;
    eprintln!("vaughan-dapp-browser: self-check at {url}");
    run_browser(LaunchOpts {
        url,
        provider_ws,
        allow,
        cdp_port: 0,
        chrome,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ws_loopback_only() {
        assert!(validate_provider_ws("ws://127.0.0.1:8745").is_ok());
        assert!(validate_provider_ws("ws://localhost:8745").is_ok());
        assert!(validate_provider_ws("ws://evil.example:8745").is_err());
        assert!(validate_provider_ws("http://127.0.0.1:8745").is_err());
    }

    #[test]
    fn redact_access_token_hides_secret() {
        let redacted = redact_access_token("ws://127.0.0.1:8745?access_token=deadbeef");
        assert_eq!(redacted, "ws://127.0.0.1:8745?access_token=<redacted>");
        assert!(!redacted.contains("deadbeef"));
        // No token → unchanged.
        assert_eq!(
            redact_access_token("ws://127.0.0.1:8745"),
            "ws://127.0.0.1:8745"
        );
    }
}
