//! Power-feature entitlement via WZRD buy-and-burn (Transfer to a dead sink).
//!
//! When [`assist_burn_gate_enabled`] is on, everything beyond a normal wallet
//! (send / receive / balances / revoke / dApp connect / wrap / networks / keys)
//! requires a single on-chain burn of at least [`ASSIST_BURN_AMOUNT_WEI`] WZRD
//! from **any account in this vault** to [`BURN_SINK`]. That unlocks Bridge,
//! token launch, contract browser, stealth, Agent advisor / MCP writes, and
//! Sentient for **every** F3 account on the same entitlement chain. **Ag, Dex,
//! and LP stay free** so users can buy WZRD (or remove LP for gas/trade capital)
//! before burning.
//!
//! Uses Alloy `eth_getLogs` + the shared [`transfer_topic0`] helper (same
//! pattern as asset discovery). No new crates. Disable locally with
//! `VAUGHAN_ASSIST_BURN_GATE=0` or CI bypass [`ASSIST_UNLOCK_BYPASS_ENV`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use alloy::primitives::{address, Address, B256, U256};
use alloy::providers::Provider;
use alloy::rpc::types::Filter;
use serde::{Deserialize, Serialize};

use crate::chains::evm::adapter::transfer_topic0;
use crate::chains::evm::EvmAdapter;
use crate::chains::{ChainTransaction, EvmTransaction};
use crate::core::lp_smoke::RPC_943;
use crate::core::transaction::TransactionService;
use crate::core::wiz4rd::{
    parse_addr, NPM_LOG_SCAN_FROM_BLOCK_943, WIZ4RD_MAINNET_CHAIN_ID, WIZ4RD_TESTNET_CHAIN_ID,
    WZRD_SMOKE_943,
};
use crate::error::WalletError;

/// Canonical burn sink (`0x…dEaD`).
pub const BURN_SINK: Address = address!("0x000000000000000000000000000000000000dEaD");

/// Minimum burn in wei (13 WZRD, 18 decimals). Larger single transfers also unlock.
pub const ASSIST_BURN_AMOUNT_WEI: u128 = 13_000_000_000_000_000_000;

/// Human amount string for UX defaults.
pub const ASSIST_BURN_AMOUNT_HUMAN: &str = "13";

/// Env flag: set to `0` / `false` / `no` / `off` to disable the burn gate (default **on**).
pub const ASSIST_BURN_GATE_ENV: &str = "VAUGHAN_ASSIST_BURN_GATE";

/// Env flag: skip on-chain check (CI / local only).
pub const ASSIST_UNLOCK_BYPASS_ENV: &str = "VAUGHAN_ASSIST_UNLOCK_BYPASS";

const CACHE_FILE: &str = "assist-unlock.json";
const POST_BURN_RETRIES: u32 = 3;
const POST_BURN_RETRY_DELAY: Duration = Duration::from_millis(1500);

/// True when the burn gate should run (default **on**).
pub fn assist_burn_gate_enabled() -> bool {
    env_flag_default_on(ASSIST_BURN_GATE_ENV)
}

/// True when CI/dev bypass is set (skips RPC).
pub fn assist_unlock_bypass() -> bool {
    env_flag_truthy(ASSIST_UNLOCK_BYPASS_ENV)
}

fn env_flag_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Env unset → enabled; explicit `0`/`false`/`no`/`off` disables.
fn env_flag_default_on(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => true,
    }
}

/// Minimum burn as [`U256`].
pub fn assist_burn_amount_u256() -> U256 {
    U256::from(ASSIST_BURN_AMOUNT_WEI)
}

/// Burn sink as checksummed hex.
pub fn burn_sink_hex() -> String {
    format!("{BURN_SINK:#x}")
}

