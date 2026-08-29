//! On-disk persistence: the encrypted vault plus user settings.
//!
//! Nothing secret is ever written here. [`EncryptedVault`] only holds the
//! Argon2id salt, AES-GCM nonce, and ciphertext; the active network/account are
//! plain metadata. Writes are atomic (temp file + rename) so a crash can never
//! leave a half-written vault. Before overwriting, the previous good file is
//! copied to `wallet.json.bak` so a corrupt primary can still be recovered.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::agent_autonomy::AgentAutonomyTier;
use crate::core::profile::OperatingMode;
use crate::error::WalletError;
use crate::security::encryption::EncryptedVault;

/// Schema version; bump when the persisted layout changes.
pub const CURRENT_VERSION: u32 = 1;

/// Default wallet data file name.
pub const WALLET_FILE: &str = "wallet.json";

/// Last-known-good copy written beside [`WALLET_FILE`] before each overwrite.
pub const WALLET_BACKUP_SUFFIX: &str = ".bak";

fn backup_path(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}{WALLET_BACKUP_SUFFIX}", path.display()))
}

/// Default profile name (human adviser / savings).
pub const DEFAULT_PROFILE: &str = "default";

/// Sentient agent profile — the agent's own seed (`vaughan-sentient` MCP).
pub const SENTIENT_PROFILE: &str = "sentient";

/// Legacy alias for [`SENTIENT_PROFILE`] (pre-rename on-disk / CLI).
pub const DEGEN_PROFILE: &str = "degen";

/// True if `name` is the sentient (agent-owned) profile, including legacy `degen`.
pub fn is_sentient_profile(name: &str) -> bool {
    name == SENTIENT_PROFILE || name == DEGEN_PROFILE
}

/// Validate a profile name for safe on-disk use.
///
/// Names come from CLI args and MCP host configs; without validation a name
/// like `../../tmp/x` is a path-traversal primitive out of `profiles/`.
/// Allowed: ASCII alphanumerics, `-`, `_`, max 64 chars. Empty is allowed
/// here and means the default profile (callers map it before use).
pub fn validate_profile_name(name: &str) -> Result<(), WalletError> {
    if name.is_empty() {
        return Ok(());
    }
    let ok = name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(WalletError::Other(format!(
            "invalid profile name {name:?} — use 1-64 chars of [a-zA-Z0-9_-]"
        )))
    }
}

/// Everything persisted to disk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedState {
    pub version: u32,
    pub vault: EncryptedVault,
    pub active_network_id: String,
    pub active_account_index: u32,
    #[serde(default)]
    pub operating_mode: OperatingMode,
    #[serde(default = "default_profile_string")]
    pub profile_name: String,
    /// User-imported ERC-20 contracts (meme coins, etc.), per chain.
    #[serde(default)]
    pub custom_tokens: Vec<CustomToken>,
    /// Whitelisted dApps: open in Freedom; origins feed the provider allowlist.
    #[serde(default)]
    pub trusted_dapps: Vec<TrustedDapp>,
    /// User-defined EVM networks (merged after built-ins).
    #[serde(default)]
    pub custom_networks: Vec<CustomNetwork>,
    /// Hardware watch accounts (address + path; no secrets). Forward-compatible.
    #[serde(default)]
    pub hardware: Vec<crate::security::hardware::HardwareAccountRecord>,
    /// When true, VB may expose loopback CDP for MCP agent navigation (FR-7.5).
    #[serde(default)]
    pub agent_browser_control: bool,
    /// MCP/VB connect autonomy: advisor = manual Connect card; operator = auto on allowlist.
    #[serde(default)]
    pub agent_autonomy_tier: AgentAutonomyTier,
    /// Per-network primary RPC override (network id → URL). Fallbacks stay built-in.
    #[serde(default)]
    pub network_rpc_primary: HashMap<String, String>,
}

/// A user-imported ERC-20 (shown in Assets even at zero balance).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomToken {
    pub chain_id: u64,
    pub address: String,
    #[serde(default)]
    pub symbol: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_token_decimals")]
    pub decimals: u8,
}

fn default_token_decimals() -> u8 {
    18
}

