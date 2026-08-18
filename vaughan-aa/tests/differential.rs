//! Byte-equality differential tests against Ambire-signed fixtures.
//!
//! Fixtures are captured *outside* this workspace: Ambire's SDK is GPL-family,
//! so we run it in a separate checkout and commit only the resulting byte
//! vectors (JSON), never the SDK source. See `tests/fixtures/README.md` for the
//! capture procedure and schema. When no fixtures are present, this test skips.

use std::path::Path;
use std::str::FromStr;

use alloy::primitives::{Address, Bytes, U256};
use serde::Deserialize;

use vaughan_aa::abi::Transaction;
use vaughan_aa::scw::ScwTransaction;

#[derive(Deserialize)]
struct Fixture {
    account: String,
    chain_id: u64,
    nonce: u64,
    txns: Vec<FixtureTxn>,
    /// Expected `keccak256(abi.encode(account, chainId, nonce, txns))`, `0x`-prefixed.
    digest: String,
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
fn digest_matches_ambire_fixtures() {
    let fixtures = load_fixtures();
    if fixtures.is_empty() {
        eprintln!("no Ambire fixtures found — skipping (see tests/fixtures/README.md)");
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
                    data: Bytes::from(hex::decode(t.data.trim_start_matches("0x")).unwrap()),
                })
                .collect(),
        };
        let expected = hex::decode(fixture.digest.trim_start_matches("0x")).unwrap();
        assert_eq!(
            tx.digest().as_slice(),
            expected.as_slice(),
            "digest mismatch in fixture {path}"
        );
    }
}