/// WZRD + scan floor + public RPC for entitlement checks on `chain_id`.
///
/// Production entitlement is **per chain**: 943 smoke never unlocks 369.
pub fn entitlement_wzrd(chain_id: u64) -> Option<EntitlementToken> {
    match chain_id {
        WIZ4RD_TESTNET_CHAIN_ID => Some(EntitlementToken {
            chain_id,
            token: parse_addr(WZRD_SMOKE_943)?,
            rpc_url: RPC_943,
            from_block: NPM_LOG_SCAN_FROM_BLOCK_943,
            network_name: "pulsechain-testnet-v4",
        }),
        WIZ4RD_MAINNET_CHAIN_ID => None, // WZRD not deployed on 369 yet
        _ => None,
    }
}

/// Chain id used for entitlement scans (fixed Pulse WZRD RPC — not F1).
///
/// Prefers mainnet when WZRD is configured there; otherwise 943 smoke dry-run.
pub fn entitlement_chain_id() -> Option<u64> {
    if entitlement_wzrd(WIZ4RD_MAINNET_CHAIN_ID).is_some() {
        Some(WIZ4RD_MAINNET_CHAIN_ID)
    } else if entitlement_wzrd(WIZ4RD_TESTNET_CHAIN_ID).is_some() {
        Some(WIZ4RD_TESTNET_CHAIN_ID)
    } else {
        None
    }
}

/// Chain-bound WZRD parameters for entitlement scans.
#[derive(Debug, Clone, Copy)]
pub struct EntitlementToken {
    pub chain_id: u64,
    pub token: Address,
    pub rpc_url: &'static str,
    pub from_block: u64,
    pub network_name: &'static str,
}

/// Whether this **vault** has a qualifying WZRD burn on `chain_id`.
///
/// `wallets` are candidate EOAs to scan (typically every HD + imported account).
/// A burn from any candidate unlocks power features for all accounts on that
/// chain. Positive results are cached per profile as vault-wide for the chain.
///
/// [`address_has_assist_burn`] is a thin wrapper for a single address.
pub async fn address_has_assist_burn(
    profile_dir: Option<&Path>,
    chain_id: u64,
    wallet: Address,
) -> Result<bool, WalletError> {
    vault_has_assist_burn(profile_dir, chain_id, &[wallet]).await
}

/// Like [`address_has_assist_burn`], scanning every address in `wallets`.
pub async fn vault_has_assist_burn(
    profile_dir: Option<&Path>,
    chain_id: u64,
    wallets: &[Address],
) -> Result<bool, WalletError> {
    if !assist_burn_gate_enabled() {
        return Ok(true);
    }
    if assist_unlock_bypass() {
        return Ok(true);
    }
    if let Some(dir) = profile_dir {
        if cache_has_chain(dir, chain_id)? {
            return Ok(true);
        }
    }
    let Some(cfg) = entitlement_wzrd(chain_id) else {
        return Ok(false);
    };
    let mut seen = std::collections::HashSet::new();
    for &wallet in wallets {
        if !seen.insert(wallet) {
            continue;
        }
        if scan_assist_burn(&cfg, wallet).await? {
            if let Some(dir) = profile_dir {
                let _ = cache_put_vault(dir, chain_id, wallet);
            }
            return Ok(true);
        }
    }
    Ok(false)
}

/// Scan + short retries (post-burn receipt race).
pub async fn address_has_assist_burn_with_retry(
    profile_dir: Option<&Path>,
    chain_id: u64,
    wallet: Address,
) -> Result<bool, WalletError> {
    vault_has_assist_burn_with_retry(profile_dir, chain_id, &[wallet]).await
}

/// Vault-wide post-burn retry (same delays as [`address_has_assist_burn_with_retry`]).
pub async fn vault_has_assist_burn_with_retry(
    profile_dir: Option<&Path>,
    chain_id: u64,
    wallets: &[Address],
) -> Result<bool, WalletError> {
    for attempt in 0..POST_BURN_RETRIES {
        if vault_has_assist_burn(profile_dir, chain_id, wallets).await? {
            return Ok(true);
        }
        if attempt + 1 < POST_BURN_RETRIES {
            tokio::time::sleep(POST_BURN_RETRY_DELAY).await;
        }
    }
    Ok(false)
}

