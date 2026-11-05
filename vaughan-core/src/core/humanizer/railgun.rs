//! RAILGUN `shield` / `transact` decoding.
//!
//! Shielding moves public tokens into encrypted notes; `transact` spends notes
//! (private transfer and/or unshield to a public address). Note recipients
//! are encrypted, so a card can say what moves but not to whom, except for
//! unshields, whose recipient is public.
//!
//! The ABI matches the deployed `RailgunSmartWallet` (enums encode as
//! `uint8`); `vaughan-railgun` tests decode railgun-rust's own calldata with
//! it, and check [`railgun_contracts`] against its deployment table.

use alloy::primitives::{address, Address, U256};
use alloy::sol;
use alloy::sol_types::SolCall;

use super::labels::label_address;
use super::{HumanizeContext, Humanized, HumanizerModule, Warning};
use crate::core::proposal_verify::VerifyRow;
use crate::core::transaction::format_display_amount;

sol! {
    interface IRailgun {
        struct G1Point { uint256 x; uint256 y; }
        struct G2Point { uint256[2] x; uint256[2] y; }
        struct SnarkProof { G1Point a; G2Point b; G1Point c; }
        struct CommitmentCiphertext {
            bytes32[4] ciphertext;
            bytes32 blindedSenderViewingKey;
            bytes32 blindedReceiverViewingKey;
            bytes annotationData;
            bytes memo;
        }
        struct BoundParams {
            uint16 treeNumber;
            uint72 minGasPrice;
            uint8 unshield;
            uint64 chainID;
            address adaptContract;
            bytes32 adaptParams;
            CommitmentCiphertext[] commitmentCiphertext;
        }
        struct TokenData { uint8 tokenType; address tokenAddress; uint256 tokenSubID; }
        struct CommitmentPreimage { bytes32 npk; TokenData token; uint120 value; }
        struct Transaction {
            SnarkProof proof;
            bytes32 merkleRoot;
            bytes32[] nullifiers;
            bytes32[] commitments;
            BoundParams boundParams;
            CommitmentPreimage unshieldPreimage;
        }
        struct ShieldCiphertext { bytes32[3] encryptedBundle; bytes32 shieldKey; }
        struct ShieldRequest { CommitmentPreimage preimage; ShieldCiphertext ciphertext; }

        struct Call {
            address to;
            bytes data;
            uint256 value;
        }

        function shield(ShieldRequest[] _shieldRequests) external payable;
        function transact(Transaction[] _transactions) external payable;
        function multicall(bool _requireSuccess, Call[] _calls) external payable;
        function wrapBase(uint256 _amount) external;
    }
}

/// `(chain id, RAILGUN proxy, current RelayAdapt)` for supported chains.
const CONTRACTS: &[(u64, Address, Address)] = &[
    (
        1,
        address!("0xfa7093cdd9ee6932b4eb2c9e1cde7ce00b1fa4b9"),
        address!("0xAc9f360Ae85469B27aEDdEaFC579Ef2d052aD405"),
    ),
    (
        56,
        address!("0x590162bf4b50f6576a459b75309ee21d92178a10"),
        address!("0xf82d00fc51f730f42a00f85e74895a2849fff2dd"),
    ),
    (
        137,
        address!("0x19b620929f97b7b990801496c3b361ca5def8c71"),
        address!("0xF82d00fC51F730F42A00F85E74895a2849ffF2Dd"),
    ),
    (
        42161,
        address!("0xfa7093cdd9ee6932b4eb2c9e1cde7ce00b1fa4b9"),
        address!("0xB4F2d77bD12c6b548Ae398244d7FAD4ABCE4D89b"),
    ),
    (
        11_155_111,
        address!("0xeCFCf3b4eC647c4Ca6D49108b311b7a7C9543fea"),
        address!("0x7e3d929EbD5bDC84d02Bd3205c777578f33A214D"),
    ),
];

