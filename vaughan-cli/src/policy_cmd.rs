//! CLI `vaughan policy` subcommands and root-approval step-up submission.
//!
//! Purpose: render current or staged pending policy state (`show`, `pending`),
//! apply flag/`<key> <value>` edits locally to a draft (`edit_policy`), and
//! drive the daemon `stage -> poll -> apply` workflow via `policy_step_up`.
//! Called by: `vaughan-cli/src/main.rs` (`Command::Policy`).
//! Must NOT live here: interactive vault-unlock prompting or secret-input crates,
//! direct filesystem access to signer store files, or duplicated step-up state
//! machine logic (`policy_step_up.rs` owns stage/poll/apply).

use std::fmt;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Args, Subcommand};
use serde_json::json;
use vaughan_agent::sentient::AgentSessionPolicy;
use vaughan_core::core::policy_step_up::{self, Staged, StepUpError, StepUpState};
use vaughan_core::core::SignerClient;

use crate::json_out;

/// Default maximum wait time (seconds) matching the daemon's 10-minute pending TTL.
pub const DEFAULT_WAIT_SECS: u64 = 600;

/// Default daemon poll interval while awaiting root approval (`policy_pending` only).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 2;

/// Typed errors returned by `vaughan policy` subcommands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyCliError {
    /// Neither `--flag` overrides nor `<key> <value>` modified the policy draft.
    NoFieldsSpecified,
    /// Caller provided `<key>` without `<value>` (or vice versa).
    MissingKeyOrValue,
    /// A field value or key rejected by `AgentSessionPolicy::set_field`.
    InvalidField(String),
    /// Signer query/response failure outside the `policy_step_up` state machine.
    Signer(String),
    /// Failure returned by the shared `policy_step_up` client flow.
    StepUp(StepUpError),
    /// Waiting was interrupted by Ctrl-C (`SIGINT`) before `apply` ran.
    Interrupted,
}

impl fmt::Display for PolicyCliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoFieldsSpecified => write!(
                f,
                "no policy fields specified — pass flags like --daily-cap-pct 10 or <key> <value>"
            ),
            Self::MissingKeyOrValue => {
                write!(
                    f,
                    "usage: policy set <key> <value> (or use --daily-cap-pct, …)"
                )
            }
            Self::InvalidField(msg) | Self::Signer(msg) => f.write_str(msg),
            Self::StepUp(err) => err.fmt(f),
            Self::Interrupted => write!(
                f,
                "not applied; the staged update stays pending until it expires (10 min) or you re-run the same command"
            ),
        }
    }
}

impl std::error::Error for PolicyCliError {}

/// CLI subcommands under `vaughan policy`.
#[derive(Debug, Subcommand)]
pub enum PolicyCmd {
    /// Print the current sentient policy and digest from vaughan-signerd (`op:"policy"`).
    Show,
    /// Print whether a staged policy update is pending approval (`op:"policy_pending"`).
    Pending,
    /// Edit sentient policy fields locally, then stage and await root approval (`sudo vaughan signerd approve`).
    Set {
        /// Policy field flags, `<key> <value>`, and wait options.
        #[command(flatten)]
        args: SetArgs,
    },
    /// Step 1 of disabling enforcement: set acknowledge_unsafe = true via root approval.
    ConfirmUnsafe {
        /// Expected digest of the active policy being replaced.
        #[arg(long)]
        expected_digest: Option<String>,
        /// Seconds to wait for root approval before timing out cleanly.
        #[arg(long, default_value_t = DEFAULT_WAIT_SECS)]
        wait_secs: u64,
    },
}

