//! Dynamic contract execution and parameter encoding/decoding.
//!
//! Enables generic read-only `eth_call` invocation on any contract using
//! dynamic ABI encoding (`alloy-dyn-abi`) and formatted type returns.

use alloy::dyn_abi::{DynSolType, DynSolValue};
use alloy::json_abi::Function;
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::Provider;
use alloy::rpc::types::eth::TransactionRequest;

/// Result of executing a dynamic contract call.
#[derive(Debug, Clone)]
pub struct CallResult {
    /// Raw returned bytes.
    pub raw_output: Bytes,
    /// Decoded return values (formatted strings), if ABI or return types available.
    pub decoded_values: Vec<String>,
}

/// Dynamic contract caller.
pub struct DynamicCaller;

impl DynamicCaller {
    /// Encode arguments for a function definition.
    pub fn encode_call(func: &Function, args: &[String]) -> Result<Bytes, String> {
        if func.inputs.len() != args.len() {
            return Err(format!(
                "Function '{}' expects {} arguments, but {} were provided",
                func.name,
                func.inputs.len(),
                args.len()
            ));
        }

        // Calculate 4-byte selector
        let selector = func.selector();
        let mut calldata = selector.to_vec();

        if args.is_empty() {
            return Ok(Bytes::from(calldata));
        }

        // Parse types and values
        let mut parsed_types = Vec::new();
        let mut parsed_values = Vec::new();

        for (param, arg_str) in func.inputs.iter().zip(args.iter()) {
            let ty_str = &param.ty;
            let sol_type: DynSolType = ty_str
                .parse()
                .map_err(|e| format!("Unsupported parameter type '{}': {}", ty_str, e))?;

            let sol_val = parse_dyn_value(&sol_type, arg_str.trim()).map_err(|e| {
                format!(
                    "Failed to parse argument '{}' as type '{}': {}",
                    arg_str, ty_str, e
                )
            })?;

            parsed_types.push(sol_type);
            parsed_values.push(sol_val);
        }

        // ABI-encode tuple
        let tuple_val = DynSolValue::Tuple(parsed_values);
        let encoded_args = tuple_val.abi_encode();
        calldata.extend_from_slice(&encoded_args);

        Ok(Bytes::from(calldata))
    }

    /// Execute a read-only `eth_call` against a contract and decode the result.
    pub async fn call_function<P: Provider>(
        provider: &P,
        target: Address,
        func: &Function,
        args: &[String],
    ) -> Result<CallResult, String> {
        let calldata = Self::encode_call(func, args)?;
        let tx = TransactionRequest::default()
            .to(target)
            .input(calldata.into());

        let raw_output = provider
            .call(tx)
            .await
            .map_err(|e| format_rpc_error(&e.to_string()))?;

        // Decode return types
        let mut decoded_values = Vec::new();
        if !func.outputs.is_empty() && !raw_output.is_empty() {
            let output_types: Result<Vec<DynSolType>, _> = func
                .outputs
                .iter()
                .map(|p| p.ty.parse::<DynSolType>())
                .collect();

            if let Ok(types) = output_types {
                let tuple_type = DynSolType::Tuple(types);
                if let Ok(DynSolValue::Tuple(items)) = tuple_type.abi_decode(&raw_output) {
                    for (idx, item) in items.into_iter().enumerate() {
                        let label = func.outputs.get(idx).map(|p| p.name.as_str()).unwrap_or("");
                        let formatted = format_dyn_value(&item);
                        if !label.is_empty() {
                            decoded_values.push(format!("{}: {}", label, formatted));
                        } else {
                            decoded_values.push(formatted);
                        }
                    }
                }
            }
        }

        Ok(CallResult {
            raw_output,
            decoded_values,
        })
    }

    /// Execute a raw `eth_call` with arbitrary calldata hex.
    pub async fn call_raw<P: Provider>(
        provider: &P,
        target: Address,
        calldata: Bytes,
    ) -> Result<Bytes, String> {
        let tx = TransactionRequest::default()
            .to(target)
            .input(calldata.into());

        provider
            .call(tx)
            .await
            .map_err(|e| format_rpc_error(&e.to_string()))
    }
}

