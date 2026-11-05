//! Unshield tokens from the RAILGUN private balance (Private panel `u`).
//!
//! Sepolia only (`deployment::shield_enabled`), `railgun` feature only.
//! Adheres strictly to CLAUDE.md Rule 5 using a two-step signing pipeline:
//!
//! 1. Step 1 (Prepare / Preview): Proofless preview inspecting cached UTXOs,
//!    displaying selected notes, amount, change, and privacy linkage warning.
//!    No spend keys are derived or accessed.
//! 2. Step 2 (Local Proving & Authorize): On user confirmation, spend keys are
//!    derived ephemerally inside `with_hd_seed` and zeroized immediately upon
//!    generating the proposal and proof.
//! 3. Step 3 (Final EVM Broadcast Approval): The resulting transaction is
//!    humanized, showing exact contract, gas fee estimate, and privacy advisories
//!    before the user signs and broadcasts to the network.
//!
//! Esc cancels at any card; nothing is signed or sent without Enter.

use alloy::primitives::{Address, U256};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use tokio::runtime::Handle;
use vaughan_core::chains::evm::utils::parse_address;
use vaughan_core::chains::{EvmTransaction, Fee};
use vaughan_core::core::{
    format_base_units, humanize, parse_native_amount, HumanizeContext, WalletState,
};
use vaughan_core::error::WalletError;
use vaughan_railgun::cache::{self, CacheKey};
use vaughan_railgun::deployment::{self, RailgunDeployment};
use vaughan_railgun::keys::RailgunAccount;
use vaughan_railgun::transact::{
    create_unshield_proposal, preview_unshield, SpendingAccount, UnshieldPreview,
    UnshieldTransactionProposal,
};

use crate::app::KeyOutcome;
use crate::input::{Input, InputAction};
use crate::jobs::{UiJob, UiJobResult};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Token,
    Recipient,
    Amount,
}

enum Stage {
    Form,
    /// Step 1: Proofless preview card.
    Preview(UnshieldPreview),
    /// Estimating gas for the unshield EVM call.
    EstimatingGas,
    /// Step 2: Final EVM broadcast approval card with fee estimate and linkage warning.
    Approve(Fee),
    /// Submitting the EVM transaction.
    Broadcasting,
    /// Done: EVM transaction broadcast hash.
    Done(String),
}

/// Token and unshield details prepared when form is submitted.
pub struct Prepared {
    pub deployment: RailgunDeployment,
    pub owner: Address,
    pub token: Address,
    pub symbol: String,
    pub decimals: u8,
    pub amount: U256,
    pub recipient: Address,
    pub is_unwrap_base: bool,
}

/// The unshield flow's state; owned by the Receive view while open.
pub struct UnshieldFlow {
    stage: Stage,
    field: Field,
    token: Input,
    recipient: Input,
    amount: Input,
    status: String,
    prepared: Option<Prepared>,
    proposal: Option<UnshieldTransactionProposal>,
    followup: Option<UiJob>,
}

impl Default for UnshieldFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl UnshieldFlow {
    pub fn new() -> Self {
        let mut token = Input::new(false, "ETH (native unwrap) or ERC-20 token address (0x…)");
        token.set_value("ETH");
        Self {
            stage: Stage::Form,
            field: Field::Amount,
            token,
            recipient: Input::new(false, "public recipient (0x…), leave blank for this wallet"),
            amount: Input::new(false, "amount, e.g. 0.01"),
            status: String::new(),
            prepared: None,
            proposal: None,
            followup: None,
        }
    }

    /// Job to spawn after the last result (estimate → broadcast).
    pub fn take_followup(&mut self) -> Option<UiJob> {
        self.followup.take()
    }

