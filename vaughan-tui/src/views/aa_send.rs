//! AA batched send: compose multiple native transfers and submit them as one
//! EIP-7702 smart-account batch.
//!
//! Unlike the single-send view, this goes through `vaughan_aa`: the account
//! EOA is delegated to Ambire's `AmbireAccount` implementation (bootstrapped
//! automatically on the first batch if it isn't already), the transfers are
//! signed as one `execute(txns, signature)` batch, and the account self-pays
//! gas. See `docs/ambire-aa.md` for the 7702 self-pay decision.

use alloy::primitives::U256;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use tokio::runtime::Handle;
use vaughan_aa::abi::Transaction;
use vaughan_aa::adapter::{
    estimate_self_pay_fee, get_account_nonce, is_delegated, submit_batch, AMBIRE_IMPLEMENTATION,
};
use vaughan_aa::scw::{ScwTransaction, SignatureMode};
use vaughan_aa::sign::sign_scw_transaction;
use vaughan_core::chains::evm::utils::{parse_address, parse_u256};
use vaughan_core::chains::evm::{EvmAdapter, EvmNetworkConfig};
use vaughan_core::core::{parse_native_amount, WalletState};
use vaughan_core::error::WalletError;
use vaughan_provider::EventBus;

use crate::app::{KeyOutcome, Screen};
use crate::brand;
use crate::input::{Input, InputAction};
use crate::views::{body_areas, render_labeled_input, status_paragraph};

enum Stage {
    Edit,
    Confirm,
    Done,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Focus {
    Recipient,
    Amount,
}

struct Row {
    recipient: Input,
    amount: Input,
}

impl Row {
    fn new() -> Self {
        Self {
            recipient: Input::new(false, "0x..."),
            amount: Input::new(false, "0.0"),
        }
    }
}

/// A validated batch + its display facts, computed before the confirm screen.
struct Prepared {
    will_bootstrap: bool,
    fee_text: String,
}

pub struct AaSendView {
    stage: Stage,
    rows: Vec<Row>,
    cursor: usize,
    focus: Focus,
    will_bootstrap: bool,
    fee: Option<String>,
    tx_hash: Option<String>,
    bootstrapped: bool,
    status: String,
}

impl Default for AaSendView {
    fn default() -> Self {
        Self {
            stage: Stage::Edit,
            rows: vec![Row::new()],
            cursor: 0,
            focus: Focus::Recipient,
            will_bootstrap: false,
            fee: None,
            tx_hash: None,
            bootstrapped: false,
            status: String::new(),
        }
    }
}

impl AaSendView {
    fn active_row_mut(&mut self) -> &mut Row {
        &mut self.rows[self.cursor]
    }

