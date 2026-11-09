//! Shield an ERC-20 into the RAILGUN private balance (Private panel `s`).
//!
//! Sepolia only (`deployment::shield_enabled`), `railgun` feature only. Each
//! signature gets its own card and a fresh Enter:
//!
//! 1. the `RAILGUN_SHIELD` message, which derives the shield key the way
//!    Railway does (no funds move, no gas);
//! 2. an exact-amount `approve` of the RAILGUN contract, only when the
//!    allowance is short;
//! 3. the `shield` transaction, showing the protocol fee, what lands in the
//!    private balance, and the recipient recomputed from this wallet's keys.
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
use vaughan_railgun::deployment::{self, RailgunDeployment};
use vaughan_railgun::keys::RailgunAccount;
use vaughan_railgun::shield::{self, ShieldKey, ShieldPlan, SHIELD_MESSAGE};
use vaughan_railgun::verify::shield_recipients;
use zeroize::Zeroizing;

use crate::app::KeyOutcome;
use crate::input::{Input, InputAction};
use crate::jobs::{UiJob, UiJobResult};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Token,
    Amount,
}

/// Which of the plan's two transactions.
#[derive(Clone, Copy)]
enum Tx {
    Approve,
    Shield,
}

enum Stage {
    Form,
    /// Card for the `RAILGUN_SHIELD` message signature.
    SignKey,
    CheckingAllowance,
    EstimatingApprove,
    Approve(Fee),
    /// Approve sent: waiting for the allowance, then the shield estimate.
    Approving,
    EstimatingShield,
    Shield(Fee),
    Shielding,
    Done(String),
}

/// Token details read when the form is submitted.
struct Prepared {
    deployment: RailgunDeployment,
    rpc_url: String,
    owner: Address,
    token: Address,
    symbol: String,
    decimals: u8,
    amount: U256,
    fee_bps: u16,
    is_native: bool,
}

/// The shield flow's state; owned by the Receive view while open.
pub struct ShieldFlow {
    stage: Stage,
    field: Field,
    token: Input,
    amount: Input,
    status: String,
    prepared: Option<Prepared>,
    plan: Option<ShieldPlan>,
    /// "Recipient" row value, recomputed from this wallet's keys.
    recipient: String,
    followup: Option<UiJob>,
}

impl Default for ShieldFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl ShieldFlow {
    pub fn new() -> Self {
        let mut token = Input::new(false, "ETH (native) or ERC-20 token address (0x…)");
        token.set_value("ETH");
        Self {
            stage: Stage::Form,
            field: Field::Amount,
            token,
            amount: Input::new(false, "amount, e.g. 0.01"),
            status: String::new(),
            prepared: None,
            plan: None,
            recipient: String::new(),
            followup: None,
        }
    }

    /// Job to spawn after the last result (allowance → estimate → …).
    pub fn take_followup(&mut self) -> Option<UiJob> {
        self.followup.take()
    }

