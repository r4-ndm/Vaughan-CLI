//! Anvil tests for approval-gate hardening: transaction humanizer, phishing
//! deny-list, and the stealth-sweep privacy warning.
//!
//! Each test builds a real approval card via `describe_approval_preview`
//! against a funded wallet on a local Anvil node (fee estimation hits the
//! chain), so the checks cover the same code path the TUI renders.
//!
//! Requires `anvil` + `cast` on PATH. Run with:
//! ```sh
//! cargo test -p vaughan-tui --test approval_hardening_anvil -- --nocapture
//! ```

mod common;

use alloy::primitives::{address, Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;
use common::{anvil_dev_address, funded_wallet, plant_announcer, Anvil};
use serde_json::json;
use tokio::runtime::Handle;
use vaughan_core::core::proposal::{ProposalType, TxProposal};
use vaughan_core::core::WalletState;
use vaughan_provider::TxParams;
use vaughan_tui::provider::{describe_approval_preview, ApprovalKind};

sol! {
    interface ITestErc20 {
        function approve(address spender, uint256 amount) external returns (bool);
    }
    interface ITestV2Router {
        function swapExactTokensForTokens(uint256 amountIn, uint256 amountOutMin, address[] path, address to, uint256 deadline) external returns (uint256[] amounts);
    }
}

/// An address that is in no Vaughan catalog and has no code on Anvil.
const UNKNOWN: Address = address!("0x00000000000000000000000000000000deadbeef");
/// A second uncatalogued address used as a "token" contract target.
const TOKEN: Address = address!("0x000000000000000000000000000000000000c0de");

fn runtime_handle() -> (tokio::runtime::Runtime, Handle) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let handle = rt.handle().clone();
    (rt, handle)
}

/// Build `TxParams` the way a dApp request arrives (JSON, decimal quantities).
fn tx(to: Address, value_wei: &str, data: Option<Vec<u8>>) -> TxParams {
    let mut v = json!({ "to": format!("{to:#x}"), "value": value_wei });
    if let Some(d) = data {
        v["data"] = json!(format!("0x{}", hex::encode(d)));
    }
    serde_json::from_value(v).expect("valid TxParams")
}

/// Render the approval card details as one string for assertions.
fn card(kind: &ApprovalKind, wallet: &WalletState, handle: &Handle) -> String {
    let preview = describe_approval_preview(kind, wallet, handle).expect("preview");
    preview.details.join("\n")
}

/// A dApp asking for an unlimited approval to an unknown spender gets a
/// plain-English summary plus both warnings on the card.
#[test]
fn dapp_unlimited_approve_to_unknown_spender_is_humanized() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();

    let data = ITestErc20::approveCall {
        spender: UNKNOWN,
        amount: U256::MAX,
    }
    .abi_encode();
    let text = card(
        &ApprovalKind::SendTransaction(tx(TOKEN, "0", Some(data))),
        &wallet,
        &handle,
    );

    assert!(text.contains("Action:  Approve UNLIMITED"), "{text}");
    assert!(text.contains("Unlimited approval"), "{text}");
    assert!(text.contains("not a known catalogued contract"), "{text}");
}

/// A swap whose proceeds go to someone other than the signer is flagged.
#[test]
fn dapp_swap_to_other_recipient_warns() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();

    let thief: Address = anvil_dev_address(1).parse().unwrap();
    let data = ITestV2Router::swapExactTokensForTokensCall {
        amountIn: U256::from(1_000u64),
        amountOutMin: U256::from(1u64),
        path: vec![TOKEN, UNKNOWN],
        to: thief,
        deadline: U256::from(4_000_000_000u64),
    }
    .abi_encode();
    let router = address!("0x0000000000000000000000000000000000000abc");
    let text = card(
        &ApprovalKind::SendTransaction(tx(router, "0", Some(data))),
        &wallet,
        &handle,
    );

    assert!(text.contains("Action:  Swap"), "{text}");
    assert!(text.contains("not your wallet"), "{text}");
}

/// A swap back to the signer does not raise the recipient warning.
#[test]
fn dapp_swap_to_self_has_no_recipient_warning() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();

    let me: Address = wallet.active_address().unwrap().parse().unwrap();
    let data = ITestV2Router::swapExactTokensForTokensCall {
        amountIn: U256::from(1_000u64),
        amountOutMin: U256::from(1u64),
        path: vec![TOKEN, UNKNOWN],
        to: me,
        deadline: U256::from(4_000_000_000u64),
    }
    .abi_encode();
    let router = address!("0x0000000000000000000000000000000000000abc");
    let text = card(
        &ApprovalKind::SendTransaction(tx(router, "0", Some(data))),
        &wallet,
        &handle,
    );

    assert!(text.contains("Action:  Swap"), "{text}");
    assert!(!text.contains("not your wallet"), "{text}");
}