    /// Parse every row into a batch of transfers. Fully-empty rows are
    /// skipped; partially-filled rows are an error naming the row.
    fn parse_rows(&self, net: &EvmNetworkConfig) -> Result<Vec<Transaction>, WalletError> {
        let mut txns = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            let recipient = row.recipient.value().trim();
            let amount = row.amount.value().trim();
            if recipient.is_empty() && amount.is_empty() {
                continue;
            }
            if recipient.is_empty() {
                return Err(WalletError::InvalidTransaction(format!(
                    "row {}: recipient is empty",
                    i + 1
                )));
            }
            if amount.is_empty() {
                return Err(WalletError::InvalidTransaction(format!(
                    "row {}: amount is empty",
                    i + 1
                )));
            }
            let to = parse_address(recipient)?;
            let wei = parse_native_amount(amount, net.decimals)?;
            txns.push(Transaction {
                to,
                value: parse_u256(&wei)?,
                data: Default::default(),
            });
        }
        Ok(txns)
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, wallet: &WalletState) {
        let [content, status_area] = body_areas(area);
        let net = wallet.networks().active();
        let testnet = if net.is_testnet { " (testnet)" } else { "" };

        match self.stage {
            Stage::Edit => {
                let hint =
                    "ctrl+a add   ctrl+d delete   ↑↓ row   tab field   enter next   esc back";
                let [hint_area, rows_area] =
                    Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(content);
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        hint,
                        Style::default().fg(Color::DarkGray),
                    ))),
                    hint_area,
                );

                let row_areas: Vec<Rect> = Layout::vertical(
                    std::iter::repeat_n(Constraint::Length(3), self.rows.len()).collect::<Vec<_>>(),
                )
                .split(rows_area)
                .iter()
                .copied()
                .collect();
                for (i, area) in row_areas.into_iter().enumerate() {
                    let row = &self.rows[i];
                    let selected = i == self.cursor;
                    // Row number gutter + recipient/amount inputs.
                    let [num, rest] =
                        Layout::horizontal([Constraint::Length(4), Constraint::Min(0)]).areas(area);
                    let num_title = if selected {
                        brand::focus_title(&format!(" {} ", i + 1))
                    } else {
                        brand::fade_line(&format!(" {} ", i + 1))
                    };
                    let num_inner = brand::render_faded_box(frame, num, Some(num_title));
                    frame
                        .render_widget(Paragraph::new(Line::from(format!("{}", i + 1))), num_inner);
                    let [rec, amt] = Layout::horizontal([
                        Constraint::Percentage(60),
                        Constraint::Percentage(40),
                    ])
                    .areas(rest);
                    render_labeled_input(
                        frame,
                        rec,
                        "Recipient",
                        &row.recipient,
                        selected && self.focus == Focus::Recipient,
                    );
                    let amount_label = format!("Amount ({})", net.native_symbol);
                    render_labeled_input(
                        frame,
                        amt,
                        &amount_label,
                        &row.amount,
                        selected && self.focus == Focus::Amount,
                    );
                }
            }
            Stage::Confirm => {
                let mut text = vec![Line::from(format!(
                    "Send batch of {} transfers:",
                    self.rows.len()
                ))];
                for (i, row) in self.rows.iter().enumerate() {
                    text.push(Line::from(format!(
                        "  {}. {} {} {}",
                        i + 1,
                        row.recipient.value(),
                        row.amount.value(),
                        net.native_symbol
                    )));
                }
                text.push(Line::from(""));
                text.push(Line::from(format!("Network: {}{testnet}", net.name)));
                if net.is_testnet {
                    text.push(Line::from(Span::styled(
                        "Testnet-first: exercise AA batches here before mainnet.",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    text.push(Line::from(Span::styled(
                        "Mainnet — confirm chain and batch before approving.",
                        Style::default().fg(Color::Yellow),
                    )));
                }
                if let Some(fee) = &self.fee {
                    text.push(Line::from(format!("Fee:      {fee}")));
                }
                if self.will_bootstrap {
                    text.push(Line::from(Span::styled(
                        "First batch: EIP-7702 delegates this EOA to AmbireAccount (one-time; permanent per EIP-7702)",
                        Style::default().fg(Color::Yellow),
                    )));
                }
                text.push(Line::from(""));
                text.push(Line::from("Enter — broadcast   Esc — cancel"));
                let inner = brand::render_faded_box(frame, content, None);
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
            Stage::Done => {
                let hash = self.tx_hash.as_deref().unwrap_or("");
                let mut text = vec![
                    Line::from("Transaction broadcast"),
                    Line::from(""),
                    Line::from(Span::styled(hash, Style::default().fg(Color::Green))),
                    Line::from(""),
                ];
                if self.bootstrapped {
                    text.push(Line::from(Span::styled(
                        "Account delegated to the smart account (one-time bootstrap)",
                        Style::default().fg(Color::Yellow),
                    )));
                    text.push(Line::from(""));
                }
                text.push(Line::from("Enter — back to dashboard"));
                let inner = brand::render_faded_box(frame, content, None);
                frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
            }
        }

        frame.render_widget(status_paragraph(&self.status), status_area);
    }

    pub fn handle_key(
        &mut self,
        key: KeyEvent,
        wallet: &WalletState,
        handle: &Handle,
        _events: &EventBus,
    ) -> KeyOutcome {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match self.stage {
            Stage::Edit => {
                // Row-level commands take priority over the focused input.
                match key.code {
                    KeyCode::Char('a') if ctrl => {
                        self.rows.push(Row::new());
                        self.cursor = self.rows.len() - 1;
                        self.focus = Focus::Recipient;
                        return KeyOutcome::Consumed;
                    }
                    KeyCode::Char('d') if ctrl => {
                        if self.rows.len() > 1 {
                            self.rows.remove(self.cursor);
                            if self.cursor >= self.rows.len() {
                                self.cursor = self.rows.len() - 1;
                            }
                        }
                        return KeyOutcome::Consumed;
                    }
                    KeyCode::Up => {
                        if self.cursor > 0 {
                            self.cursor -= 1;
                        }
                        return KeyOutcome::Consumed;
                    }
                    KeyCode::Down => {
                        if self.cursor + 1 < self.rows.len() {
                            self.cursor += 1;
                        }
                        return KeyOutcome::Consumed;
                    }
                    KeyCode::Tab => {
                        self.focus = match self.focus {
                            Focus::Recipient => Focus::Amount,
                            Focus::Amount => Focus::Recipient,
                        };
                        return KeyOutcome::Consumed;
                    }
                    _ => {}
                }

                match self.focus {
                    Focus::Recipient => {
                        if key.code == KeyCode::Esc {
                            return KeyOutcome::Navigate(Screen::Dashboard);
                        }
                        match self.active_row_mut().recipient.handle_key(key) {
                            InputAction::Ignored => KeyOutcome::NotHandled,
                            InputAction::Submitted => {
                                self.focus = Focus::Amount;
                                KeyOutcome::Consumed
                            }
                            InputAction::Consumed => KeyOutcome::Consumed,
                        }
                    }
                    Focus::Amount => {
                        if key.code == KeyCode::Esc {
                            self.focus = Focus::Recipient;
                            return KeyOutcome::Consumed;
                        }
                        match self.active_row_mut().amount.handle_key(key) {
                            InputAction::Ignored => KeyOutcome::NotHandled,
                            InputAction::Submitted => {
                                if self.cursor + 1 < self.rows.len() {
                                    self.cursor += 1;
                                    self.focus = Focus::Recipient;
                                } else {
                                    self.estimate(wallet, handle);
                                }
                                KeyOutcome::Consumed
                            }
                            InputAction::Consumed => KeyOutcome::Consumed,
                        }
                    }
                }
            }
            Stage::Confirm => match key.code {
                KeyCode::Esc => {
                    self.stage = Stage::Edit;
                    KeyOutcome::Consumed
                }
                KeyCode::Enter => {
                    self.submit(wallet, handle);
                    KeyOutcome::Consumed
                }
                _ => KeyOutcome::NotHandled,
            },
            Stage::Done => match key.code {
                KeyCode::Enter | KeyCode::Esc => KeyOutcome::Navigate(Screen::Dashboard),
                _ => KeyOutcome::NotHandled,
            },
        }
    }

    /// Validate the rows and compute the fee + bootstrap note for the confirm
    /// screen (offline signing + RPC reads only — nothing is broadcast).
    fn estimate(&mut self, wallet: &WalletState, handle: &Handle) {
        let net = wallet.networks().active();
        let txns = match self.parse_rows(net) {
            Ok(txns) => txns,
            Err(e) => {
                self.status = e.user_message();
                return;
            }
        };
        if txns.is_empty() {
            self.status = "add at least one transfer".to_string();
            return;
        }
        match handle.block_on(prepare_batch(wallet, txns)) {
            Ok(prepared) => {
                self.will_bootstrap = prepared.will_bootstrap;
                self.fee = Some(prepared.fee_text);
                self.status.clear();
                self.stage = Stage::Confirm;
            }
            Err(e) => self.status = e.user_message(),
        }
    }

    /// Broadcast the batch through the 7702 self-pay path (bootstrapping the
    /// delegation first if needed).
    fn submit(&mut self, wallet: &WalletState, handle: &Handle) {
        let net = wallet.networks().active();
        let txns = match self.parse_rows(net) {
            Ok(txns) => txns,
            Err(e) => {
                self.status = e.user_message();
                return;
            }
        };
        let net = wallet.networks().active();
        let result = handle.block_on(async {
            let signer = wallet.active_signer()?;
            let adapter = EvmAdapter::new(
                &wallet.active_rpc_url(),
                net.chain_id,
                &net.name,
                &net.fallback_rpc_urls,
            )
            .await?;
            submit_batch(&adapter, &signer, txns, AMBIRE_IMPLEMENTATION).await
        });
        match result {
            Ok(result) => {
                self.tx_hash = Some(result.tx_hash.to_string());
                self.bootstrapped = result.bootstrapped;
                self.status.clear();
                self.stage = Stage::Done;
            }
            Err(e) => {
                self.status = e.user_message();
                self.stage = Stage::Edit;
            }
        }
    }
}

/// Read-only preparation for the confirm screen: delegation status, account
/// nonce, signed-batch fee estimate. Nothing is broadcast.
async fn prepare_batch(
    wallet: &WalletState,
    txns: Vec<Transaction>,
) -> Result<Prepared, WalletError> {
    let signer = wallet.active_signer()?;
    let net = wallet.networks().active();
    let adapter = EvmAdapter::new(
        &wallet.active_rpc_url(),
        net.chain_id,
        &net.name,
        &net.fallback_rpc_urls,
    )
    .await?;
    let account = signer.address();
    let delegated = is_delegated(&adapter, account).await?;
    // An undelegated EOA has no code, so `nonce()` can't be read yet — the
    // contract nonce starts at 0.
    let nonce = if delegated {
        get_account_nonce(&adapter, account).await?
    } else {
        0
    };
    let batch = ScwTransaction {
        account,
        chain_id: net.chain_id,
        nonce,
        txns,
    };
    let signature = sign_scw_transaction(&signer, &batch, SignatureMode::RawHash)?;
    let (gas_limit, max_fee, _priority) =
        estimate_self_pay_fee(&adapter, &batch, &signature, None).await?;
    let fee_wei = U256::from(gas_limit) * U256::from(max_fee);
    let fee_text = alloy::primitives::utils::format_units(fee_wei, net.decimals)
        .map(|s| format!("~{s} {}", net.native_symbol))
        .unwrap_or_else(|_| format!("{fee_wei} wei"));
    Ok(Prepared {
        will_bootstrap: !delegated,
        fee_text,
    })
}
