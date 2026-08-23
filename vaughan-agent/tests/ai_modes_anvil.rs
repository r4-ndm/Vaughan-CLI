//! Extra Anvil coverage for Assist + Degen AI modes.
//!
//! Uses a scripted LLM for chat turns (deterministic) and real Anvil RPCs for
//! tools, proposals, quorum, and autonomous execution.

use std::process::{Child, Command};
use std::sync::Arc;
use std::time::Duration;

use alloy::eips::eip2718::Encodable2718;
use alloy::network::{Ethereum, EthereumWallet, NetworkTransactionBuilder};
use alloy::primitives::{address, Address, Bytes, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::eth::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use async_trait::async_trait;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use url::Url;

use vaughan_agent::client::{LlmClient, StreamEvent};
use vaughan_agent::degen::{CircuitBreakerConfig, DegenTrader, QuorumValidator};
use vaughan_agent::error::AgentError;
use vaughan_agent::proposal::TxProposal;
use vaughan_agent::tools::{default_assist_registry, default_sensory_registry, ToolContext};
use vaughan_agent::types::{ChatMessage, ToolCall, ToolDefinition};
use vaughan_agent::{assist_system_prompt, run_assist_turn, ChatUiEvent};

struct AnvilGuard {
    child: Child,
    rpc_url: String,
}

impl AnvilGuard {
    fn spawn(port: u16) -> Self {
        let child = Command::new("anvil")
            .args(["-p", &port.to_string(), "--silent"])
            .spawn()
            .expect("Failed to start Anvil.");
        std::thread::sleep(Duration::from_millis(400));
        Self {
            child,
            rpc_url: format!("http://127.0.0.1:{port}"),
        }
    }
}

impl Drop for AnvilGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

struct ScriptedClient {
    replies: std::sync::Mutex<Vec<ChatMessage>>,
}

impl ScriptedClient {
    fn new(replies: Vec<ChatMessage>) -> Self {
        Self {
            replies: std::sync::Mutex::new(replies),
        }
    }
}

#[async_trait]
impl LlmClient for ScriptedClient {
    fn name(&self) -> &str {
        "scripted"
    }

    async fn complete(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ToolDefinition],
    ) -> Result<ChatMessage, AgentError> {
        let mut replies = self.replies.lock().unwrap();
        replies
            .pop()
            .ok_or_else(|| AgentError::ProviderError("no scripted replies left".into()))
    }

    async fn stream(
        &self,
        _messages: &[ChatMessage],
        _tools: &[ToolDefinition],
        event_tx: mpsc::Sender<StreamEvent>,
        cancel: watch::Receiver<bool>,
    ) -> Result<ChatMessage, AgentError> {
        if *cancel.borrow() {
            return Err(AgentError::ExecutionAborted);
        }
        let message = {
            let mut replies = self.replies.lock().unwrap();
            if replies.is_empty() {
                return Err(AgentError::ProviderError("no scripted replies left".into()));
            }
            replies.remove(0)
        };
        if !message.content.is_empty() {
            let _ = event_tx
                .send(StreamEvent::Delta(message.content.clone()))
                .await;
        }
        Ok(message)
    }
}

fn provider(rpc: &str) -> RootProvider<Ethereum> {
    RootProvider::new_http(Url::parse(rpc).unwrap())
}

fn anvil_account0() -> PrivateKeySigner {
    "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
        .parse()
        .unwrap()
}

fn anvil_account1() -> PrivateKeySigner {
    "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
        .parse()
        .unwrap()
}

fn default_breaker() -> CircuitBreakerConfig {
    CircuitBreakerConfig {
        max_position_pct: 50,
        max_slippage_bps: 100,
        max_session_gas_wei: U256::from(10_000_000_000_000_000u64),
        max_consecutive_errors: 3,
        required_rpc_quorum: 1,
        ..Default::default()
    }
}

fn abi_encode_address(addr: Address) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[12..32].copy_from_slice(addr.as_slice());
    out
}

fn abi_encode_u256(v: U256) -> Vec<u8> {
    v.to_be_bytes::<32>().to_vec()
}

