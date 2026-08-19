use alloy::eips::eip2718::Encodable2718;
use alloy::network::{Ethereum, EthereumWallet, NetworkTransactionBuilder};
use alloy::primitives::{address, Bytes, U256};
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::types::eth::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use serde_json::json;
use std::process::{Child, Command};
use std::time::Duration;
use url::Url;

use vaughan_agent::degen::{CircuitBreakerConfig, DegenTrader};
use vaughan_agent::proposal::TxProposal;
use vaughan_agent::tools::{default_assist_registry, default_sensory_registry, ToolContext};

struct AnvilGuard {
    child: Child,
    rpc_url: String,
}

impl AnvilGuard {
    fn spawn(port: u16) -> Self {
        let child = Command::new("anvil")
            .args(["-p", &port.to_string(), "--silent"])
            .spawn()
            .expect("Failed to start Anvil node.");

        std::thread::sleep(Duration::from_millis(400));
        let rpc_url = format!("http://127.0.0.1:{}", port);
        Self { child, rpc_url }
    }
}

impl Drop for AnvilGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[tokio::test]
async fn test_agent_sensory_inspection_against_anvil() {
    let anvil = AnvilGuard::spawn(8558);
    let provider: RootProvider<Ethereum> =
        RootProvider::new_http(Url::parse(&anvil.rpc_url).unwrap());

    // Deploy mock token code
    let token_addr = address!("2222222222222222222222222222222222222222");
    // name() [0x06fdde03], symbol() [0x95d89b41], decimals() [0x313ce7f2], totalSupply() [0x18160ddd]
    let routes: &[([u8; 4], Vec<u8>)] = &[
        ([0x06, 0xfd, 0xde, 0x03], vec![0; 64]), // name()
        ([0x95, 0xd8, 0x9b, 0x41], vec![0; 64]), // symbol()
        ([0x31, 0x3c, 0xe7, 0xf2], vec![0; 32]), // decimals()
        ([0x18, 0x16, 0x0d, 0xdd], vec![0; 32]), // totalSupply()
    ];

    let bytecode = assemble_dispatcher(routes);
    let _: () = provider
        .raw_request(
            "anvil_setCode".into(),
            (token_addr, format!("0x{}", hex::encode(bytecode))),
        )
        .await
        .unwrap();

    let registry = default_sensory_registry();
    let context = ToolContext {
        rpc_url: anvil.rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(address!("f39Fd6e51aad88F6F4ce6aB8827279cffFb92266")),
    };

    let inspect_res = registry
        .execute(
            "inspect_contract",
            json!({ "address": format!("{token_addr:#x}") }),
            &context,
        )
        .await
        .unwrap();

    assert_eq!(inspect_res["fingerprint"]["type"], "Erc20");
    assert!(inspect_res["candidate_selectors"].as_array().unwrap().len() >= 4);
}

#[tokio::test]
async fn test_agent_proposal_to_human_approval_to_broadcast_flow() {
    let anvil = AnvilGuard::spawn(8559);
    let rpc_url = anvil.rpc_url.clone();
    let provider: RootProvider<Ethereum> = RootProvider::new_http(Url::parse(&rpc_url).unwrap());

    let signer: PrivateKeySigner =
        "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
            .parse()
            .unwrap();
    let sender = signer.address();
    let recipient = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");

    let context = ToolContext {
        rpc_url: rpc_url.clone(),
        chain_id: 31337,
        active_address: Some(sender),
    };

    let registry = default_assist_registry();

    // 1. Agent drafts a proposal (AI Advisor cannot sign)
    let prop_val = registry
        .execute(
            "propose_transfer",
            json!({
                "recipient": format!("{recipient:#x}"),
                "amount": "2000000000000000000", // 2 ETH
                "explanation": "Moving 2 ETH to savings account"
            }),
            &context,
        )
        .await
        .unwrap();

    let proposal: TxProposal = serde_json::from_value(prop_val).unwrap();
    assert_eq!(proposal.to, recipient);
    assert_eq!(proposal.value_wei.to_string(), "2000000000000000000");
    assert!(proposal.simulation_success);

    // 2. Human Approval Gate: Wallet core constructs transaction from proposal calldata/target independently
    let initial_balance = provider.get_balance(recipient).await.unwrap();

    let mut tx = TransactionRequest::default()
        .from(sender)
        .to(proposal.to)
        .input(proposal.calldata.into())
        .value(proposal.value_wei);

    let nonce = provider.get_transaction_count(sender).await.unwrap();
    tx.nonce = Some(nonce);
    tx.gas = Some(proposal.gas_limit);
    tx.max_fee_per_gas = Some(2_000_000_000);
    tx.max_priority_fee_per_gas = Some(1_000_000_000);
    tx.chain_id = Some(31337);

    let wallet = EthereumWallet::from(signer);
    let signed = tx.build(&wallet).await.unwrap();

    let pending = provider
        .send_raw_transaction(&signed.encoded_2718())
        .await
        .unwrap();

    let _receipt = pending.get_receipt().await.unwrap();

    let final_balance = provider.get_balance(recipient).await.unwrap();
    assert_eq!(
        final_balance,
        initial_balance + U256::from(2_000_000_000_000_000_000u64)
    );
}

