//! End-to-end test of the EIP-7702 self-pay path against a *forked* anvil.
//!
//! The test forks PulseChain testnet (chain 943), where Ambire's
//! `AmbireAccount` implementation is deployed at `0x2A2b…684EF`, and runs the
//! full flow:
//!
//! 1. **Bootstrap** — a fresh EOA has no on-chain key privileges, so the first
//!    7702 transaction delegates the account AND self-calls
//!    `setAddrPrivilege(account, bytes32(1))` (msg.sender == address(this)
//!    inside the delegated call, which the contract requires). Without this
//!    step the first `execute` reverts with `INSUFFICIENT_PRIVILEGE`.
//! 2. **Batch** — [`vaughan_aa::adapter::submit_self_pay`] signs the batch +
//!    authorization + envelope, broadcasts, and the batch executes on-chain.
//!
//! Requires `anvil` on PATH and network access to the testnet RPC (for the
//! fork). Skips (does not fail) when the fork cannot be reached.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use alloy::consensus::{SignableTransaction, TxEip7702};
use alloy::eips::eip2718::Encodable2718;
use alloy::eips::eip7702::Authorization;
use alloy::primitives::{Address, B256, Bytes, U256};
use alloy::signers::SignerSync;
use alloy::sol;
use alloy::sol_types::{SolCall, SolValue};
use vaughan_aa::abi::Transaction;
use vaughan_aa::adapter::submit_self_pay;
use vaughan_aa::scw::{ScwTransaction, SignatureMode};
use vaughan_aa::sign::sign_scw_transaction;
use vaughan_core::chains::evm::EvmAdapter;
use vaughan_core::security::hd_wallet::{derive_account, validate_mnemonic};

sol! {
    /// The AmbireAccount entry points the test bootstrap needs (interface
    /// facts only, from the deployed contract).
    interface AmbireAccountBootstrap {
        function setAddrPrivilege(address addr, bytes32 priv) external;
    }
}

/// Anvil's default dev mnemonic — account #0 is funded on the fork.
const ANVIL_MNEMONIC: &str = "test test test test test test test test test test test junk";
/// The deployed AmbireAccount implementation on PulseChain testnet (943).
const IMPLEMENTATION: &str = "0x2A2b85EB1054d6f0c6c2E37dA05eD3E5feA684EF";
/// PulseChain testnet v4 RPC — forked by anvil so the implementation exists.
const TESTNET_RPC: &str = "https://rpc.v4.testnet.pulsechain.com";

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

struct ForkedAnvil {
    child: Child,
    url: String,
}

impl ForkedAnvil {
    /// Start `anvil` forking the testnet, or `None` if the fork RPC is
    /// unreachable (the test then skips).
    fn start() -> Option<Self> {
        let port = free_port();
        let child = Command::new("anvil")
            .args([
                "--fork-url",
                TESTNET_RPC,
                "--chain-id",
                "943",
                "--port",
                &port.to_string(),
                "--hardfork",
                "prague", // EIP-7702 support
                "--silent",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil must be on PATH (foundry)");
        let anvil = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };

        // Bounded readiness probe: the fork must load the testnet head, so
        // give it a few seconds; on failure kill and report "skip".
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            match anvil.rpc_chain_id() {
                Ok(943) => return Some(anvil),
                Ok(_) => {
                    drop(anvil);
                    panic!("forked anvil reported the wrong chain id");
                }
                Err(_) => std::thread::sleep(Duration::from_millis(200)),
            }
        }
        drop(anvil);
        None
    }