fn assemble_dispatcher(routes: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    let mut bytecode = vec![0x60, 0x00, 0x35, 0x60, 0xe0, 0x1c];
    let dispatch_size = bytecode.len() + routes.len() * 11 + 5;
    let mut handlers = Vec::new();
    let mut current_offset = dispatch_size;

    for (_sel, ret_data) in routes {
        let handler_target = current_offset as u16;
        handlers.push((handler_target, ret_data.clone()));
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        current_offset += 1 + chunks * 36 + 6;
    }

    for (i, (sel, _)) in routes.iter().enumerate() {
        let (target, _) = handlers[i];
        bytecode.push(0x80);
        bytecode.push(0x63);
        bytecode.extend_from_slice(sel);
        bytecode.push(0x14);
        bytecode.push(0x61);
        bytecode.push((target >> 8) as u8);
        bytecode.push((target & 0xff) as u8);
        bytecode.push(0x57);
    }

    bytecode.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xfd]);

    for (target, ret_data) in handlers {
        assert_eq!(bytecode.len(), target as usize);
        bytecode.push(0x5b);
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        for c in 0..chunks {
            let start = c * 32;
            let end = (start + 32).min(ret_data.len());
            let mut chunk_bytes = [0u8; 32];
            chunk_bytes[..end - start].copy_from_slice(&ret_data[start..end]);
            bytecode.push(0x7f);
            bytecode.extend_from_slice(&chunk_bytes);
            bytecode.push(0x60);
            bytecode.push((c * 32) as u8);
            bytecode.push(0x52);
        }
        let len = ret_data.len() as u16;
        bytecode.push(0x61);
        bytecode.push((len >> 8) as u8);
        bytecode.push((len & 0xff) as u8);
        bytecode.push(0x60);
        bytecode.push(0x00);
        bytecode.push(0xf3);
    }

    bytecode
}

async fn set_code(rpc: &str, addr: Address, code: &[u8]) {
    let p = provider(rpc);
    let _: () = p
        .raw_request(
            "anvil_setCode".into(),
            (addr, format!("0x{}", hex::encode(code))),
        )
        .await
        .unwrap();
}

fn reserves_abi(r0: U256, r1: U256, ts: u64) -> Vec<u8> {
    let mut out = vec![0u8; 96];
    out[..32].copy_from_slice(&r0.to_be_bytes::<32>());
    out[32..64].copy_from_slice(&r1.to_be_bytes::<32>());
    out[64..96].copy_from_slice(&U256::from(ts).to_be_bytes::<32>());
    out
}

// --- Assist: chat turn + Anvil tools -------------------------------------------------

#[tokio::test]
async fn assist_turn_sense_then_propose_emits_proposal_on_anvil() {
    let anvil = AnvilGuard::spawn(8600);
    let signer = anvil_account0();
    let recipient = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");

    let client = Arc::new(ScriptedClient::new(vec![
        ChatMessage::assistant_with_tools(
            "",
            vec![
                ToolCall {
                    id: "1".into(),
                    name: "get_balance".into(),
                    arguments: json!({
                        "account_address": format!("{:#x}", signer.address())
                    }),
                    thought_signature: None,
                },
                ToolCall {
                    id: "2".into(),
                    name: "propose_transfer".into(),
                    arguments: json!({
                        "recipient": format!("{recipient:#x}"),
                        "amount": "1000000000000000000",
                        "explanation": "Move 1 ETH after checking balance"
                    }),
                    thought_signature: None,
                },
            ],
        ),
        ChatMessage::assistant("Proposal ready for your approval."),
    ]));

    let registry = default_assist_registry();
    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(signer.address()),
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut history = vec![assist_system_prompt()];

    run_assist_turn(
        &mut history,
        client,
        &registry,
        &context,
        "check balance then send 1 ETH",
        ui_tx,
        cancel_rx,
    )
    .await
    .unwrap();

    let mut saw_balance = false;
    let mut proposal: Option<TxProposal> = None;
    while let Ok(ev) = ui_rx.try_recv() {
        match ev {
            ChatUiEvent::ToolResult { name, result } => {
                if name == "get_balance" {
                    assert!(!result.contains("error:"));
                    saw_balance = true;
                }
                if name == "propose_transfer" {
                    assert!(
                        !result.contains("refused"),
                        "propose must be allowed: {result}"
                    );
                }
            }
            ChatUiEvent::Proposal(p) => proposal = Some(*p),
            _ => {}
        }
    }
    assert!(saw_balance);
    let proposal = proposal.expect("proposal card must be emitted");
    assert_eq!(proposal.to, recipient);
    assert!(proposal.simulation_success);
}

