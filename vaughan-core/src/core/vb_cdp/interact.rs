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
            return Ok(json!({
                "ok": true,
                "provider": provider,
                "steps": steps,
                "note": "approve eth_requestAccounts in Vaughan TUI if prompted",
            }));
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    Ok(json!({ "ok": false, "error": "wallet provider not found", "steps": steps }))
}

/// Focus ref and type text (`browser_type`). When `clear` is true, replace value first.
pub async fn cdp_type(
    cdp_http_url: &str,
    element_ref: super::ElementRef,
    text: &str,
    clear: bool,
) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let text_json =
        serde_json::to_string(text).map_err(|e| WalletError::Serialization(e.to_string()))?;
    let react = js::type_into_ref(element_ref.0, &text_json, clear, text.chars().count());
    let result = evaluate_in_all_frames(&mut page, &react).await?;
    if result.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        if let Some(inner) = result.get("result") {
            return Ok(inner.clone());
        }
    }
    if result.get("ref").is_some() {
        return Ok(result);
    }
    page.evaluate(&format!("(() => {react})()")).await
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