/// Require entitlement or return a stable MCP/TUI error code.
pub async fn require_assist_entitlement(
    profile_dir: Option<&Path>,
    chain_id: u64,
    wallet: Address,
) -> Result<(), WalletError> {
    require_power_features(profile_dir, chain_id, &[wallet]).await
}

/// Same check as [`require_assist_entitlement`] — preferred name for power-feature gates.
///
/// Pass every vault account in `wallets` so a burn from any F3 unlocks all.
pub async fn require_power_features(
    profile_dir: Option<&Path>,
    chain_id: u64,
    wallets: &[Address],
) -> Result<(), WalletError> {
    if vault_has_assist_burn(profile_dir, chain_id, wallets).await? {
        return Ok(());
    }
    Err(WalletError::Other(
        "assist_locked: burn at least 13 WZRD to the dead address from any account in this wallet \
         (Settings → Unlock tools / w), then retry"
            .into(),
    ))
}

/// Sync helper for TUI: gate off / bypass / positive cache / live scan.
pub fn power_features_unlocked_blocking(
    rt: &tokio::runtime::Handle,
    profile_dir: Option<&Path>,
    chain_id: u64,
    wallets: &[Address],
) -> bool {
    rt.block_on(vault_has_assist_burn(profile_dir, chain_id, wallets))
        .unwrap_or(false)
}

async fn scan_assist_burn(cfg: &EntitlementToken, wallet: Address) -> Result<bool, WalletError> {
    let adapter = EvmAdapter::new(cfg.rpc_url, cfg.chain_id, cfg.network_name, &[]).await?;
    let latest = adapter
        .with_provider(|provider| async move {
            provider
                .get_block_number()
                .await
                .map_err(|e| WalletError::RpcError(e.to_string()))
        })
        .await?;
    let from = if latest < cfg.from_block {
        0
    } else {
        cfg.from_block
    };
    let filter = Filter::new()
        .address(cfg.token)
        .event_signature(transfer_topic0())
        .topic1(wallet)
        .topic2(BURN_SINK)
        .from_block(from)
        .to_block(latest);
    let logs = adapter
        .with_provider(|provider| {
            let filter = filter.clone();
            async move {
                provider
                    .get_logs(&filter)
                    .await
                    .map_err(|e| WalletError::RpcError(format!("assist burn getLogs: {e}")))
            }
        })
        .await?;
    let min = assist_burn_amount_u256();
    for log in logs {
        let amount = transfer_amount_from_log_data(log.data().data.as_ref());
        if amount >= min {
            return Ok(true);
        }
    }
    Ok(false)
}

fn transfer_amount_from_log_data(data: &[u8]) -> U256 {
    if data.len() < 32 {
        return U256::ZERO;
    }
    U256::from_be_slice(&data[data.len() - 32..])
}

/// Build an unsigned ERC-20 transfer of `human_amount` WZRD to the burn sink.
///
/// `human_amount` must be ≥ 13 (decimal string, 18 decimals).
pub fn build_assist_burn_evm(
    from: &str,
    chain_id: u64,
    human_amount: &str,
) -> Result<EvmTransaction, WalletError> {
    let cfg = entitlement_wzrd(chain_id).ok_or_else(|| {
        WalletError::Other(format!(
            "WZRD assist burn not available on chain {chain_id}"
        ))
    })?;
    let amount = parse_human_wzrd_amount(human_amount)?;
    if amount < assist_burn_amount_u256() {
        return Err(WalletError::InvalidAmount(
            "burn at least 13 WZRD in one transfer (no drip)".into(),
        ));
    }
    let tx = TransactionService::new().build_erc20_transfer(
        from,
        format!("{:#x}", cfg.token),
        burn_sink_hex(),
        amount.to_string(),
        chain_id,
    )?;
    match tx {
        ChainTransaction::Evm(evm) => Ok(evm),
        _ => Err(WalletError::Other("expected EVM burn tx".into())),
    }
}