/// A user deny-list entry flags a plain send, survives a reload from disk,
/// and clears when removed.
#[test]
fn user_denied_address_flags_send_and_persists() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let kind = ApprovalKind::SendTransaction(tx(UNKNOWN, "1000", None));

    assert!(!card(&kind, &wallet, &handle).contains("DENIED"));

    wallet
        .add_denied_address(UNKNOWN, "reported drainer")
        .unwrap();
    let text = card(&kind, &wallet, &handle);
    assert!(text.contains("DENIED"), "{text}");

    // Persisted: a fresh load of the same vault file still denies it.
    let reloaded = WalletState::load(dir.path().join("wallet.json")).unwrap();
    assert!(reloaded.is_denied_address(UNKNOWN));
    assert_eq!(reloaded.denylist_entries()[0].reason, "reported drainer");

    wallet.remove_denied_address(UNKNOWN).unwrap();
    assert!(!card(&kind, &wallet, &handle).contains("DENIED"));
}

/// Adding the same address twice keeps a single entry.
#[test]
fn add_denied_address_is_idempotent() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);

    wallet.add_denied_address(UNKNOWN, "a").unwrap();
    wallet.add_denied_address(UNKNOWN, "b").unwrap();
    assert_eq!(wallet.denylist_entries().len(), 1);
}

/// An EIP-712 request whose verifyingContract is denied is flagged.
#[test]
fn typed_data_with_denied_verifying_contract_is_flagged() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let mut wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    wallet
        .add_denied_address(UNKNOWN, "permit drainer")
        .unwrap();

    let typed_data = json!({
        "types": {
            "EIP712Domain": [
                {"name": "name", "type": "string"},
                {"name": "chainId", "type": "uint256"},
                {"name": "verifyingContract", "type": "address"}
            ],
            "Permit": [
                {"name": "spender", "type": "address"},
                {"name": "value", "type": "uint256"}
            ]
        },
        "primaryType": "Permit",
        "domain": {
            "name": "Drainer",
            "chainId": 943,
            "verifyingContract": format!("{UNKNOWN:#x}")
        },
        "message": { "spender": format!("{UNKNOWN:#x}"), "value": "1" }
    });
    let kind = ApprovalKind::SignTypedData {
        address: wallet.active_address().unwrap().to_string(),
        typed_data,
    };
    let text = card(&kind, &wallet, &handle);
    assert!(text.contains("DENIED"), "{text}");
}

/// An MCP proposal for an unlimited approval to an unknown spender shows the
/// humanizer's warning in the card's safety lines.
#[test]
fn mcp_proposal_shows_humanizer_warnings() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();

    let data = ITestErc20::approveCall {
        spender: UNKNOWN,
        amount: U256::MAX,
    }
    .abi_encode();
    let proposal = TxProposal::new(
        "hardening-1",
        ProposalType::ContractCall {
            target: TOKEN,
            function_name: Some("approve".into()),
        },
        TOKEN,
        U256::ZERO,
        data.into(),
        60_000,
        true,
        "agent: totally safe approval",
    )
    .with_chain(943, Some("pulsechain-testnet-v4".into()));
    let kind = ApprovalKind::McpProposal {
        proposal_id: proposal.proposal_id.clone(),
        source: "test".into(),
        proposal: Box::new(proposal),
    };
    let text = card(&kind, &wallet, &handle);

    assert!(text.contains("Safety:"), "{text}");
    assert!(text.contains("not a known catalogued contract"), "{text}");
}

/// Full stealth flow: fund a note to ourselves, scan it, check the sweep card
/// names the active wallet, sweep, and confirm the funds really land at the
/// address the warning named.
#[test]
fn stealth_sweep_warning_names_the_real_destination() {
    let anvil = Anvil::start();
    plant_announcer(&anvil);
    let dir = tempfile::tempdir().unwrap();
    let wallet = funded_wallet(dir.path(), &anvil);
    let (_rt, handle) = runtime_handle();
    let me = wallet.active_address().unwrap().to_string();

    let uri = wallet.stealth_uri().unwrap();
    let announcement = wallet.prepare_stealth_payment(&uri).unwrap();
    handle
        .block_on(wallet.send_stealth(&announcement, "1000000000000000000"))
        .expect("send stealth");
    let notes = handle.block_on(wallet.scan_stealth_notes()).expect("scan");
    assert_eq!(notes.len(), 1, "expected one funded note");
    let note = &notes[0];

    let kind = ApprovalKind::StealthSweep {
        stealth_address: format!("{:#x}", note.announcement.stealth_address),
        balance_display: note.balance_formatted.clone(),
    };
    let text = card(&kind, &wallet, &handle);
    assert!(text.contains("Privacy:"), "{text}");
    assert!(
        text.to_lowercase().contains(&me.to_lowercase()),
        "warning must name the active wallet:\n{text}"
    );

    let before = anvil.wei_balance(&me);
    handle
        .block_on(wallet.sweep_stealth_note(note))
        .expect("sweep");
    let after = anvil.wei_balance(&me);
    assert!(after > before, "sweep must credit the named wallet");
}