/// A user-defined EVM network (persisted; not a built-in).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomNetwork {
    /// Stable id (e.g. `custom-1337`).
    pub id: String,
    pub name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    #[serde(default = "default_native_symbol")]
    pub native_symbol: String,
    #[serde(default)]
    pub is_testnet: bool,
}

fn default_native_symbol() -> String {
    "ETH".into()
}

impl CustomNetwork {
    /// Convert to the runtime network config used by adapters.
    pub fn to_evm_config(&self) -> crate::chains::evm::networks::EvmNetworkConfig {
        let mut cfg = crate::chains::evm::networks::EvmNetworkConfig::new(
            self.id.clone(),
            self.name.clone(),
            self.chain_id,
            self.rpc_url.clone(),
            self.native_symbol.clone(),
            self.native_symbol.clone(),
            self.is_testnet,
        );
        cfg.decimals = 18;
        cfg
    }
}

/// A bookmarked / allowlisted dApp URL.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrustedDapp {
    pub name: String,
    pub url: String,
    /// Extra host suffixes for in-tab navigation (e.g. IPFS gateways for PulseX).
    #[serde(default)]
    pub extra_hosts: Vec<String>,
}

/// Common IPFS gateway hosts used by PulseX and similar directory frontends.
pub fn default_ipfs_gateway_hosts() -> Vec<&'static str> {
    vec![
        "ipfs.io",
        "cloudflare-ipfs.com",
        "dweb.link",
        "gateway.pinata.cloud",
        "cf-ipfs.com",
    ]
}

fn push_unique_host(out: &mut Vec<String>, s: String) {
    if !s.is_empty() && !out.iter().any(|x| x == &s) {
        out.push(s);
    }
}

/// `app.pulsex.com` → `app.pulsex.com` + `pulsex.com` (parent for redirects).
fn expand_host_suffixes(host: &str) -> Vec<String> {
    let host = host.trim().trim_start_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return Vec::new();
    }
    let mut out = vec![host.clone()];
    let parts: Vec<&str> = host.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() >= 3 {
        out.push(format!(
            "{}.{}",
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        ));
    }
    out
}

/// Host suffixes for `vaughan-dapp-browser --allow-host` from all trusted dApps.
pub fn trusted_dapp_allow_hosts(dapps: &[TrustedDapp]) -> Vec<String> {
    let mut out = Vec::new();
    for d in dapps {
        if let Ok(u) = url::Url::parse(&d.url) {
            if let Some(h) = u.host_str() {
                for s in expand_host_suffixes(h) {
                    push_unique_host(&mut out, s);
                }
            }
        }
        for h in &d.extra_hosts {
            for s in expand_host_suffixes(h) {
                push_unique_host(&mut out, s);
            }
        }
    }
    out
}

/// Built-in PulseChain dApps seeded into every vault (provider origins + launcher).
pub fn default_trusted_dapps() -> Vec<TrustedDapp> {
    vec![
        TrustedDapp {
            name: "SquirrelSwap".into(),
            url: "https://app.squirrelswap.pro/#/".into(),
            extra_hosts: vec![],
        },
        TrustedDapp {
            name: "PulseSwap".into(),
            url: "https://pulseswap.io/?chain=pulsechain".into(),
            extra_hosts: vec![],
        },
        TrustedDapp {
            name: "Piteas".into(),
            url: "https://app.piteas.io/".into(),
            extra_hosts: vec![],
        },
        TrustedDapp {
            name: "Switch.win".into(),
            url: "https://www.switch.win/".into(),
            extra_hosts: vec!["beta.switch.win".into()],
        },
        TrustedDapp {
            name: "9mm swap".into(),
            url: "https://9mm.pro/swap".into(),
            extra_hosts: vec![],
        },
        TrustedDapp {
            name: "Internet Money".into(),
            url: "https://internetmoney.io/".into(),
            extra_hosts: vec![],
        },
        TrustedDapp {
            name: "LibertySwap".into(),
            url: "https://libertyswap.finance/".into(),
            extra_hosts: vec![],
        },
        // app.pulsex.com is a gateway *directory* (IPFS mirrors), not the DEX UI.
        // Open a listed IPFS link from there; mirror hosts must be allowlisted for
        // in-tab navigation while the extension Origin still attests page_origin.
        TrustedDapp {
            name: "PulseX (pick IPFS mirror)".into(),
            url: "https://app.pulsex.com/".into(),
            extra_hosts: default_ipfs_gateway_hosts()
                .into_iter()
                .map(str::to_string)
                .collect(),
        },
        TrustedDapp {
            name: "9inch".into(),
            url: "https://app.9inch.io/swap?chain=pulse".into(),
            extra_hosts: vec![],
        },
    ]
}