/// WZRD token address for `chain_id` (smoke on 943).
pub fn wzrd_token_hex(chain_id: u64) -> Option<String> {
    entitlement_wzrd(chain_id).map(|c| format!("{:#x}", c.token))
}

fn parse_human_wzrd_amount(human: &str) -> Result<U256, WalletError> {
    use alloy::primitives::utils::parse_units;
    let s = human.trim();
    if s.is_empty() {
        return Err(WalletError::InvalidAmount("empty burn amount".into()));
    }
    let parsed = parse_units(s, 18).map_err(|e| WalletError::InvalidAmount(e.to_string()))?;
    Ok(parsed.into())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct AssistUnlockCache {
    /// Entries as `"chainId:0xaddress"` or `"chainId:vault"` (lowercase).
    /// Any entry for a chain unlocks the whole vault on that chain.
    unlocked: Vec<String>,
}

fn cache_key(chain_id: u64, wallet: Address) -> String {
    format!("{chain_id}:{}", format!("{wallet:#x}").to_lowercase())
}

fn cache_vault_key(chain_id: u64) -> String {
    format!("{chain_id}:vault")
}

fn cache_path(profile_dir: &Path) -> PathBuf {
    profile_dir.join(CACHE_FILE)
}

/// True when this profile has unlocked power features on `chain_id` (any burner).
fn cache_has_chain(profile_dir: &Path, chain_id: u64) -> Result<bool, WalletError> {
    let path = cache_path(profile_dir);
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| WalletError::Other(format!("assist cache read: {e}")))?;
    let cache: AssistUnlockCache = serde_json::from_str(&raw)
        .map_err(|e| WalletError::Other(format!("assist cache parse: {e}")))?;
    let prefix = format!("{chain_id}:");
    Ok(cache.unlocked.iter().any(|k| k.starts_with(&prefix)))
}

