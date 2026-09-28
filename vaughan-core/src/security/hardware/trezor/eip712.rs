//! EIP-712 streaming for Trezor Model T / Safe (`EthereumSignTypedData`).
//!
//! The firmware pulls the typed data piece by piece so it can show each field:
//! first struct layouts (`EthereumTypedDataStructRequest` → member types), then
//! values by member path (`EthereumTypedDataValueRequest` → encoded bytes).
//! This module answers those requests from `eth_signTypedData_v4` JSON; the USB
//! loop lives in `usb.rs`. Protocol shapes follow Trezor's reference host
//! library (`trezorlib.ethereum.sign_typed_data`) and the EIP-712 spec.
//!
//! Trezor One has no streaming mode — it signs the two hashes instead
//! (see `crate::security::signing::eip712_hashes`).

use std::str::FromStr;

use alloy::primitives::{I256, U256};
use serde_json::{Map, Value};
use trezor_client::protos::ethereum_typed_data_struct_ack::{
    EthereumDataType, EthereumFieldType, EthereumStructMember,
};

use crate::error::WalletError;

/// One `{name, type}` entry from the EIP-712 `types` map.
#[derive(Debug, Clone)]
struct Member {
    name: String,
    type_name: String,
}

/// Typed-data payload indexed for the device's struct / value requests.
pub(crate) struct TypedDataStream {
    types: Map<String, Value>,
    primary_type: String,
    domain: Value,
    message: Value,
}

impl TypedDataStream {
    /// Parse `eth_signTypedData_v4` JSON. Adds an `EIP712Domain` type from the
    /// domain keys when the dApp omitted it (the device always asks for it).
    pub(crate) fn new(payload: &Value) -> Result<Self, WalletError> {
        let invalid = |m: &str| WalletError::InvalidTransaction(format!("EIP-712: {m}"));
        let mut types = payload
            .get("types")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| invalid("missing types"))?;
        let primary_type = payload
            .get("primaryType")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid("missing primaryType"))?
            .to_string();
        let domain = payload
            .get("domain")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));
        let message = payload
            .get("message")
            .cloned()
            .unwrap_or(Value::Object(Map::new()));
        if !types.contains_key("EIP712Domain") {
            types.insert("EIP712Domain".into(), inferred_domain_type(&domain));
        }
        if !types.contains_key(&primary_type) {
            return Err(invalid("primaryType is not in types"));
        }
        Ok(Self {
            types,
            primary_type,
            domain,
            message,
        })
    }

    pub(crate) fn primary_type(&self) -> &str {
        &self.primary_type
    }

    /// Answer `EthereumTypedDataStructRequest { name }`.
    pub(crate) fn struct_members(
        &self,
        name: &str,
    ) -> Result<Vec<EthereumStructMember>, WalletError> {
        self.members(name)?
            .into_iter()
            .map(|m| {
                let mut member = EthereumStructMember::new();
                member.type_ = Some(self.field_type(&m.type_name)?).into();
                member.set_name(m.name);
                Ok(member)
            })
            .collect()
    }

    /// Answer `EthereumTypedDataValueRequest { member_path }`: index 0 is the
    /// domain, 1 the message; later indices walk struct members / array items.
    /// Arrays answer with their length (u16 BE); the device then asks per item.
    pub(crate) fn value_at(&self, member_path: &[u32]) -> Result<Vec<u8>, WalletError> {
        let (mut type_name, mut data) = match member_path.first() {
            Some(0) => ("EIP712Domain".to_string(), &self.domain),
            Some(1) => (self.primary_type.clone(), &self.message),
            _ => return Err(eip712_err("root index must be 0 or 1")),
        };
        for &index in &member_path[1..] {
            let index = index as usize;
            if let Some(obj) = data.as_object() {
                let member = self
                    .members(&type_name)?
                    .into_iter()
                    .nth(index)
                    .ok_or_else(|| eip712_err("member index out of range"))?;
                data = obj
                    .get(&member.name)
                    .ok_or_else(|| eip712_err(&format!("missing field {}", member.name)))?;
                type_name = member.type_name;
            } else if let Some(items) = data.as_array() {
                type_name = array_entry_type(&type_name)
                    .ok_or_else(|| eip712_err("array value for non-array type"))?
                    .to_string();
                data = items
                    .get(index)
                    .ok_or_else(|| eip712_err("array index out of range"))?;
            } else {
                return Err(eip712_err("path walks into an atomic value"));
            }
        }
        if let Some(items) = data.as_array() {
            let len = u16::try_from(items.len()).map_err(|_| eip712_err("array too long"))?;
            return Ok(len.to_be_bytes().to_vec());
        }
        encode_atomic(data, &type_name)
    }

    fn members(&self, name: &str) -> Result<Vec<Member>, WalletError> {
        let list = self
            .types
            .get(name)
            .and_then(Value::as_array)
            .ok_or_else(|| eip712_err(&format!("unknown struct {name}")))?;
        list.iter()
            .map(|m| {
                let field = |k: &str| {
                    m.get(k)
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .ok_or_else(|| eip712_err(&format!("struct {name}: member missing {k}")))
                };
                Ok(Member {
                    name: field("name")?,
                    type_name: field("type")?,
                })
            })
            .collect()
    }

    fn field_type(&self, type_name: &str) -> Result<EthereumFieldType, WalletError> {
        let mut ft = EthereumFieldType::new();
        if let Some(entry) = array_entry_type(type_name) {
            let entry_type = self.field_type(entry)?;
            if entry_type.data_type() == EthereumDataType::ARRAY {
                return Err(eip712_err("nested arrays are not supported by Trezor"));
            }
            let len = &type_name[entry.len() + 1..type_name.len() - 1];
            if !len.is_empty() {
                ft.set_size(
                    len.parse()
                        .map_err(|_| eip712_err(&format!("bad array length in {type_name}")))?,
                );
            }
            ft.set_data_type(EthereumDataType::ARRAY);
            ft.entry_type = Some(entry_type).into();
            return Ok(ft);
        }
        if let Some(list) = self.types.get(type_name).and_then(Value::as_array) {
            ft.set_data_type(EthereumDataType::STRUCT);
            ft.set_size(list.len() as u32);
            ft.set_struct_name(type_name.to_string());
            return Ok(ft);
        }
        match atomic_kind(type_name)? {
            Atomic::Uint(size) => {
                ft.set_data_type(EthereumDataType::UINT);
                ft.set_size(size);
            }
            Atomic::Int(size) => {
                ft.set_data_type(EthereumDataType::INT);
                ft.set_size(size);
            }
            Atomic::Bytes(size) => {
                ft.set_data_type(EthereumDataType::BYTES);
                if let Some(n) = size {
                    ft.set_size(n);
                }
            }
            Atomic::String => ft.set_data_type(EthereumDataType::STRING),
            Atomic::Bool => ft.set_data_type(EthereumDataType::BOOL),
            Atomic::Address => ft.set_data_type(EthereumDataType::ADDRESS),
        }
        Ok(ft)
    }
}

