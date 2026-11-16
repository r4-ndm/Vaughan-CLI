//! Anvil coverage for the one claim a mock cannot make (gas-ceiling spec test 17):
//! what the gas ceiling reserves is what the node is actually asked to charge.
//!
//! `MockBroadcaster` tests prove the daemon forwards `quote.tx` unchanged; this one
//! quotes a real transfer through `WalletBroadcaster`, broadcasts THAT quote, and
//! compares `eth_getTransactionByHash` against the quoted `gas` / `maxFeePerGas`.
//! Requires `anvil` on PATH (chain id 943 matches the built-in testnet).

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use alloy::primitives::{Address, U256};
use secrecy::SecretString;
use serde_json::{json, Value};
use vaughan_cli::signerd::{TransactionBroadcaster, WalletBroadcaster};
use vaughan_core::chains::EvmTransaction;
use vaughan_core::core::WalletState;
use vaughan_core::security::hd_wallet::validate_mnemonic;

const ANVIL_MNEMONIC: &str = "test test test test test test test test test test test junk";
const PASSWORD: &str = "BombProof123!";
/// Anvil default account #0, which the restored vault derives at the same HD path.
const ACCOUNT_0: &str = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";

struct Anvil {
    child: Child,
    url: String,
}

impl Drop for Anvil {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Anvil {
    fn start() -> Self {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let child = Command::new("anvil")
            .args(["--port", &port.to_string(), "--chain-id", "943", "--silent"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("anvil must be on PATH (foundry)");
        let anvil = Self {
            child,
            url: format!("http://127.0.0.1:{port}"),
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if rpc(&anvil.url, "eth_chainId", json!([])).is_ok() {
                return anvil;
            }
            if Instant::now() > deadline {
                panic!("anvil did not start");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn rpc(url: &str, method: &str, params: Value) -> Result<Value, String> {
    let body = json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    let out = Command::new("curl")
        .args([
            "-s",
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-d",
        ])
        .arg(body.to_string())
        .arg(url)
        .output()
        .expect("curl");
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    if let Some(err) = v.get("error") {
        return Err(err.to_string());
    }
    Ok(v["result"].clone())
}

fn hex_u256(value: &Value) -> U256 {
    let s = value
        .as_str()
        .unwrap_or_else(|| panic!("expected hex string, got {value}"));
    U256::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex U256")
}

fn tx_by_hash(url: &str, hash: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match rpc(url, "eth_getTransactionByHash", json!([hash])) {
            Ok(tx) if !tx.is_null() => return tx,
            _ => {}
        }
        if Instant::now() > deadline {
            panic!("node never returned a tx for {hash}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test]
async fn test_quoted_fee_is_the_fee_the_node_sees() {
    let anvil = Anvil::start();
    let dir = tempfile::tempdir().unwrap();

    let mut wallet = WalletState::load(dir.path().join("wallet.json")).unwrap();
    wallet
        .create(
            &SecretString::from(PASSWORD.to_string()),
            validate_mnemonic(ANVIL_MNEMONIC).unwrap(),
        )
        .unwrap();
    wallet.set_active_network("pulsechain-testnet-v4").unwrap();
    wallet.set_rpc_override(&anvil.url);
    assert_eq!(
        wallet.active_address().unwrap().parse::<Address>().unwrap(),
        ACCOUNT_0.parse::<Address>().unwrap(),
        "the test vault must derive anvil's funded account #0"
    );

    let broadcaster = WalletBroadcaster::new(Arc::new(tokio::sync::Mutex::new(wallet)));

    let tx = EvmTransaction {
        from: ACCOUNT_0.to_string(),
        to: format!("{:#x}", Address::repeat_byte(0x11)),
        value: "1000000000000000".to_string(), // 0.001 native
        data: None,
        gas_limit: None,
        gas_price: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
        nonce: None,
        chain_id: 943,
    };

    let quote = broadcaster
        .quote_fee(tx)
        .await
        .expect("a live node must price a plain native transfer");
    let quoted_gas_limit = quote.tx.gas_limit.expect("quote fills gas_limit");
    let quoted_max_fee = U256::from_str(
        quote
            .tx
            .max_fee_per_gas
            .as_deref()
            .expect("quote fills max_fee"),
    )
    .unwrap();
    assert_eq!(
        quote.max_fee_wei,
        U256::from(quoted_gas_limit).saturating_mul(quoted_max_fee),
        "max_fee_wei is what the ceiling reserves: gas_limit * max_fee_per_gas"
    );

    // Broadcast the QUOTED tx, not a re-derived one: that is the invariant under test.
    let hash = broadcaster
        .broadcast_phased(quote.tx.clone())
        .await
        .expect("broadcast of the quoted tx must succeed against anvil");

    let onchain = tx_by_hash(&anvil.url, &hash);
    assert_eq!(
        hex_u256(&onchain["gas"]),
        U256::from(quoted_gas_limit),
        "the node's gas limit must equal the quoted gas_limit, or the ceiling priced a \
         different transaction than the one signed"
    );
    assert_eq!(
        hex_u256(&onchain["maxFeePerGas"]),
        quoted_max_fee,
        "the node's maxFeePerGas must equal the quoted max_fee_per_gas"
    );

    // The ceiling's reservation must bound what the chain can actually charge for this tx.
    let paid_ceiling = hex_u256(&onchain["gas"]).saturating_mul(hex_u256(&onchain["maxFeePerGas"]));
    assert_eq!(paid_ceiling, quote.max_fee_wei);
}