/// RAILGUN proxy and current RelayAdapt on `chain_id`, if supported.
pub fn railgun_contracts(chain_id: u64) -> Option<(Address, Address)> {
    CONTRACTS
        .iter()
        .find(|(id, ..)| *id == chain_id)
        .map(|&(_, proxy, relay)| (proxy, relay))
}

/// Label for a RAILGUN contract on `chain_id`.
pub(super) fn contract_label(chain_id: u64, address: Address) -> Option<&'static str> {
    let (proxy, relay) = railgun_contracts(chain_id)?;
    if address == proxy {
        Some("RAILGUN")
    } else if address == relay {
        Some("RAILGUN RelayAdapt")
    } else {
        None
    }
}

const SELF_BROADCAST: &str =
    "Sent from your public address, this reveals that you paid for a private RAILGUN transaction";

pub struct Railgun;

impl HumanizerModule for Railgun {
    fn name(&self) -> &'static str {
        "railgun"
    }

    fn try_humanize(
        &self,
        ctx: &HumanizeContext<'_>,
        to: Address,
        value: U256,
        calldata: &[u8],
    ) -> Option<Humanized> {
        if let Ok(c) = IRailgun::shieldCall::abi_decode(calldata) {
            return Some(shield(ctx, to, value, &c._shieldRequests));
        }
        if let Ok(c) = IRailgun::transactCall::abi_decode(calldata) {
            return Some(transact(ctx, to, &c._transactions));
        }
        if let Ok(m) = IRailgun::multicallCall::abi_decode(calldata) {
            return multicall_shield(ctx, to, value, &m._calls);
        }
        None
    }
}

fn shield(
    ctx: &HumanizeContext<'_>,
    to: Address,
    value: U256,
    requests: &[IRailgun::ShieldRequest],
) -> Humanized {
    let mut rows = vec![row("Action", "Shield into RAILGUN (private balance)")];
    for r in requests {
        let (token, amount) = token_amount(ctx, &r.preimage);
        rows.push(row("Token", &token));
        rows.push(row("Amount (raw)", &amount));
    }
    rows.push(row(
        "Recipient",
        "encrypted 0zk note (not readable from calldata)",
    ));
    rows.push(row("Contract", &label_address(ctx.chain_id, to)));

    let summary = match requests {
        [one] => {
            let (token, amount) = token_amount(ctx, &one.preimage);
            format!("Shield {amount} {token} into RAILGUN")
        }
        many => format!("Shield {} tokens into RAILGUN", many.len()),
    };
    let mut warnings = common_warnings(ctx, to);
    warnings.push(Warning::Advisory(
        "RAILGUN deducts its shield fee from the shielded amount".into(),
    ));
    if !value.is_zero() {
        warnings.push(Warning::Advisory(format!(
            "{} {} sent along is not shielded by this call",
            format_display_amount(&value.to_string(), ctx.native_decimals, 8),
            ctx.native_symbol
        )));
    }
    Humanized {
        summary,
        rows,
        warnings,
    }
}

fn multicall_shield(
    ctx: &HumanizeContext<'_>,
    to: Address,
    value: U256,
    calls: &[IRailgun::Call],
) -> Option<Humanized> {
    let mut requests = Vec::new();
    let mut has_wrap = false;
    for call in calls {
        if let Ok(c) = IRailgun::shieldCall::abi_decode(&call.data) {
            requests.extend(c._shieldRequests);
        }
        if IRailgun::wrapBaseCall::abi_decode(&call.data).is_ok() {
            has_wrap = true;
        }
    }
    if requests.is_empty() {
        return None;
    }

    let action = if has_wrap {
        format!(
            "Wrap and Shield {} into RAILGUN (private balance)",
            ctx.native_symbol
        )
    } else {
        "Shield into RAILGUN via RelayAdapt (private balance)".into()
    };

    let mut rows = vec![row("Action", &action)];
    if has_wrap && !value.is_zero() {
        rows.push(row(
            "Native Amount",
            &format!(
                "{} {}",
                format_display_amount(&value.to_string(), ctx.native_decimals, 8),
                ctx.native_symbol
            ),
        ));
    }
    for r in &requests {
        let (token, amount) = token_amount(ctx, &r.preimage);
        rows.push(row("Token", &token));
        rows.push(row("Amount (raw)", &amount));
    }
    rows.push(row(
        "Recipient",
        "encrypted 0zk note (not readable from calldata)",
    ));
    rows.push(row("Contract", &label_address(ctx.chain_id, to)));

    let summary = if has_wrap {
        let amt_str = format_display_amount(&value.to_string(), ctx.native_decimals, 6);
        format!(
            "Wrap and Shield {amt_str} {} into RAILGUN",
            ctx.native_symbol
        )
    } else {
        match requests.as_slice() {
            [one] => {
                let (token, amount) = token_amount(ctx, &one.preimage);
                format!("Shield {amount} {token} into RAILGUN")
            }
            many => format!("Shield {} tokens into RAILGUN", many.len()),
        }
    };

    let mut warnings = common_warnings(ctx, to);
    warnings.push(Warning::Advisory(
        "RAILGUN deducts its shield fee from the shielded amount".into(),
    ));
    Some(Humanized {
        summary,
        rows,
        warnings,
    })
}