fn eip712_err(msg: &str) -> WalletError {
    WalletError::InvalidTransaction(format!("EIP-712: {msg}"))
}

/// `Foo[]` / `Foo[3]` → `Foo`.
fn array_entry_type(type_name: &str) -> Option<&str> {
    if !type_name.ends_with(']') {
        return None;
    }
    type_name.rfind('[').map(|i| &type_name[..i])
}

/// Canonical EIP-712 domain field order, for payloads that omit `EIP712Domain`.
fn inferred_domain_type(domain: &Value) -> Value {
    const FIELDS: [(&str, &str); 5] = [
        ("name", "string"),
        ("version", "string"),
        ("chainId", "uint256"),
        ("verifyingContract", "address"),
        ("salt", "bytes32"),
    ];
    Value::Array(
        FIELDS
            .iter()
            .filter(|(k, _)| domain.get(*k).is_some())
            .map(|(k, t)| serde_json::json!({ "name": k, "type": t }))
            .collect(),
    )
}

enum Atomic {
    Uint(u32),
    Int(u32),
    Bytes(Option<u32>),
    String,
    Bool,
    Address,
}

/// Solidity atomic type → wire kind + byte size (`uint256` → 32).
fn atomic_kind(type_name: &str) -> Result<Atomic, WalletError> {
    let bad = || eip712_err(&format!("unsupported type {type_name}"));
    let int_bytes = |bits: &str| -> Result<u32, WalletError> {
        if bits.is_empty() {
            return Ok(32);
        }
        let bits: u32 = bits.parse().map_err(|_| bad())?;
        if bits == 0 || bits > 256 || !bits.is_multiple_of(8) {
            return Err(bad());
        }
        Ok(bits / 8)
    };
    Ok(match type_name {
        "string" => Atomic::String,
        "bool" => Atomic::Bool,
        "address" => Atomic::Address,
        "bytes" => Atomic::Bytes(None),
        t if t.starts_with("uint") => Atomic::Uint(int_bytes(&t[4..])?),
        t if t.starts_with("int") => Atomic::Int(int_bytes(&t[3..])?),
        t if t.starts_with("bytes") => {
            let n: u32 = t[5..].parse().map_err(|_| bad())?;
            if n == 0 || n > 32 {
                return Err(bad());
            }
            Atomic::Bytes(Some(n))
        }
        _ => return Err(bad()),
    })
}