    fn rpc_chain_id(&self) -> Result<u64, String> {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}"#;
        let out = Command::new("curl")
            .args(["-s", "-X", "POST", "-H", "Content-Type: application/json", "-d", body])
            .arg(&self.url)
            .output()
            .map_err(|e| e.to_string())?;
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
        v["result"]
            .as_str()
            .map(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap())
            .ok_or_else(|| "no chain id".to_string())
    }

    fn rpc(&self, method: &str, params: &str) -> serde_json::Value {
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"{method}","params":{params}}}"#
        );
        let out = Command::new("curl")
            .args(["-s", "-X", "POST", "-H", "Content-Type: application/json", "-d", &body])
            .arg(&self.url)
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    }

    fn wei_balance(&self, address: &str) -> u128 {
        let v = self.rpc("eth_getBalance", &format!(r#"["{address}","latest"]"#));
        u128::from_str_radix(v["result"].as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
    }

    fn code(&self, address: &str) -> String {
        let v = self.rpc("eth_getCode", &format!(r#"["{address}","latest"]"#));
        v["result"].as_str().unwrap().to_string()
    }

    /// Wait for `tx_hash` to be mined; returns its status ("0x0"/"0x1") or
    /// `None` on timeout.
    fn wait_mined(&self, tx_hash: &str) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(25);
        while Instant::now() < deadline {
            let receipt = self.rpc("eth_getTransactionReceipt", &format!(r#"["{tx_hash}"]"#));
            if let Some(status) = receipt["result"]["status"].as_str() {
                return Some(status.to_string());
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        None
    }
}

impl Drop for ForkedAnvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The full 7702 self-pay flow on a forked testnet: bootstrap the account key
/// privilege, then submit a signed batch and verify it executes on-chain.
#[tokio::test]
async fn self_pay_7702_executes_batch_on_forked_testnet() {
    let Some(anvil) = ForkedAnvil::start() else {
        eprintln!("testnet RPC unreachable — skipping (network required for the fork)");
        return;
    };

    let signer = derive_account(&validate_mnemonic(ANVIL_MNEMONIC).unwrap(), 0).unwrap();
    let recipient = derive_account(&validate_mnemonic(ANVIL_MNEMONIC).unwrap(), 1).unwrap();
    let account = signer.address();
    let implementation = Address::parse_checksummed(IMPLEMENTATION, None).unwrap();

    let adapter = EvmAdapter::new(&anvil.url, 943, "anvil-fork", &[]).await.unwrap();

    // ---- 1. Bootstrap: delegate + grant the account execute privilege ----
    // A fresh EOA has no keys in AmbireAccount storage, so a direct `execute`
    // would revert INSUFFICIENT_PRIVILEGE. This first 7702 tx self-calls
    // setAddrPrivilege(account, bytes32(1)) (msg.sender == address(this)).
    let nonce0 = adapter.get_pending_nonce(&account.to_string()).await.unwrap();
    let bootstrap_calldata = AmbireAccountBootstrap::setAddrPrivilegeCall {
        addr: account,
        r#priv: B256::from(U256::from(1u64)),
    }
    .abi_encode();

    let auth = Authorization {
        chain_id: U256::from(943),
        address: implementation,
        nonce: nonce0 + 1, // EIP-7702: checked after the sender nonce increments
    };
    let auth_hash = auth.signature_hash();
    let signed_auth = auth.into_signed(signer.sign_hash_sync(&auth_hash).unwrap());

    let bootstrap_tx = TxEip7702 {
        chain_id: 943,
        nonce: nonce0,
        gas_limit: 200_000,
        max_fee_per_gas: 100_000_000_000, // generous on testnet's sub-gwei fees
        max_priority_fee_per_gas: 1_000_000_000,
        to: account,
        value: U256::ZERO,
        access_list: alloy::eips::eip2930::AccessList::default(),
        authorization_list: vec![signed_auth],
        input: bootstrap_calldata.into(),
    };
    let envelope_sig = signer.sign_hash_sync(&bootstrap_tx.signature_hash()).unwrap();
    let raw = bootstrap_tx.into_signed(envelope_sig).encoded_2718();
    let bootstrap_hash = adapter.broadcast_raw(raw).await.expect("bootstrap broadcast");
    let status = anvil.wait_mined(&bootstrap_hash.0).expect("bootstrap must mine");
    assert_eq!(status, "0x1", "bootstrap (setAddrPrivilege) must succeed");

    // Verify the privilege landed in the account's storage (mapping slot 0:
    // keccak256(abi.encode(account, 0))).
    let slot = alloy::primitives::keccak256(
        [&account.abi_encode()[..], &U256::ZERO.abi_encode()[..]].concat(),
    );
    let stored = anvil.rpc(
        "eth_getStorageAt",
        &format!(r#"["{account}","0x{}","latest"]"#, hex::encode(slot)),
    );
    assert_eq!(
        stored["result"].as_str().unwrap(),
        "0x0000000000000000000000000000000000000000000000000000000000000001",
        "account must hold bytes32(1) privilege after bootstrap"
    );

    // ---- 2. Batch: sign + submit the 7702 self-pay tx ----
    let value_wei = U256::from(10u128.pow(18)); // 1 tPLS
    let batch = ScwTransaction {
        account,
        chain_id: 943,
        nonce: 0, // AmbireAccount's internal nonce — untouched by bootstrap
        txns: vec![Transaction {
            to: recipient.address(),
            value: value_wei,
            data: Bytes::new(),
        }],
    };
    let signature = sign_scw_transaction(&signer, &batch, SignatureMode::RawHash).unwrap();
    assert_eq!(signature.len(), 66, "r‖s‖v‖mode signature");

    let before = anvil.wei_balance(&recipient.address().to_string());
    let batch_hash = submit_self_pay(
        &adapter,
        &signer,
        &batch,
        &signature,
        implementation,
        None,
    )
    .await
    .expect("7702 self-pay broadcast must succeed");
    let status = anvil.wait_mined(&batch_hash.0).expect("batch tx must mine");
    if status != "0x1" {
        let trace = anvil.rpc(
            "debug_traceTransaction",
            &format!(r#"["{}",{{"tracer":"callTracer"}}]"#, batch_hash.0),
        );
        eprintln!("DEBUG batch revert: {}", trace["result"]["revertReason"]);
    }
    assert_eq!(status, "0x1", "batch execute must succeed (tx {batch_hash})");

    // The batch executed on-chain: the recipient received exactly the value,
    // which only happens if the delegation was applied and `execute` ran
    // against the real AmbireAccount implementation.
    assert_eq!(
        anvil.wei_balance(&recipient.address().to_string()),
        before + value_wei.to::<u128>(),
        "batch must execute on-chain (tx {batch_hash})"
    );
    // Each 7702 tx increments the account nonce twice: once as the tx
    // sender, once as the authority in its authorization list (EIP-7702
    // bumps the authority's nonce too). Two 7702 txs => +4.
    assert_eq!(
        adapter.get_pending_nonce(&account.to_string()).await.unwrap(),
        nonce0 + 4,
        "sender nonce must advance 4 (bootstrap + batch, sender + authority each)"
    );
    // EIP-7702 delegations are *persistent* (the final spec kept them, per
    // the EIP's "Persistence of code delegation" rationale — the transient
    // variant was dropped). The account stays delegated to the
    // implementation: its code is the designator `0xef0100 || impl`.
    assert_eq!(
        anvil.code(&account.to_string()),
        format!("0xef0100{}", hex::encode(implementation.as_slice())),
        "7702 delegation must persist after the batch tx"
    );
}
