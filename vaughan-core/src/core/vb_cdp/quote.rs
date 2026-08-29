//! Swap quote parsing from visible page text (`browser_read_quote`).
//!
//! Two extraction layers: labeled output rows (`Total Output` / `Expected
//! Output` / `Minimum Output` — Switch.win style) and the generic
//! amount-near-ticker heuristic used by other venues.

use std::time::Duration;

use serde_json::{json, Value};

use super::client::{deep_click_in_all_frames, CdpPage};
use super::snapshot::collect_visible_lines_all_frames;
use crate::error::WalletError;

fn parse_amount_token(raw: &str) -> Option<f64> {
    let raw = raw.trim().trim_matches(|c: char| {
        matches!(
            c,
            ',' | '$' | '~' | '≈' | '(' | ')' | '[' | ']' | '←' | '→' | '⇒'
        )
    });
    let mut num = String::new();
    for c in raw.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else if c == ',' {
            // Thousands separator inside a formatted amount (1,611,295.2965).
            continue;
        } else if !num.is_empty() {
            break;
        }
    }
    if num.is_empty() {
        return None;
    }
    num.parse::<f64>()
        .ok()
        .filter(|v: &f64| *v > 0.0 && v.is_finite() && *v < 1e15)
}

fn extract_amounts_near_token(line: &str, token: &str) -> Vec<f64> {
    let ll = line.to_ascii_lowercase();
    let token_lower = token.to_ascii_lowercase();
    if !ll.contains(&token_lower) {
        return Vec::new();
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        if part.eq_ignore_ascii_case(token) {
            if i > 0 {
                if let Some(v) = parse_amount_token(parts[i - 1]) {
                    out.push(v);
                }
            }
            if i + 1 < parts.len() {
                if let Some(v) = parse_amount_token(parts[i + 1]) {
                    out.push(v);
                }
            }
        }
    }
    for part in parts {
        if let Some(v) = parse_amount_token(part) {
            out.push(v);
        }
    }
    out.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    out
}

fn pick_best_output_amount(candidates: &[f64], token_out: &str) -> Option<f64> {
    if candidates.is_empty() {
        return None;
    }
    let plausible: Vec<f64> = candidates
        .iter()
        .copied()
        .filter(|v| *v >= 0.000_001)
        .collect();
    if plausible.is_empty() {
        return None;
    }
    // Prefer the largest amount that is not a common sell preset when multiple exist.
    let without_unit = plausible
        .iter()
        .copied()
        .filter(|v| (*v - 1.0).abs() > 1e-9 || plausible.len() == 1)
        .collect::<Vec<_>>();
    let pool = if without_unit.is_empty() {
        &plausible
    } else {
        &without_unit
    };
    let best = pool
        .iter()
        .copied()
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))?;
    if token_out.eq_ignore_ascii_case("HEX") && best < 0.01 {
        return None;
    }
    Some(best)
}

/// Ticker-shaped token (`M3M3`, `9INCH`, `WPLS`) — not a UI word or amount.
fn looks_like_ticker(s: &str) -> bool {
    let len = s.chars().count();
    if !(2..=12).contains(&len) {
        return false;
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
        return false;
    }
    if !s.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !s.chars().any(|c| c.is_ascii_uppercase()) {
        return false;
    }
    !matches!(
        s.to_ascii_uppercase().as_str(),
        "USD" | "MAX" | "SELL" | "BUY" | "GET" | "SWAP" | "LIMIT" | "BALANCE"
    )
}

/// Infer the output ticker from the swap form's `Get` / `You receive` label.
///
/// Venue UIs render `Get` then the selected symbol on the next line(s); the
/// explicit `token_out` argument always wins over this heuristic.
pub fn infer_token_out(lines: &[String]) -> Option<String> {
    const LEG_LABELS: [&str; 5] = ["get", "you receive", "receive", "buy", "to"];
    for (i, line) in lines.iter().enumerate() {
        if !LEG_LABELS.contains(&line.trim().to_ascii_lowercase().as_str()) {
            continue;
        }
        for next in lines.iter().skip(i + 1).take(3) {
            let t = next.trim();
            if looks_like_ticker(t) {
                return Some(t.to_ascii_uppercase());
            }
        }
    }
    None
}