/// Grouped policy modification flags and wait parameters for `policy set`.
#[derive(Debug, Clone, Args)]
pub struct SetArgs {
    /// Enforcement mode (`enforced`, `warn-only`, or `disabled`).
    #[arg(long)]
    pub enforcement: Option<String>,
    /// Explicitly acknowledge unsafe settings (`true` or `false`).
    #[arg(long)]
    pub acknowledge_unsafe: Option<bool>,
    /// Single-trade maximum position percentage (`1..=100`).
    #[arg(long)]
    pub max_position_pct: Option<u8>,
    /// Single-approval maximum percentage (`1..=100`).
    #[arg(long)]
    pub max_approve_pct: Option<u8>,
    /// Rolling 24-hour cumulative spend cap percentage (`1..=100`).
    #[arg(long)]
    pub daily_cap_pct: Option<u8>,
    /// Comma-separated transfer allowlist addresses.
    #[arg(long, value_delimiter = ',')]
    pub allow_transfer_to: Option<Vec<String>>,
    /// Remove all entries from `allow_transfer_to`.
    #[arg(long)]
    pub clear_allow_transfer_to: bool,
    /// Maximum swap slippage tolerance in basis points.
    #[arg(long)]
    pub max_slippage_bps: Option<u32>,
    /// Maximum cumulative session gas expenditure in wei.
    #[arg(long)]
    pub max_session_gas_wei: Option<String>,
    /// Maximum consecutive runtime errors before tripping the breaker.
    #[arg(long)]
    pub max_consecutive_errors: Option<u32>,
    /// Minimum RPC endpoints agreeing for quorum checks.
    #[arg(long)]
    pub required_rpc_quorum: Option<usize>,
    /// Expected active policy digest for optimistic concurrency checks.
    #[arg(long)]
    pub expected_digest: Option<String>,
    /// Maximum seconds to poll for root approval (`sudo vaughan signerd approve`).
    #[arg(long, default_value_t = DEFAULT_WAIT_SECS)]
    pub wait_secs: u64,
    /// Optional key for `policy set <key> <value>` syntax.
    pub key: Option<String>,
    /// Optional value for `policy set <key> <value>` syntax.
    pub value: Option<String>,
}

impl Default for SetArgs {
    fn default() -> Self {
        Self {
            enforcement: None,
            acknowledge_unsafe: None,
            max_position_pct: None,
            max_approve_pct: None,
            daily_cap_pct: None,
            allow_transfer_to: None,
            clear_allow_transfer_to: false,
            max_slippage_bps: None,
            max_session_gas_wei: None,
            max_consecutive_errors: None,
            required_rpc_quorum: None,
            expected_digest: None,
            wait_secs: DEFAULT_WAIT_SECS,
            key: None,
            value: None,
        }
    }
}

/// Dispatch `vaughan policy <subcommand>` using the default signer socket client.
pub async fn run_policy_cli(json_mode: bool, action: PolicyCmd) -> anyhow::Result<()> {
    let client = SignerClient::default();
    let res = match action {
        PolicyCmd::Show => show(&client, json_mode).await,
        PolicyCmd::Pending => pending(&client, json_mode).await,
        PolicyCmd::Set { args } => set(&client, json_mode, args).await,
        PolicyCmd::ConfirmUnsafe {
            expected_digest,
            wait_secs,
        } => confirm_unsafe(&client, json_mode, expected_digest, wait_secs).await,
    };
    res.map_err(|e| anyhow::anyhow!("{e}"))
}

async fn fetch_raw_policy(
    client: &SignerClient,
) -> Result<(serde_json::Value, Option<String>), PolicyCliError> {
    let resp = client.policy().await.map_err(PolicyCliError::Signer)?;
    let policy_val = resp
        .policy
        .ok_or_else(|| PolicyCliError::Signer("missing policy in signer response".to_string()))?;
    Ok((policy_val, resp.policy_digest))
}

async fn fetch_typed_policy(
    client: &SignerClient,
) -> Result<(AgentSessionPolicy, Option<String>), PolicyCliError> {
    let (policy_val, digest) = fetch_raw_policy(client).await?;
    let policy = serde_json::from_value(policy_val)
        .map_err(|e| PolicyCliError::Signer(format!("invalid policy from signer: {e}")))?;
    Ok((policy, digest))
}

/// Print the active policy and digest from `vaughan-signerd` (`{"op":"policy"}`).
pub async fn show(client: &SignerClient, json_mode: bool) -> Result<(), PolicyCliError> {
    let (policy_val, digest_opt) = fetch_raw_policy(client).await?;
    let digest = digest_opt.unwrap_or_default();
    let data = json!({
        "policy": policy_val,
        "policy_digest": digest,
    });
    json_out::print_json_value(json_mode, &data, || {
        println!("policy_digest: {digest}");
        if let Ok(policy) = serde_json::from_value::<AgentSessionPolicy>(policy_val.clone()) {
            if let Ok(toml_str) = toml::to_string_pretty(&policy) {
                print!("{toml_str}");
                return;
            }
        }
        if let Ok(pretty) = serde_json::to_string_pretty(&policy_val) {
            println!("{pretty}");
        }
    });
    Ok(())
}