/// Encode one atomic value the way the firmware expects (big-endian ints sized
/// to the type, raw bytes / address, UTF-8 string, one-byte bool).
fn encode_atomic(value: &Value, type_name: &str) -> Result<Vec<u8>, WalletError> {
    let bad = |what: &str| eip712_err(&format!("{type_name}: {what}"));
    match atomic_kind(type_name)? {
        Atomic::String => value
            .as_str()
            .map(|s| s.as_bytes().to_vec())
            .ok_or_else(|| bad("expected a string")),
        Atomic::Bool => value
            .as_bool()
            .map(|b| vec![u8::from(b)])
            .ok_or_else(|| bad("expected true/false")),
        Atomic::Address => {
            let bytes = hex_value(value).ok_or_else(|| bad("expected 0x address"))?;
            if bytes.len() != 20 {
                return Err(bad("address must be 20 bytes"));
            }
            Ok(bytes)
        }
        Atomic::Bytes(size) => {
            let bytes = hex_value(value).ok_or_else(|| bad("expected 0x hex"))?;
            if size.is_some_and(|n| bytes.len() != n as usize) {
                return Err(bad("wrong byte length"));
            }
            Ok(bytes)
        }
        Atomic::Uint(size) => {
            let n = match value {
                Value::Number(n) => n.as_u64().map(U256::from),
                Value::String(s) => U256::from_str(s.trim()).ok(),
                _ => None,
            }
            .ok_or_else(|| bad("expected an unsigned integer"))?;
            let full = n.to_be_bytes::<32>();
            let cut = 32 - size as usize;
            if full[..cut].iter().any(|b| *b != 0) {
                return Err(bad("value does not fit the type"));
            }
            Ok(full[cut..].to_vec())
        }
        Atomic::Int(size) => {
            let n = match value {
                Value::Number(n) => n.as_i64().map(I256::try_from).and_then(Result::ok),
                Value::String(s) => parse_i256(s.trim()),
                _ => None,
            }
            .ok_or_else(|| bad("expected an integer"))?;
            let full = n.to_be_bytes::<32>();
            let cut = 32 - size as usize;
            let fill = if n.is_negative() { 0xff } else { 0x00 };
            let sign_ok = full[cut] & 0x80 == fill & 0x80;
            if full[..cut].iter().any(|b| *b != fill) || !sign_ok {
                return Err(bad("value does not fit the type"));
            }
            Ok(full[cut..].to_vec())
        }
    }
}

fn parse_i256(s: &str) -> Option<I256> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let magnitude = if body.starts_with("0x") || body.starts_with("0X") {
        I256::from_hex_str(body).ok()?
    } else {
        I256::from_dec_str(body).ok()?
    };
    Some(if neg { -magnitude } else { magnitude })
}