/// Extract `$`-denominated valuations from visible lines (`$12,060`,
/// `≈ $5,242`). The sell-side valuation is the largest on standard swap
/// forms: buy = sell discounted by fees and price impact. `$0` balance
/// lines drop out via `parse_amount_token`'s positive-only filter.
pub fn extract_usd_values(lines: &[String]) -> Vec<f64> {
    let mut out = Vec::new();
    for line in lines {
        let bytes = line.as_bytes();
        for (i, _) in line.match_indices('$') {
            if let Some(rest) = line.get(i..) {
                if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
                    if let Some(v) = parse_amount_token(rest) {
                        out.push(v);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    out
}

/// Compare the page's sell-side USD valuation against the intended trade's
/// expected value (`intended_amount × oracle unit price`).
///
/// Venue-side amount misparse (e.g. an ATM-style mask shifting the decimal
/// three places) shows up as a ratio far from 1, while spot-price drift
/// between the venue's feed and the oracle is a few percent — so a wide
/// band keeps false positives near zero. Generic: no venue names, works for
/// any dApp that renders a sell-side `$` estimate.
pub fn assess_sell_value(page_sell_usd: f64, expected_usd: f64) -> Value {
    let ratio = if expected_usd > 0.0 {
        page_sell_usd / expected_usd
    } else {
        0.0
    };
    let suspected = !(0.5..=2.0).contains(&ratio);
    json!({
        "page_sell_usd": page_sell_usd,
        "expected_usd": expected_usd,
        "ratio": ratio,
        "suspected_amount_misparse": suspected,
    })
}

/// Labeled output rows (`Expected Output` → value on same or following line).
fn extract_labeled_outputs(lines: &[String]) -> serde_json::Map<String, Value> {
    const LABELS: [(&str, &str); 5] = [
        ("total output", "total_output"),
        ("expected output", "expected_output"),
        ("minimum output", "minimum_output"),
        // 9mm 9X style.
        ("minimum received", "minimum_output"),
        ("min received", "minimum_output"),
    ];
    let mut out = serde_json::Map::new();
    for (i, line) in lines.iter().enumerate() {
        let ll = line.to_ascii_lowercase();
        for (prefix, key) in LABELS {
            if !ll.starts_with(prefix) || out.contains_key(key) {
                continue;
            }
            let same_line = parse_amount_token(&line[prefix.len()..]);
            let value = same_line.or_else(|| {
                (1..=3)
                    .filter_map(|k| lines.get(i + k))
                    .find(|l| !l.trim_start().starts_with('('))
                    .and_then(|l| parse_amount_token(l))
            });
            if let Some(v) = value {
                out.insert(key.to_string(), json!(v));
            }
        }
    }
    out
}

/// Parse swap quote hints from visible page lines (all frames).
pub fn parse_quote_hints(lines: &[String], token_out: &str) -> Value {
    let mut amounts = Vec::new();
    let mut status_lines = Vec::new();
    let mut connect_wallet = false;

    for line in lines {
        let ll = line.to_ascii_lowercase();
        if line.starts_with("//") || ll.contains("9mm") && token_out.eq_ignore_ascii_case("HEX") {
            continue;
        }
        if ll.contains("connect wallet") {
            connect_wallet = true;
        }
        if ll.contains("insufficient")
            || ll.contains("enter an amount")
            || ll.contains("select token")
            || ll.contains("no route")
            || ll.contains("no quote")
        {
            status_lines.push(line.clone());
        }
        for v in extract_amounts_near_token(line, token_out) {
            amounts.push(json!({
                "amount": v,
                "token": token_out,
                "line": line,
            }));
        }
    }

    let labeled = extract_labeled_outputs(lines);
    // Largest `$` valuation on the form — the sell-side estimate heuristic
    // used by the amount-misparse cross-check.
    let sell_usd = extract_usd_values(lines).into_iter().next();
    // Expected output is the post-fee number the user actually receives;
    // minimum output is the worst case — never the headline quote.
    let labeled_best = labeled
        .get("expected_output")
        .or_else(|| labeled.get("total_output"))
        .and_then(|v| v.as_f64());

    let numeric: Vec<f64> = amounts
        .iter()
        .filter_map(|a| a.get("amount").and_then(|v| v.as_f64()))
        .collect();
    let best = labeled_best.or_else(|| pick_best_output_amount(&numeric, token_out));
    let summary = best.map(|v| format!("{v} {token_out}"));

    json!({
        "ok": best.is_some(),
        "token_out": token_out,
        "summary": summary,
        "best": best,
        "labeled": labeled,
        "amounts": amounts,
        "status_lines": status_lines,
        "connect_wallet_visible": connect_wallet,
        "sell_usd": sell_usd,
        "line_count": lines.len(),
    })
}

/// Read the visible swap quote from all frames.
///
/// `token_out` resolution: explicit argument → inferred from the page's
/// `Get <SYM>` line → `HEX` fallback. When no amounts are visible but a
/// collapsed `Swap details` toggle is, it is expanded once and re-read.
pub async fn cdp_read_quote(
    cdp_http_url: &str,
    token_out: Option<&str>,
) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let mut lines = collect_visible_lines_all_frames(&mut page).await?;
    let resolve = |lines: &[String]| {
        token_out
            .map(|s| s.to_string())
            .or_else(|| infer_token_out(lines))
            .unwrap_or_else(|| "HEX".to_string())
    };
    let mut sym = resolve(&lines);
    let mut hints = parse_quote_hints(&lines, &sym);
    let mut expanded = false;

    let has_amounts = hints.get("best").and_then(|v| v.as_f64()).is_some();
    let details_toggle = lines
        .iter()
        .any(|l| l.trim().eq_ignore_ascii_case("swap details"));
    if !has_amounts && details_toggle {
        let clicked = deep_click_in_all_frames(&mut page, "Swap details").await?;
        if super::client::click_result_ok(&clicked) {
            tokio::time::sleep(Duration::from_millis(900)).await;
            lines = collect_visible_lines_all_frames(&mut page).await?;
            sym = resolve(&lines);
            hints = parse_quote_hints(&lines, &sym);
            expanded = true;
        }
    }

    if let Some(obj) = hints.as_object_mut() {
        obj.insert(
            "lines_sample".to_string(),
            json!(lines.iter().take(80).collect::<Vec<_>>()),
        );
        obj.insert("expanded_swap_details".to_string(), json!(expanded));
    }
    Ok(hints)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_quote_hints_finds_hex_on_plain_line() {
        let lines = vec![
            "Swap".into(),
            "1".into(),
            "PLS".into(),
            "13.8542 HEX".into(),
            "Refresh quote".into(),
        ];
        let q = parse_quote_hints(&lines, "HEX");
        assert_eq!(q.get("best").and_then(|v| v.as_f64()), Some(13.8542));
        assert_eq!(
            q.get("summary").and_then(|v| v.as_str()),
            Some("13.8542 HEX")
        );
    }

    #[test]
    fn parse_quote_hints_skips_connect_and_insufficient() {
        let lines = vec![
            "Connect Wallet".into(),
            "INSUFFICIENT_HEX".into(),
            "0.0".into(),
        ];
        let q = parse_quote_hints(&lines, "HEX");
        assert!(!q.get("ok").and_then(|v| v.as_bool()).unwrap_or(true));
        assert!(q.get("connect_wallet_visible").and_then(|v| v.as_bool()) == Some(true));
        assert_eq!(
            q.get("status_lines")
                .and_then(|v| v.as_array())
                .map(|a| a.len()),
            Some(1)
        );
    }

    #[test]
    fn infer_token_out_reads_get_leg() {
        let lines = vec![
            "Sell".into(),
            "INC".into(),
            "$0.59".into(),
            "Get".into(),
            "M3M3".into(),
            "Balance 0".into(),
        ];
        assert_eq!(infer_token_out(&lines), Some("M3M3".to_string()));
    }

    #[test]
    fn infer_token_out_ignores_amounts_and_words() {
        let lines = vec!["Get".into(), "$0".into(), "0.00".into(), "Balance 0".into()];
        assert_eq!(infer_token_out(&lines), None);
    }

    #[test]
    fn usd_values_pick_sell_side_as_max() {
        // Real Switch.win lines from 2026-08-29: sell $12,060 / buy $11,898.62.
        let lines = vec![
            "Sell".into(),
            "PLS".into(),
            "$12,060".into(),
            "USD".into(),
            "Balance 0 PLS".into(),
            "$0".into(),
            "Get".into(),
            "USDC".into(),
            "$11,898.62".into(),
        ];
        let usd = extract_usd_values(&lines);
        assert_eq!(usd.first().copied(), Some(12060.0));
        assert!(usd.contains(&11898.62));
        // "$0" balance lines are dropped.
        assert!(!usd.contains(&0.0));
        let q = parse_quote_hints(&lines, "USDC");
        assert_eq!(q.get("sell_usd").and_then(|v| v.as_f64()), Some(12060.0));
    }

    #[test]
    fn sell_value_check_flags_thousandfold_misparse_only() {
        // Venue parsed 1,000 PLS instead of 1,000,000 → ratio ≈ 0.001.
        let flagged = assess_sell_value(12.06, 12060.0);
        assert_eq!(
            flagged
                .get("suspected_amount_misparse")
                .and_then(|v| v.as_bool()),
            Some(true)
        );
        // Correct parse with a few % of price-feed drift → no flag.
        let ok = assess_sell_value(12060.0, 12069.20);
        assert_eq!(
            ok.get("suspected_amount_misparse")
                .and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            ok.get("ratio")
                .and_then(|v| v.as_f64())
                .map(|r| (r * 1000.0).round() / 1000.0),
            Some(0.999)
        );
    }

    #[test]
    fn labeled_outputs_prefer_expected_over_minimum() {
        let lines = vec![
            "Swap details".into(),
            "Total Output".into(),
            "1,611,295.2965".into(),
            "Expected Output".into(),
            "(after 0.1% input fee)".into(),
            "1,609,694.5634".into(),
            "Minimum Output".into(),
            "1,601,646.0906".into(),
        ];
        let q = parse_quote_hints(&lines, "M3M3");
        assert_eq!(
            q.pointer("/labeled/expected_output")
                .and_then(|v| v.as_f64()),
            Some(1609694.5634)
        );
        assert_eq!(
            q.pointer("/labeled/minimum_output")
                .and_then(|v| v.as_f64()),
            Some(1601646.0906)
        );
        // Best is the expected (post-fee) output, never the minimum.
        assert_eq!(q.get("best").and_then(|v| v.as_f64()), Some(1609694.5634));
        assert_eq!(
            q.get("summary").and_then(|v| v.as_str()),
            Some("1609694.5634 M3M3")
        );
    }
}
