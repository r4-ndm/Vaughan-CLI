//! Swap-form driving: token pickers, sell amount, quote CTA, one-shot setup.
//!
//! Picker flow per leg: open the modal (retrying past venue chrome that looks
//! like a ticker), probe that a modal really opened, type the symbol into the
//! search box when one exists, then click the matching row from fresh DOM.

use std::time::Duration;

use serde_json::{json, Value};

use super::client::{deep_click_in_all_frames, evaluate_in_all_frames, CdpPage};
use super::js;
use crate::error::WalletError;

/// Which leg of a swap form to target (top ≈ input, bottom ≈ output on most UIs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapTokenSide {
    Input,
    Output,
}

impl SwapTokenSide {
    fn as_str(self) -> &'static str {
        match self {
            Self::Input => "input",
            Self::Output => "output",
        }
    }
}

/// Normalize user/agent token labels to a search symbol (uppercase ticker).
pub fn normalize_swap_symbol(raw: &str) -> String {
    let s = raw.trim().to_ascii_uppercase();
    match s.as_str() {
        "NATIVE" | "PLS" | "PULSE" | "ETH" | "TPLS" => "PLS".into(),
        "WPLS" => "WPLS".into(),
        other if other.starts_with("0X") => other.to_string(),
        other => other.to_string(),
    }
}

fn json_string(s: &str) -> Result<String, WalletError> {
    serde_json::to_string(s).map_err(|e| WalletError::Serialization(e.to_string()))
}

/// True when the picker modal probe found a dialog/search box in any frame.
async fn picker_modal_open(page: &mut CdpPage) -> bool {
    evaluate_in_all_frames(page, js::MODAL_PROBE)
        .await
        .map(|r| {
            r.pointer("/result/ok").and_then(|v| v.as_bool()) == Some(true)
                || r.get("ok").and_then(|v| v.as_bool()) == Some(true)
        })
        .unwrap_or(false)
}

/// Open the token picker on `side` and choose `symbol` (e.g. PLS, HEX, M3M3).
pub async fn cdp_select_swap_token(
    cdp_http_url: &str,
    symbol: &str,
    side: SwapTokenSide,
) -> Result<Value, WalletError> {
    let sym = normalize_swap_symbol(symbol);
    let sym_json = json_string(&sym)?;
    let side_js = side.as_str();

    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;

    // Step 1: click the token selector on the requested leg. Venue tabs
    // ("Switch", "Limit") can match the ticker shape, so if no modal appears
    // after the first click, retry once while skipping that label.
    let mut open = evaluate_in_all_frames(
        &mut page,
        &js::open_token_picker(&sym_json, side_js, "null"),
    )
    .await?;
    let mut open_inner = open.get("result").cloned().unwrap_or(open.clone());
    if open_inner.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(open_inner);
    }
    if open_inner.get("already").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(open_inner);
    }

    tokio::time::sleep(Duration::from_millis(1000)).await;
    if !picker_modal_open(&mut page).await {
        let shown = open_inner
            .get("shown")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !shown.is_empty() {
            let avoid_json = json_string(&shown)?;
            open = evaluate_in_all_frames(
                &mut page,
                &js::open_token_picker(&sym_json, side_js, &avoid_json),
            )
            .await?;
            open_inner = open.get("result").cloned().unwrap_or(open.clone());
            if open_inner.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                return Ok(
                    json!({ "open": open_inner, "pick": Value::Null, "symbol": sym, "side": side_js }),
                );
            }
            if open_inner.get("already").and_then(|v| v.as_bool()) == Some(true) {
                return Ok(open_inner);
            }
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }

    // Step 2: type the symbol into the picker search box (no-op when the
    // venue lists all tokens), then let the filtered list settle.
    let search = evaluate_in_all_frames(&mut page, &js::search_token(&sym_json)).await?;
    let searched = search
        .pointer("/result/searched")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    tokio::time::sleep(Duration::from_millis(if searched { 700 } else { 200 })).await;

    // Step 3: click the matching row from fresh (post-search) DOM.
    let mut pick = evaluate_in_all_frames(&mut page, &js::pick_token(&sym_json)).await?;
    if pick.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        tokio::time::sleep(Duration::from_millis(900)).await;
        pick = evaluate_in_all_frames(&mut page, &js::pick_token(&sym_json)).await?;
    }
    if pick.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        pick = deep_click_in_all_frames(&mut page, &sym).await?;
    }
    if pick.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let _ = page.dispatch_key("Escape", true).await;
        let _ = page.dispatch_key("Escape", false).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    Ok(json!({
        "open": open,
        "search": search,
        "pick": pick,
        "symbol": sym,
        "side": side_js,
    }))
}

