//! Live PulseChain testnet (943) stealth flow: send → announce → scan → sweep.
//!
//! Requires the canonical announcer already deployed at [`ERC5564_ANNOUNCER`].
//! Skipped in default CI. Run with:
//! ```sh
//! STEALTH_E2E_MNEMONIC="..." cargo test -p vaughan-core --test stealth_943 -- --ignored --nocapture
//! ```
//! Do not commit a mnemonic. The funded account is BIP-44 index 0 of that phrase.

use alloy::network::EthereumWallet;
use alloy::primitives::U256;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{BlockNumberOrTag, Filter, TransactionRequest};
use alloy::signers::Signer;
use bip39::Mnemonic;
use vaughan_core::security::hd_wallet::derive_account;
use vaughan_core::security::stealth::{
    announcement_topic0, check_stealth_address, compute_stealth_key, encode_announce_calldata,
    generate_stealth_address, native_announce_metadata, stealth_announcement_from_log,
    stealth_signer, StealthMetaKeys, ERC5564_ANNOUNCER,
};

const RPC: &str = "https://rpc.v4.testnet.pulsechain.com";
const CHAIN_ID: u64 = 943;
/// 0.05 tPLS — enough stipend for a sweep, leftover comes back to the EOA.
const SEND_WEI: u128 = 50_000_000_000_000_000;

#[tokio::test]
#[ignore = "live PulseChain testnet 943; export STEALTH_E2E_MNEMONIC"]
async fn send_announce_scan_sweep_on_943() {
    let phrase = std::env::var("STEALTH_E2E_MNEMONIC")
        .expect("set STEALTH_E2E_MNEMONIC to a funded 943 testnet phrase");
    let mnemonic = Mnemonic::parse(&phrase).expect("valid BIP-39 mnemonic");

    let eoa = derive_account(&mnemonic, 0)
        .expect("derive EOA")
        .with_chain_id(Some(CHAIN_ID));
    let alice = eoa.address();
    let url = RPC.parse().expect("rpc url");
    let provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(eoa))
        .connect_http(url);

    let alice_before = provider.get_balance(alice).await.expect("alice balance");
    assert!(
        alice_before > U256::from(SEND_WEI),
        "EOA {alice} needs >0.05 tPLS, has {alice_before}"
    );

    let keys = StealthMetaKeys::from_mnemonic(&mnemonic).expect("stealth keys");
    let announcement = generate_stealth_address(&keys.meta_address(), None).expect("stealth dest");
    let stealth = announcement.stealth_address;
    let send_value = U256::from(SEND_WEI);

    let from_block = provider.get_block_number().await.expect("block number");

    let send_receipt = provider
        .send_transaction(TransactionRequest::default().to(stealth).value(send_value))
        .await
        .expect("send to stealth")
        .get_receipt()
        .await
        .expect("send receipt");
    assert!(send_receipt.status(), "native send to stealth reverted");

    let metadata = native_announce_metadata(announcement.view_tag, send_value);
    let announce_receipt = provider
        .send_transaction(
            TransactionRequest::default()
                .to(ERC5564_ANNOUNCER)
                .input(encode_announce_calldata(&announcement, &metadata).into()),
        )
        .await
        .expect("announce")
        .get_receipt()
        .await
        .expect("announce receipt");
    assert!(announce_receipt.status(), "announce reverted");

    let logs = provider
        .get_logs(
            &Filter::new()
                .address(ERC5564_ANNOUNCER)
                .event_signature(announcement_topic0())
                .from_block(from_block)
                .to_block(BlockNumberOrTag::Latest),
        )
        .await
        .expect("getLogs");

    let scanned = logs
        .iter()
        .filter_map(|log| stealth_announcement_from_log(log).ok())
        .find(|got| {
            check_stealth_address(
                keys.viewing_key(),
                &keys.meta_address().spending_pubkey,
                got,
            )
            .unwrap_or(false)
                && got.stealth_address == stealth
        })
        .expect("scanned announcement matching this payment");
    assert_eq!(scanned.view_tag, announcement.view_tag);
    assert_eq!(scanned.ephemeral_pubkey, announcement.ephemeral_pubkey);

    let stealth_sk = compute_stealth_key(&keys, &scanned).expect("stealth spending key");
    let stealth_eoa = stealth_signer(stealth_sk).with_chain_id(Some(CHAIN_ID));
    assert_eq!(stealth_eoa.address(), stealth);
    let stealth_provider = ProviderBuilder::new()
        .wallet(EthereumWallet::from(stealth_eoa))
        .connect_http(RPC.parse().expect("rpc url"));

    let stealth_bal = stealth_provider
        .get_balance(stealth)
        .await
        .expect("stealth balance");
    assert_eq!(
        stealth_bal, send_value,
        "stealth should hold the send amount"
    );

    let fees = stealth_provider
        .estimate_eip1559_fees()
        .await
        .expect("eip1559 fees");
    let gas_limit = U256::from(21_000u64);
    let max_fee = U256::from(fees.max_fee_per_gas) * U256::from(2u64);
    let gas_cost = gas_limit * max_fee;
    assert!(stealth_bal > gas_cost, "stipend too small to sweep");
    let sweep_value = stealth_bal - gas_cost;

    let sweep_receipt = stealth_provider
        .send_transaction(
            TransactionRequest::default()
                .to(alice)
                .value(sweep_value)
                .max_fee_per_gas(u128::try_from(max_fee).expect("fee fits u128"))
                .max_priority_fee_per_gas(fees.max_priority_fee_per_gas)
                .gas_limit(21_000),
        )
        .await
        .expect("sweep")
        .get_receipt()
        .await
        .expect("sweep receipt");
    assert!(sweep_receipt.status(), "sweep reverted");

    let stealth_after = stealth_provider
        .get_balance(stealth)
        .await
        .expect("stealth after");
    assert!(
        stealth_after < U256::from(1_000_000_000_000_000u64),
        "stealth leftover should be < 0.001 tPLS after sweep, was {stealth_after}"
    );

    let alice_after = provider.get_balance(alice).await.expect("alice after");
    assert!(
        alice_after > alice_before - send_value,
        "alice should have recovered the stealth payload minus gas"
    );

    eprintln!(
        "943 stealth E2E ok: send={} announce={} sweep={} stealth={stealth} alice={alice}",
        send_receipt.transaction_hash,
        announce_receipt.transaction_hash,
        sweep_receipt.transaction_hash
    );
}