fn dapp_origin(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let origin = u.origin().ascii_serialization();
    if origin == "null" {
        None
    } else {
        Some(origin)
    }
}

/// Append any missing [`default_trusted_dapps`] entries (matched by origin).
/// Also backfills `extra_hosts` on existing defaults (e.g. PulseX IPFS gateways).
/// Returns `true` when the list changed.
pub fn merge_default_trusted_dapps(list: &mut Vec<TrustedDapp>) -> bool {
    let mut changed = false;
    for dapp in default_trusted_dapps() {
        let Some(want) = dapp_origin(&dapp.url) else {
            continue;
        };
        if let Some(existing) = list
            .iter_mut()
            .find(|e| dapp_origin(&e.url).as_deref() == Some(want.as_str()))
        {
            if existing.extra_hosts.is_empty() && !dapp.extra_hosts.is_empty() {
                existing.extra_hosts = dapp.extra_hosts.clone();
                changed = true;
            }
            continue;
        }
        list.push(dapp);
        changed = true;
    }
    changed
}

fn default_profile_string() -> String {
    DEFAULT_PROFILE.to_string()
}

impl PersistedState {
    pub fn new(vault: EncryptedVault, active_network_id: impl Into<String>) -> Self {
        Self {
            version: CURRENT_VERSION,
            vault,
            active_network_id: active_network_id.into(),
            active_account_index: 0,
            operating_mode: OperatingMode::HumanOnly,
            profile_name: DEFAULT_PROFILE.to_string(),
            custom_tokens: Vec::new(),
            trusted_dapps: default_trusted_dapps(),
            custom_networks: Vec::new(),
            hardware: Vec::new(),
            agent_browser_control: false,
            agent_autonomy_tier: AgentAutonomyTier::default(),
            network_rpc_primary: HashMap::new(),
        }
    }

    pub fn with_mode_and_profile(
        vault: EncryptedVault,
        active_network_id: impl Into<String>,
        operating_mode: OperatingMode,
        profile_name: impl Into<String>,
    ) -> Self {
        Self {
            version: CURRENT_VERSION,
            vault,
            active_network_id: active_network_id.into(),
            active_account_index: 0,
            operating_mode,
            profile_name: profile_name.into(),
            custom_tokens: Vec::new(),
            trusted_dapps: default_trusted_dapps(),
            custom_networks: Vec::new(),
            hardware: Vec::new(),
            agent_browser_control: false,
            agent_autonomy_tier: AgentAutonomyTier::default(),
            network_rpc_primary: HashMap::new(),
        }
    }
}

/// Loads and saves [`PersistedState`] to a JSON file on disk.
pub struct StateManager {
    path: PathBuf,
}

/// Profile metadata for the unlock-screen picker (metadata only — no decryption).
#[derive(Debug, Clone)]
pub struct ProfileMeta {
    /// Profile name (`default`, `sentient`, …).
    pub name: String,
    /// Vault file path for this profile.
    pub path: PathBuf,
    /// True when the vault file exists and parses (wallet created).
    pub initialized: bool,
    /// True for the agent-owned sentient profile (incl. legacy `degen`).
    pub is_sentient: bool,
}

impl StateManager {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The default location: `<data_dir>/vaughan-cli/wallet.json`.
    pub fn default_path() -> Result<PathBuf, WalletError> {
        Self::profile_path(DEFAULT_PROFILE)
    }