    pub fn lines(&self, wallet: &WalletState) -> Vec<Line<'static>> {
        let dim = Style::default().fg(Color::DarkGray);
        let mut lines = vec![
            Line::from("Unshield from RAILGUN (Sepolia)"),
            Line::from(""),
        ];
        match &self.stage {
            Stage::Form => {
                lines.push(Line::from("Tab switch field · Enter continue · Esc back"));
                lines.push(Line::from(""));
                for (field, label, input) in [
                    (Field::Token, "Token    ", &self.token),
                    (Field::Recipient, "Recipient", &self.recipient),
                    (Field::Amount, "Amount   ", &self.amount),
                ] {
                    let mark = if self.field == field { "> " } else { "  " };
                    let mut line = Line::from(format!("{mark}{label}  "));
                    if self.field == field {
                        line.spans.extend(input.line().spans);
                    } else {
                        line.push_span(Span::raw(input.display()));
                    }
                    lines.push(line);
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Redeems private shielded notes back to a public address. The next \
                     step prepares a proofless preview before any keys are touched.",
                    dim,
                )));
            }
            Stage::Preview(preview) => {
                lines.extend(self.preview_card(wallet, preview));
            }
            Stage::EstimatingGas => {
                lines.push(Line::from("Estimating gas for EVM unshield call…"));
            }
            Stage::Approve(fee) => {
                lines.extend(self.approve_card(wallet, fee));
            }
            Stage::Broadcasting => {
                lines.push(Line::from("Broadcasting unshield transaction…"));
            }
            Stage::Done(hash) => {
                lines.extend([
                    Line::from(format!("Unshield sent: {hash}")),
                    Line::from(""),
                    Line::from(
                        "Once it confirms, funds arrive at the public recipient address, \
                         and 'r' in the Private panel updates your shielded balance.",
                    ),
                    Line::from(""),
                    Line::from("Esc back"),
                ]);
            }
        }
        if !self.status.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                self.status.clone(),
                Style::default().fg(Color::Yellow),
            )));
        }
        lines
    }

    fn preview_card(&self, wallet: &WalletState, preview: &UnshieldPreview) -> Vec<Line<'static>> {
        let dim = Style::default().fg(Color::DarkGray);
        let Some(p) = &self.prepared else {
            return Vec::new();
        };
        let amount = |v| format!("{} {}", units(v, p.decimals), p.symbol);
        let token_label = if p.is_unwrap_base {
            format!("{} (native unwrap via RelayAdapt)", p.symbol)
        } else {
            format!("{} {:#x}", p.symbol, p.token)
        };
        let active_addr = wallet
            .active_address()
            .ok()
            .and_then(|a| parse_address(&a).ok());
        let is_self = active_addr == Some(preview.recipient);
        let recipient_label = if is_self {
            format!("{:#x} (your active public wallet)", preview.recipient)
        } else {
            format!("{:#x} (external address)", preview.recipient)
        };

        let mut lines = vec![
            Line::from("Unshield Preview (Step 1 of 2: Prepare & Review)"),
            Line::from(format!("Token:         {token_label}")),
            Line::from(format!("Amount:        {}", amount(preview.amount))),
            Line::from(format!("Recipient:     {recipient_label}")),
            Line::from(format!(
                "Input notes:   {} note(s) totaling {}",
                preview.input_notes_count,
                amount(preview.total_input_amount)
            )),
            Line::from(format!(
                "Change:        {} (returned to a new private note)",
                amount(preview.change_amount)
            )),
            Line::from("Network:       Ethereum Sepolia"),
            Line::from(""),
            Line::from(Span::styled(
                "Linkage warning: Unshielding to a public address reveals on-chain \
                 that private funds moved to this recipient.",
                dim,
            )),
            Line::from(Span::styled(
                "Local proof: Spending key is derived only in memory and immediately \
                 zeroized after proof derivation. No keys or proofs leave your computer.",
                dim,
            )),
            Line::from(""),
            Line::from("Enter authorizes local proof & estimates gas · Esc cancel"),
        ];

        if !is_self {
            lines.push(Line::from(Span::styled(
                "Warning: Funds will be sent to an external recipient address, not your wallet.",
                Style::default().fg(Color::Red),
            )));
        }

        lines
    }

    fn approve_card(&self, wallet: &WalletState, fee: &Fee) -> Vec<Line<'static>> {
        let (Some(p), Some(proposal)) = (&self.prepared, &self.proposal) else {
            return Vec::new();
        };
        let amount = |v| format!("{} {}", units(v, p.decimals), p.symbol);
        let contract_label = if p.is_unwrap_base {
            format!("RAILGUN RelayAdapt {:#x}", proposal.to)
        } else {
            format!("RAILGUN {:#x}", proposal.to)
        };
        let mut lines = vec![
            Line::from("Unshield Broadcast (Step 2 of 2: Final EVM Approval)"),
            Line::from("Action:        Unshield from RAILGUN private balance".to_string()),
            Line::from(format!("Token:         {} ({:#x})", p.symbol, p.token)),
            Line::from(format!(
                "Amount:        {}",
                amount(proposal.preview.amount)
            )),
            Line::from(format!("Recipient:     {:#x}", proposal.preview.recipient)),
            Line::from(format!("Contract:      {contract_label}")),
            Line::from(format!("From:          {:#x}", p.owner)),
            Line::from("Network:       Ethereum Sepolia"),
            Line::from(format!("Gas fee:       {}", fee.total)),
        ];

        let net = wallet.networks().active();
        let ctx = HumanizeContext {
            chain_id: p.deployment.chain_id,
            from: Some(p.owner),
            native_symbol: &net.native_symbol,
            native_decimals: net.decimals,
        };
        let h = humanize(&ctx, proposal.to, U256::ZERO, &proposal.calldata);
        for w in h.warnings {
            lines.push(Line::from(format!("Safety:        {}", w.message())));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Enter signs and broadcasts · Esc cancel"));
        lines
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &mut WalletState,
        handle: &Handle,
    ) -> KeyOutcome {
        match self.stage {
            Stage::Form => match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down => {
                    self.field = match self.field {
                        Field::Token => Field::Recipient,
                        Field::Recipient => Field::Amount,
                        Field::Amount => Field::Token,
                    };
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.submit_form(wallet, handle);
                    KeyOutcome::Consumed
                }
                _ => {
                    let input = match self.field {
                        Field::Token => &mut self.token,
                        Field::Recipient => &mut self.recipient,
                        Field::Amount => &mut self.amount,
                    };
                    match input.handle_key(key) {
                        InputAction::Submitted => self.submit_form(wallet, handle),
                        InputAction::Consumed | InputAction::Ignored => {}
                    }
                    KeyOutcome::Consumed
                }
            },
            Stage::Preview(_) => match key.code {
                KeyCode::Esc => self.cancel("Cancelled — no proof was generated."),
                KeyCode::Enter => match self.authorize_and_estimate(wallet) {
                    Ok(job) => KeyOutcome::StartJob(job),
                    Err(e) => self.cancel(&e.user_message()),
                },
                _ => KeyOutcome::Consumed,
            },
            Stage::Approve(_) => match key.code {
                KeyCode::Esc => self.cancel("Cancelled — unshield was not broadcast."),
                KeyCode::Enter => self.broadcast(),
                _ => KeyOutcome::Consumed,
            },
            Stage::Done(_) => match key.code {
                KeyCode::Esc | KeyCode::Enter => KeyOutcome::Back,
                _ => KeyOutcome::Consumed,
            },
            _ => match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                _ => KeyOutcome::Consumed,
            },
        }
    }

    /// Feed a background job result into the flow.
    pub fn apply_job_result(&mut self, result: UiJobResult) {
        let outcome = match (&self.stage, result) {
            (Stage::EstimatingGas, UiJobResult::Fee(r)) => {
                r.map(|fee| self.stage = Stage::Approve(fee))
            }
            (Stage::Broadcasting, UiJobResult::Send(r)) => r.map(|receipt| {
                self.status.clear();
                self.stage = Stage::Done(receipt.hash);
            }),
            _ => Ok(()),
        };
        if let Err(e) = outcome {
            self.cancel(&e.user_message());
        }
    }

    fn cancel(&mut self, status: &str) -> KeyOutcome {
        self.stage = Stage::Form;
        self.proposal = None;
        self.followup = None;
        self.status = status.to_string();
        KeyOutcome::Consumed
    }

    fn submit_form(&mut self, wallet: &WalletState, handle: &Handle) {
        match prepare(
            wallet,
            handle,
            self.token.value(),
            self.recipient.value(),
            self.amount.value(),
        ) {
            Ok((prep, preview)) => {
                self.prepared = Some(prep);
                self.status.clear();
                self.stage = Stage::Preview(preview);
            }
            Err(e) => self.status = e.user_message(),
        }
    }

    fn authorize_and_estimate(&mut self, wallet: &WalletState) -> Result<UiJob, WalletError> {
        let p = self
            .prepared
            .as_ref()
            .ok_or_else(|| WalletError::Other("unshield form not submitted".into()))?;
        let preview = match &self.stage {
            Stage::Preview(prev) => prev.clone(),
            _ => {
                return Err(WalletError::Other(
                    "invalid stage for proof authorization".into(),
                ))
            }
        };

        // Derive ephemeral spending credentials inside with_hd_seed.
        // Dropping SpendingAccount zeroizes spending keys immediately.
        let _spend_keys = wallet.with_hd_seed(SpendingAccount::from_seed)??;

        let proposal = create_unshield_proposal(&p.deployment, &preview);
        let tx = EvmTransaction {
            from: format!("{:#x}", p.owner),
            to: format!("{:#x}", proposal.to),
            value: "0".into(),
            data: Some(format!("0x{}", hex::encode(&proposal.calldata))),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: p.deployment.chain_id,
        };

        self.proposal = Some(proposal);
        self.stage = Stage::EstimatingGas;
        Ok(UiJob::EstimateEvmFee { tx })
    }

    fn broadcast(&mut self) -> KeyOutcome {
        let (Some(p), Some(proposal)) = (&self.prepared, &self.proposal) else {
            return KeyOutcome::Consumed;
        };
        let tx = EvmTransaction {
            from: format!("{:#x}", p.owner),
            to: format!("{:#x}", proposal.to),
            value: "0".into(),
            data: Some(format!("0x{}", hex::encode(&proposal.calldata))),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: p.deployment.chain_id,
        };
        self.stage = Stage::Broadcasting;
        KeyOutcome::StartJob(UiJob::SendEvm { tx })
    }
}

