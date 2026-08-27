//! VB (`vaughan-dapp-browser`) session metadata, allowlist checks, and CDP helpers.
//!
//! Used by MCP `browser_*` tools to open/navigate/status without pulling CEF into core.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use url::Url;

use crate::core::persistence::{trusted_dapp_allow_hosts, StateManager};
use crate::error::WalletError;

/// Default CDP port when MCP opens VB (`VAUGHAN_DAPP_BROWSER_CDP_PORT` overrides).
pub const DEFAULT_CDP_PORT: u16 = 9222;

/// On-disk session written by `vaughan-dapp-browser` when `--cdp-port` is set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VbSession {
    pub cdp_url: String,
    pub cdp_token: String,
    #[serde(default)]
    pub allow_suffixes: Vec<String>,
    pub updated_at: u64,
}

/// Path to `vb.session` under the Vaughan data dir.
pub fn vb_session_path() -> Option<PathBuf> {
    Some(dirs::data_dir()?.join("vaughan-cli").join("vb.session"))
}

/// Read the latest VB CDP session file, if present.
pub fn read_vb_session() -> Result<Option<VbSession>, WalletError> {
    let Some(path) = vb_session_path() else {
        return Ok(None);
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(None);
    };
    let session: VbSession = serde_json::from_slice(&bytes)
        .map_err(|e| WalletError::Other(format!("vb.session parse: {e}")))?;
    Ok(Some(session))
}

/// Resolve CDP port from env or default.
pub fn resolve_cdp_port() -> u16 {
    std::env::var("VAUGHAN_DAPP_BROWSER_CDP_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p != 0)
        .unwrap_or(DEFAULT_CDP_PORT)
}

/// Chrome internals / empty loads — do not treat as allowlist violations.
pub fn is_ephemeral_url(raw: &str) -> bool {
    let t = raw.trim();
    t.is_empty()
        || t == "about:blank"
        || t.starts_with("chrome://")
        || t.starts_with("chrome-error://")
        || t.starts_with("chrome-extension://")
        || t.starts_with("devtools://")
        || t.starts_with("data:")
        || t.starts_with("blob:")
}

/// Validate `raw` against host suffixes (same semantics as `vaughan-dapp-browser` allowlist).
pub fn check_url_allowed(raw: &str, suffixes: &[String]) -> Result<(), WalletError> {
    if is_ephemeral_url(raw) {
        return Ok(());
    }
    let u = Url::parse(raw).map_err(|e| WalletError::InvalidTransaction(format!("url: {e}")))?;
    match u.scheme() {
        "https" => {}
        "http" => {
            let host = u.host_str().unwrap_or("");
            if host != "localhost" && host != "127.0.0.1" {
                return Err(WalletError::InvalidTransaction(
                    "http only allowed for localhost".into(),
                ));
            }
        }
        other => {
            return Err(WalletError::InvalidTransaction(format!(
                "unsupported scheme `{other}`"
            )));
        }
    }
    let host = u
        .host_str()
        .ok_or_else(|| WalletError::InvalidTransaction("url missing host".into()))?
        .to_ascii_lowercase();
    if suffixes
        .iter()
        .any(|suf| host == *suf || host.ends_with(&format!(".{suf}")))
    {
        Ok(())
    } else {
        Err(WalletError::InvalidTransaction(format!(
            "host `{host}` not in VB allowlist"
        )))
    }
}

/// Build allow suffixes for a profile: trusted dApp hosts + optional URL host.
pub fn allow_suffixes_for_profile(profile: &str, url: &str) -> Result<Vec<String>, WalletError> {
    let wallet_path = StateManager::profile_path(profile)?;
    let mgr = StateManager::new(wallet_path);
    let state = mgr.load()?;
    let mut suffixes = trusted_dapp_allow_hosts(&state.trusted_dapps);
    if let Ok(parsed) = Url::parse(url) {
        if let Some(host) = parsed.host_str() {
            push_host_suffixes(&mut suffixes, host);
        }
    }
    Ok(suffixes)
}

fn push_host_suffixes(out: &mut Vec<String>, host: &str) {
    let h = host.trim().trim_start_matches('.').to_ascii_lowercase();
    if h.is_empty() {
        return;
    }
    if !out.iter().any(|x| x == &h) {
        out.push(h.clone());
    }
    let parts: Vec<&str> = h.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 {
        let parent = format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1]);
        if !out.iter().any(|x| x == &parent) {
            out.push(parent);
        }
    }
}