fn cache_put_vault(
    profile_dir: &Path,
    chain_id: u64,
    burned_by: Address,
) -> Result<(), WalletError> {
    let path = cache_path(profile_dir);
    let mut cache = if path.exists() {
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| WalletError::Other(format!("assist cache read: {e}")))?;
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        AssistUnlockCache::default()
    };
    for key in [cache_vault_key(chain_id), cache_key(chain_id, burned_by)] {
        if !cache.unlocked.iter().any(|k| k == &key) {
            cache.unlocked.push(key);
        }
    }
    let raw = serde_json::to_string_pretty(&cache)
        .map_err(|e| WalletError::Other(format!("assist cache encode: {e}")))?;
    std::fs::write(&path, raw)
        .map_err(|e| WalletError::Other(format!("assist cache write: {e}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Topic filter uses wallet as `from` and sink as `to` (for tests).
pub fn assist_burn_filter_topics(wallet: Address) -> (B256, Address, Address) {
    (transfer_topic0(), wallet, BURN_SINK)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn burn_amount_is_13e18() {
        assert_eq!(
            assist_burn_amount_u256(),
            U256::from_str("13000000000000000000").unwrap()
        );
    }

    #[test]
    fn filter_topics_order() {
        let w = address!("0x9274c57e08d9cdabca11d7b9c1db04466789574f");
        let (t0, from, to) = assist_burn_filter_topics(w);
        assert_eq!(t0, transfer_topic0());
        assert_eq!(from, w);
        assert_eq!(to, BURN_SINK);
    }

    #[test]
    fn log_data_decodes_amount() {
        let mut data = [0u8; 32];
        data[31] = 42;
        assert_eq!(transfer_amount_from_log_data(&data), U256::from(42));
        assert_eq!(transfer_amount_from_log_data(&[]), U256::ZERO);
    }

    #[test]
    fn human_amount_parses_minimum_and_more() {
        let under = parse_human_wzrd_amount("12").unwrap();
        assert!(under < assist_burn_amount_u256());
        assert_eq!(
            parse_human_wzrd_amount("13").unwrap(),
            assist_burn_amount_u256()
        );
        assert!(parse_human_wzrd_amount("20").unwrap() > assist_burn_amount_u256());
    }

    #[test]
    fn cache_key_includes_chain() {
        let w = address!("0x9274c57e08d9cdabca11d7b9c1db04466789574f");
        assert_ne!(cache_key(943, w), cache_key(369, w));
    }

    #[test]
    fn entitlement_943_resolves_smoke_wzrd() {
        let t = entitlement_wzrd(943).expect("943");
        assert_eq!(
            format!("{:#x}", t.token).to_lowercase(),
            WZRD_SMOKE_943.to_lowercase()
        );
        assert!(entitlement_wzrd(369).is_none());
        assert!(entitlement_wzrd(1).is_none());
    }

    #[test]
    fn gate_defaults_on() {
        std::env::remove_var(ASSIST_BURN_GATE_ENV);
        assert!(assist_burn_gate_enabled());
    }

    #[test]
    fn gate_env_can_disable() {
        std::env::set_var(ASSIST_BURN_GATE_ENV, "0");
        assert!(!assist_burn_gate_enabled());
        std::env::remove_var(ASSIST_BURN_GATE_ENV);
    }

    #[test]
    fn entitlement_chain_prefers_smoke_until_mainnet() {
        assert_eq!(entitlement_chain_id(), Some(WIZ4RD_TESTNET_CHAIN_ID));
    }

    /// Live getLogs against public 943 RPC (manual / CI opt-in).
    #[tokio::test]
    #[ignore = "live 943 RPC — run with --ignored"]
    async fn live_943_zero_address_has_no_assist_burn() {
        std::env::remove_var(ASSIST_UNLOCK_BYPASS_ENV);
        let ok = address_has_assist_burn(None, WIZ4RD_TESTNET_CHAIN_ID, Address::ZERO)
            .await
            .expect("rpc");
        assert!(!ok, "zero address must not be entitled");
    }

    #[test]
    fn cache_any_chain_entry_unlocks_vault() {
        let dir = tempfile::tempdir().unwrap();
        let burner = address!("0xAe089fF30590206F24E4E6627Ea61E4944cFc895");
        let other = address!("0x9274c57e08d9cdabca11d7b9c1db04466789574f");
        assert!(!cache_has_chain(dir.path(), 943).unwrap());
        cache_put_vault(dir.path(), 943, burner).unwrap();
        assert!(cache_has_chain(dir.path(), 943).unwrap());
        assert!(!cache_has_chain(dir.path(), 369).unwrap());
        // Legacy single-address key alone is enough (prefix match).
        let legacy = AssistUnlockCache {
            unlocked: vec![cache_key(943, other)],
        };
        std::fs::write(
            cache_path(dir.path()),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();
        assert!(cache_has_chain(dir.path(), 943).unwrap());
    }

    #[test]
    fn build_burn_rejects_under_13() {
        let err = build_assist_burn_evm("0x9274c57e08d9cdabca11d7b9c1db04466789574f", 943, "12")
            .unwrap_err();
        assert!(err.user_message().contains("at least 13"));
    }

    #[test]
    fn build_burn_ok_at_13() {
        let tx =
            build_assist_burn_evm("0x9274c57e08d9cdabca11d7b9c1db04466789574f", 943, "13").unwrap();
        assert_eq!(tx.to.to_lowercase(), WZRD_SMOKE_943.to_lowercase());
        assert!(tx.data.as_ref().is_some_and(|d| !d.is_empty()));
    }
}