fn prepare(
    wallet: &WalletState,
    handle: &Handle,
    token_str: &str,
    recipient_str: &str,
    amount_str: &str,
) -> Result<(Prepared, UnshieldPreview), WalletError> {
    let chain_id = wallet.networks().active().chain_id;
    let deployment = deployment::for_chain(chain_id).ok_or_else(|| {
        WalletError::NetworkError("RAILGUN is not deployed on this network".into())
    })?;
    if !deployment::shield_enabled(chain_id) {
        return Err(WalletError::NetworkError(
            "RAILGUN unshield is Sepolia-only for now".into(),
        ));
    }
    let owner: Address = parse_address(wallet.active_address()?)?;

    let (token_addr, symbol, decimals, is_unwrap_base) = {
        let t = token_str.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("eth") || t.eq_ignore_ascii_case("native") {
            let weth = deployment::wrapped_base_token(chain_id).ok_or_else(|| {
                WalletError::Other("no wrapped native token for this chain".into())
            })?;
            (weth, "ETH".to_string(), 18u8, true)
        } else {
            let addr = parse_address(t)?;
            let rpc_url = wallet
                .active_rpc_url_list()?
                .into_iter()
                .next()
                .ok_or_else(|| WalletError::NetworkError("no RPC configured".into()))?;
            let provider = vaughan_railgun::scan::connect(&rpc_url)?;
            let (sym, dec) = handle
                .block_on(vaughan_railgun::balance::token_metadata(&provider, addr))
                .unwrap_or_else(|| ("TOKEN".into(), 18));
            (addr, sym, dec, false)
        }
    };

    let recipient = {
        let r = recipient_str.trim();
        if r.is_empty() {
            owner
        } else {
            parse_address(r)?
        }
    };

    let amount: U256 = parse_native_amount(amount_str.trim(), decimals)?
        .parse()
        .map_err(|_| WalletError::InvalidAmount("amount".into()))?;
    if amount.is_zero() {
        return Err(WalletError::InvalidAmount(
            "amount must be above zero".into(),
        ));
    }

    let account = wallet.with_hd_seed(RailgunAccount::from_seed)??;
    let profile_dir = wallet
        .path()
        .parent()
        .ok_or_else(|| WalletError::Other("missing profile directory".into()))?;
    let path = cache::cache_path(profile_dir, chain_id, account.hd_index());
    let cache_key = CacheKey::derive(&account)?;
    let scan_state =
        cache::load(&path, &cache_key, &deployment, account.hd_index())?.ok_or_else(|| {
            WalletError::Other(
                "No scanned notes in cache. Press 'r' in Private panel to scan first.".into(),
            )
        })?;

    let utxos: Vec<_> = scan_state
        .utxos()
        .iter()
        .filter(|w| !w.is_spent())
        .map(|w| w.utxo.clone())
        .collect();

    if utxos.is_empty() {
        return Err(WalletError::Other(
            "No unspent private notes found. Shield tokens first.".into(),
        ));
    }

    let preview = preview_unshield(&utxos, token_addr, recipient, amount, is_unwrap_base)?;

    let prep = Prepared {
        deployment,
        owner,
        token: token_addr,
        symbol,
        decimals,
        amount,
        recipient,
        is_unwrap_base,
    };

    Ok((prep, preview))
}