/// True when the CDP HTTP endpoint responds.
pub async fn cdp_alive(cdp_url: &str) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let url = format!("{}/json/version", cdp_url.trim_end_matches('/'));
    client.get(url).send().await.is_ok()
}

/// Open a new tab at `url` via CDP HTTP `PUT /json/new?{url}`.
pub async fn cdp_open_url(cdp_url: &str, url: &str) -> Result<(), WalletError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| WalletError::NetworkError(format!("cdp http: {e}")))?;
    let put_url = format!("{}/json/new?{}", cdp_url.trim_end_matches('/'), url);
    let resp = client
        .put(put_url)
        .send()
        .await
        .map_err(|e| WalletError::NetworkError(format!("cdp new tab: {e}")))?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(WalletError::NetworkError(format!(
            "cdp new tab HTTP {}",
            resp.status()
        )))
    }
}

/// List page targets from CDP `/json/list`.
pub async fn cdp_list_pages(cdp_url: &str) -> Result<Vec<serde_json::Value>, WalletError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| WalletError::NetworkError(format!("cdp http: {e}")))?;
    let url = format!("{}/json/list", cdp_url.trim_end_matches('/'));
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| WalletError::NetworkError(format!("cdp list: {e}")))?;
    let resp = resp
        .error_for_status()
        .map_err(|e| WalletError::NetworkError(format!("cdp list: {e}")))?;
    resp.json()
        .await
        .map_err(|e| WalletError::NetworkError(format!("cdp list json: {e}")))
}

/// Spawn `vaughan-dapp-browser` detached (same flags as TUI soft-launch).
pub fn spawn_dapp_browser(
    url: &str,
    allow_hosts: &[String],
    cdp_port: u16,
) -> Result<(), WalletError> {
    let bin = resolve_dapp_browser_bin().ok_or_else(|| {
        WalletError::Other(
            "vaughan-dapp-browser not found on PATH — install with cargo install -p vaughan-dapp-browser"
                .into(),
        )
    })?;
    let mut cmd = Command::new(&bin);
    cmd.arg("--url").arg(url);
    for h in allow_hosts {
        let t = h.trim();
        if !t.is_empty() {
            cmd.arg("--allow-host").arg(t);
        }
    }
    if cdp_port != 0 {
        cmd.arg("--cdp-port").arg(cdp_port.to_string());
    }
    if let Ok(chrome) = std::env::var("VAUGHAN_DAPP_BROWSER_CHROME") {
        let t = chrome.trim();
        if !t.is_empty() {
            cmd.arg("--chrome").arg(t);
        }
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
        .map_err(|e| WalletError::Other(format!("spawn vaughan-dapp-browser: {e}")))?;
    Ok(())
}

fn resolve_dapp_browser_bin() -> Option<PathBuf> {
    if let Some(path) = find_on_path("vaughan-dapp-browser") {
        return Some(path);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("vaughan-dapp-browser");
            if sibling.is_file() {
                return Some(sibling);
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
                return Some(p);
            }
        }
    }
    None
}

fn find_on_path(bin: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&paths) {
        let candidate = dir.join(bin);
        if is_executable_file(&candidate) {
            return Some(candidate);
        }
    }
    None
}

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

/// Poll until CDP responds or timeout.
pub async fn wait_for_cdp(cdp_url: &str, max_wait: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < max_wait {
        if cdp_alive(cdp_url).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_url_allows_suffix_and_subdomain() {
        let suffixes = vec!["pulsex.com".into()];
        check_url_allowed("https://app.pulsex.com/swap", &suffixes).unwrap();
        assert!(check_url_allowed("https://evil.com/", &suffixes).is_err());
    }

    #[test]
    fn ephemeral_urls_pass() {
        assert!(check_url_allowed("about:blank", &[]).is_ok());
        assert!(is_ephemeral_url("chrome://newtab/"));
    }

    #[test]
    fn resolve_cdp_port_default() {
        std::env::remove_var("VAUGHAN_DAPP_BROWSER_CDP_PORT");
        assert_eq!(resolve_cdp_port(), DEFAULT_CDP_PORT);
    }

    #[test]
    fn http_localhost_still_requires_allowlist_suffix() {
        assert!(check_url_allowed("http://127.0.0.1:8080/", &[]).is_err());
        check_url_allowed("http://127.0.0.1:8080/", &["127.0.0.1".into()]).unwrap();
        assert!(check_url_allowed("http://evil.com/", &[]).is_err());
    }
}
