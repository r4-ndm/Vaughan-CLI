//! End-to-end liquidity lifecycle against the **real PancakeSwap V3 contracts**
//! on BSC mainnet, running on an [`anvil`](https://book.getfoundry.sh/anvil/)
//! fork of the chain.
//!
//! The full lifecycle, every step through the SDK's own builders + readers:
//!
//! 1. wrap BNB → WBNB (native leg for the pool)
//! 2. swap WBNB → USDT through the real SwapRouter (second leg)
//! 3. approve the NonfungiblePositionManager for both tokens
//! 4. mint a position in a range around the current price
//! 5. decode the `IncreaseLiquidity` log for the tokenId
//! 6. read the position back via `positions(tokenId)` + `list_positions_from`
//! 7. decrease liquidity to zero, then collect — both land on-chain
//! 8. assert the token balances moved (principal back after collect)
//!
//! Run with `cargo test -p wiz4rd-sdk --test anvil_e2e -- --ignored --nocapture`.
//! Requires the `anvil` binary (foundry) and network access to a BSC RPC.

use std::process::{Child, Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use alloy::network::EthereumWallet;
use alloy::primitives::{Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::{SolCall, SolEvent};

use wiz4rd_sdk::abi::{IERC20Minimal, INonfungiblePositionManager};
use wiz4rd_sdk::config::Config;
use wiz4rd_sdk::tx::liquidity::{build_collect_tx, build_decrease_liquidity_tx, build_mint_tx};
use wiz4rd_sdk::tx::swap::{
    apply_slippage, apply_slippage_up, build_swap_exact_in, build_swap_exact_out,
};

// ---- BSC reference deployment (the fork's chain) ----
const WBNB: &str = "0xbb4CdB9CBd36B01bD1cBaEBF2De08d9173bc095c";
const USDT: &str = "0x55d398326f99059fF775485246999027B3197955";
const DOGE: &str = "0xbA2aE424d960c26247Dd6c32edC70B295c744C43";
const FACTORY: &str = "0x0BFbCF9fa4f9C56B0F40a671Ad40E0805A091865";
const ROUTER: &str = "0x1b81D678ffb9C0263b24A97847620C99d213eB14"; // v3 SwapRouter
const NPM: &str = "0x46A15B0b27311cedF172AB29E4f4766fbE7F4364";
const FORK_RPC: &str = "https://bsc-dataseed.binance.org";

/// anvil's first dev account: pre-funded on forks, deterministic key.
const ANVIL_KEY: &str = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// Tests in one binary run concurrently in the same process, so a port derived
/// from the pid alone would collide — add a per-spawn counter.
static PORT_COUNTER: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(0);

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

fn spawn_anvil() -> Anvil {
    let port = 18546
        + (std::process::id() % 137) as u16
        + PORT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst) * 2;
    let url = format!("http://127.0.0.1:{port}");
    let child = Command::new("anvil")
        .args([
            "--fork-url",
            FORK_RPC,
            "--port",
            &port.to_string(),
            "--silent",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn anvil — install foundry (https://book.getfoundry.sh)");
    Anvil { child, url }
}

/// Send a prebuilt tx with the wallet provider and return the receipt.
async fn send_receipt(
    wp: &impl Provider,
    tx: TransactionRequest,
    step: &str,
) -> alloy::rpc::types::TransactionReceipt {
    let pending = wp.send_transaction(tx).await.unwrap_or_else(|e| panic!("{step}: broadcast failed: {e}"));
    let receipt = pending.get_receipt().await.unwrap_or_else(|e| panic!("{step}: no receipt: {e}"));
    assert!(receipt.status(), "{step}: tx reverted: {}", receipt.transaction_hash);
    eprintln!("{step}: ok ({})", receipt.transaction_hash);
    receipt
}

/// `n` whole units of an 18-decimal token (e.g. `units(1) = 1e18`).
/// Avoids the `x.pow(18)` footgun in raw literals.
fn units(n: u128) -> U256 {
    U256::from(n) * U256::from(10u128.pow(18))
}

async fn erc20_balance(provider: &impl Provider, token: Address, who: Address) -> U256 {
    let call = IERC20Minimal::balanceOfCall { account: who };
    let raw = provider
        .call(TransactionRequest::default().to(token).input(call.abi_encode().into()))
        .await
        .expect("balanceOf failed");
    IERC20Minimal::balanceOfCall::abi_decode_returns(&raw).expect("bad balanceOf return")
}

#[tokio::test]
#[ignore = "requires anvil (foundry) + network fork of BSC mainnet"]
async fn liquidity_lifecycle_on_bsc_fork() {
    let anvil = spawn_anvil();

    let cfg = Config {
        chain_id: 56,
        rpc_url: Some(anvil.url.clone()),
        factory: Some(FACTORY.parse().unwrap()),
        swap_router: Some(ROUTER.parse().unwrap()),
        position_manager: Some(NPM.parse().unwrap()),
        protocol_fee: 0,
        vaughan_provider: None,
        vaughan_origin: None,
    };
    let provider = cfg.provider().expect("read provider");

    // Wait for anvil to accept RPC (fork sync is lazy, so this is fast).
    let mut ready = false;
    for _ in 0..60 {
        if provider.get_chain_id().await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(ready, "anvil did not come up on {}", anvil.url);

    let signer: PrivateKeySigner = ANVIL_KEY.parse().expect("anvil key");
    let me = signer.address();
    let wp = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(anvil.url.parse().unwrap());

    let wbnb: Address = WBNB.parse().unwrap();
    let usdt: Address = USDT.parse().unwrap();
    let router: Address = ROUTER.parse().unwrap();
    let npm: Address = NPM.parse().unwrap();
    let deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;

    // ---- 1. Wrap BNB → WBNB (WBNB.deposit(), selector 0xd0e30db0) ----
    let deposit = TransactionRequest::default()
        .to(wbnb)
        .value(units(1))
        .input(vec![0xd0, 0xe3, 0x0d, 0xb0].into());
    send_receipt(&wp, deposit, "wrap").await;
    let wbnb_after_wrap = erc20_balance(&provider, wbnb, me).await;
    assert_eq!(wbnb_after_wrap, units(1), "wrapped exactly 1 BNB");

    // ---- 2. Swap 0.1 WBNB → USDT through the real router ----
    let pool = wiz4rd_sdk::pool::get_pool_info_for_tokens(&provider, &cfg, wbnb, usdt, 500)
        .await
        .expect("WBNB/USDT 500 pool exists on the fork");
    assert_eq!(pool.token0, usdt, "USDT sorts before WBNB");
    assert_eq!(pool.token1, wbnb);

    let swap_in = units(1) / U256::from(10); // 0.1 WBNB
    // The router pulls WBNB from us in the swap callback — approve it first.
    for tx in wiz4rd_sdk::allowance::ensure_allowance_txs(&provider, wbnb, me, router, swap_in)
        .await
        .expect("router allowance plan")
    {
        send_receipt(&wp, tx, "approve router WBNB").await;
    }
    let quote = wiz4rd_sdk::quote_exact_in(&pool, swap_in, true)
        .expect("exact-in quote");
    let swap_tx = build_swap_exact_in(&cfg, &pool, wbnb, swap_in, U256::ZERO, me, deadline, None)
        .expect("swap builder");
    send_receipt(&wp, swap_tx, "swap").await;
    let usdt_after_swap = erc20_balance(&provider, usdt, me).await;
    assert!(usdt_after_swap > U256::ZERO, "received USDT from the swap");
    assert!(
        usdt_after_swap >= quote.amount_out * U256::from(999) / U256::from(1000),
        "received within ~0.1% of the quoted output"
    );

    // ---- 3. Approve the NPM for both legs ----
    let wbnb_remaining = units(1) - swap_in; // 0.9 WBNB left after the swap
    let usdt_for_mint = units(30); // 30 USDT, ~0.05 WBNB at ~600
    for (token, amount) in [(wbnb, wbnb_remaining), (usdt, usdt_for_mint)] {
        let txs = wiz4rd_sdk::allowance::ensure_allowance_txs(&provider, token, me, npm, amount)
            .await
            .expect("allowance plan");
        for tx in txs {
            send_receipt(&wp, tx, &format!("approve {token}")).await;
        }
        let allowance = wiz4rd_sdk::allowance::get_allowance(&provider, token, me, npm)
            .await
            .expect("allowance read");
        assert!(allowance >= amount, "NPM approved for {token}");
    }

    // ---- 4. Mint a position in a ±100-tick band around the current price ----
    let spacing = 10; // fee 500 → tick spacing 10
    let tick = pool.tick;
    let tick_lower = (tick / spacing - 10) * spacing;
    let tick_upper = (tick / spacing + 10) * spacing;
    assert!(tick_lower < tick_upper);

    let wbnb_for_mint = units(1) / U256::from(20); // 0.05 WBNB
    let (amount0_desired, amount1_desired) = (usdt_for_mint, wbnb_for_mint); // token0 = USDT
    let mint_tx = build_mint_tx(
        &cfg,
        pool.token0,
        pool.token1,
        500,
        tick_lower,
        tick_upper,
        amount0_desired,
        amount1_desired,
        U256::ZERO,
        U256::ZERO,
        me,
        deadline,
    )
    .expect("mint builder");
    let mint_receipt = send_receipt(&wp, mint_tx, "mint").await;

    // ---- 5. Decode the IncreaseLiquidity log for the tokenId ----
    let inc = mint_receipt
        .logs()
        .iter()
        .find_map(|log| {
            // RPC logs wrap the primitives `Log`; decode against the inner one.
            INonfungiblePositionManager::IncreaseLiquidity::decode_log_validate(&log.inner).ok()
        })
        .expect("IncreaseLiquidity log in mint receipt");
    let token_id = inc.tokenId;
    let minted_liquidity = inc.liquidity;
    assert!(!token_id.is_zero(), "mint returned a tokenId");

    // ---- 6. Read the position back ----
    let pos = wiz4rd_sdk::positions::get_position(&provider, &cfg, token_id)
        .await
        .expect("positions(tokenId)");
    assert_eq!(pos.token0, pool.token0);
    assert_eq!(pos.token1, pool.token1);
    assert_eq!(pos.fee, 500);
    assert_eq!(pos.tick_lower, tick_lower);
    assert_eq!(pos.tick_upper, tick_upper);
    assert_eq!(pos.liquidity, minted_liquidity, "liquidity persisted");

    // list_positions_from must find it (anvil serves getLogs for its own blocks).
    let mint_block = mint_receipt.block_number.expect("block number");
    let owned = wiz4rd_sdk::positions::list_positions_from(&provider, &cfg, me, Some(mint_block), None)
        .await
        .expect("list positions");
    assert!(
        owned.iter().any(|p| p.token_id == token_id),
        "position {} appears in the owner's list",
        token_id
    );

    // ---- 7. Decrease to zero, then collect ----
    let wbnb_before_collect = erc20_balance(&provider, wbnb, me).await;
    let usdt_before_collect = erc20_balance(&provider, usdt, me).await;

    let dec_tx = build_decrease_liquidity_tx(
        &cfg,
        token_id,
        pos.liquidity,
        U256::ZERO,
        U256::ZERO,
        deadline,
    )
    .expect("decrease builder");
    send_receipt(&wp, dec_tx, "decrease").await;

    let collect_tx = build_collect_tx(&cfg, token_id, me, u128::MAX, u128::MAX)
        .expect("collect builder");
    send_receipt(&wp, collect_tx, "collect").await;

    // ---- 8. Principal is back (minus swap fees earned/lost on the range) ----
    let wbnb_after_collect = erc20_balance(&provider, wbnb, me).await;
    let usdt_after_collect = erc20_balance(&provider, usdt, me).await;
    assert!(
        wbnb_after_collect > wbnb_before_collect,
        "WBNB principal returned: {} → {}",
        wbnb_before_collect,
        wbnb_after_collect
    );
    assert!(
        usdt_after_collect > usdt_before_collect,
        "USDT principal returned: {} → {}",
        usdt_before_collect,
        usdt_after_collect
    );

    // Position is now empty of liquidity.
    let drained = wiz4rd_sdk::positions::get_position(&provider, &cfg, token_id)
        .await
        .expect("positions(tokenId) after drain");
    assert_eq!(drained.liquidity, 0, "liquidity fully removed");
}

/// The slippage bounds the SDK computes off-chain must actually hold when the
/// swaps execute against the real contracts:
///
/// - **exact-in**: received `>= amountOutMinimum` (99.5% of the quote), and
///   close to the quote itself
/// - **exact-out**: spent `<= amountInMaximum` (100.5% of the quote), and the
///   recipient gets *exactly* the requested output
/// - **guard**: an unreachable `amountOutMinimum` reverts the tx on-chain
///   ("Too little received"), proving the bound is enforced, not advisory
#[tokio::test]
#[ignore = "requires anvil (foundry) + network fork of BSC mainnet"]
async fn swap_bounds_hold_on_bsc_fork() {
    let anvil = spawn_anvil();
    let cfg = Config {
        chain_id: 56,
        rpc_url: Some(anvil.url.clone()),
        factory: Some(FACTORY.parse().unwrap()),
        swap_router: Some(ROUTER.parse().unwrap()),
        position_manager: Some(NPM.parse().unwrap()),
        protocol_fee: 0,
        vaughan_provider: None,
        vaughan_origin: None,
    };
    let provider = cfg.provider().expect("read provider");

    let mut ready = false;
    for _ in 0..60 {
        if provider.get_chain_id().await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(ready, "anvil did not come up on {}", anvil.url);

    let signer: PrivateKeySigner = ANVIL_KEY.parse().expect("anvil key");
    let me = signer.address();
    let wp = ProviderBuilder::new()
        .wallet(EthereumWallet::from(signer))
        .connect_http(anvil.url.parse().unwrap());

    let wbnb: Address = WBNB.parse().unwrap();
    let usdt: Address = USDT.parse().unwrap();
    let router: Address = ROUTER.parse().unwrap();
    let deadline = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;

    // Wrap 1 BNB and approve the router for the whole budget up front (covers
    // both swaps; the revert probe spends nothing).
    let deposit = TransactionRequest::default()
        .to(wbnb)
        .value(units(1))
        .input(vec![0xd0, 0xe3, 0x0d, 0xb0].into());
    send_receipt(&wp, deposit, "wrap").await;
    for tx in wiz4rd_sdk::allowance::ensure_allowance_txs(&provider, wbnb, me, router, units(1))
        .await
        .expect("router allowance plan")
    {
        send_receipt(&wp, tx, "approve router WBNB").await;
    }

    let pool = wiz4rd_sdk::pool::get_pool_info_for_tokens(&provider, &cfg, wbnb, usdt, 500)
        .await
        .expect("WBNB/USDT 500 pool");
    assert_eq!(pool.token1, wbnb, "WBNB is token1, so both swaps are one-for-zero");

    // ---- exact-in: sell 0.05 WBNB, receive USDT ----
    let in_amount = units(1) / U256::from(20); // 0.05 WBNB
    let quote = wiz4rd_sdk::quote_exact_in(&pool, in_amount, false)
        .expect("exact-in quote");
    let min_out = apply_slippage(quote.amount_out, 50); // 0.5% tolerance
    assert!(min_out < quote.amount_out);

    let usdt_before = erc20_balance(&provider, usdt, me).await;
    let tx = build_swap_exact_in(&cfg, &pool, wbnb, in_amount, min_out, me, deadline, None)
        .expect("exact-in builder");
    send_receipt(&wp, tx, "swap exact-in").await;
    let actual_out = erc20_balance(&provider, usdt, me).await - usdt_before;

    assert!(actual_out >= min_out, "exact-in: got {actual_out} USDT < min {min_out}");
    assert!(
        actual_out <= quote.amount_out,
        "exact-in: got {actual_out} > quote {}",
        quote.amount_out
    );
    assert!(
        actual_out >= quote.amount_out * U256::from(999) / U256::from(1000),
        "exact-in: quote drifted by >0.1% ({} vs {})",
        actual_out,
        quote.amount_out
    );

    // ---- exact-out: buy exactly 20 USDT with WBNB ----
    let out_amount = units(20);
    let quote = wiz4rd_sdk::quote_exact_out(&pool, out_amount, false)
        .expect("exact-out quote");
    let max_in = apply_slippage_up(quote.amount_in, 50); // 0.5% tolerance
    assert!(max_in > quote.amount_in);

    let wbnb_before = erc20_balance(&provider, wbnb, me).await;
    let usdt_before = erc20_balance(&provider, usdt, me).await;
    let tx = build_swap_exact_out(&cfg, &pool, wbnb, out_amount, max_in, me, deadline, None)
        .expect("exact-out builder");
    send_receipt(&wp, tx, "swap exact-out").await;
    let actual_in = wbnb_before - erc20_balance(&provider, wbnb, me).await;
    let usdt_delta = erc20_balance(&provider, usdt, me).await - usdt_before;

    assert!(actual_in <= max_in, "exact-out: spent {actual_in} WBNB > max {max_in}");
    assert!(
        actual_in <= quote.amount_in * U256::from(1001) / U256::from(1000),
        "exact-out: spent {actual_in} > 0.1% over required {}",
        quote.amount_in
    );
    assert_eq!(usdt_delta, out_amount, "exact-out: received exactly the requested USDT");

    // ---- guard: an unreachable minimum reverts on-chain ----
    // Re-quote at the *current* pool state (the exact-out swap above moved the
    // price); anvil gives us exact execution, so 1% above the quote must revert.
    let pool = wiz4rd_sdk::pool::get_pool_info_for_tokens(&provider, &cfg, wbnb, usdt, 500)
        .await
        .expect("WBNB/USDT 500 pool");
    let quote = wiz4rd_sdk::quote_exact_in(&pool, in_amount, false).expect("fresh exact-in quote");
    let over_min = quote.amount_out * U256::from(101) / U256::from(100); // 1% above quote
    let tx = build_swap_exact_in(&cfg, &pool, wbnb, in_amount, over_min, me, deadline, None)
        .expect("over-min builder");
    // anvil executes txs synchronously on send, so a revert surfaces as a
    // broadcast error rather than a receipt with status 0. Accept either.
    match wp.send_transaction(tx).await {
        Ok(pending) => {
            let receipt = pending.get_receipt().await.expect("receipt");
            assert!(
                !receipt.status(),
                "over-generous amountOutMinimum must revert on-chain ({})",
                receipt.transaction_hash
            );
        }
        Err(e) => {
            let text = format!("{e:#}");
            assert!(
                text.contains("revert") || text.contains("Too little received"),
                "expected revert, got: {text}"
            );
        }
    }
}

/// The CLI's default `--max-price-impact` (5%). The swap command hard-stops
/// when the quoted impact exceeds this unless `--yes` is passed.
const DEFAULT_MAX_PRICE_IMPACT: f64 = 5.0;

#[tokio::test]
#[ignore = "requires anvil (foundry) + network fork of BSC mainnet"]
async fn price_impact_hard_stop_on_bsc_fork() {
    let anvil = spawn_anvil();

    let cfg = Config {
        chain_id: 56,
        rpc_url: Some(anvil.url.clone()),
        factory: Some(FACTORY.parse().unwrap()),
        swap_router: Some(ROUTER.parse().unwrap()),
        position_manager: Some(NPM.parse().unwrap()),
        protocol_fee: 0,
        vaughan_provider: None,
        vaughan_origin: None,
    };
    let provider = cfg.provider().expect("read provider");

    let mut ready = false;
    for _ in 0..60 {
        if provider.get_chain_id().await.is_ok() {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(ready, "anvil did not come up on {}", anvil.url);

    let wbnb: Address = WBNB.parse().unwrap();
    let usdt: Address = USDT.parse().unwrap();
    let doge: Address = DOGE.parse().unwrap();

    // Both pools trade WBNB against a token that sorts before it, so both
    // swaps are the same direction (sell WBNB, one-for-zero) — apples to
    // apples.
    let deep = wiz4rd_sdk::pool::get_pool_info_for_tokens(&provider, &cfg, wbnb, usdt, 500)
        .await
        .expect("WBNB/USDT 500 pool");
    let shallow = wiz4rd_sdk::pool::get_pool_info_for_tokens(&provider, &cfg, wbnb, doge, 500)
        .await
        .expect("WBNB/DOGE 500 pool");
    assert_eq!(deep.token1, wbnb, "USDT sorts before WBNB");
    assert_eq!(shallow.token1, wbnb, "DOGE sorts before WBNB");

    // Deep pool must be orders of magnitude deeper than the shallow one, or
    // the rest of the test is meaningless.
    assert!(
        deep.liquidity > shallow.liquidity * 10_000,
        "expected a deep pool ({}) vs a shallow pool ({})",
        deep.liquidity,
        shallow.liquidity
    );

    // ---- same swap size, both pools: the 5% hard-stop must split them ----
    let normal = units(1) / U256::from(20); // 0.05 WBNB
    let deep_q = wiz4rd_sdk::quote_exact_in(&deep, normal, false).expect("deep quote");
    let shallow_q = wiz4rd_sdk::quote_exact_in(&shallow, normal, false).expect("shallow quote");
    let deep_impact = wiz4rd_sdk::price_impact_pct(&deep, deep_q.amount_in, deep_q.amount_out, false);
    let shallow_impact =
        wiz4rd_sdk::price_impact_pct(&shallow, shallow_q.amount_in, shallow_q.amount_out, false);

    eprintln!("deep pool liquidity   : {}", deep.liquidity);
    eprintln!("shallow pool liquidity: {}", shallow.liquidity);
    eprintln!("0.05 WBNB on deep    : {deep_impact:.4}% impact");
    eprintln!("0.05 WBNB on shallow : {shallow_impact:.2}% impact");

    assert!(
        deep_impact < DEFAULT_MAX_PRICE_IMPACT,
        "deep pool must NOT hard-stop: impact {deep_impact:.4}% >= 5%"
    );
    assert!(
        deep_impact < 1.0,
        "deep pool impact should be tiny: {deep_impact:.4}%"
    );
    assert!(
        shallow_impact > DEFAULT_MAX_PRICE_IMPACT,
        "shallow pool must hard-stop: impact {shallow_impact:.2}% <= 5%"
    );

    // ---- size also matters: a huge swap on the *deep* pool trips the stop ----
    let huge = units(5_000); // 5,000 WBNB
    let huge_q = wiz4rd_sdk::quote_exact_in(&deep, huge, false).expect("huge deep quote");
    let huge_impact = wiz4rd_sdk::price_impact_pct(&deep, huge_q.amount_in, huge_q.amount_out, false);
    eprintln!("5,000 WBNB on deep   : {huge_impact:.2}% impact");
    assert!(
        huge_impact > DEFAULT_MAX_PRICE_IMPACT,
        "huge swap on the deep pool must also hard-stop: {huge_impact:.2}%"
    );

    // ---- the CLI's decision rule (swap.rs hard_stopped) applied to real
    //      on-chain numbers ----
    assert!(!(deep_impact > DEFAULT_MAX_PRICE_IMPACT), "deep swap must proceed");
    assert!(
        shallow_impact > DEFAULT_MAX_PRICE_IMPACT,
        "shallow swap must be refused by the CLI"
    );
}