fn transact(
    ctx: &HumanizeContext<'_>,
    to: Address,
    transactions: &[IRailgun::Transaction],
) -> Humanized {
    let mut rows = vec![row("Action", "Private RAILGUN transaction")];
    let mut warnings = common_warnings(ctx, to);
    let mut unshields = Vec::new();

    for tx in transactions {
        let bound = &tx.boundParams;
        if bound.chainID != ctx.chain_id {
            warnings.push(Warning::Advisory(format!(
                "Proof is bound to chain {}, not this chain ({}); it will fail",
                bound.chainID, ctx.chain_id
            )));
        }
        if bound.adaptContract != Address::ZERO {
            rows.push(row(
                "Via",
                &label_address(ctx.chain_id, bound.adaptContract),
            ));
        }
        if bound.unshield != 0 {
            let recipient = Address::from_slice(&tx.unshieldPreimage.npk[12..]);
            let (token, amount) = token_amount(ctx, &tx.unshieldPreimage);
            rows.push(row("Unshield", &format!("{amount} {token}")));
            rows.push(row("To", &format!("{recipient:#x}")));
            if ctx.from.is_some_and(|from| from != recipient) {
                warnings.push(Warning::Advisory(format!(
                    "Unshielded funds go to {recipient:#x}, not your wallet"
                )));
            }
            unshields.push((amount, token, recipient));
        }
    }
    let private_outputs: usize = transactions
        .iter()
        .map(|t| t.commitments.len())
        .sum::<usize>()
        .saturating_sub(unshields.len());
    rows.push(row(
        "Private outputs",
        &format!("{private_outputs} (amounts and recipients encrypted)"),
    ));

    let summary = match unshields.as_slice() {
        [] => "Private RAILGUN transfer (amounts and recipients encrypted)".to_string(),
        [(amount, token, recipient)] => format!(
            "Unshield {amount} {token} from RAILGUN to {}",
            label_address(ctx.chain_id, *recipient)
        ),
        many => format!("RAILGUN transaction with {} unshields", many.len()),
    };
    warnings.push(Warning::Advisory(SELF_BROADCAST.into()));
    Humanized {
        summary,
        rows,
        warnings,
    }
}

fn common_warnings(ctx: &HumanizeContext<'_>, to: Address) -> Vec<Warning> {
    if contract_label(ctx.chain_id, to) == Some("RAILGUN") {
        return Vec::new();
    }
    vec![Warning::Advisory(format!(
        "{to:#x} is not the RAILGUN contract on this chain — do not sign unless you expected this"
    ))]
}

/// Token label and raw amount; NFTs show their token id instead of decimals.
fn token_amount(ctx: &HumanizeContext<'_>, p: &IRailgun::CommitmentPreimage) -> (String, String) {
    let token = label_address(ctx.chain_id, p.token.tokenAddress);
    match p.token.tokenType {
        0 => (token, p.value.to_string()),
        1 => (format!("{token} NFT #{}", p.token.tokenSubID), "1".into()),
        _ => (
            format!("{token} #{}", p.token.tokenSubID),
            p.value.to_string(),
        ),
    }
}

