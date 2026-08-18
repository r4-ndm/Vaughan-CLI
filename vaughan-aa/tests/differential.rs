//! Byte-equality differential tests against Solidity-reference fixtures.
//!
//! Fixtures are captured with Solidity's own `abi.encode`, executed by the EVM
//! (a throwaway forge project under `.fixtures-capture/`, gitignored) — the
//! canonical encoder the on-chain `AmbireAccount.execute` verifies against.
//! Only the resulting byte vectors (JSON) are committed. See
//! `tests/fixtures/README.md` for the capture procedure and schema. When no
//! fixtures are present, this test skips.

use std::path::Path;
use std::str::FromStr;

use alloy::primitives::{Address, Bytes, U256};
use serde::Deserialize;

use vaughan_aa::abi::Transaction;
use vaughan_aa::encode::encode_execute;
use vaughan_aa::scw::ScwTransaction;

#[derive(Deserialize)]
struct Fixture {
    account: String,
    chain_id: u64,
    nonce: u64,
    txns: Vec<FixtureTxn>,
    /// Expected `keccak256(abi.encode(account, chainId, nonce, txns))`, `0x`-prefixed.
    digest: String,
    /// Expected `abi.encode(account, chainId, nonce, txns)` — the full digest
    /// preimage, `0x`-prefixed (captured alongside the digest).
    #[serde(default)]
    preimage: Option<String>,
    /// The signature used to pin the `execute` calldata, `0x`-prefixed.
    #[serde(default)]
    signature: Option<String>,
    /// Expected `abi.encodeCall(execute, (txns, signature))`, `0x`-prefixed.
    #[serde(default)]
    execute_calldata: Option<String>,
}

#[derive(Deserialize)]
struct FixtureTxn {
    to: String,
    /// Decimal or `0x`-prefixed.
    value: String,
    /// `0x`-prefixed.
    data: String,
}

fn parse_u256(s: &str) -> U256 {
    if let Some(hex) = s.strip_prefix("0x") {
        U256::from_str_radix(hex, 16).unwrap()
    } else {
        U256::from_str(s).unwrap()
    }
}

fn parse_bytes(s: &str) -> Vec<u8> {
    hex::decode(s.trim_start_matches("0x")).unwrap()
}

fn load_fixtures() -> Vec<(String, Fixture)> {
    let dir = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures"));
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(fixture) = serde_json::from_str::<Fixture>(&text) {
                out.push((path.display().to_string(), fixture));
            }
        }
    }
    out
}

#[test]
fn matches_solidity_reference_fixtures() {
    let fixtures = load_fixtures();
    if fixtures.is_empty() {
        eprintln!("no Solidity-reference fixtures found — skipping (see tests/fixtures/README.md)");
        return;
    }

    for (path, fixture) in fixtures {
        let tx = ScwTransaction {
            account: Address::from_str(&fixture.account).unwrap(),
            chain_id: fixture.chain_id,
            nonce: fixture.nonce,
            txns: fixture
                .txns
                .iter()
                .map(|t| Transaction {
                    to: Address::from_str(&t.to).unwrap(),
                    value: parse_u256(&t.value),
                    data: Bytes::from(parse_bytes(&t.data)),
                })
                .collect(),
        };

        // The encoded preimage must match Solidity's abi.encode byte-for-byte.
        if let Some(preimage) = &fixture.preimage {
            assert_eq!(
                tx.encode_for_digest(),
                parse_bytes(preimage),
                "preimage mismatch in fixture {path}"
            );
        }

        // The digest must match keccak256(preimage) as computed by the EVM.
        assert_eq!(
            tx.digest().as_slice(),
            parse_bytes(&fixture.digest),
            "digest mismatch in fixture {path}"
        );

        // The full execute calldata must match abi.encodeCall byte-for-byte.
        if let (Some(signature), Some(calldata)) = (&fixture.signature, &fixture.execute_calldata) {
            assert_eq!(
                encode_execute(&tx.txns, &parse_bytes(signature)),
                parse_bytes(calldata),
                "execute calldata mismatch in fixture {path}"
            );
        }
    }
}