/// Query and print read-only staged policy status (`{"op":"policy_pending"}`).
pub async fn pending(client: &SignerClient, json_mode: bool) -> Result<(), PolicyCliError> {
    let resp = client
        .policy_pending()
        .await
        .map_err(PolicyCliError::Signer)?;
    if !resp.ok {
        let msg = resp
            .message
            .or(resp.code)
            .unwrap_or_else(|| "policy_pending failed".to_string());
        return Err(PolicyCliError::Signer(msg));
    }
    match resp.pending {
        None => {
            let data = json!({ "pending": null });
            json_out::print_json_value(json_mode, &data, || {
                println!("no staged update");
            });
        }
        Some(info) => {
            let seconds_left = info
                .expires_at_unix
                .saturating_sub(vaughan_agent::dca::now_unix());
            let data = json!({
                "policy_digest": info.policy_digest,
                "expires_at_unix": info.expires_at_unix,
                "seconds_left": seconds_left,
                "approval_present": info.approval_present,
            });
            json_out::print_json_value(json_mode, &data, || {
                println!("policy_digest: {}", info.policy_digest);
                println!(
                    "expires_at_unix: {} ({seconds_left}s left)",
                    info.expires_at_unix
                );
                println!("approval_present: {}", info.approval_present);
            });
        }
    }
    Ok(())
}

fn apply_scalar_flags(
    policy: &mut AgentSessionPolicy,
    args: &SetArgs,
) -> Result<bool, PolicyCliError> {
    let mut changed = false;
    if let Some(ref mode) = args.enforcement {
        policy
            .set_field("enforcement", mode)
            .map_err(|e| PolicyCliError::InvalidField(e.to_string()))?;
        changed = true;
    }
    if let Some(ack) = args.acknowledge_unsafe {
        policy.acknowledge_unsafe = ack;
        changed = true;
    }
    if let Some(pct) = args.max_position_pct {
        policy.max_position_pct = pct;
        changed = true;
    }
    if let Some(pct) = args.max_approve_pct {
        policy.max_approve_pct = Some(pct);
        changed = true;
    }
    if let Some(pct) = args.daily_cap_pct {
        policy.daily_cap_pct = pct;
        changed = true;
    }
    if let Some(bps) = args.max_slippage_bps {
        policy.max_slippage_bps = bps;
        changed = true;
    }
    if let Some(ref gas) = args.max_session_gas_wei {
        policy.max_session_gas_wei = gas.clone();
        changed = true;
    }
    if let Some(errs) = args.max_consecutive_errors {
        policy.max_consecutive_errors = errs;
        changed = true;
    }
    if let Some(quorum) = args.required_rpc_quorum {
        policy.required_rpc_quorum = quorum;
        changed = true;
    }
    Ok(changed)
}

fn apply_transfer_flags(policy: &mut AgentSessionPolicy, args: &SetArgs) -> bool {
    let mut changed = false;
    if let Some(ref list) = args.allow_transfer_to {
        policy.allow_transfer_to = list
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        changed = true;
    }
    if args.clear_allow_transfer_to {
        policy.allow_transfer_to.clear();
        changed = true;
    }
    changed
}

/// Apply all flag overrides and optional `<key> <value>` edits to `policy`.
///
/// Fails with [`PolicyCliError::MissingKeyOrValue`] on odd positional arguments
/// or [`PolicyCliError::NoFieldsSpecified`] when no flag or key/value was passed.
pub fn edit_policy(
    policy: &mut AgentSessionPolicy,
    args: &SetArgs,
) -> Result<bool, PolicyCliError> {
    let mut changed = apply_scalar_flags(policy, args)?;
    if apply_transfer_flags(policy, args) {
        changed = true;
    }
    match (&args.key, &args.value) {
        (Some(k), Some(v)) => {
            policy
                .set_field(k, v)
                .map_err(|e| PolicyCliError::InvalidField(e.to_string()))?;
            changed = true;
        }
        (Some(_), None) | (None, Some(_)) => return Err(PolicyCliError::MissingKeyOrValue),
        (None, None) => {}
    }
    if !changed {
        return Err(PolicyCliError::NoFieldsSpecified);
    }
    Ok(true)
}

