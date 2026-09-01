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
///
/// Only used as the enablement sentinel and as a last-resort fallback — fresh
/// spawns get a random loopback port via [`spawn_cdp_port`] so a malicious
/// local process cannot squat the well-known 9222 and impersonate VB.
pub const DEFAULT_CDP_PORT: u16 = 9222;

/// On-disk session written by `vaughan-dapp-browser` when `--cdp-port` is set.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VbSession {
    pub cdp_url: String,
    pub cdp_token: String,
    #[serde(default)]
    pub allow_suffixes: Vec<String>,
    /// PID of the `vaughan-dapp-browser` launcher process (0 for legacy files
    /// written before PID binding — treated as stale by [`vb_session_pid_matches`]).
    #[serde(default)]
    pub pid: u32,
    /// Per-launch extension seal key (hex, 32 bytes). The provider bridge
    /// learns it here and requires sealed page-origin assertions.
    #[serde(default)]
    pub extension_secret: String,
    /// Provider `access_token` baked into the extension at launch. When the
    /// TUI unlock rotates `provider.session`, a live VB with a stale token
    /// cannot reach the bridge — [`vb_session_provider_token_stale`] detects
    /// this so MCP respawns instead of reusing a dead session.
    #[serde(default)]
    pub provider_token: String,
    pub updated_at: u64,
}

/// Redacted: `cdp_token` and `extension_secret` are session secrets and must
/// never land in logs or debug output.
impl std::fmt::Debug for VbSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VbSession")
            .field("cdp_url", &self.cdp_url)
            .field("cdp_token", &"<redacted>")
            .field("allow_suffixes", &self.allow_suffixes)
            .field("pid", &self.pid)
            .field("extension_secret", &"<redacted>")
            .field("provider_token", &"<redacted>")
            .field("updated_at", &self.updated_at)
            .finish()
    }
}

/// Pinned CDP page target written by the MCP side after `browser_open` /
/// `browser_navigate`, so follow-up tools stay on the tab the agent opened
/// instead of silently attaching to whatever tab happens to be first.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VbTargetPin {
    pub cdp_url: String,
    pub target_id: String,
    pub updated_at: u64,
}

/// Vaughan data dir for VB session files. `VAUGHAN_VB_STATE_DIR` overrides
/// the default — tests point it at a tempdir so a live VB session on the
/// host machine can't contaminate assertions.
fn vb_state_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VAUGHAN_VB_STATE_DIR") {
        return Some(PathBuf::from(dir));
    }
    Some(dirs::data_dir()?.join("vaughan-cli"))
}

/// Path to `vb.session` under the Vaughan data dir.
pub fn vb_session_path() -> Option<PathBuf> {
    Some(vb_state_dir()?.join("vb.session"))
}

/// Path to the MCP-side pinned target file (`vb.target`) under the data dir.
pub fn vb_target_pin_path() -> Option<PathBuf> {
    Some(vb_state_dir()?.join("vb.target"))
}

/// Path to the VB spawn log (`vb.log`) under the Vaughan data dir.
pub fn vb_log_path() -> Option<PathBuf> {
    Some(vb_state_dir()?.join("vb.log"))
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

/// Persist the pinned page target (owner-only; session metadata, not a secret).
pub fn write_target_pin(cdp_url: &str, target_id: &str) -> Result<(), WalletError> {
    let Some(path) = vb_target_pin_path() else {
        return Ok(());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| WalletError::Other(format!("vb.target dir: {e}")))?;
    }
    let pin = VbTargetPin {
        cdp_url: cdp_url.to_string(),
        target_id: target_id.to_string(),
        updated_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    };
    let bytes = serde_json::to_vec(&pin)
        .map_err(|e| WalletError::Serialization(format!("vb.target json: {e}")))?;
    write_owner_only(&path, &bytes)
}

/// Read the pinned page target, if any.
pub fn read_target_pin() -> Option<VbTargetPin> {
    let path = vb_target_pin_path()?;
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Drop the pinned target (fresh spawn, stale session, agent control off).
pub fn clear_target_pin() {
    if let Some(path) = vb_target_pin_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(unix)]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), WalletError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| WalletError::Other(format!("write {}: {e}", path.display())))?;
    f.write_all(bytes)
        .map_err(|e| WalletError::Other(format!("write {}: {e}", path.display())))
}

