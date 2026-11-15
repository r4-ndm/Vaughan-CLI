//! Live funded native-in V2 swap proof on PulseChain testnet v4 (chain 943).
//!
//! Run:
//! ```sh
//! cargo test -p vaughan-cli --test signerd_v2_live_943 -- --ignored --nocapture --test-threads=1
//! ```

use std::fs;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use alloy::primitives::{address, Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::sol;
use alloy::sol_types::SolCall;
use secrecy::SecretString;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use vaughan_agent::sentient::policy::{AgentSessionPolicy, EnforcementMode};
use vaughan_cli::signerd::{
    LiveBalanceSource, LiveSwapQuoter, Signer, SignerResponse, WalletBroadcaster,
};
use vaughan_core::core::signer_client::ENV_SIGNER_SOCKET;
use vaughan_core::core::{quote_v2_exact_in, OperatingMode, WalletState};
use vaughan_core::security::hd_wallet::generate_mnemonic;

const CHAIN_ID_943: u64 = 943;
const RPC_943: &str = "https://rpc.v4.testnet.pulsechain.com";
const FAUCET_CLAIM_URL: &str = "https://faucet.v4.testnet.pulsechain.com/api/claim";

/// PulseX V2 SwapRouter on PulseChain testnet v4 (943).
const ROUTER_943: Address = address!("DaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7");
/// Wrapped Pulse (WPLS / tWPLS) on PulseChain testnet v4 (943).
const WPLS_943: Address = address!("70499adEBB11Efd915E3b69E700c331778628707");
/// PulseX token (PLSX) paired with WPLS at `allPairs(0)` on factory `0xFf0538782D122d3112F75dc7121F61562261c0f7`.
const PLSX_943: Address = address!("8a810ea8B121d08342E9e7696f4a9915cBE494B7");

static TEST_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

sol! {
    interface IERC20Balance {
        function balanceOf(address account) external view returns (uint256);
    }
}

struct EnvSocketGuard;

impl Drop for EnvSocketGuard {
    fn drop(&mut self) {
        std::env::remove_var(ENV_SIGNER_SOCKET);
    }
}

fn funded_proof_v2_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("private")
        .join("funded-proof")
        .join("v2")
}

/// Load or generate the throwaway testnet password at `private/funded-proof/v2/throwaway.pw` (0600).
/// Never prints or logs the password.
fn load_or_create_password(v2_dir: &Path) -> SecretString {
    let pw_path = v2_dir.join("throwaway.pw");
    if pw_path.is_file() {
        let _ = fs::set_permissions(&pw_path, fs::Permissions::from_mode(0o600));
        let raw = fs::read_to_string(&pw_path).expect("read throwaway.pw");
        return SecretString::from(raw.trim().to_string());
    }

    let entropy = generate_mnemonic().expect("generate entropy for throwaway password");
    let seed = entropy.to_seed("");
    let pw_plain = format!("Vt943!{}Aa1#", hex::encode(&seed[..12]));
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true).mode(0o600);
    let mut file = opts.open(&pw_path).expect("create throwaway.pw (0600)");
    file.write_all(pw_plain.as_bytes())
        .expect("write throwaway.pw");
    file.sync_all().expect("sync throwaway.pw");
    let _ = fs::set_permissions(&pw_path, fs::Permissions::from_mode(0o600));
    SecretString::from(pw_plain)
}

/// Create (first run) or reload (re-runs) the throwaway testnet wallet under `private/funded-proof/v2/store`.
/// Never prints or logs any mnemonic or password.
fn load_or_create_wallet(store_dir: &Path, v2_dir: &Path, password: &SecretString) -> WalletState {
    let wallet_path = store_dir.join("wallet.json");
    let backup_wallet_path = v2_dir.join("wallet.backup.json");

    if !wallet_path.is_file() && backup_wallet_path.is_file() {
        fs::copy(&backup_wallet_path, &wallet_path).expect("restore wallet.json from backup");
        let _ = fs::set_permissions(&wallet_path, fs::Permissions::from_mode(0o600));
    }

    let mut wallet = WalletState::load_with_session(
        wallet_path.clone(),
        OperatingMode::SentientTrader,
        "sentient",
    )
    .expect("load_with_session for throwaway store");

    if !wallet.is_initialized() {
        let mnemonic = generate_mnemonic().expect("generate throwaway BIP-39 mnemonic");
        wallet
            .create(password, mnemonic)
            .expect("create throwaway wallet");
        let _ = fs::set_permissions(&wallet_path, fs::Permissions::from_mode(0o600));
    }

    wallet.unlock(password).expect("unlock throwaway wallet");
    wallet
        .set_active_network("pulsechain-testnet-v4")
        .expect("switch throwaway wallet to pulsechain-testnet-v4 (943)");
    wallet.set_rpc_override(RPC_943);
    let _ = fs::set_permissions(&wallet_path, fs::Permissions::from_mode(0o600));

    if !backup_wallet_path.is_file() {
        let _ = fs::copy(&wallet_path, &backup_wallet_path);
        let _ = fs::set_permissions(&backup_wallet_path, fs::Permissions::from_mode(0o600));
    }

    wallet
}