#[tokio::test]
async fn test_degen_mode_circuit_breaker_halts_on_risk_violation() {
    let anvil = AnvilGuard::spawn(8560);
    let rpc_url = anvil.rpc_url.clone();

    let burner: PrivateKeySigner =
        "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
            .parse()
            .unwrap();

    let trader = DegenTrader::new(
        burner,
        vec![rpc_url],
        31337,
        CircuitBreakerConfig {
            max_position_pct: 10,  // Max 10%
            max_slippage_bps: 100, // Max 1%
            max_session_gas_wei: U256::from(1_000_000_000_000_000u64),
            max_consecutive_errors: 2,
            required_rpc_quorum: 1,
        },
    );

    // Trade 1: Slippage violation (2% > 1%) -> Immediately trips breaker and halts
    let target = address!("70997970C51812dc3A010C7d01b50e0d17dc79C8");
    let err = trader
        .execute_swap(
            target,
            None,
            Bytes::new(),
            U256::from(100),
            U256::from(100),
            200, // 200 bps = 2.0% slippage -> Violation!
        )
        .await
        .unwrap_err();

    assert!(err.to_string().contains("maximum allowable slippage"));
    assert!(trader.circuit_breaker().is_tripped());

    // Subsequent trades are strictly blocked while circuit breaker is tripped
    let err2 = trader
        .execute_swap(
            target,
            None,
            Bytes::new(),
            U256::from(10),
            U256::from(10),
            10,
        )
        .await
        .unwrap_err();

    assert!(err2.to_string().contains("Trading halted"));
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
            (ret_data.len() + 31) / 32
        };
        current_offset += 1 + chunks * 36 + 6;
    }

    for (i, (sel, _)) in routes.iter().enumerate() {
        let (target, _) = handlers[i];
        bytecode.push(0x80); // DUP1
        bytecode.push(0x63); // PUSH4
        bytecode.extend_from_slice(sel);
        bytecode.push(0x14); // EQ
        bytecode.push(0x61); // PUSH2
        bytecode.push((target >> 8) as u8);
        bytecode.push((target & 0xff) as u8);
        bytecode.push(0x57); // JUMPI
    }

    // Fallback revert
    bytecode.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xfd]);

    for (target, ret_data) in handlers {
        assert_eq!(bytecode.len(), target as usize);
        bytecode.push(0x5b); // JUMPDEST
        let chunks = if ret_data.is_empty() {
            0
        } else {
            (ret_data.len() + 31) / 32
        };
        for c in 0..chunks {
            let start = c * 32;
            let end = (start + 32).min(ret_data.len());
            let mut chunk_bytes = [0u8; 32];
            chunk_bytes[..end - start].copy_from_slice(&ret_data[start..end]);
            bytecode.push(0x7f); // PUSH32
            bytecode.extend_from_slice(&chunk_bytes);
            bytecode.push(0x60); // PUSH1
            bytecode.push((c * 32) as u8);
            bytecode.push(0x52); // MSTORE
        }
        let len = ret_data.len() as u16;
        bytecode.push(0x61); // PUSH2
        bytecode.push((len >> 8) as u8);
        bytecode.push((len & 0xff) as u8);
        bytecode.push(0x60); // PUSH1
        bytecode.push(0x00);
        bytecode.push(0xf3); // RETURN
    }

    bytecode
}