#[cfg(not(unix))]
fn write_owner_only(path: &Path, bytes: &[u8]) -> Result<(), WalletError> {
    std::fs::write(path, bytes)
        .map_err(|e| WalletError::Other(format!("write {}: {e}", path.display())))
}

/// True when the PID recorded in `vb.session` is alive AND belongs to a
/// `vaughan-dapp-browser` process. Fail-closed: PID 0 (legacy file), a dead
/// PID, or a recycled PID now running something else all return false, so a
/// foreign Chromium squatting on the CDP port is never driven as "VB".
#[cfg(unix)]
pub fn vb_session_pid_matches(session: &VbSession) -> bool {
    if session.pid == 0 {
        return false;
    }
    let cmdline = match std::fs::read(format!("/proc/{}/cmdline", session.pid)) {
        Ok(b) => b,
        Err(_) => return false,
    };
    cmdline
        .split(|b| *b == 0)
        .next()
        .map(|argv0| {
            let s = String::from_utf8_lossy(argv0);
            s.rsplit('/')
                .next()
                .unwrap_or(&s)
                .starts_with("vaughan-dapp-browser")
        })
        .unwrap_or(false)
}

/// Non-unix fallback: no /proc, so only the presence of a nonzero PID is
/// checkable (documented weaker than the Linux binding).
#[cfg(not(unix))]
pub fn vb_session_pid_matches(session: &VbSession) -> bool {
    session.pid != 0
}

/// Resolve CDP port: env override wins; else toggle; default off (FR-7.5).
///
/// This is the *enablement* check (0 = agent control off). The value returned
/// for an enabled toggle is only a sentinel — fresh spawns must use
/// [`spawn_cdp_port`] to get a random loopback port instead of the well-known
/// [`DEFAULT_CDP_PORT`].
pub fn resolve_cdp_port(agent_control_enabled: bool) -> u16 {
    if let Ok(s) = std::env::var("VAUGHAN_DAPP_BROWSER_CDP_PORT") {
        if let Ok(p) = s.parse::<u16>() {
            return p;
        }
    }
    if agent_control_enabled {
        DEFAULT_CDP_PORT
    } else {
        0
    }
}

/// Port to pass to a fresh VB spawn: the env override wins (dev flow with a
/// fixed port); otherwise a random free loopback port so the CDP endpoint is
/// not at a well-known address a local process can squat. Returns 0 when
/// agent control is disabled.
pub fn spawn_cdp_port(agent_control_enabled: bool) -> u16 {
    if let Ok(s) = std::env::var("VAUGHAN_DAPP_BROWSER_CDP_PORT") {
        if let Ok(p) = s.parse::<u16>() {
            return p;
        }
    }
    if !agent_control_enabled {
        return 0;
    }
    free_loopback_port().unwrap_or(DEFAULT_CDP_PORT)
}

/// Ask the OS for a free loopback port. There is an inherent TOCTOU gap
/// between releasing the probe socket and Chromium binding the port; the
/// PID-binding check in [`vb_session_pid_matches`] is the real guard against
/// attaching to a foreign endpoint.
fn free_loopback_port() -> Option<u16> {
    let l = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    l.local_addr().ok().map(|a| a.port())
}

/// Remove stale VB CDP session metadata (e.g. when disabling agent control).
pub fn clear_vb_session() {
    if let Some(path) = vb_session_path() {
        let _ = std::fs::remove_file(path);
    }
    clear_target_pin();
}