async fn get_erc20_balance<P: Provider>(provider: &P, token: Address, account: Address) -> U256 {
    let calldata = IERC20Balance::balanceOfCall { account }.abi_encode();
    let req = TransactionRequest::default()
        .to(token)
        .input(calldata.into());
    let raw = provider
        .call(req)
        .await
        .expect("eth_call ERC20 balanceOf on 943");
    IERC20Balance::balanceOfCall::abi_decode_returns(&raw).expect("decode balanceOf return")
}

async fn send_socket_line(socket_path: &Path, line: &str) -> (String, SignerResponse) {
    let stream = UnixStream::connect(socket_path)
        .await
        .expect("connect to temp signerd socket");
    let (reader, mut writer) = stream.into_split();
    let payload = format!("{}\n", line.trim());
    writer
        .write_all(payload.as_bytes())
        .await
        .expect("write intent to socket");
    writer.flush().await.expect("flush socket writer");

    let mut buf = BufReader::new(reader);
    let mut resp_line = String::new();
    let n = buf
        .read_line(&mut resp_line)
        .await
        .expect("read response line from socket");
    assert!(n > 0, "signerd closed socket without replying");
    let trimmed = resp_line.trim().to_string();
    let parsed: SignerResponse =
        serde_json::from_str(&trimmed).expect("deserialize SignerResponse JSON");
    (trimmed, parsed)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "live PulseChain testnet 943 funded V2 swap proof"]