fn report_wait_progress(staged: &Staged, state: &StepUpState, banner_shown: &AtomicBool) {
    let expires_at = match state {
        StepUpState::Waiting { expires_at_unix } => *expires_at_unix,
        _ => staged.expires_at_unix,
    };
    if !banner_shown.swap(true, Ordering::Relaxed) {
        eprintln!("Staged. In a root console run: sudo vaughan signerd approve");
        eprintln!("  policy_digest: {}", staged.policy_digest);
        eprintln!("  expires_at_unix: {expires_at}");
    }
    if std::io::stderr().is_terminal() {
        let remaining = expires_at.saturating_sub(vaughan_agent::dca::now_unix());
        eprint!("\rWaiting for root approval ({remaining}s remaining)... ");
        let _ = std::io::stderr().flush();
    }
}

fn finish_tty_countdown(banner_shown: &AtomicBool) {
    if banner_shown.load(Ordering::Relaxed) && std::io::stderr().is_terminal() {
        eprintln!();
    }
}

/// Stage a modified policy draft via [`policy_step_up::run_to_completion`], print approval
/// instructions once, poll on the 2-second default interval, handle Ctrl-C cleanly, and apply.
pub async fn submit(
    client: &SignerClient,
    policy: serde_json::Value,
    expected_digest: Option<String>,
    wait: Duration,
) -> Result<String, PolicyCliError> {
    let banner_shown = AtomicBool::new(false);
    let interval = Duration::from_secs(DEFAULT_POLL_INTERVAL_SECS);
    tokio::select! {
        res = policy_step_up::run_to_completion(
            client,
            policy,
            expected_digest,
            interval,
            wait,
            |staged, state| report_wait_progress(staged, state, &banner_shown),
        ) => {
            finish_tty_countdown(&banner_shown);
            res.map_err(PolicyCliError::StepUp)
        }
        sig = tokio::signal::ctrl_c() => {
            finish_tty_countdown(&banner_shown);
            match sig {
                Ok(()) => Err(PolicyCliError::Interrupted),
                Err(e) => Err(PolicyCliError::Signer(format!("signal handler error: {e}"))),
            }
        }
    }
}

/// Edit the current daemon policy with `args` and submit via root-approval step-up.
pub async fn set(
    client: &SignerClient,
    json_mode: bool,
    args: SetArgs,
) -> Result<(), PolicyCliError> {
    let (mut policy, current_digest) = fetch_typed_policy(client).await?;
    edit_policy(&mut policy, &args)?;
    let expected = args.expected_digest.or(current_digest);
    let wait = Duration::from_secs(args.wait_secs);
    let new_val =
        serde_json::to_value(&policy).map_err(|e| PolicyCliError::Signer(e.to_string()))?;
    let new_digest = submit(client, new_val, expected, wait).await?;
    let data = json!({ "policy_digest": new_digest });
    json_out::print_json_value(json_mode, &data, || {
        println!("policy_digest: {new_digest}");
    });
    Ok(())
}