#[tokio::test]
async fn assist_turn_sense_propose_then_human_broadcast_on_anvil() {
    let anvil = AnvilGuard::spawn(8601);
    let rpc = anvil.rpc_url.clone();
    let p = provider(&rpc);
    let signer = anvil_account0();
    let sender = signer.address();
    let recipient = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
    let amount = U256::from(1_500_000_000_000_000_000u64);

    let client = Arc::new(ScriptedClient::new(vec![
        ChatMessage::assistant_with_tools(
            "",
            vec![
                ToolCall {
                    id: "1".into(),
                    name: "simulate_call".into(),
                    arguments: json!({
                        "to": format!("{recipient:#x}"),
                        "data": "0x",
                        "value": amount.to_string()
                    }),
                    thought_signature: None,
                },
                ToolCall {
                    id: "2".into(),
                    name: "propose_transfer".into(),
                    arguments: json!({
                        "recipient": format!("{recipient:#x}"),
                        "amount": amount.to_string(),
                        "explanation": "Simulated then propose"
                    }),
                    thought_signature: None,
                },
            ],
        ),
        ChatMessage::assistant("Awaiting human approval."),
    ]));

    let registry = default_assist_registry();
    let context = ToolContext {
        rpc_url: rpc.clone(),
        chain_id: 31337,
        active_address: Some(sender),
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut history = vec![assist_system_prompt()];

    run_assist_turn(
        &mut history,
        client,
        &registry,
        &context,
        "simulate then propose",
        ui_tx,
        cancel_rx,
    )
    .await
    .unwrap();

    let mut proposal: Option<TxProposal> = None;
    while let Ok(ev) = ui_rx.try_recv() {
        if let ChatUiEvent::Proposal(p) = ev {
            proposal = Some(*p);
        }
    }
    let proposal = proposal.expect("proposal");

    let before = p.get_balance(recipient).await.unwrap();

    let mut tx = TransactionRequest::default()
        .from(sender)
        .to(proposal.to)
        .input(proposal.calldata.into())
        .value(proposal.value_wei);
    tx.nonce = Some(p.get_transaction_count(sender).await.unwrap());
    tx.gas = Some(proposal.gas_limit);
    tx.max_fee_per_gas = Some(2_000_000_000);
    tx.max_priority_fee_per_gas = Some(1_000_000_000);
    tx.chain_id = Some(31337);

    let wallet = EthereumWallet::from(signer);
    let signed = tx.build(&wallet).await.unwrap();
    let pending = p
        .send_raw_transaction(&signed.encoded_2718())
        .await
        .unwrap();
    let receipt = pending.get_receipt().await.unwrap();
    assert!(receipt.status());

    let after = p.get_balance(recipient).await.unwrap();
    assert_eq!(after, before + amount);
}

#[tokio::test]
async fn assist_turn_refuses_propose_without_sense_on_live_registry() {
    let anvil = AnvilGuard::spawn(8602);
    let client = Arc::new(ScriptedClient::new(vec![
        ChatMessage::assistant_with_tools(
            "",
            vec![ToolCall {
                id: "1".into(),
                name: "propose_transfer".into(),
                arguments: json!({
                    "recipient": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
                    "amount": "1",
                    "explanation": "no look first"
                }),
                thought_signature: None,
            }],
        ),
        ChatMessage::assistant("Understood."),
    ]));

    let registry = default_assist_registry();
    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(anvil_account0().address()),
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut history = vec![assist_system_prompt()];

    run_assist_turn(
        &mut history,
        client,
        &registry,
        &context,
        "send without looking",
        ui_tx,
        cancel_rx,
    )
    .await
    .unwrap();

    let mut refused = false;
    let mut saw_proposal = false;
    while let Ok(ev) = ui_rx.try_recv() {
        match ev {
            ChatUiEvent::ToolResult { name, result } => {
                if name == "propose_transfer" {
                    assert!(result.contains("refused"));
                    refused = true;
                }
            }
            ChatUiEvent::Proposal(_) => saw_proposal = true,
            _ => {}
        }
    }
    assert!(refused);
    assert!(!saw_proposal);
}

#[tokio::test]
async fn assist_turn_failed_sensory_does_not_unlock_propose() {
    let anvil = AnvilGuard::spawn(8603);
    let client = Arc::new(ScriptedClient::new(vec![
        ChatMessage::assistant_with_tools(
            "",
            vec![
                ToolCall {
                    id: "1".into(),
                    name: "get_balance".into(),
                    arguments: json!({ "account_address": "not-an-address" }),
                    thought_signature: None,
                },
                ToolCall {
                    id: "2".into(),
                    name: "propose_transfer".into(),
                    arguments: json!({
                        "recipient": "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
                        "amount": "1",
                        "explanation": "should still refuse"
                    }),
                    thought_signature: None,
                },
            ],
        ),
        ChatMessage::assistant("ok"),
    ]));

    let registry = default_assist_registry();
    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(anvil_account0().address()),
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel();
    let (_cancel_tx, cancel_rx) = watch::channel(false);
    let mut history = vec![assist_system_prompt()];

    run_assist_turn(
        &mut history,
        client,
        &registry,
        &context,
        "bad balance then propose",
        ui_tx,
        cancel_rx,
    )
    .await
    .unwrap();

    let mut balance_errored = false;
    let mut propose_refused = false;
    while let Ok(ev) = ui_rx.try_recv() {
        if let ChatUiEvent::ToolResult { name, result } = ev {
            if name == "get_balance" {
                assert!(result.contains("error:"));
                balance_errored = true;
            }
            if name == "propose_transfer" {
                assert!(result.contains("refused"));
                propose_refused = true;
            }
        }
    }
    assert!(balance_errored);
    assert!(propose_refused);
}

// --- Assist proposal tools -----------------------------------------------------------

#[tokio::test]
async fn propose_swap_native_in_simulates_on_mock_router() {
    let anvil = AnvilGuard::spawn(8604);
    let router = address!("1111111111111111111111111111111111111111");
    let weth = address!("2222222222222222222222222222222222222222");
    let token = address!("3333333333333333333333333333333333333333");
    // swapExactETHForTokens(uint256,address[],address,uint256)
    let sel = [0x7f, 0xf3, 0x6a, 0xb5];
    set_code(
        &anvil.rpc_url,
        router,
        &assemble_dispatcher(&[(sel, vec![])]),
    )
    .await;

    let registry = default_assist_registry();
    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(anvil_account0().address()),
    };

    let raw = registry
        .execute(
            "propose_swap",
            json!({
                "router_address": format!("{router:#x}"),
                "path": [format!("{weth:#x}"), format!("{token:#x}")],
                "amount_in": "1000000000000000000",
                "min_amount_out": "1",
                "is_native_in": true,
                "explanation": "ETH -> token via mock router"
            }),
            &context,
        )
        .await
        .unwrap();

    let prop: TxProposal = serde_json::from_value(raw).unwrap();
    assert_eq!(prop.to, router);
    assert_eq!(prop.value_wei, U256::from(1_000_000_000_000_000_000u64));
    assert!(prop.simulation_success);
    assert!(prop.calldata.as_ref().starts_with(&sel));
}