async fn v2_native_in_swap_live_943() {
    let _mutex_guard = TEST_MUTEX.lock().await;

    // 1. Setup durable directories and load/create throwaway wallet
    let v2_dir = funded_proof_v2_dir();
    let store_dir = v2_dir.join("store");
    fs::create_dir_all(&store_dir).expect("create private/funded-proof/v2/store");
    fs::set_permissions(&v2_dir, fs::Permissions::from_mode(0o700))
        .expect("chmod 0700 private/funded-proof/v2");
    fs::set_permissions(&store_dir, fs::Permissions::from_mode(0o700))
        .expect("chmod 0700 private/funded-proof/v2/store");

    // Non-destructively archive any prior run's spend ledger so re-runs keep all files and start a fresh window
    let ledger_path = store_dir.join("sentient-spend.json");
    if ledger_path.is_file() {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let archived = v2_dir.join(format!("sentient-spend.{ts}.prev.json"));
        fs::rename(&ledger_path, &archived).expect("non-destructively archive prior spend ledger");
    }

    let password = load_or_create_password(&v2_dir);
    let wallet = load_or_create_wallet(&store_dir, &v2_dir, &password);
    let active_addr_str = wallet.active_address().expect("active_address");
    let active_addr = Address::from_str(active_addr_str).expect("parse active_address");
    assert_eq!(
        wallet.networks().active().chain_id,
        CHAIN_ID_943,
        "wallet active chain_id must be 943"
    );
    println!("=== Step 1: Throwaway Wallet Ready ===");
    println!("address: {}", active_addr.to_checksum(None));
    println!("store_dir: {}", store_dir.display());

    // Persist Enforced policy in store (max_slippage_bps = 250 so 98% min_amount_out passes 200 bps floor)
    let policy = AgentSessionPolicy {
        enforcement: EnforcementMode::Enforced,
        acknowledge_unsafe: false,
        max_position_pct: 50,
        max_approve_pct: None,
        daily_cap_pct: 50,
        allow_transfer_to: Vec::new(),
        max_slippage_bps: 250,
        max_session_gas_wei: "500000000000000000".into(),
        max_consecutive_errors: 3,
        required_rpc_quorum: 1,
    };
    vaughan_agent::sentient::save_policy(&store_dir, &policy)
        .expect("save sentient-policy.toml in store");

    // 2. Check balance on 943; claim faucet if < 1 tPLS
    let rpc_url: alloy::transports::http::reqwest::Url = RPC_943.parse().expect("parse RPC_943");
    let provider = ProviderBuilder::new().connect_http(rpc_url);
    let one_tpls = U256::from(1_000_000_000_000_000_000u128);
    let mut balance = provider
        .get_balance(active_addr)
        .await
        .expect("query initial native balance on 943");
    println!("initial_balance_wei: {balance}");

    if balance < one_tpls {
        println!("=== Step 2: Balance < 1 tPLS; claiming faucet at {FAUCET_CLAIM_URL} ===");
        let form_arg = format!("address={}", active_addr.to_checksum(None));
        let out = Command::new("curl")
            .args([
                "-s",
                "-w",
                "\n%{http_code}",
                "-X",
                "POST",
                FAUCET_CLAIM_URL,
                "-F",
                &form_arg,
            ])
            .output()
            .expect("curl must be available for faucet claim");
        let stdout_str = String::from_utf8_lossy(&out.stdout).to_string();
        let (body, code_str) = stdout_str
            .trim_end()
            .rsplit_once('\n')
            .unwrap_or((&stdout_str, "000"));
        let http_code: u16 = code_str.trim().parse().unwrap_or(0);
        println!("faucet_http_code: {http_code}");
        println!("faucet_response_body: {body}");

        if !(200..300).contains(&http_code)
            || body.to_ascii_lowercase().contains("wait")
            || body.to_ascii_lowercase().contains("cooldown")
            || body.to_ascii_lowercase().contains("limit")
            || body.to_ascii_lowercase().contains("invalid")
            || body.to_ascii_lowercase().contains("error")
        {
            panic!("FAUCET_BLOCKED: HTTP {http_code} body={body}");
        }

        let poll_deadline = Instant::now() + Duration::from_secs(90);
        loop {
            tokio::time::sleep(Duration::from_secs(3)).await;
            balance = provider
                .get_balance(active_addr)
                .await
                .expect("poll native balance on 943");
            println!("polled_balance_wei: {balance}");
            if balance >= one_tpls {
                break;
            }
            if Instant::now() >= poll_deadline {
                panic!(
                    "FAUCET_BLOCKED: balance remained {balance} (< 1 tPLS) after 90s; faucet response was: {body}"
                );
            }
        }
    } else {
        println!("=== Step 2: Balance >= 1 tPLS ({balance} wei); skipping faucet claim ===");
    }

    // 3. Spawn Signer in-process on a temp unix socket
    let pid = std::process::id();
    let ts_millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let socket_path = v2_dir.join(format!("signerd-{pid}-{ts_millis}.sock"));
    if socket_path.exists() {
        let _ = fs::remove_file(&socket_path);
    }

    let wallet_arc = Arc::new(tokio::sync::Mutex::new(wallet));
    let broadcaster = Arc::new(WalletBroadcaster::new(wallet_arc));
    let quoter = Arc::new(LiveSwapQuoter);
    let balances = Arc::new(LiveBalanceSource);
    let signer = Signer::new(
        store_dir.clone(),
        active_addr,
        CHAIN_ID_943,
        RPC_943.to_string(),
        quoter,
        broadcaster,
        balances,
    );

    let listener = UnixListener::bind(&socket_path).expect("bind temp unix socket");
    let signer_for_loop = signer.clone();
    let server_task = tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let signer = signer_for_loop.clone();
            tokio::spawn(async move {
                let (reader, mut writer) = stream.into_split();
                let mut buf = BufReader::new(reader);
                let mut line = String::new();
                if buf.read_line(&mut line).await.is_ok() {
                    let resp = signer.handle_line(&line).await + "\n";
                    let _ = writer.write_all(resp.as_bytes()).await;
                }
            });
        }
    });

    std::env::set_var(ENV_SIGNER_SOCKET, &socket_path);
    let _env_guard = EnvSocketGuard;
    let active_socket =
        PathBuf::from(std::env::var(ENV_SIGNER_SOCKET).expect("VAUGHAN_SIGNER_SOCKET must be set"));
    println!("=== Step 3: Spawned Signer on Socket ===");
    println!("socket_path: {}", active_socket.display());

    // 4. Live V2 native-in swap: 0.1 tPLS -> PLSX
    let amount_in = U256::from(100_000_000_000_000_000u64); // 0.1 tPLS
    let path = vec![WPLS_943, PLSX_943];
    let plsx_before = get_erc20_balance(&provider, PLSX_943, active_addr).await;
    println!("plsx_balance_before: {plsx_before}");

    let live_quote = quote_v2_exact_in(RPC_943, ROUTER_943, amount_in, &path)
        .await
        .expect("live getAmountsOut quote on 943 V2 router");
    assert!(
        !live_quote.amount_out.is_zero(),
        "live getAmountsOut returned zero"
    );
    let min_amount_out = (live_quote.amount_out * U256::from(98u64)) / U256::from(100u64);
    println!("live_getAmountsOut: {}", live_quote.amount_out);
    println!("computed_min_amount_out (98%): {min_amount_out}");

    let swap_req = json!({
        "v": 1,
        "op": "sign",
        "intent": {
            "type": "swap",
            "chain_id": 943,
            "family": "v2",
            "router": "0xDaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7",
            "path": [
                "0x70499adEBB11Efd915E3b69E700c331778628707",
                "0x8a810ea8B121d08342E9e7696f4a9915cBE494B7"
            ],
            "amount_in": "100000000000000000",
            "min_amount_out": min_amount_out.to_string()
        }
    });
    let swap_req_line = serde_json::to_string(&swap_req).unwrap();
    println!("=== Step 4: Submitting V2 Native-In Swap Intent ===");
    println!("request_json: {swap_req_line}");

    let (swap_resp_raw, swap_resp) = send_socket_line(&active_socket, &swap_req_line).await;
    println!("response_json: {swap_resp_raw}");
    assert!(
        swap_resp.ok,
        "V2 native-in swap failed in signerd: {swap_resp:?}"
    );

    let tx_hash_str = swap_resp
        .tx_hash
        .as_ref()
        .expect("tx_hash present in SignerResponse");
    assert!(
        tx_hash_str.starts_with("0x") && tx_hash_str.len() == 66,
        "expected 66-char 0x-prefixed tx_hash, got: {tx_hash_str}"
    );
    assert!(
        tx_hash_str[2..].chars().all(|c| c.is_ascii_hexdigit()),
        "tx_hash must be valid hex: {tx_hash_str}"
    );
    assert_ne!(
        *tx_hash_str,
        format!("0x{}", "a".repeat(64)),
        "tx_hash must not be mock placeholder"
    );
    println!("broadcast_tx_hash: {tx_hash_str}");

    // Poll eth_getTransactionReceipt up to 90s
    let receipt_deadline = Instant::now() + Duration::from_secs(90);
    let receipt_val = loop {
        let res: Value = provider
            .raw_request("eth_getTransactionReceipt".into(), json!([tx_hash_str]))
            .await
            .expect("RPC eth_getTransactionReceipt");
        if !res.is_null() {
            break res;
        }
        if Instant::now() >= receipt_deadline {
            panic!("timed out after 90s waiting for receipt of {tx_hash_str}");
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    };

    let receipt_status = receipt_val["status"].as_str().unwrap_or("");
    let block_number = receipt_val["blockNumber"].as_str().unwrap_or("");
    let gas_used = receipt_val["gasUsed"].as_str().unwrap_or("");
    let effective_gas_price = receipt_val["effectiveGasPrice"].as_str().unwrap_or("");
    let logs_count = receipt_val["logs"].as_array().map(|a| a.len()).unwrap_or(0);
    println!("=== On-Chain Receipt (Explorer Fields) ===");
    println!("transactionHash: {tx_hash_str}");
    println!("status: {receipt_status}");
    println!("blockNumber: {block_number}");
    println!("from: {}", receipt_val["from"].as_str().unwrap_or(""));
    println!("to: {}", receipt_val["to"].as_str().unwrap_or(""));
    println!("gasUsed: {gas_used}");
    println!("effectiveGasPrice: {effective_gas_price}");
    println!("logsCount: {logs_count}");
    assert_eq!(
        receipt_status, "0x1",
        "receipt status must be 0x1, got: {receipt_val}"
    );

    // Verify broadcast calldata selector (0x7ff36ab5) and value on-chain via eth_getTransactionByHash
    let tx_val: Value = provider
        .raw_request("eth_getTransactionByHash".into(), json!([tx_hash_str]))
        .await
        .expect("RPC eth_getTransactionByHash");
    let input_hex = tx_val["input"].as_str().expect("tx input field");
    let onchain_value_hex = tx_val["value"]
        .as_str()
        .expect("tx value field")
        .trim_start_matches("0x");
    let onchain_value =
        U256::from_str_radix(onchain_value_hex, 16).expect("parse on-chain tx value");
    println!("onchain_tx_value_wei: {onchain_value}");
    println!(
        "onchain_calldata_selector: {}",
        &input_hex[..10.min(input_hex.len())]
    );
    assert!(
        input_hex.starts_with("0x7ff36ab5"),
        "broadcast calldata selector must be 0x7ff36ab5 (swapExactETHForTokens), got: {input_hex}"
    );
    assert_eq!(
        onchain_value, amount_in,
        "broadcast tx value must equal amount_in"
    );

    // Verify PLSX balance increased
    let plsx_after = get_erc20_balance(&provider, PLSX_943, active_addr).await;
    println!("plsx_balance_after: {plsx_after}");
    println!(
        "plsx_balance_delta: {}",
        plsx_after.saturating_sub(plsx_before)
    );
    assert!(
        plsx_after > plsx_before,
        "PLSX balance must increase after swap: before={plsx_before}, after={plsx_after}"
    );

    // 5. Replay the identical intent: assert refusal containing spend_already_reserved
    println!("=== Step 5: Replaying Identical Intent ===");
    let (replay_raw, replay_resp) = send_socket_line(&active_socket, &swap_req_line).await;
    println!("replay_response_json: {replay_raw}");
    assert!(
        !replay_resp.ok,
        "replay of identical swap intent must be refused"
    );
    assert!(
        replay_raw.contains("spend_already_reserved"),
        "replay refusal must contain spend_already_reserved, got: {replay_raw}"
    );

    // Read <store>/sentient-spend.json directly
    let ledger_raw_1 =
        fs::read_to_string(&ledger_path).expect("read <store>/sentient-spend.json after replay");
    println!("=== Ledger JSON (<store>/sentient-spend.json) ===");
    println!("{ledger_raw_1}");
    let ledger_json_1: Value = serde_json::from_str(&ledger_raw_1).expect("parse ledger JSON");
    let asset_key = format!("{CHAIN_ID_943}:{WPLS_943:#x}");
    let spent_str = ledger_json_1["assets"][&asset_key]["spent"]
        .as_str()
        .expect("asset spent string in sentient-spend.json");
    assert_eq!(
        spent_str,
        amount_in.to_string(),
        "ledger spent must equal amount_in"
    );
    let recent_arr = ledger_json_1["recent"]
        .as_array()
        .expect("recent array in sentient-spend.json");
    assert_eq!(
        recent_arr.len(),
        1,
        "ledger must have exactly one recent entry"
    );
    assert_eq!(
        recent_arr[0]["amount"].as_str(),
        Some(amount_in.to_string().as_str()),
        "recent[0].amount must equal amount_in"
    );

    // 6. Negative test: submit a second intent with min_amount_out = "0"
    println!("=== Step 6: Submitting Negative Intent (min_amount_out = 0) ===");
    let zero_min_req = json!({
        "v": 1,
        "op": "sign",
        "intent": {
            "type": "swap",
            "chain_id": 943,
            "family": "v2",
            "router": "0xDaE9dd3d1A52CfCe9d5F2fAC7fDe164D500E50f7",
            "path": [
                "0x70499adEBB11Efd915E3b69E700c331778628707",
                "0x8a810ea8B121d08342E9e7696f4a9915cBE494B7"
            ],
            "amount_in": "100000000000000000",
            "min_amount_out": "0"
        }
    });
    let zero_min_line = serde_json::to_string(&zero_min_req).unwrap();
    let (zero_raw, zero_resp) = send_socket_line(&active_socket, &zero_min_line).await;
    println!("zero_min_response_json: {zero_raw}");
    assert!(
        !zero_resp.ok,
        "intent with min_amount_out = 0 must be refused"
    );
    assert_eq!(
        zero_resp.code.as_deref(),
        Some("zero_min_amount_out"),
        "expected zero_min_amount_out error code, got: {zero_raw}"
    );

    let ledger_raw_2 = fs::read_to_string(&ledger_path)
        .expect("read <store>/sentient-spend.json after negative test");
    let ledger_json_2: Value =
        serde_json::from_str(&ledger_raw_2).expect("parse ledger JSON after negative test");
    assert_eq!(
        ledger_json_2, ledger_json_1,
        "ledger must remain completely unchanged after zero_min_amount_out refusal"
    );
    println!("=== Ledger JSON After Negative Test (Unchanged) ===");
    println!("{ledger_raw_2}");

    server_task.abort();
    let _ = fs::remove_file(&socket_path);
}