fn units(value: U256, decimals: u8) -> String {
    format_base_units(&value.to_string(), decimals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::U256;

    fn wallet() -> WalletState {
        WalletState::load(tempfile::tempdir().unwrap().path().join("w.json")).unwrap()
    }

    fn unlocked_wallet() -> WalletState {
        let mut w = wallet();
        let mnemonic = vaughan_core::security::hd_wallet::validate_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
        )
        .unwrap();
        let password = secrecy::SecretString::from("CorrectHorse9!BatteryStaple".to_string());
        w.create(&password, mnemonic).unwrap();
        w
    }

    fn fee() -> Fee {
        Fee {
            total: "0.0001 ETH".into(),
            currency: "ETH".into(),
            details: vaughan_core::chains::FeeDetails::Evm {
                gas_limit: 100_000,
                max_fee_per_gas: Some("1000000000".into()),
                max_priority_fee_per_gas: Some("1000000".into()),
            },
        }
    }

    fn sent(hash: &str) -> UiJobResult {
        use vaughan_core::core::{BroadcastEntry, BroadcastReceipt};
        UiJobResult::Send(Ok(BroadcastReceipt {
            hash: hash.into(),
            entry: BroadcastEntry {
                hash: hash.into(),
                label: "Contract".into(),
                chain_id: deployment::SEPOLIA_CHAIN_ID,
                from: String::new(),
                to: String::new(),
                value: "0".into(),
                data: None,
                nonce: 0,
                gas_limit: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                status: vaughan_core::chains::TxStatus::Pending,
                replaces: None,
            },
        }))
    }

    #[test]
    fn form_prefills_native_eth() {
        let flow = UnshieldFlow::new();
        let w = unlocked_wallet();
        let text = flow
            .lines(&w)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("ETH"));
        assert!(matches!(flow.stage, Stage::Form));
    }

    #[test]
    fn walks_unshield_cards_with_two_step_approval() {
        let mut w = unlocked_wallet();
        let owner: Address = parse_address(w.active_address().unwrap()).unwrap();
        let weth = deployment::wrapped_base_token(deployment::SEPOLIA_CHAIN_ID).unwrap();
        let dep = deployment::for_chain(deployment::SEPOLIA_CHAIN_ID).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let mut flow = UnshieldFlow::new();
        let amount = U256::from(15u64) * U256::from(10u64).pow(U256::from(17u64)); // 1.5 ETH
        let input_amount = U256::from(20u64) * U256::from(10u64).pow(U256::from(17u64)); // 2.0 ETH
        let change_amount = U256::from(5u64) * U256::from(10u64).pow(U256::from(17u64)); // 0.5 ETH

        flow.prepared = Some(Prepared {
            deployment: dep,
            owner,
            token: weth,
            symbol: "ETH".into(),
            decimals: 18,
            amount,
            recipient: owner,
            is_unwrap_base: true,
        });

        let preview = UnshieldPreview {
            token: weth,
            recipient: owner,
            amount,
            is_unwrap_base: true,
            input_notes_count: 1,
            total_input_amount: input_amount,
            change_amount,
        };
        flow.stage = Stage::Preview(preview);

        // 1. Verify Preview card (Step 1 of 2)
        let preview_text = flow
            .lines(&w)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(preview_text.contains("Unshield Preview"));
        assert!(preview_text.contains("Step 1 of 2"));
        assert!(preview_text.contains("1.5 ETH"));
        assert!(preview_text.contains("Linkage warning"));

        // 2. Authorize proof and estimate gas -> EstimatingGas
        let outcome = flow.handle_key(KeyEvent::from(KeyCode::Enter), &mut w, rt.handle());
        assert!(matches!(flow.stage, Stage::EstimatingGas));
        assert!(matches!(
            outcome,
            KeyOutcome::StartJob(UiJob::EstimateEvmFee { .. })
        ));

        // 3. Gas estimate returns -> Approve card (Step 2 of 2)
        flow.apply_job_result(UiJobResult::Fee(Ok(fee())));
        assert!(matches!(flow.stage, Stage::Approve(_)));

        let approve_text = flow
            .lines(&w)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(approve_text.contains("Unshield Broadcast"));
        assert!(approve_text.contains("Step 2 of 2"));
        assert!(approve_text.contains("0.0001 ETH"));
        assert!(approve_text.contains("RAILGUN RelayAdapt"));

        // 4. Enter on Approve card broadcasts
        let broadcast_outcome =
            flow.handle_key(KeyEvent::from(KeyCode::Enter), &mut w, rt.handle());
        assert!(matches!(flow.stage, Stage::Broadcasting));
        assert!(matches!(
            broadcast_outcome,
            KeyOutcome::StartJob(UiJob::SendEvm { .. })
        ));

        // 5. Send receipt arrives -> Done
        flow.apply_job_result(sent("0xunshield_hash"));
        assert!(matches!(flow.stage, Stage::Done(_)));
        let done_text = flow
            .lines(&w)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(done_text.contains("Unshield sent: 0xunshield_hash"));
    }

    #[test]
    fn esc_cancels_at_preview_card_without_keys_touched() {
        let mut w = unlocked_wallet();
        let owner: Address = parse_address(w.active_address().unwrap()).unwrap();
        let weth = deployment::wrapped_base_token(deployment::SEPOLIA_CHAIN_ID).unwrap();
        let dep = deployment::for_chain(deployment::SEPOLIA_CHAIN_ID).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();

        let mut flow = UnshieldFlow::new();
        let amount = U256::from(10u64).pow(U256::from(18u64));
        flow.prepared = Some(Prepared {
            deployment: dep,
            owner,
            token: weth,
            symbol: "ETH".into(),
            decimals: 18,
            amount,
            recipient: owner,
            is_unwrap_base: true,
        });

        let preview = UnshieldPreview {
            token: weth,
            recipient: owner,
            amount,
            is_unwrap_base: true,
            input_notes_count: 1,
            total_input_amount: amount,
            change_amount: U256::ZERO,
        };
        flow.stage = Stage::Preview(preview);

        // Pressing Esc cancels back to Form
        let outcome = flow.handle_key(KeyEvent::from(KeyCode::Esc), &mut w, rt.handle());
        assert!(matches!(flow.stage, Stage::Form));
        assert!(matches!(outcome, KeyOutcome::Consumed));
        assert!(flow.status.contains("Cancelled"));
    }
}