#[tokio::test]
async fn search_pairs_enumerates_planted_factory_on_anvil() {
    let anvil = AnvilGuard::spawn(8605);
    let factory = address!("2222222222222222222222222222222222222222");
    let pair = address!("3333333333333333333333333333333333333333");
    let routes = vec![
        ([0x1e, 0x3d, 0xd1, 0x8b], abi_encode_address(pair)), // allPairs(uint256)
        ([0x57, 0x4f, 0x2b, 0xa3], abi_encode_u256(U256::from(1))), // allPairsLength()
    ];
    set_code(&anvil.rpc_url, factory, &assemble_dispatcher(&routes)).await;

    let registry = default_sensory_registry();
    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: None,
    };

    let res = registry
        .execute(
            "search_pairs",
            json!({
                "factory_address": format!("{factory:#x}"),
                "start_index": 0,
                "limit": 1
            }),
            &context,
        )
        .await
        .unwrap();

    assert_eq!(
        res["total_pairs_count"].as_u64().unwrap(),
        1,
        "full response: {res}"
    );
    assert_eq!(res["count"].as_u64().unwrap(), 1, "full response: {res}");
    let listed = res["pairs"][0].as_str().unwrap();
    assert!(
        listed.eq_ignore_ascii_case(&format!("{pair:#x}"))
            || listed.eq_ignore_ascii_case(&pair.to_string()),
        "pair listing: {listed}"
    );
}

// --- Degen -----------------------------------------------------------------------