fn row(label: &str, value: &str) -> VerifyRow {
    VerifyRow {
        label: label.into(),
        value: value.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{aliases::U120, B256};

    const SEPOLIA: u64 = 11_155_111;
    const ME: Address = address!("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266");
    const WETH: Address = address!("0xfff9976782d46cc05630d1f6ebab18b2324d6b14");

    fn ctx() -> HumanizeContext<'static> {
        HumanizeContext {
            chain_id: SEPOLIA,
            from: Some(ME),
            native_symbol: "ETH",
            native_decimals: 18,
        }
    }

    fn proxy() -> Address {
        railgun_contracts(SEPOLIA).unwrap().0
    }

    fn preimage(npk: B256, value: u64) -> IRailgun::CommitmentPreimage {
        IRailgun::CommitmentPreimage {
            npk,
            token: IRailgun::TokenData {
                tokenType: 0,
                tokenAddress: WETH,
                tokenSubID: U256::ZERO,
            },
            value: U120::from(value),
        }
    }

    fn shield_data(value: u64) -> Vec<u8> {
        IRailgun::shieldCall {
            _shieldRequests: vec![IRailgun::ShieldRequest {
                preimage: preimage(B256::repeat_byte(1), value),
                ciphertext: IRailgun::ShieldCiphertext {
                    encryptedBundle: [B256::ZERO; 3],
                    shieldKey: B256::ZERO,
                },
            }],
        }
        .abi_encode()
    }

    fn transaction(unshield_to: Option<Address>, chain_id: u64) -> IRailgun::Transaction {
        let npk = unshield_to.map_or(B256::repeat_byte(2), |a| a.into_word());
        IRailgun::Transaction {
            proof: IRailgun::SnarkProof {
                a: IRailgun::G1Point {
                    x: U256::ZERO,
                    y: U256::ZERO,
                },
                b: IRailgun::G2Point {
                    x: [U256::ZERO; 2],
                    y: [U256::ZERO; 2],
                },
                c: IRailgun::G1Point {
                    x: U256::ZERO,
                    y: U256::ZERO,
                },
            },
            merkleRoot: B256::ZERO,
            nullifiers: vec![B256::repeat_byte(3)],
            commitments: vec![B256::repeat_byte(4), B256::repeat_byte(5)],
            boundParams: IRailgun::BoundParams {
                treeNumber: 0,
                minGasPrice: Default::default(),
                unshield: u8::from(unshield_to.is_some()),
                chainID: chain_id,
                adaptContract: Address::ZERO,
                adaptParams: B256::ZERO,
                commitmentCiphertext: Vec::new(),
            },
            unshieldPreimage: preimage(npk, 500),
        }
    }

    fn transact_data(txs: Vec<IRailgun::Transaction>) -> Vec<u8> {
        IRailgun::transactCall { _transactions: txs }.abi_encode()
    }

    fn advisories(h: &Humanized) -> Vec<String> {
        h.warnings.iter().map(Warning::message).collect()
    }

    #[test]
    fn shield_shows_token_amount_and_encrypted_recipient() {
        let h = Railgun
            .try_humanize(&ctx(), proxy(), U256::ZERO, &shield_data(1_000))
            .unwrap();
        assert!(h.summary.starts_with("Shield 1000 "), "{}", h.summary);
        assert!(h.rows.iter().any(|r| r.value.contains("encrypted 0zk")));
        assert!(h.rows.iter().any(|r| r.value == "RAILGUN"));
        assert!(!advisories(&h).iter().any(|m| m.contains("not the RAILGUN")));
    }

    #[test]
    fn shield_to_another_contract_warns() {
        let fake = address!("0x00000000000000000000000000000000deadbeef");
        let h = Railgun
            .try_humanize(&ctx(), fake, U256::ZERO, &shield_data(1))
            .unwrap();
        assert!(advisories(&h).iter().any(|m| m.contains("not the RAILGUN")));
    }

    #[test]
    fn private_transfer_hides_recipients_and_warns_about_linkage() {
        let data = transact_data(vec![transaction(None, SEPOLIA)]);
        let h = Railgun
            .try_humanize(&ctx(), proxy(), U256::ZERO, &data)
            .unwrap();
        assert!(h.summary.contains("Private RAILGUN transfer"));
        assert!(h
            .rows
            .iter()
            .any(|r| r.label == "Private outputs" && r.value.starts_with('2')));
        assert!(advisories(&h)
            .iter()
            .any(|m| m.contains("paid for a private")));
    }

    #[test]
    fn unshield_shows_public_recipient() {
        let other = address!("0x1111111111111111111111111111111111111111");
        let data = transact_data(vec![transaction(Some(other), SEPOLIA)]);
        let h = Railgun
            .try_humanize(&ctx(), proxy(), U256::ZERO, &data)
            .unwrap();
        assert!(h.summary.starts_with("Unshield 500 "), "{}", h.summary);
        assert!(h
            .rows
            .iter()
            .any(|r| r.label == "To" && r.value == "0x1111111111111111111111111111111111111111"));
        assert!(advisories(&h).iter().any(|m| m.contains("not your wallet")));

        let to_me = transact_data(vec![transaction(Some(ME), SEPOLIA)]);
        let h = Railgun
            .try_humanize(&ctx(), proxy(), U256::ZERO, &to_me)
            .unwrap();
        assert!(!advisories(&h).iter().any(|m| m.contains("not your wallet")));
    }

    #[test]
    fn wrong_chain_proof_is_flagged() {
        let data = transact_data(vec![transaction(None, 1)]);
        let h = Railgun
            .try_humanize(&ctx(), proxy(), U256::ZERO, &data)
            .unwrap();
        assert!(advisories(&h)
            .iter()
            .any(|m| m.contains("bound to chain 1")));
    }

    #[test]
    fn other_calldata_is_not_ours() {
        assert!(Railgun
            .try_humanize(&ctx(), proxy(), U256::ZERO, &[0xde, 0xad, 0xbe, 0xef])
            .is_none());
    }

    #[test]
    fn multicall_wrap_and_shield_decodes_cleanly() {
        let (_proxy_addr, relay_addr) = railgun_contracts(SEPOLIA).unwrap();
        let wrap_data = IRailgun::wrapBaseCall {
            _amount: U256::from(10_000_000u64),
        }
        .abi_encode();
        let shield_data = IRailgun::shieldCall {
            _shieldRequests: vec![IRailgun::ShieldRequest {
                preimage: IRailgun::CommitmentPreimage {
                    npk: [0x55; 32].into(),
                    token: IRailgun::TokenData {
                        tokenType: 0,
                        tokenAddress: address!("0xfff9976782d46cc05630d1f6ebab18b2324d6b14"),
                        tokenSubID: U256::ZERO,
                    },
                    value: U120::from(10_000_000u64),
                },
                ciphertext: IRailgun::ShieldCiphertext {
                    encryptedBundle: [[0u8; 32].into(); 3],
                    shieldKey: [0u8; 32].into(),
                },
            }],
        }
        .abi_encode();

        let multi_data = IRailgun::multicallCall {
            _requireSuccess: true,
            _calls: vec![
                IRailgun::Call {
                    to: relay_addr,
                    data: wrap_data.into(),
                    value: U256::ZERO,
                },
                IRailgun::Call {
                    to: relay_addr,
                    data: shield_data.into(),
                    value: U256::ZERO,
                },
            ],
        }
        .abi_encode();

        let h = Railgun
            .try_humanize(&ctx(), relay_addr, U256::from(10_000_000u64), &multi_data)
            .unwrap();
        assert!(h.summary.contains("Wrap and Shield"), "{}", h.summary);
        assert!(h
            .rows
            .iter()
            .any(|r| r.label == "Contract" && r.value == "RAILGUN RelayAdapt"));
    }
}
