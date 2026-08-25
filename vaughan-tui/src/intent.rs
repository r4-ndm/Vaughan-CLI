//! Intent macros — thin wrappers that jump to existing TUI surfaces.
//!
//! Slash commands used in the Contract Browser REPL (and documented for MCP
//! agents). No new protocol logic: navigate / prefill only.

/// Destination produced by an intent macro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentNav {
    /// Ag (`g`) with optional amount / token-out prefill.
    Aggregator {
        amount: Option<String>,
        token_out: Option<String>,
    },
    /// Browser (`c`) — inspect this address (browse).
    BrowserInspect { address: String },
    /// Approvals manager (`j`) for revoke.
    Approvals,
    /// Receive (`v`) — public address + stealth URI.
    Receive,
}

/// Parse a slash intent. Returns `None` if `cmd` is not a known macro.
///
/// Supported:
/// - `/swap [amount] [tokenOut]`
/// - `/inspect <0xAddress>`
/// - `/revoke`
/// - `/stealth receive` (also `/stealth`)
pub fn parse_intent(cmd: &str) -> Option<Result<IntentNav, String>> {
    let trimmed = cmd.trim();
    if !trimmed.starts_with('/') {
        return None;
    }
    let body = trimmed.trim_start_matches('/').trim();
    if body.is_empty() {
        return Some(Err(
            "empty intent — try /swap, /inspect, /revoke, /stealth receive".into(),
        ));
    }

    let mut parts = body.split_whitespace();
    let head = parts.next()?.to_ascii_lowercase();
    Some(match head.as_str() {
        "swap" => {
            let amount = parts.next().map(|s| s.to_string());
            let token_out = parts.next().map(|s| s.to_string());
            if parts.next().is_some() {
                Err("Usage: /swap [amount] [tokenOut]".into())
            } else {
                Ok(IntentNav::Aggregator { amount, token_out })
            }
        }
        "inspect" => {
            let address = parts.next().map(|s| s.to_string());
            if parts.next().is_some() {
                return Some(Err("Usage: /inspect <0xAddress>".into()));
            }
            match address {
                Some(address) if address.starts_with("0x") || address.starts_with("0X") => {
                    Ok(IntentNav::BrowserInspect { address })
                }
                Some(_) => Err("Usage: /inspect <0xAddress>".into()),
                None => Err("Usage: /inspect <0xAddress>".into()),
            }
        }
        "revoke" => {
            if parts.next().is_some() {
                Err("Usage: /revoke".into())
            } else {
                Ok(IntentNav::Approvals)
            }
        }
        "stealth" => {
            let sub = parts.next().map(|s| s.to_ascii_lowercase());
            if parts.next().is_some() {
                return Some(Err("Usage: /stealth receive".into()));
            }
            match sub.as_deref() {
                None | Some("receive") => Ok(IntentNav::Receive),
                Some(other) => Err(format!(
                    "unknown /stealth subcommand '{other}' — try /stealth receive"
                )),
            }
        }
        other => Err(format!(
            "unknown intent '/{other}' — try /swap, /inspect, /revoke, /stealth receive"
        )),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_swap_variants() {
        assert_eq!(
            parse_intent("/swap").unwrap().unwrap(),
            IntentNav::Aggregator {
                amount: None,
                token_out: None
            }
        );
        assert_eq!(
            parse_intent("/swap 1").unwrap().unwrap(),
            IntentNav::Aggregator {
                amount: Some("1".into()),
                token_out: None
            }
        );
        assert_eq!(
            parse_intent("/swap 0.5 0xabc").unwrap().unwrap(),
            IntentNav::Aggregator {
                amount: Some("0.5".into()),
                token_out: Some("0xabc".into())
            }
        );
    }

    #[test]
    fn parses_inspect_revoke_stealth() {
        assert_eq!(
            parse_intent("/inspect 0x1111111111111111111111111111111111111111")
                .unwrap()
                .unwrap(),
            IntentNav::BrowserInspect {
                address: "0x1111111111111111111111111111111111111111".into()
            }
        );
        assert_eq!(
            parse_intent("/revoke").unwrap().unwrap(),
            IntentNav::Approvals
        );
        assert_eq!(
            parse_intent("/stealth receive").unwrap().unwrap(),
            IntentNav::Receive
        );
        assert_eq!(
            parse_intent("/stealth").unwrap().unwrap(),
            IntentNav::Receive
        );
    }

    #[test]
    fn non_slash_is_none() {
        assert!(parse_intent("browse 0x1").is_none());
        assert!(parse_intent("help").is_none());
    }
}