/// Step 1 of disabling enforcement: set `acknowledge_unsafe = true` via root-approval step-up.
pub async fn confirm_unsafe(
    client: &SignerClient,
    json_mode: bool,
    expected_digest: Option<String>,
    wait_secs: u64,
) -> Result<(), PolicyCliError> {
    let (mut policy, current_digest) = fetch_typed_policy(client).await?;
    policy.acknowledge_unsafe = true;
    let new_val =
        serde_json::to_value(&policy).map_err(|e| PolicyCliError::Signer(e.to_string()))?;
    let new_digest = submit(
        client,
        new_val,
        expected_digest.or(current_digest),
        Duration::from_secs(wait_secs),
    )
    .await?;
    let data = json!({
        "acknowledge_unsafe": true,
        "policy_digest": new_digest,
    });
    json_out::print_json_value(json_mode, &data, || {
        println!("acknowledge_unsafe: true");
        println!("policy_digest: {new_digest}");
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vaughan_agent::sentient::EnforcementMode;

    #[test]
    fn edit_policy_no_fields_returns_no_fields_specified_error() {
        let mut policy = AgentSessionPolicy::default();
        let err = edit_policy(&mut policy, &SetArgs::default()).unwrap_err();
        assert_eq!(err, PolicyCliError::NoFieldsSpecified);
        assert!(err.to_string().contains("no policy fields specified"));
    }

    #[test]
    fn edit_policy_odd_positional_args_return_missing_key_or_value() {
        let mut policy = AgentSessionPolicy::default();
        let key_only = SetArgs {
            key: Some("daily_cap_pct".into()),
            ..SetArgs::default()
        };
        assert_eq!(
            edit_policy(&mut policy, &key_only).unwrap_err(),
            PolicyCliError::MissingKeyOrValue
        );

        let val_only = SetArgs {
            value: Some("25".into()),
            ..SetArgs::default()
        };
        assert_eq!(
            edit_policy(&mut policy, &val_only).unwrap_err(),
            PolicyCliError::MissingKeyOrValue
        );
    }

    #[test]
    fn edit_policy_applies_all_flags_and_clear_allowlist() {
        let mut policy = AgentSessionPolicy::default();
        let args = SetArgs {
            enforcement: Some("warn-only".into()),
            acknowledge_unsafe: Some(true),
            max_position_pct: Some(12),
            max_approve_pct: Some(25),
            daily_cap_pct: Some(40),
            allow_transfer_to: Some(vec![
                " 0x1111111111111111111111111111111111111111 ".into(),
                "".into(),
                "0x2222222222222222222222222222222222222222".into(),
            ]),
            max_slippage_bps: Some(150),
            max_session_gas_wei: Some("900000000000000000".into()),
            max_consecutive_errors: Some(7),
            required_rpc_quorum: Some(2),
            ..SetArgs::default()
        };
        assert!(edit_policy(&mut policy, &args).unwrap());
        assert_eq!(policy.enforcement, EnforcementMode::WarnOnly);
        assert!(policy.acknowledge_unsafe);
        assert_eq!(policy.max_position_pct, 12);
        assert_eq!(policy.max_approve_pct, Some(25));
        assert_eq!(policy.daily_cap_pct, 40);
        assert_eq!(
            policy.allow_transfer_to,
            vec![
                "0x1111111111111111111111111111111111111111",
                "0x2222222222222222222222222222222222222222"
            ]
        );
        assert_eq!(policy.max_slippage_bps, 150);
        assert_eq!(policy.max_session_gas_wei, "900000000000000000");
        assert_eq!(policy.max_consecutive_errors, 7);
        assert_eq!(policy.required_rpc_quorum, 2);

        let clear = SetArgs {
            clear_allow_transfer_to: true,
            ..SetArgs::default()
        };
        assert!(edit_policy(&mut policy, &clear).unwrap());
        assert!(policy.allow_transfer_to.is_empty());
    }

    #[test]
    fn edit_policy_positional_key_value_syntax_and_invalid_key() {
        let mut policy = AgentSessionPolicy::default();
        let kv = SetArgs {
            key: Some("daily_cap_pct".into()),
            value: Some("33".into()),
            ..SetArgs::default()
        };
        assert!(edit_policy(&mut policy, &kv).unwrap());
        assert_eq!(policy.daily_cap_pct, 33);

        let bad = SetArgs {
            key: Some("nonexistent_key".into()),
            value: Some("1".into()),
            ..SetArgs::default()
        };
        let err = edit_policy(&mut policy, &bad).unwrap_err();
        assert!(matches!(err, PolicyCliError::InvalidField(msg) if msg.contains("unknown")));
    }

    #[test]
    fn policy_cli_error_interrupted_and_wait_defaults_match_spec() {
        assert_eq!(DEFAULT_WAIT_SECS, 600);
        assert_eq!(DEFAULT_POLL_INTERVAL_SECS, 2);
        assert_eq!(
            PolicyCliError::Interrupted.to_string(),
            "not applied; the staged update stays pending until it expires (10 min) or you re-run the same command"
        );
    }
}