/// Set the sell amount on the swap form.
///
/// Uses the same verified real-pipeline typing as `browser_type` — the
/// response carries `value` (post-mask read-back) and `verified`, so a
/// mis-parsed amount is visible instead of silently quoting the wrong size.
pub async fn cdp_set_swap_amount(cdp_http_url: &str, amount: &str) -> Result<Value, WalletError> {
    cdp_set_swap_amount_with_strategy(cdp_http_url, amount, super::interact::TypeStrategy::Auto)
        .await
}

/// [`cdp_set_swap_amount`] with an explicit typing strategy override.
pub async fn cdp_set_swap_amount_with_strategy(
    cdp_http_url: &str,
    amount: &str,
    strategy: super::interact::TypeStrategy,
) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let focused = evaluate_in_all_frames(&mut page, js::focus_amount()).await?;
    let focus_inner = focused.get("result").cloned().unwrap_or(focused.clone());
    if focus_inner.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(focus_inner);
    }
    let mut out = super::interact::type_into_marked_with(&mut page, amount, strategy).await?;
    if let Some(obj) = out.as_object_mut() {
        obj.insert("amount_in".to_string(), json!(amount));
    }
    Ok(out)
}

/// Click the primary quote/swap CTA after tokens + amount (e.g. Switch.win "Switch Now").
pub async fn cdp_click_swap_submit(cdp_http_url: &str) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let mut result = evaluate_in_all_frames(&mut page, js::CLICK_SWAP_SUBMIT).await?;
    if result.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        for label in [
            "Switch Now",
            "Swap Now",
            "Get quote",
            "Review swap",
            "Swap",
            "Refresh quote",
        ] {
            result = deep_click_in_all_frames(&mut page, label).await?;
            if result.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                break;
            }
        }
    }
    if result.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(result);
    }
    if let Some(inner) = result.get("result") {
        return Ok(inner.clone());
    }
    Ok(result)
}

/// Select input/output tokens and set sell amount — explicit swap setup (not page defaults).
pub async fn cdp_setup_swap(
    cdp_http_url: &str,
    token_in: &str,
    token_out: &str,
    amount_in: &str,
    submit_quote: bool,
) -> Result<Value, WalletError> {
    cdp_setup_swap_with_strategy(
        cdp_http_url,
        token_in,
        token_out,
        amount_in,
        submit_quote,
        super::interact::TypeStrategy::Auto,
    )
    .await
}

/// [`cdp_setup_swap`] with an explicit typing strategy override.
#[allow(clippy::too_many_arguments)]
pub async fn cdp_setup_swap_with_strategy(
    cdp_http_url: &str,
    token_in: &str,
    token_out: &str,
    amount_in: &str,
    submit_quote: bool,
    strategy: super::interact::TypeStrategy,
) -> Result<Value, WalletError> {
    let in_res = cdp_select_swap_token(cdp_http_url, token_in, SwapTokenSide::Input).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let out_res = cdp_select_swap_token(cdp_http_url, token_out, SwapTokenSide::Output).await?;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let amt_res = cdp_set_swap_amount_with_strategy(cdp_http_url, amount_in, strategy).await?;
    let submit_res = if submit_quote {
        tokio::time::sleep(Duration::from_millis(800)).await;
        Some(cdp_click_swap_submit(cdp_http_url).await?)
    } else {
        None
    };
    Ok(json!({
        "token_in": normalize_swap_symbol(token_in),
        "token_out": normalize_swap_symbol(token_out),
        "amount_in": amount_in,
        "input": in_res,
        "output": out_res,
        "amount": amt_res,
        "submit": submit_res,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_swap_symbol_native() {
        assert_eq!(normalize_swap_symbol("native"), "PLS");
        assert_eq!(normalize_swap_symbol("pls"), "PLS");
        assert_eq!(normalize_swap_symbol("HEX"), "HEX");
    }
}