    /// Profile-specific location:
    /// - "default" -> `<data_dir>/vaughan-cli/wallet.json`
    /// - other (e.g. "degen") -> `<data_dir>/vaughan-cli/profiles/<name>/wallet.json`
    ///
    /// Profile names are validated: they arrive from CLI args and MCP host
    /// configs, and an unvalidated name is a path-traversal vector
    /// (`../../etc` would escape `profiles/`).
    pub fn profile_path(profile_name: &str) -> Result<PathBuf, WalletError> {
        validate_profile_name(profile_name)?;
        let base = dirs::data_dir().ok_or_else(|| {
            WalletError::Io("could not determine the user data directory".to_string())
        })?;
        let vaughan_base = base.join("vaughan-cli");
        if profile_name.is_empty() || profile_name == DEFAULT_PROFILE {
            Ok(vaughan_base.join(WALLET_FILE))
        } else {
            Ok(vaughan_base
                .join("profiles")
                .join(profile_name)
                .join(WALLET_FILE))
        }
    }

    /// Construct a StateManager for a specific profile.
    pub fn for_profile(profile_name: &str) -> Result<Self, WalletError> {
        let path = Self::profile_path(profile_name)?;
        Ok(Self::new(path))
    }

    /// List known profiles for the unlock picker: `default` first, then every
    /// `profiles/<name>/` directory sorted by name. Metadata only — no secrets.
    ///
    /// A profile directory without `wallet.json` is still listed
    /// (`initialized: false`): `preset apply` writes policy there before the
    /// vault exists, and picking it routes to onboarding.
    pub fn list_profiles() -> Vec<ProfileMeta> {
        let Ok(default_path) = Self::default_path() else {
            return Vec::new();
        };
        Self::list_profiles_at(&default_path)
    }

