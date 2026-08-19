//! EVM bytecode selector extraction.
//!
//! Scans contract runtime bytecode and extracts candidate 4-byte function
//! selectors from `PUSH4` (opcode 0x63) instructions, skipping intermediate
//! push-data payloads correctly.

use std::collections::HashSet;

/// Candidate 4-byte function selector.
pub type Selector = [u8; 4];

/// Extract unique candidate 4-byte function selectors from raw EVM bytecode.
///
/// Handles EVM opcode layout:
/// - Skips PUSH1..PUSH32 (0x60..0x7F) immediate data bytes so data bytes
///   coincidentally equal to 0x63 are not falsely treated as PUSH4 opcodes.
/// - Returns candidate selectors in the order they were first encountered.
pub fn extract_selectors(bytecode: &[u8]) -> Vec<Selector> {
    let mut seen = HashSet::new();
    let mut selectors = Vec::new();

    let mut i = 0;
    while i < bytecode.len() {
        let op = bytecode[i];
        i += 1;

        if (0x60..=0x7f).contains(&op) {
            let push_size = (op - 0x60 + 1) as usize;
            if op == 0x63 && i + 4 <= bytecode.len() {
                let mut sel = [0u8; 4];
                sel.copy_from_slice(&bytecode[i..i + 4]);
                if seen.insert(sel) {
                    selectors.push(sel);
                }
            }
            i += push_size;
        }
    }

    selectors
}

/// Convert a 4-byte selector to its standard hex representation (e.g. `0xa9059cbb`).
pub fn selector_to_hex(sel: Selector) -> String {
    format!("0x{}", hex::encode(sel))
}

/// Parse a hex string (with or without `0x` prefix) into a 4-byte selector.
pub fn parse_selector_hex(hex_str: &str) -> Result<Selector, String> {
    let clean = hex_str.trim().strip_prefix("0x").unwrap_or(hex_str.trim());
    if clean.len() != 8 {
        return Err(format!(
            "Selector hex must be exactly 4 bytes (8 hex characters), got '{}'",
            hex_str
        ));
    }
    let bytes =
        hex::decode(clean).map_err(|e| format!("Invalid hex selector '{}': {}", hex_str, e))?;
    let mut sel = [0u8; 4];
    sel.copy_from_slice(&bytes);
    Ok(sel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_push4_selectors() {
        // PUSH4 0xa9059cbb (transfer) + PUSH1 0x00 + PUSH4 0x70a08231 (balanceOf)
        let bytecode = hex::decode("63a9059cbb60006370a08231").unwrap();
        let selectors = extract_selectors(&bytecode);
        assert_eq!(selectors.len(), 2);
        assert_eq!(selectors[0], [0xa9, 0x05, 0x9c, 0xbb]);
        assert_eq!(selectors[1], [0x70, 0xa0, 0x82, 0x31]);
    }

    #[test]
    fn skips_push32_data_containing_0x63() {
        // PUSH32 containing 0x63 within data: should NOT be parsed as PUSH4
        let mut bytecode = vec![0x7f]; // PUSH32
        bytecode.extend_from_slice(&[0x00; 10]);
        bytecode.extend_from_slice(&[0x63, 0x11, 0x22, 0x33, 0x44]); // 0x63 inside PUSH32 payload
        bytecode.extend_from_slice(&[0x00; 17]); // total 32 bytes of push data
        bytecode.extend_from_slice(&[0x63, 0xaa, 0xbb, 0xcc, 0xdd]); // Real PUSH4

        let selectors = extract_selectors(&bytecode);
        assert_eq!(selectors.len(), 1);
        assert_eq!(selectors[0], [0xaa, 0xbb, 0xcc, 0xdd]);
    }

    #[test]
    fn handles_empty_or_truncated_bytecode() {
        assert!(extract_selectors(&[]).is_empty());
        // Truncated PUSH4 (only 2 bytes after opcode)
        assert!(extract_selectors(&[0x63, 0x01, 0x02]).is_empty());
    }

    #[test]
    fn selector_hex_conversion() {
        let sel = [0xa9, 0x05, 0x9c, 0xbb];
        assert_eq!(selector_to_hex(sel), "0xa9059cbb");
        assert_eq!(parse_selector_hex("0xa9059cbb").unwrap(), sel);
        assert_eq!(parse_selector_hex("a9059cbb").unwrap(), sel);
        assert!(parse_selector_hex("0xa905").is_err());
    }
}