/// Parse string input into a `DynSolValue` matching the specified `DynSolType`.
fn parse_dyn_value(ty: &DynSolType, input: &str) -> Result<DynSolValue, String> {
    match ty {
        DynSolType::Address => {
            let addr: Address = input
                .parse()
                .map_err(|e| format!("Invalid address: {}", e))?;
            Ok(DynSolValue::Address(addr))
        }
        DynSolType::Bool => {
            let b: bool = input
                .parse()
                .map_err(|e| format!("Invalid boolean: {}", e))?;
            Ok(DynSolValue::Bool(b))
        }
        DynSolType::Uint(bits) => {
            let val = if input.starts_with("0x") || input.starts_with("0X") {
                U256::from_str_radix(input.trim_start_matches("0x").trim_start_matches("0X"), 16)
                    .map_err(|e| format!("Invalid hex uint: {}", e))?
            } else {
                U256::from_str_radix(input, 10).map_err(|e| format!("Invalid uint: {}", e))?
            };
            Ok(DynSolValue::Uint(val, *bits))
        }
        DynSolType::Int(bits) => {
            let val: alloy::primitives::I256 = if input.starts_with("0x") || input.starts_with("0X")
            {
                let u = U256::from_str_radix(
                    input.trim_start_matches("0x").trim_start_matches("0X"),
                    16,
                )
                .map_err(|e| format!("Invalid hex int: {}", e))?;
                alloy::primitives::I256::from_raw(u)
            } else {
                input.parse().map_err(|e| format!("Invalid int: {}", e))?
            };
            Ok(DynSolValue::Int(val, *bits))
        }
        DynSolType::String => Ok(DynSolValue::String(input.to_string())),
        DynSolType::Bytes => {
            let clean = input.trim_start_matches("0x").trim_start_matches("0X");
            let bytes = hex::decode(clean).map_err(|e| format!("Invalid bytes hex: {}", e))?;
            Ok(DynSolValue::Bytes(bytes))
        }
        DynSolType::FixedBytes(size) => {
            let clean = input.trim_start_matches("0x").trim_start_matches("0X");
            let bytes = hex::decode(clean).map_err(|e| format!("Invalid bytes hex: {}", e))?;
            if bytes.len() != *size {
                return Err(format!("Expected {} bytes, got {}", size, bytes.len()));
            }
            let mut word = [0u8; 32];
            word[..*size].copy_from_slice(&bytes);
            Ok(DynSolValue::FixedBytes(
                alloy::primitives::B256::from(word),
                *size,
            ))
        }
        _ => Err(format!(
            "Complex types not directly supported from single string input: {:?}",
            ty
        )),
    }
}

/// Format a `DynSolValue` for terminal output.
pub fn format_dyn_value(val: &DynSolValue) -> String {
    match val {
        DynSolValue::Address(addr) => addr.to_checksum(None),
        DynSolValue::Bool(b) => b.to_string(),
        DynSolValue::Uint(u, _) => u.to_string(),
        DynSolValue::Int(i, _) => i.to_string(),
        DynSolValue::String(s) => s.clone(),
        DynSolValue::Bytes(b) => format!("0x{}", hex::encode(b)),
        DynSolValue::FixedBytes(b, size) => format!("0x{}", hex::encode(&b.as_slice()[..*size])),
        DynSolValue::Array(arr) => {
            let inner: Vec<String> = arr.iter().map(format_dyn_value).collect();
            format!("[{}]", inner.join(", "))
        }
        DynSolValue::Tuple(tuple) => {
            let inner: Vec<String> = tuple.iter().map(format_dyn_value).collect();
            format!("({})", inner.join(", "))
        }
        _ => format!("{:?}", val),
    }
}

/// Format RPC errors cleanly (e.g. extract revert messages).
fn format_rpc_error(err: &str) -> String {
    if err.contains("execution reverted") {
        "Execution reverted on-chain".to_string()
    } else {
        err.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::json_abi::Param;

    #[test]
    fn encode_zero_arg_function() {
        let func = Function {
            name: "totalSupply".to_string(),
            inputs: vec![],
            outputs: vec![Param {
                name: "".to_string(),
                ty: "uint256".to_string(),
                components: vec![],
                internal_type: None,
            }],
            state_mutability: alloy::json_abi::StateMutability::View,
        };

        let encoded = DynamicCaller::encode_call(&func, &[]).unwrap();
        assert_eq!(hex::encode(encoded), "18160ddd");
    }

    #[test]
    fn encode_function_with_args() {
        let func = Function {
            name: "balanceOf".to_string(),
            inputs: vec![Param {
                name: "account".to_string(),
                ty: "address".to_string(),
                components: vec![],
                internal_type: None,
            }],
            outputs: vec![],
            state_mutability: alloy::json_abi::StateMutability::View,
        };

        let addr = "0x0000000000000000000000000000000000000001";
        let encoded = DynamicCaller::encode_call(&func, &[addr.to_string()]).unwrap();
        assert!(encoded.starts_with(&[0x70, 0xa0, 0x82, 0x31]));
        assert_eq!(encoded.len(), 4 + 32);
    }
}