    /// [`list_profiles`] against an explicit default-vault path (tests).
    pub fn list_profiles_at(default_path: &std::path::Path) -> Vec<ProfileMeta> {
        let mut out = vec![ProfileMeta {
            name: DEFAULT_PROFILE.to_string(),
            initialized: Self::new(default_path.to_path_buf()).load().is_ok(),
            path: default_path.to_path_buf(),
            is_sentient: false,
        }];
        let Some(profiles_dir) = default_path.parent().map(|p| p.join("profiles")) else {
            return out;
        };
        let Ok(entries) = std::fs::read_dir(&profiles_dir) else {
            return out;
        };
        let mut named: Vec<ProfileMeta> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                // Skip names that would fail profile_path validation (e.g.
                // stray dotdirs) so the picker never offers an unusable entry.
                if validate_profile_name(&name).is_err() {
                    return None;
                }
                let path = e.path().join(WALLET_FILE);
                Some(ProfileMeta {
                    is_sentient: is_sentient_profile(&name),
                    initialized: Self::new(path.clone()).load().is_ok(),
                    name,
                    path,
                })
            })
            .collect();
        named.sort_by(|a, b| a.name.cmp(&b.name));
        out.extend(named);
        out
    }

    /// Whether agent browser control (loopback CDP) is enabled for `profile`.
    ///
    /// Returns `false` when the vault is missing or unreadable.
    pub fn agent_browser_control_for_profile(profile_name: &str) -> bool {
        Self::for_profile(profile_name)
            .ok()
            .and_then(|sm| sm.load().ok())
            .is_some_and(|s| s.agent_browser_control)
    }

    /// Persist agent browser control for `profile` (metadata only — no unlock).
    pub fn set_agent_browser_control_for_profile(
        profile_name: &str,
        enabled: bool,
    ) -> Result<(), WalletError> {
        let sm = Self::for_profile(profile_name)?;
        let mut state = sm.load()?;
        state.agent_browser_control = enabled;
        sm.save(&state)?;
        if !enabled {
            crate::core::vb_browser::clear_vb_session();
        }
        Ok(())
    }

    /// Agent autonomy tier for `profile` (defaults to advisor when vault missing).
    pub fn agent_autonomy_tier_for_profile(profile_name: &str) -> AgentAutonomyTier {
        Self::for_profile(profile_name)
            .ok()
            .and_then(|sm| sm.load().ok())
            .map(|s| s.agent_autonomy_tier)
            .unwrap_or_default()
    }

    /// Persist agent autonomy tier for `profile` (metadata only — no unlock).
    pub fn set_agent_autonomy_tier_for_profile(
        profile_name: &str,
        tier: AgentAutonomyTier,
    ) -> Result<(), WalletError> {
        let sm = Self::for_profile(profile_name)?;
        let mut state = sm.load()?;
        state.agent_autonomy_tier = tier;
        sm.save(&state)
    }

    /// Primary RPC override for `network_id` on `profile`, if set.
    pub fn network_rpc_primary_for_profile(profile_name: &str, network_id: &str) -> Option<String> {
        Self::for_profile(profile_name)
            .ok()
            .and_then(|sm| sm.load().ok())
            .and_then(|s| {
                s.network_rpc_primary
                    .get(&network_id.trim().to_ascii_lowercase())
                    .cloned()
            })
    }

    /// Persist or clear the primary RPC for a built-in/custom network (metadata only).
    pub fn set_network_rpc_primary_for_profile(
        profile_name: &str,
        network_id: &str,
        rpc_url: Option<&str>,
    ) -> Result<(), WalletError> {
        let sm = Self::for_profile(profile_name)?;
        let mut state = sm.load()?;
        let key = network_id.trim().to_ascii_lowercase();
        if let Some(custom) = state
            .custom_networks
            .iter_mut()
            .find(|n| n.id.eq_ignore_ascii_case(&key))
        {
            let Some(url) = rpc_url.filter(|u| !u.trim().is_empty()) else {
                return Ok(());
            };
            let parsed = url::Url::parse(url.trim()).map_err(|_| {
                WalletError::InvalidTransaction("RPC URL must be a valid http(s) URL".into())
            })?;
            if !matches!(parsed.scheme(), "http" | "https") {
                return Err(WalletError::InvalidTransaction(
                    "RPC URL must be http or https".into(),
                ));
            }
            custom.rpc_url = url.trim().to_string();
            state.network_rpc_primary.remove(&key);
            sm.save(&state)?;
            return Ok(());
        }
        match rpc_url {
            None | Some("") => {
                state.network_rpc_primary.remove(&key);
            }
            Some(url) => {
                let parsed = url::Url::parse(url.trim()).map_err(|_| {
                    WalletError::InvalidTransaction("RPC URL must be a valid http(s) URL".into())
                })?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(WalletError::InvalidTransaction(
                        "RPC URL must be http or https".into(),
                    ));
                }
                state
                    .network_rpc_primary
                    .insert(key, url.trim().to_string());
            }
        }
        sm.save(&state)?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// On Unix: restrict the vault directory to `0o700` and the vault file to
    /// `0o600` so other local users can never read the ciphertext, regardless
    /// of umask. Failures are logged, not fatal — reading the vault must not
    /// break on filesystems that reject chmod (e.g. some mounted/FAT paths).
    #[cfg(unix)]
    fn lockdown_permissions(&self) {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = self.path.parent() {
            if let Err(e) = fs::set_permissions(parent, fs::Permissions::from_mode(0o700)) {
                tracing::warn!("could not restrict vault dir permissions: {e}");
            }
        }
        if let Err(e) = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600)) {
            tracing::warn!("could not restrict vault file permissions: {e}");
        }
    }

    /// Serialize `state` to disk atomically.
    ///
    /// If a previous vault exists, it is copied to `*.bak` first (best effort).
    /// Then write `*.tmp` → fsync → rename over the primary. On Unix the vault
    /// directory is `0o700` and vault / temp / bak are `0o600`.
    pub fn save(&self, state: &PersistedState) -> Result<(), WalletError> {
        let json = serde_json::to_string_pretty(state)?;
        let parent = self
            .path
            .parent()
            .ok_or_else(|| WalletError::Io("wallet path has no parent directory".to_string()))?;
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }

        // Preserve last good vault before overwrite (ignore errors: first save).
        if self.path.exists() {
            let bak = backup_path(&self.path);
            if let Err(e) = fs::copy(&self.path, &bak) {
                tracing::warn!("could not write vault backup {}: {e}", bak.display());
            } else {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = fs::set_permissions(&bak, fs::Permissions::from_mode(0o600));
                }
            }
        }

        let tmp = PathBuf::from(format!("{}.tmp", self.path.display()));
        fs::write(&tmp, json.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(&tmp, &self.path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// Load the persisted state, or `WalletError::NotInitialized` when absent.
    ///
    /// If the primary file is corrupt JSON, attempts `*.bak` once and returns a
    /// clear corruption error when both fail.
    pub fn load(&self) -> Result<PersistedState, WalletError> {
        if !self.exists() {
            return Err(WalletError::NotInitialized);
        }
        // Lock down permissions on existing vaults written before the
        // 0o600/0o700 rules existed (best effort; see `lockdown_permissions`).
        #[cfg(unix)]
        self.lockdown_permissions();

        match self.load_from_path(&self.path) {
            Ok(state) => Ok(state),
            Err(primary_err) => {
                let bak = backup_path(&self.path);
                if !bak.exists() {
                    return Err(primary_err);
                }
                tracing::warn!(
                    "primary vault unreadable ({}); trying backup {}",
                    primary_err,
                    bak.display()
                );
                match self.load_from_path(&bak) {
                    Ok(state) => {
                        // Restore primary from backup so next unlock is clean.
                        if let Err(e) = fs::copy(&bak, &self.path) {
                            tracing::warn!("could not restore primary from backup: {e}");
                        } else {
                            #[cfg(unix)]
                            self.lockdown_permissions();
                        }
                        Ok(state)
                    }
                    Err(_) => Err(WalletError::Serialization(format!(
                        "wallet file is corrupt (and backup failed). Primary error: {primary_err}"
                    ))),
                }
            }
        }
    }

    fn load_from_path(&self, path: &Path) -> Result<PersistedState, WalletError> {
        let json = fs::read_to_string(path)?;
        let state: PersistedState = serde_json::from_str(&json).map_err(|e| {
            WalletError::Serialization(format!("corrupt wallet data at {}: {e}", path.display()))
        })?;
        if state.version > CURRENT_VERSION {
            return Err(WalletError::Serialization(format!(
                "wallet file version {} is newer than supported {}",
                state.version, CURRENT_VERSION
            )));
        }
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_vault() -> EncryptedVault {
        EncryptedVault {
            salt: "aa".into(),
            nonce: "bb".into(),
            ciphertext: "cc".into(),
        }
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let sm = StateManager::new(path);
        let state = PersistedState::new(dummy_vault(), "sepolia");
        sm.save(&state).unwrap();

        let loaded = sm.load().unwrap();
        assert_eq!(loaded.active_network_id, "sepolia");
        assert_eq!(loaded.active_account_index, 0);
        assert_eq!(loaded.vault.ciphertext, "cc");
        assert_eq!(loaded.version, CURRENT_VERSION);
        assert_eq!(loaded.trusted_dapps.len(), default_trusted_dapps().len());
        assert!(loaded.hardware.is_empty());
    }

    #[test]
    fn hardware_field_roundtrip() {
        use crate::security::hardware::{HardwareAccountRecord, HardwareVendor, HwChainFamily};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let sm = StateManager::new(path);
        let mut state = PersistedState::new(dummy_vault(), "pulsechain-testnet-v4");
        state.hardware.push(HardwareAccountRecord {
            vendor: HardwareVendor::Trezor,
            family: HwChainFamily::Evm,
            derivation_path: "m/44'/60'/0'/0/1".into(),
            network_id: Some("943".into()),
            address: "0x2222222222222222222222222222222222222222".into(),
            label: "trezor-1".into(),
        });
        sm.save(&state).unwrap();
        let loaded = sm.load().unwrap();
        assert_eq!(loaded.hardware.len(), 1);
        assert_eq!(loaded.hardware[0].label, "trezor-1");
        assert_eq!(loaded.hardware[0].vendor, HardwareVendor::Trezor);
    }

    #[test]
    fn legacy_json_without_hardware_deserializes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let json =
            serde_json::to_string(&PersistedState::new(dummy_vault(), "pulsechain")).unwrap();
        // Strip hardware if present — simulate pre-Phase-0 file.
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        if let Some(obj) = v.as_object_mut() {
            obj.remove("hardware");
        }
        std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
        let sm = StateManager::new(path);
        let loaded = sm.load().unwrap();
        assert!(loaded.hardware.is_empty());
    }

    #[test]
    fn merge_default_dapps_is_idempotent() {
        let mut list = default_trusted_dapps();
        let defaults = list.len();
        assert!(!merge_default_trusted_dapps(&mut list));
        assert_eq!(list.len(), defaults);
        list.clear();
        assert!(merge_default_trusted_dapps(&mut list));
        assert_eq!(list.len(), defaults);
        assert!(list.iter().any(|d| d.url.contains("squirrelswap")));
        assert!(list.iter().any(|d| d.url.contains("libertyswap")));
        assert!(list.iter().any(|d| d.url.contains("pulsex")));
        assert!(list.iter().any(|d| d.url.contains("9inch")));
    }

    #[test]
    fn merge_backfills_pulsex_ipfs_gateways() {
        let mut list = vec![TrustedDapp {
            name: "PulseX".into(),
            url: "https://app.pulsex.com/".into(),
            extra_hosts: vec![],
        }];
        assert!(merge_default_trusted_dapps(&mut list));
        let pulsex = list.iter().find(|d| d.url.contains("pulsex")).unwrap();
        assert!(pulsex.extra_hosts.iter().any(|h| h == "ipfs.io"));
    }

    #[test]
    fn trusted_dapp_allow_hosts_includes_ipfs_for_pulsex() {
        let dapps = default_trusted_dapps();
        let hosts = trusted_dapp_allow_hosts(&dapps);
        assert!(hosts.iter().any(|h| h == "ipfs.io"));
        assert!(hosts.iter().any(|h| h == "app.pulsex.com"));
        assert!(hosts.iter().any(|h| h == "app.9inch.io"));
    }

    #[test]
    fn load_missing_is_not_initialized() {
        let dir = tempfile::tempdir().unwrap();
        let sm = StateManager::new(dir.path().join("nope.json"));
        assert!(matches!(sm.load(), Err(WalletError::NotInitialized)));
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let sm = StateManager::new(path.clone());
        sm.save(&PersistedState::new(dummy_vault(), "pulsechain"))
            .unwrap();
        assert!(path.exists());

        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("tmp"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[cfg(unix)]
    #[test]
    fn vault_permissions_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("vaughan-cli");
        let path = sub.join(WALLET_FILE);
        let sm = StateManager::new(path.clone());
        sm.save(&PersistedState::new(dummy_vault(), "pulsechain"))
            .unwrap();

        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            file_mode, 0o600,
            "vault file must not be group/other readable"
        );
        let dir_mode = fs::metadata(&sub).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            dir_mode, 0o700,
            "vault dir must not be group/other readable"
        );
    }

    #[cfg(unix)]
    #[test]
    fn load_locks_down_existing_vault() {
        use std::os::unix::fs::PermissionsExt;

        // Simulate a vault written by older code: valid state, world-readable.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let json =
            serde_json::to_string(&PersistedState::new(dummy_vault(), "pulsechain")).unwrap();
        fs::write(&path, json).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();

        let sm = StateManager::new(path.clone());
        assert!(sm.load().is_ok());

        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "load must lock down the vault file");
        let dir_mode = fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "load must lock down the vault dir");
    }

    #[test]
    fn profile_mode_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sentient_wallet.json");
        let sm = StateManager::new(path);
        let state = PersistedState::with_mode_and_profile(
            dummy_vault(),
            "pulsechain-testnet-v4",
            OperatingMode::SentientTrader,
            SENTIENT_PROFILE,
        );
        sm.save(&state).unwrap();

        let loaded = sm.load().unwrap();
        assert_eq!(loaded.operating_mode, OperatingMode::SentientTrader);
        assert_eq!(loaded.profile_name, SENTIENT_PROFILE);
        assert_eq!(loaded.active_network_id, "pulsechain-testnet-v4");
    }

    #[test]
    fn profile_path_resolution() {
        let default_path = StateManager::profile_path(DEFAULT_PROFILE).unwrap();
        assert!(default_path.ends_with("vaughan-cli/wallet.json"));

        let legacy_path = StateManager::profile_path(DEGEN_PROFILE).unwrap();
        assert!(legacy_path.ends_with("vaughan-cli/profiles/degen/wallet.json"));

        let sentient_path = StateManager::profile_path(SENTIENT_PROFILE).unwrap();
        assert!(sentient_path.ends_with("vaughan-cli/profiles/sentient/wallet.json"));

        let custom_path = StateManager::profile_path("bot1").unwrap();
        assert!(custom_path.ends_with("vaughan-cli/profiles/bot1/wallet.json"));
    }

    #[test]
    fn profile_name_validation_blocks_traversal() {
        assert!(StateManager::profile_path("../../etc").is_err());
        assert!(StateManager::profile_path("a/b").is_err());
        assert!(StateManager::profile_path("a\\b").is_err());
        assert!(StateManager::profile_path("..").is_err());
        assert!(StateManager::profile_path("evil name").is_err());
        assert!(StateManager::profile_path("evil.name").is_err());
        assert!(StateManager::profile_path(&"x".repeat(65)).is_err());
        assert!(StateManager::profile_path("sentient").is_ok());
        assert!(StateManager::profile_path("Bot-1_x").is_ok());
        assert!(StateManager::profile_path("").is_ok()); // empty = default
    }

    #[test]
    fn sentient_profile_includes_legacy_degen_alias() {
        assert!(is_sentient_profile(SENTIENT_PROFILE));
        assert!(is_sentient_profile(DEGEN_PROFILE));
        assert!(!is_sentient_profile(DEFAULT_PROFILE));
        assert!(!is_sentient_profile("bot1"));
    }

    #[test]
    fn list_profiles_enumerates_default_and_named_profiles() {
        let dir = tempfile::tempdir().unwrap();
        let default_path = dir.path().join("wallet.json");

        // Default only, uninitialized.
        let list = StateManager::list_profiles_at(&default_path);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name, DEFAULT_PROFILE);
        assert!(!list[0].initialized);
        assert!(!list[0].is_sentient);

        // Initialize default; add an initialized sentient vault and a
        // policy-only custom profile dir (preset applied, vault not created).
        StateManager::new(&default_path)
            .save(&PersistedState::with_mode_and_profile(
                dummy_vault(),
                "pulsechain-testnet-v4",
                OperatingMode::HumanOnly,
                DEFAULT_PROFILE,
            ))
            .unwrap();
        let sentient_dir = dir.path().join("profiles").join(SENTIENT_PROFILE);
        std::fs::create_dir_all(&sentient_dir).unwrap();
        StateManager::new(sentient_dir.join(WALLET_FILE))
            .save(&PersistedState::with_mode_and_profile(
                dummy_vault(),
                "pulsechain-testnet-v4",
                OperatingMode::SentientTrader,
                SENTIENT_PROFILE,
            ))
            .unwrap();
        std::fs::create_dir_all(dir.path().join("profiles").join("bot1")).unwrap();

        let list = StateManager::list_profiles_at(&default_path);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].name, DEFAULT_PROFILE);
        assert!(list[0].initialized);
        // Named profiles sorted by name: bot1 before sentient.
        assert_eq!(list[1].name, "bot1");
        assert!(!list[1].initialized);
        assert!(!list[1].is_sentient);
        assert_eq!(list[2].name, SENTIENT_PROFILE);
        assert!(list[2].initialized);
        assert!(list[2].is_sentient);
    }

    #[test]
    fn save_writes_bak_of_previous_vault() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let bak = backup_path(&path);
        let sm = StateManager::new(path.clone());

        let first = PersistedState::new(dummy_vault(), "sepolia");
        sm.save(&first).unwrap();
        assert!(!bak.exists(), "first save has nothing to back up");

        let second = PersistedState::new(dummy_vault(), "pulsechain");
        sm.save(&second).unwrap();
        assert!(bak.exists(), "second save must leave wallet.json.bak");

        let bak_state: PersistedState =
            serde_json::from_str(&fs::read_to_string(&bak).unwrap()).unwrap();
        assert_eq!(bak_state.active_network_id, "sepolia");
        assert_eq!(sm.load().unwrap().active_network_id, "pulsechain");
    }

    #[test]
    fn load_recovers_from_bak_when_primary_corrupt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(WALLET_FILE);
        let bak = backup_path(&path);
        let sm = StateManager::new(path.clone());

        sm.save(&PersistedState::new(dummy_vault(), "pulsechain"))
            .unwrap();
        sm.save(&PersistedState::new(dummy_vault(), "sepolia"))
            .unwrap();
        // Bak holds pulsechain; overwrite primary with garbage.
        fs::write(&path, "{not-json").unwrap();
        assert!(bak.exists());

        let loaded = sm.load().unwrap();
        assert_eq!(loaded.active_network_id, "pulsechain");
        // Primary should be restored from bak.
        let primary: PersistedState =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(primary.active_network_id, "pulsechain");
    }
}
