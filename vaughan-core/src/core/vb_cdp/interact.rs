//! Generic page interaction: click/type/press/wait, modal dismissal, wallet connect.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::client::{click_result_ok, click_text_cascade, evaluate_in_all_frames, CdpPage};
use super::js;
use crate::error::WalletError;

/// Click an element by snapshot ref (`browser_click`).
pub async fn cdp_click(
    cdp_http_url: &str,
    element_ref: super::ElementRef,
) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    page.evaluate(&js::click_ref(element_ref.0)).await
}

/// Click the first visible element whose text contains `text` (all frames, shadow DOM, DOM search).
pub async fn cdp_click_by_text(cdp_http_url: &str, text: &str) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    click_text_cascade(&mut page, text).await
}

/// Dismiss common consent / cookie / disclaimer overlays before driving swap UI.
pub async fn cdp_dismiss_modals(cdp_http_url: &str) -> Result<Value, WalletError> {
    let labels = [
        "I Understand & Accept",
        "I Understand",
        "Accept",
        "Got it",
        "Agree",
        "Continue",
    ];
    let mut steps = Vec::new();
    for label in labels {
        let r = cdp_click_by_text(cdp_http_url, label).await?;
        let ok = click_result_ok(&r);
        steps.push(json!({ "label": label, "ok": ok }));
        if ok {
            tokio::time::sleep(Duration::from_millis(600)).await;
            return Ok(json!({ "ok": true, "dismissed": label, "steps": steps }));
        }
    }
    Ok(json!({ "ok": false, "steps": steps }))
}