fn hex_value(value: &Value) -> Option<Vec<u8>> {
    let s = value.as_str()?.trim();
    let body = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X"))?;
    hex::decode(body).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail() -> Value {
        serde_json::json!({
            "types": {
                "EIP712Domain": [
                    {"name": "name", "type": "string"},
                    {"name": "version", "type": "string"},
                    {"name": "chainId", "type": "uint256"},
                    {"name": "verifyingContract", "type": "address"}
                ],
                "Person": [
                    {"name": "name", "type": "string"},
                    {"name": "wallets", "type": "address[]"}
                ],
                "Mail": [
                    {"name": "from", "type": "Person"},
                    {"name": "to", "type": "Person[]"},
                    {"name": "contents", "type": "string"},
                    {"name": "nonce", "type": "int8"},
                    {"name": "salt", "type": "bytes4"}
                ]
            },
            "primaryType": "Mail",
            "domain": {
                "name": "Ether Mail", "version": "1", "chainId": 1,
                "verifyingContract": "0xCcCCccccCCCCcCCCCCCcCcCccCcCCCcCcccccccC"
            },
            "message": {
                "from": {"name": "Cow", "wallets": ["0xCD2a3d9F938E13CD947Ec05AbC7FE734Df8DD826"]},
                "to": [
                    {"name": "Bob", "wallets": []},
                    {"name": "Eve", "wallets": ["0xbBbBBBBbbBBBbbbBbbBbbbbBBbBbbbbBbBbbBBbB"]}
                ],
                "contents": "Hello, Bob!",
                "nonce": "-2",
                "salt": "0xdeadbeef"
            }
        })
    }

    #[test]
    fn struct_members_describe_types_for_device() {
        let s = TypedDataStream::new(&mail()).unwrap();
        let members = s.struct_members("Mail").unwrap();
        assert_eq!(members.len(), 5);
        assert_eq!(members[0].name(), "from");
        assert_eq!(members[0].type_.data_type(), EthereumDataType::STRUCT);
        assert_eq!(members[0].type_.struct_name(), "Person");
        assert_eq!(members[0].type_.size(), 2);
        assert_eq!(members[1].type_.data_type(), EthereumDataType::ARRAY);
        assert!(!members[1].type_.has_size());
        assert_eq!(members[1].type_.entry_type.struct_name(), "Person");
        assert_eq!(members[3].type_.data_type(), EthereumDataType::INT);
        assert_eq!(members[3].type_.size(), 1);
        assert_eq!(members[4].type_.data_type(), EthereumDataType::BYTES);
        assert_eq!(members[4].type_.size(), 4);
        let domain = s.struct_members("EIP712Domain").unwrap();
        assert_eq!(domain[2].type_.data_type(), EthereumDataType::UINT);
        assert_eq!(domain[2].type_.size(), 32);
    }

    #[test]
    fn values_walk_paths_and_encode_like_trezorlib() {
        let s = TypedDataStream::new(&mail()).unwrap();
        assert_eq!(s.value_at(&[0, 0]).unwrap(), b"Ether Mail");
        let mut chain = vec![0u8; 31];
        chain.push(1);
        assert_eq!(s.value_at(&[0, 2]).unwrap(), chain);
        assert_eq!(s.value_at(&[0, 3]).unwrap().len(), 20);
        assert_eq!(s.value_at(&[1, 0, 0]).unwrap(), b"Cow");
        assert_eq!(s.value_at(&[1, 1]).unwrap(), vec![0, 2]);
        assert_eq!(s.value_at(&[1, 1, 0, 1]).unwrap(), vec![0, 0]);
        assert_eq!(s.value_at(&[1, 1, 1, 0]).unwrap(), b"Eve");
        assert_eq!(s.value_at(&[1, 1, 1, 1, 0]).unwrap().len(), 20);
        assert_eq!(s.value_at(&[1, 3]).unwrap(), vec![0xfe]);
        assert_eq!(s.value_at(&[1, 4]).unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);
        assert!(s.value_at(&[2]).is_err());
        assert!(s.value_at(&[1, 9]).is_err());
    }

    #[test]
    fn out_of_range_ints_are_rejected() {
        assert!(encode_atomic(&serde_json::json!("256"), "uint8").is_err());
        assert_eq!(
            encode_atomic(&serde_json::json!("0xff"), "uint8").unwrap(),
            vec![0xff]
        );
        assert!(encode_atomic(&serde_json::json!("128"), "int8").is_err());
        assert!(encode_atomic(&serde_json::json!("-129"), "int8").is_err());
        assert_eq!(
            encode_atomic(&serde_json::json!("-128"), "int8").unwrap(),
            vec![0x80]
        );
    }

    #[test]
    fn missing_domain_type_is_inferred_in_canonical_order() {
        let mut p = mail();
        p["types"].as_object_mut().unwrap().remove("EIP712Domain");
        let s = TypedDataStream::new(&p).unwrap();
        let names: Vec<String> = s
            .struct_members("EIP712Domain")
            .unwrap()
            .iter()
            .map(|m| m.name().to_string())
            .collect();
        assert_eq!(names, ["name", "version", "chainId", "verifyingContract"]);
    }
}
