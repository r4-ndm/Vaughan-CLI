//! Minimal EVM mock bytecode helpers shared by Anvil integration tests.

use alloy::primitives::Address;
use serde_json::json;

use super::Anvil;

/// ABI-encode a single `uint256` return.
pub fn ret_u256(v: u64) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[24..].copy_from_slice(&v.to_be_bytes());
    out
}

/// ABI-encode an `address` return.
pub fn ret_address(addr: Address) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[12..32].copy_from_slice(addr.as_slice());
    out
}

/// `getAmountsOut` return: `[amountIn, amountOut]` for a two-token path.
pub fn ret_amounts_out_pair(amount_in: u64, amount_out: u64) -> Vec<u8> {
    let mut out = vec![0u8; 128];
    out[31] = 0x20;
    out[63] = 0x02;
    out[88..96].copy_from_slice(&amount_in.to_be_bytes());
    out[120..128].copy_from_slice(&amount_out.to_be_bytes());
    out
}

/// Selector dispatcher with correct jump targets and large-return MSTORE offsets.
pub fn assemble_dispatcher(routes: &[([u8; 4], Vec<u8>)]) -> Vec<u8> {
    fn chunk_store_cost(offset: usize) -> usize {
        1 + 32 + if offset <= 255 { 2 } else { 3 } + 1
    }

    let mut bytecode = vec![0x60, 0x00, 0x35, 0x60, 0xe0, 0x1c];
    let dispatch_size = bytecode.len() + routes.len() * 11 + 5;
    let mut handlers = Vec::new();
    let mut current_offset = dispatch_size;

    for (_sel, ret_data) in routes {
        let handler_target = current_offset as u16;
        handlers.push((handler_target, ret_data.clone()));
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        let chunk_bytes: usize = (0..chunks).map(|c| chunk_store_cost(c * 32)).sum();
        current_offset += 1 + chunk_bytes + 6;
    }

    for (i, (sel, _)) in routes.iter().enumerate() {
        let (target, _) = handlers[i];
        bytecode.push(0x80);
        bytecode.push(0x63);
        bytecode.extend_from_slice(sel);
        bytecode.push(0x14);
        bytecode.push(0x61);
        bytecode.push((target >> 8) as u8);
        bytecode.push((target & 0xff) as u8);
        bytecode.push(0x57);
    }
    bytecode.extend_from_slice(&[0x60, 0x00, 0x60, 0x00, 0xfd]);

    for (target, ret_data) in handlers {
        assert_eq!(bytecode.len(), target as usize);
        bytecode.push(0x5b);
        let chunks = if ret_data.is_empty() {
            0
        } else {
            ret_data.len().div_ceil(32)
        };
        for c in 0..chunks {
            let start = c * 32;
            let end = (start + 32).min(ret_data.len());
            let mut word = [0u8; 32];
            word[..end - start].copy_from_slice(&ret_data[start..end]);
            bytecode.push(0x7f);
            bytecode.extend_from_slice(&word);
            let off = c * 32;
            if off <= 255 {
                bytecode.push(0x60);
                bytecode.push(off as u8);
            } else {
                bytecode.push(0x61);
                bytecode.push((off >> 8) as u8);
                bytecode.push((off & 0xff) as u8);
            }
            bytecode.push(0x52);
        }
        let size = ret_data.len() as u16;
        bytecode.push(0x61);
        bytecode.push((size >> 8) as u8);
        bytecode.push((size & 0xff) as u8);
        bytecode.extend_from_slice(&[0x60, 0x00, 0xf3]);
    }
    bytecode
}

pub fn plant_code(anvil: &Anvil, at: Address, code: &[u8]) {
    anvil
        .rpc(
            "anvil_setCode",
            json!([format!("{at:#x}"), format!("0x{}", hex::encode(code))]),
        )
        .expect("anvil_setCode");
}