/// Connect wallet on a dApp: open Connect → pick Vaughan/Injected (shadow DOM + snapshot refs).
///
/// After the provider click, the page is polled for connected state (address
/// chip / Connect CTA gone). Clicking "Vaughan" only *requests* accounts —
/// `eth_requestAccounts` may await TUI approval or never fire — so the result
/// reports `connected` separately from click success.
pub async fn cdp_connect_vaughan_wallet(cdp_http_url: &str) -> Result<Value, WalletError> {
    let mut steps = Vec::new();
    let mut modal_open = false;
    for label in ["Connect Wallet", "Connect"] {
        let r = cdp_click_by_text(cdp_http_url, label).await?;
        steps.push(json!({ "step": "open_modal", "label": label, "result": r.clone() }));
        if click_result_ok(&r) {
            modal_open = true;
            tokio::time::sleep(Duration::from_millis(800)).await;
            break;
        }
    }
    if !modal_open {
        return Ok(json!({ "ok": false, "error": "connect button not found", "steps": steps }));
    }
    for provider in ["Vaughan", "Injected", "MetaMask"] {
        let r = cdp_click_by_text(cdp_http_url, provider).await?;
        steps.push(json!({ "step": "pick_provider", "provider": provider, "result": r.clone() }));
        if click_result_ok(&r) {
            tokio::time::sleep(Duration::from_millis(1200)).await;
            let connected = poll_connect_state(cdp_http_url, Duration::from_secs(8)).await;
            return Ok(json!({
                "ok": true,
                "provider": provider,
                "steps": steps,
                "connected": connected,
                "note": if connected {
                    "dApp sees the wallet"
                } else {
                    "no connected state observed — approve eth_requestAccounts in the Vaughan TUI if prompted; some dApps also require a chain switch"
                },
            }));
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    Ok(json!({ "ok": false, "error": "wallet provider not found", "steps": steps }))
}

/// Poll the page for post-connect state (address chip visible / Connect CTA gone).
async fn poll_connect_state(cdp_http_url: &str, max_wait: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < max_wait {
        if let Ok(mut page) = CdpPage::connect_first_page(cdp_http_url).await {
            if let Ok(r) = evaluate_in_all_frames(&mut page, js::connect_state()).await {
                let inner = r.get("result").cloned().unwrap_or(r);
                if inner
                    .get("connected")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    return true;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    false
}

/// Numeric-aware comparison for amount inputs: masks add thousands separators
/// and trailing decimals, so compare parsed values rather than raw strings.
fn amount_value_matches(intended: &str, actual: &str) -> bool {
    let norm = |s: &str| -> Option<f64> {
        let t: String = s
            .trim()
            .chars()
            .filter(|c| !matches!(c, ',' | '_' | ' '))
            .collect();
        if t.is_empty() {
            return None;
        }
        t.parse::<f64>().ok().filter(|v| v.is_finite())
    };
    match (norm(intended), norm(actual)) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-9 * a.abs().max(1.0),
        _ => intended.trim() == actual.trim(),
    }
}

/// Read the marked type target's value from whichever frame holds it.
async fn read_marked_value(page: &mut CdpPage) -> Option<String> {
    let r = evaluate_in_all_frames(page, js::read_type_target())
        .await
        .ok()?;
    let inner = r.get("result").cloned().unwrap_or(r);
    inner
        .get("value")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Read the marked field after masks settle, then re-read after debounced /
/// async reformatting would have fired. Returns the final value and whether
/// it matches the intended text.
///
/// The delayed second read matters: Switch.win was observed showing the typed
/// `1000000` correctly at +450ms, then a debounced mask rewrote it to
/// `1000.000` and quoted a 1000× smaller trade.
async fn verify_marked_value(page: &mut CdpPage, text: &str) -> (Option<String>, bool) {
    tokio::time::sleep(Duration::from_millis(450)).await;
    let first = read_marked_value(page).await;
    if !first
        .as_deref()
        .map(|v| amount_value_matches(text, v))
        .unwrap_or(false)
    {
        return (first, false);
    }
    tokio::time::sleep(Duration::from_millis(1400)).await;
    let settled = read_marked_value(page).await;
    let ok = settled
        .as_deref()
        .map(|v| amount_value_matches(text, v))
        .unwrap_or(false);
    (settled.or(first), ok)
}

/// Type into the marked element as faithfully as a human, then verify the
/// settled value matches. Strategy order: real per-char key events →
/// whole-string `Input.insertText` → legacy native-setter write. The first
/// strategy whose value survives the delayed re-verify wins; if none does,
/// the result reports `verified: false` with the field's final value so the
/// agent knows the quote is for the wrong amount.
pub(crate) async fn type_into_marked(page: &mut CdpPage, text: &str) -> Result<Value, WalletError> {
    let text_json =
        serde_json::to_string(text).map_err(|e| WalletError::Serialization(e.to_string()))?;

    // Strategy 1: per-char real key events (indistinguishable from typing).
    for ch in text.chars() {
        page.type_char(ch).await?;
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let (value, verified) = verify_marked_value(page, text).await;
    if verified {
        return Ok(
            json!({ "ok": true, "value": value, "verified": true, "strategy": "key-events" }),
        );
    }
    let mut last = value;

    // Strategy 2: whole-string insertText (paste-like, real input events).
    let _ = evaluate_in_all_frames(page, js::reselect_type_target()).await;
    page.insert_text(text).await?;
    let (value, verified) = verify_marked_value(page, text).await;
    if verified {
        return Ok(
            json!({ "ok": true, "value": value, "verified": true, "strategy": "insert-text" }),
        );
    }
    last = value.or(last);

    // Strategy 3: legacy native-setter write (unmasked inputs only, really).
    let _ = evaluate_in_all_frames(page, &js::set_marked_value(&text_json)).await;
    let (value, verified) = verify_marked_value(page, text).await;
    Ok(json!({
        "ok": true,
        "value": value.clone().or(last),
        "verified": verified,
        "strategy": "native-setter",
        "warning": if verified { Value::Null } else { json!("field value does not match intended input — quote may be for the wrong amount") },
    }))
}

/// Focus ref and type text (`browser_type`). When `clear` is true, replace value first.
///
/// The write goes through the real input pipeline and is verified by reading
/// the field back; the result carries `value` and `verified` so agents can
/// trust (or catch) what the dApp actually parsed.
pub async fn cdp_type(
    cdp_http_url: &str,
    element_ref: super::ElementRef,
    text: &str,
    clear: bool,
) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let focus = js::focus_ref(element_ref.0, clear);
    let focused = evaluate_in_all_frames(&mut page, &focus).await?;
    let focus_inner = focused.get("result").cloned().unwrap_or(focused.clone());
    if focus_inner.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(focus_inner);
    }
    let mut out = type_into_marked(&mut page, text).await?;
    if let Some(obj) = out.as_object_mut() {
        obj.insert("ref".to_string(), json!(format!("e{}", element_ref.0)));
        obj.insert("typed_len".to_string(), json!(text.chars().count()));
    }
    Ok(out)
}

/// Press a key on the focused page (`browser_press`).
pub async fn cdp_press(cdp_http_url: &str, key: &str) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    page.dispatch_key(key, true).await?;
    page.dispatch_key(key, false).await?;
    Ok(json!({ "ok": true, "key": key }))
}

/// Wait for text, selector, or URL substring (`browser_wait`).
pub async fn cdp_wait(
    cdp_http_url: &str,
    text: Option<&str>,
    selector: Option<&str>,
    url_contains: Option<&str>,
    timeout: Duration,
) -> Result<Value, WalletError> {
    if text.is_none() && selector.is_none() && url_contains.is_none() {
        return Err(WalletError::InvalidTransaction(
            "browser_wait: provide text, selector, or url_contains".into(),
        ));
    }
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        let expr = build_wait_expression(text, selector, url_contains);
        let v = page.evaluate(&expr).await?;
        if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
            return Ok(v);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(WalletError::NetworkError(format!(
        "browser_wait timeout after {}ms",
        timeout.as_millis()
    )))
}

fn build_wait_expression(
    text: Option<&str>,
    selector: Option<&str>,
    url_contains: Option<&str>,
) -> String {
    let text_lit = text
        .map(|s| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()))
        .unwrap_or_else(|| "null".into());
    let sel_lit = selector
        .map(|s| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()))
        .unwrap_or_else(|| "null".into());
    let url_lit = url_contains
        .map(|s| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()))
        .unwrap_or_else(|| "null".into());
    js::wait_probe(&text_lit, &sel_lit, &url_lit)
}

#[cfg(test)]
mod tests {
    use super::amount_value_matches;

    #[test]
    fn amount_match_tolerates_mask_formatting() {
        assert!(amount_value_matches("1000000", "1,000,000"));
        assert!(amount_value_matches("1000000", "1000000.000"));
        assert!(amount_value_matches("1.5", "1.50"));
    }

    #[test]
    fn amount_match_catches_mask_misparse() {
        // The observed ÷1000 failure: typed 1000000, field settled on 1000.000.
        assert!(!amount_value_matches("1000000", "1000.000"));
        assert!(!amount_value_matches("5000", "5.000"));
        assert!(!amount_value_matches("1000000", ""));
    }
}