#[tokio::test]
async fn degen_dry_run_then_live_broadcast_on_anvil() {
    let anvil = AnvilGuard::spawn(8606);
    let p = provider(&anvil.rpc_url);
    // Account #2 — must not be the burner (account #1), or balance delta is gas-only.
    let recipient = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
    let amount = U256::from(1_000_000_000_000_000_000u64);

    let mut trader = DegenTrader::new(
        anvil_account1(),
        vec![anvil.rpc_url.clone()],
        31337,
        default_breaker(),
    )
    .with_dry_run(true);

    let before = p.get_balance(recipient).await.unwrap();

    let dry = trader
        .execute_swap(recipient, None, Bytes::new(), amount, amount, 50)
        .await
        .unwrap();
    assert!(dry.dry_run);
    assert!(dry.tx_hash.is_zero());
    assert_eq!(p.get_balance(recipient).await.unwrap(), before);

    trader.set_dry_run(false);
    assert!(!trader.is_dry_run());

    let live = trader
        .execute_swap(recipient, None, Bytes::new(), amount, amount, 50)
        .await
        .unwrap();
    assert!(!live.dry_run);
    assert!(!live.tx_hash.is_zero());
    assert_eq!(p.get_balance(recipient).await.unwrap(), before + amount);
}

#[tokio::test]
async fn degen_position_size_violation_rejects_without_halt_on_anvil() {
    let anvil = AnvilGuard::spawn(8607);
    let trader = DegenTrader::new(
        anvil_account1(),
        vec![anvil.rpc_url.clone()],
        31337,
        CircuitBreakerConfig {
            max_position_pct: 1,
            max_slippage_bps: 100,
            max_session_gas_wei: U256::from(10_000_000_000_000_000u64),
            max_consecutive_errors: 3,
            required_rpc_quorum: 1,
            ..Default::default()
        },
    );

    // Account #1 has ~10_000 ETH; 1% = 100 ETH — trade strictly above that.
    let trade = U256::from(200) * U256::from(10).pow(U256::from(18));
    let err = trader
        .execute_swap(
            address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9"),
            None,
            Bytes::new(),
            trade,
            trade,
            50,
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("position size") || err.to_string().contains("max allowed"));
    assert!(
        !trader.circuit_breaker().is_tripped(),
        "oversized trade must soft-reject without halting the session"
    );

    // Smaller trade under the cap must still be allowed to proceed past validation
    // (may fail later on simulation with empty calldata — that's fine).
    let small = U256::from(1) * U256::from(10).pow(U256::from(18));
    let _ = trader
        .execute_swap(
            address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9"),
            None,
            Bytes::new(),
            small,
            small,
            50,
        )
        .await;
    assert!(!trader.circuit_breaker().is_tripped());
}

#[tokio::test]
async fn degen_simulation_revert_records_failure_then_trips() {
    let anvil = AnvilGuard::spawn(8608);
    let revert_target = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
    // Always revert
    set_code(
        &anvil.rpc_url,
        revert_target,
        &[0x60, 0x00, 0x60, 0x00, 0xfd],
    )
    .await;

    let trader = DegenTrader::new(
        anvil_account1(),
        vec![anvil.rpc_url.clone()],
        31337,
        CircuitBreakerConfig {
            max_position_pct: 50,
            max_slippage_bps: 100,
            max_session_gas_wei: U256::from(10_000_000_000_000_000u64),
            max_consecutive_errors: 2,
            required_rpc_quorum: 1,
            ..Default::default()
        },
    );

    let amount = U256::from(1_000_000_000_000_000u64);
    let e1 = trader
        .execute_swap(revert_target, None, Bytes::new(), amount, amount, 50)
        .await
        .unwrap_err();
    assert!(e1.to_string().contains("simulation reverted"));
    assert!(!trader.circuit_breaker().is_tripped());

    let e2 = trader
        .execute_swap(revert_target, None, Bytes::new(), amount, amount, 50)
        .await
        .unwrap_err();
    assert!(e2.to_string().contains("simulation reverted"));
    assert!(trader.circuit_breaker().is_tripped());
}

#[tokio::test]
async fn degen_dry_run_gas_ceiling_trips_without_broadcast() {
    let anvil = AnvilGuard::spawn(8609);
    let p = provider(&anvil.rpc_url);
    let recipient = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
    let before = p.get_balance(recipient).await.unwrap();

    let trader = DegenTrader::new(
        anvil_account1(),
        vec![anvil.rpc_url.clone()],
        31337,
        CircuitBreakerConfig {
            max_position_pct: 50,
            max_slippage_bps: 100,
            max_session_gas_wei: U256::from(1u64), // tiny ceiling
            max_consecutive_errors: 3,
            required_rpc_quorum: 1,
            ..Default::default()
        },
    )
    .with_dry_run(true);

    let amount = U256::from(1_000_000_000_000_000_000u64);
    let err = trader
        .execute_swap(recipient, None, Bytes::new(), amount, amount, 50)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("gas ceiling"));
    assert!(trader.circuit_breaker().is_tripped());
    assert_eq!(p.get_balance(recipient).await.unwrap(), before);
}