/// Extract `access_token` from a resolved provider WebSocket URL.
pub fn extract_provider_access_token(ws_url: &str) -> Option<String> {
    let (_, tail) = ws_url.split_once("access_token=")?;
    let token = tail.split('&').next()?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Newest non-empty `provider.session` token across profile dirs (same rule as
/// `vaughan-dapp-browser` launch). Absent when the wallet is locked.
pub fn read_latest_provider_session_token() -> Option<String> {
    let base = dirs::data_dir()?.join("vaughan-cli");
    let mut paths = vec![base.join(crate::core::PROVIDER_SESSION_FILE)];
    if let Ok(rd) = std::fs::read_dir(base.join("profiles")) {
        for e in rd.flatten() {
            paths.push(e.path().join(crate::core::PROVIDER_SESSION_FILE));
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

/// True when VB's baked provider token no longer matches the live TUI session.
pub fn vb_session_provider_token_stale(session: &VbSession) -> bool {
    let Some(current) = read_latest_provider_session_token() else {
        return false;
    };
    session.provider_token.is_empty() || session.provider_token != current
}

/// Back-fill `provider_token` when an older `vaughan-dapp-browser` wrote a
/// session without it (extension still has the right token from launch).
pub fn patch_vb_session_provider_token() -> Result<(), WalletError> {
    let Some(mut session) = read_vb_session()? else {
        return Ok(());
    };
    if !session.provider_token.is_empty() {
        return Ok(());
    }
    let Some(token) = read_latest_provider_session_token() else {
        return Ok(());
    };
    session.provider_token = token;
    let Some(path) = vb_session_path() else {
        return Ok(());
    };
    let bytes = serde_json::to_vec(&session)
        .map_err(|e| WalletError::Serialization(format!("vb.session json: {e}")))?;
    write_owner_only(&path, &bytes)
}

/// Best-effort terminate of a stale VB launcher (Unix `kill`; no-op elsewhere).
pub fn terminate_vb_process(session: &VbSession) {
    if session.pid > 0 {
        #[cfg(unix)]
        {
            let _ = Command::new("kill").arg(session.pid.to_string()).status();
        }
    }
    clear_vb_session();
}

/// Chrome internals / empty loads — do not treat as allowlist violations.
///
/// `data:` and `blob:` are deliberately NOT ephemeral: as navigation targets
/// they would bypass the host allowlist with attacker-controlled markup, so
/// `check_url_allowed` rejects them as unsupported schemes.
pub fn is_ephemeral_url(raw: &str) -> bool {
    let t = raw.trim();
    t.is_empty()
        || t == "about:blank"
        || t.starts_with("chrome://")
        || t.starts_with("chrome-error://")
        || t.starts_with("chrome-extension://")
        || t.starts_with("devtools://")
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
///
/// The URL is percent-encoded into the query value — a raw interpolation
/// would let `&` / `#` in the target URL corrupt the request. Returns the new
/// tab's target id when Chrome reports it (used for target pinning).
pub async fn cdp_open_url(cdp_url: &str, url: &str) -> Result<Option<String>, WalletError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| WalletError::NetworkError(format!("cdp http: {e}")))?;
    let encoded: String = url::form_urlencoded::byte_serialize(url.as_bytes()).collect();
    let put_url = format!("{}/json/new?{}", cdp_url.trim_end_matches('/'), encoded);
    let resp = client
        .put(put_url)
        .send()
        .await
        .map_err(|e| WalletError::NetworkError(format!("cdp new tab: {e}")))?;
    if !resp.status().is_success() {
        return Err(WalletError::NetworkError(format!(
            "cdp new tab HTTP {}",
            resp.status()
        )));
    }
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| WalletError::NetworkError(format!("cdp new tab json: {e}")))?;
    Ok(body.get("id").and_then(|v| v.as_str()).map(str::to_string))
}

/// Open `url`, reusing an existing same-origin tab when one is open.
///
/// Agent navigations used to mint a fresh tab every call, so a quote tour
/// across two venues left half a dozen tabs and "first page" reads landed on
/// stale ones. Reuse matches agent intent ("go to Switch" = the Switch tab).
/// When the tab is already on the same path, only focuses it — no reload, so
/// wallet connect state is preserved. Returns `(target_id, reused)`.
pub async fn cdp_open_or_reuse(
    cdp_url: &str,
    url: &str,
) -> Result<(Option<String>, bool), WalletError> {
    let origin = Url::parse(url)
        .ok()
        .map(|u| u.origin().ascii_serialization());
    if let Some(origin) = origin {
        if let Ok(pages) = cdp_list_pages(cdp_url).await {
            for page in &pages {
                if page.get("type").and_then(|t| t.as_str()) != Some("page") {
                    continue;
                }
                let page_origin = page
                    .get("url")
                    .and_then(|u| u.as_str())
                    .and_then(|u| Url::parse(u).ok())
                    .map(|u| u.origin().ascii_serialization());
                if page_origin.as_deref() != Some(origin.as_str()) {
                    continue;
                }
                if let Some(id) = page.get("id").and_then(|i| i.as_str()) {
                    let page_url = page.get("url").and_then(|u| u.as_str()).unwrap_or("");
                    if same_dapp_path(page_url, url) {
                        crate::core::vb_cdp::cdp_focus_target(cdp_url, id).await?;
                    } else {
                        crate::core::vb_cdp::cdp_navigate_target(cdp_url, id, url).await?;
                    }
                    return Ok((Some(id.to_string()), true));
                }
            }
        }
    }
    let id = cdp_open_url(cdp_url, url).await?;
    Ok((id, false))
}

/// Same origin + path — ignore query/hash so reuse does not reload mid-session.
fn same_dapp_path(current: &str, target: &str) -> bool {
    match (Url::parse(current), Url::parse(target)) {
        (Ok(a), Ok(b)) => a.origin() == b.origin() && a.path() == b.path(),
        _ => current.trim_end_matches('/') == target.trim_end_matches('/'),
    }
}

/// Current URL of the pinned page target (or the first page when unpinned).
///
/// Used to re-check the allowlist before mutating tools: the in-tab nav gate
/// is the primary control, this is defense in depth against gate bypass.
pub async fn cdp_current_page_url(cdp_url: &str) -> Option<String> {
    let pages = cdp_list_pages(cdp_url).await.ok()?;
    let pin = read_target_pin()
        .filter(|p| p.cdp_url.trim_end_matches('/') == cdp_url.trim_end_matches('/'));
    let mut first_page: Option<&serde_json::Value> = None;
    for page in &pages {
        if page.get("type").and_then(|t| t.as_str()) != Some("page") {
            continue;
        }
        if first_page.is_none() {
            first_page = Some(page);
        }
        if let Some(pin) = &pin {
            if page.get("id").and_then(|i| i.as_str()) == Some(pin.target_id.as_str()) {
                return page.get("url").and_then(|u| u.as_str()).map(str::to_string);
            }
        }
    }
    first_page
        .and_then(|p| p.get("url"))
        .and_then(|u| u.as_str())
        .map(str::to_string)
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
///
/// The child is re-parented into its own session via `setsid` (util-linux,
/// preinstalled on Linux) so it survives the spawner's process group going
/// away — without it, an MCP-host restart or shell cleanup kills VB silently.
/// stdout/stderr append to `vb.log` in the data dir so spawn failures are
/// diagnosable (previously discarded to null).
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

    // setsid execs the browser directly when the spawned child is not a
    // process-group leader (always true for Command::spawn), so the Child
    // handle tracks the browser itself — no extra fork layer.
    let setsid = find_on_path("setsid");
    let mut cmd = match &setsid {
        Some(s) => {
            let mut c = Command::new(s);
            c.arg(&bin);
            c
        }
        None => Command::new(&bin),
    };
    // Headless MCP hosts (cursor-agent, Claude Desktop) spawn us without a
    // display; Chromium then exits at launch and CDP never comes up. Adopt
    // the local X server's default display when one is present.
    if std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
        && std::path::Path::new("/tmp/.X11-unix/X0").exists()
    {
        cmd.env("DISPLAY", ":0");
    }
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
    cmd.stdin(Stdio::null());
    match open_vb_log() {
        Some(f) => {
            let err = f
                .try_clone()
                .map(Stdio::from)
                .unwrap_or_else(|_| Stdio::null());
            cmd.stdout(Stdio::from(f));
            cmd.stderr(err);
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    cmd.spawn()
        .map_err(|e| WalletError::Other(format!("spawn vaughan-dapp-browser: {e}")))?;
    Ok(())
}

/// Open `vb.log` for appending, creating the data dir if needed.
///
/// Owner-only from creation: the log captures spawn flags and extension paths
/// for a wallet session, so it must never be world-readable even on a
/// permissive umask.
fn open_vb_log() -> Option<std::fs::File> {
    let path = vb_log_path()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path).ok()
}

fn resolve_dapp_browser_bin() -> Option<PathBuf> {
    // Prefer the sibling next to `vaughan` / `vaughan-cli` (cargo run MCP) so
    // spawn uses the same build as the agent — PATH may point at an older
    // `cargo install` that omits newer vb.session fields (e.g. provider_token).
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("vaughan-dapp-browser");
            if sibling.is_file() {
                return Some(sibling);
            }
        }
    }
    if let Some(path) = find_on_path("vaughan-dapp-browser") {
        return Some(path);
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

    static CDP_ENV_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn resolve_cdp_port_default_off() {
        let _guard = CDP_ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var("VAUGHAN_DAPP_BROWSER_CDP_PORT");
        assert_eq!(resolve_cdp_port(false), 0);
        assert_eq!(resolve_cdp_port(true), DEFAULT_CDP_PORT);
    }

    #[test]
    fn resolve_cdp_port_env_overrides_toggle() {
        let _guard = CDP_ENV_TEST_LOCK.lock().unwrap();
        std::env::set_var("VAUGHAN_DAPP_BROWSER_CDP_PORT", "9333");
        assert_eq!(resolve_cdp_port(false), 9333);
        assert_eq!(resolve_cdp_port(true), 9333);
        std::env::set_var("VAUGHAN_DAPP_BROWSER_CDP_PORT", "0");
        assert_eq!(resolve_cdp_port(true), 0);
        std::env::remove_var("VAUGHAN_DAPP_BROWSER_CDP_PORT");
    }

    #[test]
    fn spawn_cdp_port_random_when_enabled() {
        let _guard = CDP_ENV_TEST_LOCK.lock().unwrap();
        std::env::remove_var("VAUGHAN_DAPP_BROWSER_CDP_PORT");
        assert_eq!(spawn_cdp_port(false), 0);
        let a = spawn_cdp_port(true);
        let b = spawn_cdp_port(true);
        assert_ne!(a, 0);
        assert_ne!(b, 0);
        // Random ephemeral ports — not the well-known default.
        assert_ne!(a, DEFAULT_CDP_PORT);
        assert_ne!(b, DEFAULT_CDP_PORT);
        std::env::set_var("VAUGHAN_DAPP_BROWSER_CDP_PORT", "9444");
        assert_eq!(spawn_cdp_port(true), 9444);
        std::env::remove_var("VAUGHAN_DAPP_BROWSER_CDP_PORT");
    }

    #[test]
    fn data_and_blob_urls_rejected_as_nav_targets() {
        assert!(!is_ephemeral_url("data:text/html,<script>1</script>"));
        assert!(!is_ephemeral_url("blob:https://app.pulsex.com/abc"));
        assert!(check_url_allowed("data:text/html,x", &["pulsex.com".into()]).is_err());
        assert!(
            check_url_allowed("blob:https://app.pulsex.com/abc", &["pulsex.com".into()]).is_err()
        );
    }

    #[test]
    fn http_localhost_still_requires_allowlist_suffix() {
        assert!(check_url_allowed("http://127.0.0.1:8080/", &[]).is_err());
        check_url_allowed("http://127.0.0.1:8080/", &["127.0.0.1".into()]).unwrap();
        assert!(check_url_allowed("http://evil.com/", &[]).is_err());
    }

    #[test]
    fn provider_token_extract_and_stale_check() {
        let url = "ws://127.0.0.1:8745?access_token=abc123&foo=bar";
        assert_eq!(
            extract_provider_access_token(url),
            Some("abc123".to_string())
        );
        let base = VbSession {
            cdp_url: "http://127.0.0.1:9222".into(),
            cdp_token: String::new(),
            allow_suffixes: vec![],
            pid: 0,
            extension_secret: String::new(),
            provider_token: String::new(),
            updated_at: 0,
        };
        if let Some(current) = read_latest_provider_session_token() {
            let fresh = VbSession {
                provider_token: current,
                ..base.clone()
            };
            assert!(!vb_session_provider_token_stale(&fresh));
            let stale = VbSession {
                provider_token: "definitely-not-the-live-token".into(),
                ..base
            };
            assert!(vb_session_provider_token_stale(&stale));
        }
    }
}