    pub fn lines(&self, wallet: &WalletState) -> Vec<Line<'static>> {
        let dim = Style::default().fg(Color::DarkGray);
        let mut lines = vec![Line::from("Shield into RAILGUN (Sepolia)"), Line::from("")];
        match &self.stage {
            Stage::Form => {
                lines.push(Line::from("Tab switch field · Enter continue · Esc back"));
                lines.push(Line::from(""));
                for (field, label, input) in [
                    (Field::Token, "Token ", &self.token),
                    (Field::Amount, "Amount", &self.amount),
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
                    "Moves public tokens into your private balance. RAILGUN keeps a \
                     small fee; the next card shows it before anything is signed.",
                    dim,
                )));
            }
            Stage::SignKey => {
                let from = wallet
                    .active_address()
                    .map(|a| a.to_string())
                    .unwrap_or_default();
                lines.extend([
                    Line::from("Sign message (1 of 2–3)"),
                    Line::from(format!("Message:  \"{SHIELD_MESSAGE}\"")),
                    Line::from(format!("Wallet:   {from}")),
                    Line::from("Network:  Ethereum Sepolia"),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Derives your shield key the same way Railway does. Not a \
                         transaction: no funds move and no gas is paid.",
                        dim,
                    )),
                    Line::from(""),
                    Line::from("Enter signs · Esc cancel"),
                ]);
            }
            Stage::Approve(fee) => {
                if let (Some(p), Some(plan)) = (&self.prepared, &self.plan) {
                    lines.extend([
                        Line::from("Approve token (2 of 3)"),
                        Line::from(format!(
                            "Allow:    RAILGUN to move {} {} (exact amount, not unlimited)",
                            units(plan.amount, p.decimals),
                            p.symbol
                        )),
                        Line::from(format!("Token:    {} {:#x}", p.symbol, p.token)),
                        Line::from(format!("Spender:  RAILGUN {:#x}", plan.contract)),
                        Line::from(format!("From:     {:#x}", p.owner)),
                        Line::from("Network:  Ethereum Sepolia"),
                        Line::from(format!("Gas fee:  {}", fee.total)),
                        Line::from(""),
                        Line::from("Enter signs and sends · Esc cancel"),
                    ]);
                }
            }
            Stage::Shield(fee) => lines.extend(self.shield_card(wallet, fee)),
            Stage::Done(hash) => {
                lines.extend([
                    Line::from(format!("Shield sent: {hash}")),
                    Line::from(""),
                    Line::from(
                        "Once it confirms, r in the Private panel rescans and shows the \
                         new balance.",
                    ),
                    Line::from("Esc back"),
                ]);
            }
            Stage::CheckingAllowance => lines.push(Line::from("Checking token allowance…")),
            Stage::EstimatingApprove | Stage::EstimatingShield => {
                lines.push(Line::from("Estimating gas…"));
            }
            Stage::Approving => lines.push(Line::from(
                "Approval sent; waiting for it to confirm before the shield card…",
            )),
            Stage::Shielding => lines.push(Line::from("Sending shield…")),
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

    fn shield_card(&self, wallet: &WalletState, fee: &Fee) -> Vec<Line<'static>> {
        let (Some(p), Some(plan)) = (&self.prepared, &self.plan) else {
            return Vec::new();
        };
        let amount = |v| format!("{} {}", units(v, p.decimals), p.symbol);
        let percent = format!("{}.{:02}%", plan.fee_bps / 100, plan.fee_bps % 100);
        let (token_label, receive_label) = if p.is_native {
            (
                format!("{} (native → wraps to WETH)", p.symbol),
                format!("{} WETH (private)", units(plan.net, p.decimals)),
            )
        } else {
            (
                format!("{} {:#x}", p.symbol, p.token),
                format!("{} (private)", amount(plan.net)),
            )
        };
        let contract_label = if p.is_native {
            format!("RAILGUN RelayAdapt {:#x}", plan.contract)
        } else {
            format!("RAILGUN {:#x}", plan.contract)
        };
        let mut lines = vec![
            Line::from("Shield (last step)"),
            Line::from(format!("Token:        {token_label}")),
            Line::from(format!("Amount:       {}", amount(plan.amount))),
            Line::from(format!(
                "Protocol fee: {} ({percent}, kept by RAILGUN)",
                amount(plan.fee)
            )),
            Line::from(format!("You receive:  {receive_label}")),
            Line::from(format!("Recipient:    {}", self.recipient)),
            Line::from(format!("Contract:     {contract_label}")),
            Line::from(format!("From:         {:#x}", p.owner)),
            Line::from("Network:      Ethereum Sepolia"),
            Line::from(format!("Gas fee:      {}", fee.total)),
        ];
        let net = wallet.networks().active();
        let ctx = HumanizeContext {
            chain_id: p.deployment.chain_id,
            from: Some(p.owner),
            native_symbol: &net.native_symbol,
            native_decimals: net.decimals,
        };
        let h = humanize(&ctx, plan.contract, plan.value, &plan.shield_calldata);
        for w in h.warnings {
            lines.push(Line::from(format!("Safety:       {}", w.message())));
        }
        lines.push(Line::from(Span::styled(
            "Shielding from your public address shows on-chain that you deposited.",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from("Enter signs and sends · Esc cancel"));
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
                        Field::Token => Field::Amount,
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
                        Field::Amount => &mut self.amount,
                    };
                    match input.handle_key(key) {
                        InputAction::Submitted => self.submit_form(wallet, handle),
                        InputAction::Consumed | InputAction::Ignored => {}
                    }
                    KeyOutcome::Consumed
                }
            },
            Stage::SignKey => match key.code {
                KeyCode::Esc => self.cancel("Cancelled — nothing was signed."),
                KeyCode::Enter => match self.sign_and_plan(wallet) {
                    Ok(job) => KeyOutcome::StartJob(job),
                    Err(e) => self.cancel(&e.user_message()),
                },
                _ => KeyOutcome::Consumed,
            },
            Stage::Approve(_) => match key.code {
                KeyCode::Esc => self.cancel("Cancelled — nothing was sent."),
                KeyCode::Enter => self.send(Stage::Approving, Tx::Approve),
                _ => KeyOutcome::Consumed,
            },
            Stage::Shield(_) => match key.code {
                KeyCode::Esc => self.cancel("Cancelled — the shield wasn't sent."),
                KeyCode::Enter => self.send(Stage::Shielding, Tx::Shield),
                _ => KeyOutcome::Consumed,
            },
            Stage::Done(_) => match key.code {
                KeyCode::Esc | KeyCode::Enter => KeyOutcome::Back,
                _ => KeyOutcome::Consumed,
            },
            // Waiting on a job; Esc leaves (a sent tx can't be recalled).
            _ => match key.code {
                KeyCode::Esc => KeyOutcome::Back,
                _ => KeyOutcome::Consumed,
            },
        }
    }

    /// Next step after a background job (allowance, estimate, send).
    pub fn apply_job_result(&mut self, result: UiJobResult) {
        let outcome = match (&self.stage, result) {
            (Stage::CheckingAllowance, UiJobResult::DexAllowanceCheck(r)) => r.map(|enough| {
                if enough {
                    self.estimate(Stage::EstimatingShield, Tx::Shield);
                } else {
                    self.estimate(Stage::EstimatingApprove, Tx::Approve);
                }
            }),
            (Stage::EstimatingApprove, UiJobResult::Fee(r)) => {
                r.map(|fee| self.stage = Stage::Approve(fee))
            }
            (Stage::EstimatingShield, UiJobResult::Fee(r)) => {
                r.map(|fee| self.stage = Stage::Shield(fee))
            }
            (Stage::Approving, UiJobResult::Send(r)) => r.map(|receipt| {
                self.status = format!("Approval sent: {}", receipt.hash);
                self.after_approve();
            }),
            // DexSwapEstimateAfterApprove reports the shield estimate as `Fee`.
            (Stage::Approving, UiJobResult::Fee(r)) => r.map(|fee| self.stage = Stage::Shield(fee)),
            (Stage::Shielding, UiJobResult::Send(r)) => r.map(|receipt| {
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
        self.plan = None;
        self.followup = None;
        self.status = status.to_string();
        KeyOutcome::Consumed
    }

    /// Validate the form and read token metadata, balance and the shield fee.
    fn submit_form(&mut self, wallet: &WalletState, handle: &Handle) {
        match prepare(wallet, handle, self.token.value(), self.amount.value()) {
            Ok(p) => {
                self.prepared = Some(p);
                self.status.clear();
                self.stage = Stage::SignKey;
            }
            Err(e) => self.status = e.user_message(),
        }
    }

    /// After the `RAILGUN_SHIELD` approval: sign, derive the key, build and
    /// verify the plan, then check allowance or proceed directly to gas estimation.
    fn sign_and_plan(&mut self, wallet: &WalletState) -> Result<UiJob, WalletError> {
        let p = self
            .prepared
            .as_ref()
            .ok_or_else(|| WalletError::Other("shield form not filled".into()))?;
        let account = wallet.with_hd_seed(RailgunAccount::from_seed)??;
        let signature = Zeroizing::new(wallet.sign_message(SHIELD_MESSAGE.as_bytes())?);
        let key = ShieldKey::from_signature_hex(&signature)?;
        let plan = if p.is_native {
            shield::plan_native(&p.deployment, &account, &key, p.amount, p.fee_bps)?
        } else {
            shield::plan(&p.deployment, &account, &key, p.token, p.amount, p.fee_bps)?
        };
        self.recipient = shield_recipients(&account, &plan.shield_calldata)
            .map(|r| r.card_value())
            .unwrap_or_default();
        if p.is_native {
            let tx = EvmTransaction {
                from: format!("{:#x}", p.owner),
                to: format!("{:#x}", plan.contract),
                value: plan.value.to_string(),
                data: Some(format!("0x{}", hex::encode(&plan.shield_calldata))),
                gas_limit: None,
                gas_price: None,
                max_fee_per_gas: None,
                max_priority_fee_per_gas: None,
                nonce: None,
                chain_id: p.deployment.chain_id,
            };
            self.plan = Some(plan);
            self.stage = Stage::EstimatingShield;
            return Ok(UiJob::EstimateEvmFee { tx });
        }
        let job = UiJob::DexAllowanceCheck {
            rpc_url: p.rpc_url.clone(),
            token_in: format!("{:#x}", p.token),
            owner: format!("{:#x}", p.owner),
            router: format!("{:#x}", plan.contract),
            amount_in: plan.amount.to_string(),
        };
        self.stage = Stage::CheckingAllowance;
        self.plan = Some(plan);
        Ok(job)
    }

    fn tx(&self, which: Tx) -> Option<EvmTransaction> {
        let (p, plan) = (self.prepared.as_ref()?, self.plan.as_ref()?);
        let (to, data, value) = match which {
            Tx::Approve => (p.token, &plan.approve_calldata, "0".into()),
            Tx::Shield => (plan.contract, &plan.shield_calldata, plan.value.to_string()),
        };
        Some(EvmTransaction {
            from: format!("{:#x}", p.owner),
            to: format!("{to:#x}"),
            value,
            data: Some(format!("0x{}", hex::encode(data))),
            gas_limit: None,
            gas_price: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
            nonce: None,
            chain_id: p.deployment.chain_id,
        })
    }

    fn estimate(&mut self, next: Stage, which: Tx) {
        if let Some(tx) = self.tx(which) {
            self.followup = Some(UiJob::EstimateEvmFee { tx });
            self.stage = next;
        }
    }

    fn after_approve(&mut self) {
        let (Some(p), Some(plan), Some(tx)) = (
            self.prepared.as_ref(),
            self.plan.as_ref(),
            self.tx(Tx::Shield),
        ) else {
            return;
        };
        self.followup = Some(UiJob::DexSwapEstimateAfterApprove {
            rpc_url: p.rpc_url.clone(),
            token_in: format!("{:#x}", p.token),
            owner: format!("{:#x}", p.owner),
            router: format!("{:#x}", plan.contract),
            amount_in: plan.amount.to_string(),
            tx,
        });
    }

    fn send(&mut self, next: Stage, which: Tx) -> KeyOutcome {
        let fee = match &self.stage {
            Stage::Approve(fee) | Stage::Shield(fee) => fee.clone(),
            _ => return KeyOutcome::Consumed,
        };
        match self.tx(which) {
            Some(tx) => {
                self.stage = next;
                KeyOutcome::StartJob(UiJob::SendEvmWithFee { tx, fee })
            }
            None => self.cancel("Shield plan missing — start again."),
        }
    }
}

/// Read and validate everything the cards need before any signature.
fn prepare(
    wallet: &WalletState,
    handle: &Handle,
    token: &str,
    amount: &str,
) -> Result<Prepared, WalletError> {
    let net = wallet.networks().active();
    let deployment = deployment::for_chain(net.chain_id)
        .filter(|d| deployment::shield_enabled(d.chain_id))
        .ok_or_else(|| WalletError::Other("Shielding is Sepolia-only for now.".into()))?;
    let owner = parse_address(wallet.active_address()?)?;
    let rpc_url = wallet
        .active_rpc_url_list()?
        .into_iter()
        .next()
        .ok_or_else(|| WalletError::NetworkError("no RPC configured".into()))?;

    let trimmed = token.trim();
    let is_native = trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("eth")
        || trimmed.eq_ignore_ascii_case("native")
        || trimmed == "0x0"
        || trimmed == "0x0000000000000000000000000000000000000000";

    let (token_addr, symbol, decimals, balance) = if is_native {
        let weth = deployment::wrapped_base_token(net.chain_id).ok_or_else(|| {
            WalletError::Other("no wrapped base token configured for this chain".into())
        })?;
        let symbol = net.native_symbol.clone();
        let decimals = net.decimals;
        let bal = handle.block_on(wallet.balance())?;
        let balance: U256 = bal
            .raw
            .parse()
            .map_err(|_| WalletError::Other("native balance".into()))?;
        (weth, symbol, decimals, balance)
    } else {
        let token_addr = parse_address(trimmed)?;
        let token_hex = format!("{token_addr:#x}");
        let (symbol, _, decimals) = handle.block_on(wallet.resolve_erc20_metadata(&token_hex))?;
        let bal = handle.block_on(wallet.token_balance(&token_hex))?;
        let balance: U256 = bal
            .raw
            .parse()
            .map_err(|_| WalletError::Other("token balance".into()))?;
        (token_addr, symbol, decimals, balance)
    };

    let amount: U256 = parse_native_amount(amount.trim(), decimals)?
        .parse()
        .map_err(|_| WalletError::InvalidAmount("amount".into()))?;
    if amount.is_zero() {
        return Err(WalletError::InvalidAmount(
            "amount must be above zero".into(),
        ));
    }
    if balance < amount {
        return Err(WalletError::InvalidAmount(format!(
            "only {} {symbol} in this wallet",
            units(balance, decimals)
        )));
    }
    let provider = vaughan_railgun::scan::connect(&rpc_url)?;
    let fee_bps = handle.block_on(shield::shield_fee_bps(&provider, &deployment))?;
    Ok(Prepared {
        deployment,
        rpc_url,
        owner,
        token: token_addr,
        symbol,
        decimals,
        amount,
        fee_bps,
        is_native,
    })
}

fn units(value: U256, decimals: u8) -> String {
    format_base_units(&value.to_string(), decimals)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallet() -> WalletState {
        WalletState::load(tempfile::tempdir().unwrap().path().join("w.json")).unwrap()
    }

    fn text(flow: &ShieldFlow) -> String {
        flow.lines(&wallet())
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn form_prefills_native_eth() {
        let flow = ShieldFlow::new();
        assert!(text(&flow).contains("ETH"));
        assert!(matches!(flow.stage, Stage::Form));
    }

    #[test]
    fn job_error_returns_to_form_with_message() {
        let mut flow = ShieldFlow::new();
        flow.stage = Stage::CheckingAllowance;
        flow.apply_job_result(UiJobResult::DexAllowanceCheck(Err(
            WalletError::NetworkError("boom".into()),
        )));
        assert!(matches!(flow.stage, Stage::Form));
        assert!(flow.plan.is_none());
        assert!(!flow.status.is_empty());
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

    fn enter() -> KeyEvent {
        KeyEvent::from(KeyCode::Enter)
    }

    fn sends_to(outcome: KeyOutcome, to: Address) -> bool {
        matches!(outcome, KeyOutcome::StartJob(UiJob::SendEvmWithFee { tx, .. })
            if tx.to == format!("{to:#x}"))
    }

    /// Offline walk through every card: key signature, exact approve, shield.
    #[test]
    fn walks_sign_approve_shield_cards() {
        let mut w = unlocked_wallet();
        let handle = tokio::runtime::Runtime::new().unwrap().handle().clone();
        let owner: Address = w.active_address().unwrap().parse().unwrap();
        let token: Address = deployment::SEPOLIA_WETH;
        let dep = deployment::for_chain(deployment::SEPOLIA_CHAIN_ID).unwrap();
        let mut flow = ShieldFlow::new();
        flow.prepared = Some(Prepared {
            deployment: dep,
            rpc_url: "http://127.0.0.1:1".into(),
            owner,
            token,
            symbol: "WETH".into(),
            decimals: 18,
            amount: U256::from(10u64).pow(U256::from(16u64)),
            fee_bps: 25,
            is_native: false,
        });
        flow.stage = Stage::SignKey;
        assert!(text(&flow).contains("RAILGUN_SHIELD"));

        let outcome = flow.handle_key(enter(), &mut w, &handle);
        assert!(matches!(
            outcome,
            KeyOutcome::StartJob(UiJob::DexAllowanceCheck { .. })
        ));
        assert_eq!(flow.recipient, "your RAILGUN wallet (verified)");

        flow.apply_job_result(UiJobResult::DexAllowanceCheck(Ok(false)));
        assert!(matches!(
            flow.take_followup(),
            Some(UiJob::EstimateEvmFee { tx }) if tx.to == format!("{token:#x}")
        ));
        flow.apply_job_result(UiJobResult::Fee(Ok(fee())));
        let card = flow
            .lines(&w)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(card.contains("exact amount, not unlimited"), "{card}");
        assert!(sends_to(flow.handle_key(enter(), &mut w, &handle), token));

        flow.apply_job_result(sent("0xapprove"));
        assert!(matches!(
            flow.take_followup(),
            Some(UiJob::DexSwapEstimateAfterApprove { .. })
        ));
        flow.apply_job_result(UiJobResult::Fee(Ok(fee())));
        let card = flow
            .lines(&w)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            card.contains("Protocol fee: 0.000025 WETH (0.25%"),
            "{card}"
        );
        assert!(card.contains("You receive:  0.009975 WETH"), "{card}");
        assert!(
            card.contains("Recipient:    your RAILGUN wallet (verified)"),
            "{card}"
        );
        assert!(sends_to(
            flow.handle_key(enter(), &mut w, &handle),
            dep.contract
        ));

        flow.apply_job_result(sent("0xshield"));
        assert!(matches!(flow.stage, Stage::Done(ref h) if h == "0xshield"));
    }

    /// Walk through native ETH wrap-and-shield via RelayAdapt: skips approve card.
    #[test]
    fn walks_native_eth_shield_cards() {
        let mut w = unlocked_wallet();
        let handle = tokio::runtime::Runtime::new().unwrap().handle().clone();
        let owner: Address = w.active_address().unwrap().parse().unwrap();
        let token: Address = deployment::SEPOLIA_WETH;
        let dep = deployment::for_chain(deployment::SEPOLIA_CHAIN_ID).unwrap();
        let mut flow = ShieldFlow::new();
        let amount = U256::from(10u64).pow(U256::from(16u64));
        flow.prepared = Some(Prepared {
            deployment: dep,
            rpc_url: "http://127.0.0.1:1".into(),
            owner,
            token,
            symbol: "ETH".into(),
            decimals: 18,
            amount,
            fee_bps: 25,
            is_native: true,
        });
        flow.stage = Stage::SignKey;
        assert!(text(&flow).contains("RAILGUN_SHIELD"));

        // Confirming sign-key card directly schedules gas estimation for the shield tx
        let outcome = flow.handle_key(enter(), &mut w, &handle);
        assert!(matches!(
            &outcome,
            KeyOutcome::StartJob(UiJob::EstimateEvmFee { tx })
                if tx.to == format!("{:#x}", dep.relay_adapt_contract.expect("relay adapt")) && tx.value == amount.to_string()
        ));
        assert_eq!(flow.recipient, "your RAILGUN wallet (verified)");

        // Receive fee estimate -> shield card shown
        flow.apply_job_result(UiJobResult::Fee(Ok(fee())));
        let card = flow
            .lines(&w)
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(card.contains("ETH (native → wraps to WETH)"), "{card}");
        assert!(card.contains("Protocol fee: 0.000025 ETH (0.25%"), "{card}");
        assert!(
            card.contains("You receive:  0.009975 WETH (private)"),
            "{card}"
        );
        assert!(card.contains("RAILGUN RelayAdapt"), "{card}");

        // Hitting enter signs and sends to RelayAdapt with value = amount
        assert!(sends_to(
            flow.handle_key(enter(), &mut w, &handle),
            dep.relay_adapt_contract.expect("relay adapt")
        ));
        flow.apply_job_result(sent("0xnative_shield"));
        assert!(matches!(flow.stage, Stage::Done(_)));
    }

    #[test]
    fn esc_on_a_card_signs_nothing() {
        let mut w = wallet();
        let handle = tokio::runtime::Runtime::new().unwrap().handle().clone();
        let mut flow = ShieldFlow::new();
        flow.stage = Stage::Shield(fee());
        let outcome = flow.handle_key(KeyEvent::from(KeyCode::Esc), &mut w, &handle);
        assert!(matches!(outcome, KeyOutcome::Consumed));
        assert!(matches!(flow.stage, Stage::Form));
        assert!(flow.status.contains("wasn't sent"));
    }

    #[test]
    fn results_for_other_stages_are_ignored() {
        let mut flow = ShieldFlow::new();
        flow.apply_job_result(UiJobResult::DexAllowanceCheck(Ok(true)));
        assert!(matches!(flow.stage, Stage::Form));
        assert!(flow.take_followup().is_none());
    }
}