#[tokio::test]
async fn degen_emergency_stop_blocks_swap_on_anvil() {
    let anvil = AnvilGuard::spawn(8610);
    let trader = DegenTrader::new(
        anvil_account1(),
        vec![anvil.rpc_url.clone()],
        31337,
        default_breaker(),
    );
    trader.emergency_stop("operator kill switch");

    let err = trader
        .execute_swap(
            address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9"),
            None,
            Bytes::new(),
            U256::from(1),
            U256::from(1),
            10,
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("Trading halted"));
}

#[tokio::test]
async fn degen_quorum_agrees_across_two_anvils() {
    let a = AnvilGuard::spawn(8611);
    let b = AnvilGuard::spawn(8612);
    let pair = address!("2222222222222222222222222222222222222222");
    let code = assemble_dispatcher(&[(
        [0x09, 0x02, 0xf1, 0xac],
        reserves_abi(U256::from(1000), U256::from(2000), 1_700_000_000),
    )]);
    set_code(&a.rpc_url, pair, &code).await;
    set_code(&b.rpc_url, pair, &code).await;

    let reserves =
        QuorumValidator::validate_pair_reserves(&[a.rpc_url.clone(), b.rpc_url.clone()], pair, 2)
            .await
            .unwrap();

    assert_eq!(reserves.reserve0, U256::from(1000));
    assert_eq!(reserves.reserve1, U256::from(2000));
}

#[tokio::test]
async fn degen_quorum_rejects_divergent_reserves() {
    let a = AnvilGuard::spawn(8613);
    let b = AnvilGuard::spawn(8614);
    let pair = address!("2222222222222222222222222222222222222222");

    set_code(
        &a.rpc_url,
        pair,
        &assemble_dispatcher(&[(
            [0x09, 0x02, 0xf1, 0xac],
            reserves_abi(U256::from(1000), U256::from(2000), 1),
        )]),
    )
    .await;
    set_code(
        &b.rpc_url,
        pair,
        &assemble_dispatcher(&[(
            [0x09, 0x02, 0xf1, 0xac],
            reserves_abi(U256::from(5000), U256::from(9000), 1),
        )]),
    )
    .await;

    let err =
        QuorumValidator::validate_pair_reserves(&[a.rpc_url.clone(), b.rpc_url.clone()], pair, 2)
            .await
            .unwrap_err();

    assert!(
        err.to_string().contains("quorum divergence")
            || err.to_string().contains("Quorum failed")
            || err.to_string().to_lowercase().contains("divergence"),
        "unexpected: {err}"
    );
}

#[tokio::test]
async fn degen_execute_swap_with_pair_quorum_on_two_rpcs() {
    let a = AnvilGuard::spawn(8615);
    let b = AnvilGuard::spawn(8616);
    let pair = address!("2222222222222222222222222222222222222222");
    let recipient = address!("0x165C3410fC91EF562C50559f7d2289fEbed552d9");
    let code = assemble_dispatcher(&[(
        [0x09, 0x02, 0xf1, 0xac],
        reserves_abi(
            U256::from(100) * U256::from(10).pow(U256::from(18)),
            U256::from(200) * U256::from(10).pow(U256::from(18)),
            1_700_000_000,
        ),
    )]);
    set_code(&a.rpc_url, pair, &code).await;
    set_code(&b.rpc_url, pair, &code).await;

    let trader = DegenTrader::new(
        anvil_account1(),
        vec![a.rpc_url.clone(), b.rpc_url.clone()],
        31337,
        CircuitBreakerConfig {
            max_position_pct: 50,
            max_slippage_bps: 100,
            max_session_gas_wei: U256::from(10_000_000_000_000_000u64),
            max_consecutive_errors: 3,
            required_rpc_quorum: 2,
            ..Default::default()
        },
    );

    let amount = U256::from(1_000_000_000_000_000_000u64);
    let outcome = trader
        .execute_swap(recipient, Some(pair), Bytes::new(), amount, amount, 50)
        .await
        .unwrap();

    assert!(!outcome.dry_run);
    assert!(!outcome.tx_hash.is_zero());
}
