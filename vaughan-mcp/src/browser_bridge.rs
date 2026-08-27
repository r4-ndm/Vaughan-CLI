//! MCP browser control tools (VB / CDP B1).
//!
//! Opens or navigates the optional `vaughan-dapp-browser` shell. Never signs —
//! signing stays in the TUI/provider.

use serde_json::{json, Value};
use std::time::Duration;

use vaughan_core::core::vb_browser::{
    allow_suffixes_for_profile, cdp_alive, cdp_list_pages, cdp_open_url, check_url_allowed,
    read_vb_session, resolve_cdp_port, spawn_dapp_browser, wait_for_cdp,
};
use vaughan_core::core::vb_cdp::{self, ElementRef};
use vaughan_core::error::WalletError;

use crate::McpContext;

fn empty_object_schema() -> Value {
    json!({ "type": "object", "properties": {} })
}

fn url_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "HTTPS URL (must pass VB allowlist)"
            }
        },
        "required": ["url"]
    })
}

/// MCP `tools/list` entries for browser B1 + B2 tools.
pub fn browser_tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "browser_open",
            "description": "Open allowlisted URL in vaughan-dapp-browser (VB). Requires VB binary on PATH; enables CDP when VAUGHAN_DAPP_BROWSER_CDP_PORT is set (default 9222). Never signs.",
            "inputSchema": url_schema(),
        }),
        json!({
            "name": "browser_navigate",
            "description": "Navigate the active VB session to an allowlisted URL via CDP. Requires a running VB child with CDP.",
            "inputSchema": url_schema(),
        }),
        json!({
            "name": "browser_status",
            "description": "VB session + CDP health: cdp_url, open pages, allowlist suffix count.",
            "inputSchema": empty_object_schema(),
        }),
        json!({
            "name": "browser_snapshot",
            "description": "Snapshot interactive page elements (refs e0..e49) via CDP. Requires browser_open + CDP. Never signs.",
            "inputSchema": empty_object_schema(),
        }),
        json!({
            "name": "browser_click",
            "description": "Click element by ref from browser_snapshot (e.g. e3). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string", "description": "Element ref from browser_snapshot (e0, e1, …)" }
                },
                "required": ["ref"]
            }),
        }),
        json!({
            "name": "browser_type",
            "description": "Focus element ref and insert text. Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "ref": { "type": "string" },
                    "text": { "type": "string" }
                },
                "required": ["ref", "text"]
            }),
        }),
        json!({
            "name": "browser_press",
            "description": "Press key on active page (Enter, Tab, Escape, Arrow*, Space, Backspace, Delete). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string" }
                },
                "required": ["key"]
            }),
        }),
        json!({
            "name": "browser_wait",
            "description": "Wait until text, CSS selector, or URL substring matches (default timeout 10s). Never signs.",
            "inputSchema": json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string" },
                    "selector": { "type": "string" },
                    "url_contains": { "type": "string" },
                    "timeout_ms": { "type": "integer", "description": "Default 10000" }
                }
            }),
        }),
    ]
}

pub fn browser_tool_names() -> &'static [&'static str] {
    &[
        "browser_open",
        "browser_navigate",
        "browser_status",
        "browser_snapshot",
        "browser_click",
        "browser_type",
        "browser_press",
        "browser_wait",
    ]
}

fn wallet_err(e: WalletError) -> String {
    e.user_message()
}

fn suffixes_for_nav(ctx: &McpContext, url: &str, session_suffixes: &[String]) -> Vec<String> {
    if !session_suffixes.is_empty() {
        return session_suffixes.to_vec();
    }
    allow_suffixes_for_profile(&ctx.profile, url).unwrap_or_default()
}

pub async fn browser_open(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing url".to_string())?;
    let suffixes = allow_suffixes_for_profile(&ctx.profile, url).map_err(wallet_err)?;
    check_url_allowed(url, &suffixes).map_err(wallet_err)?;

    let cdp_port = resolve_cdp_port();
    spawn_dapp_browser(url, &suffixes, cdp_port).map_err(wallet_err)?;

    let cdp_url = format!("http://127.0.0.1:{cdp_port}");
    let alive = if cdp_port != 0 {
        wait_for_cdp(&cdp_url, Duration::from_secs(5)).await
    } else {
        false
    };

    Ok(json!({
        "status": "opened",
        "url": url,
        "cdp_port": cdp_port,
        "cdp_alive": alive,
        "allow_suffixes": suffixes.len(),
        "hint": if alive {
            "VB running with CDP — use browser_navigate / browser_status"
        } else if cdp_port == 0 {
            "VB opened without CDP — set VAUGHAN_DAPP_BROWSER_CDP_PORT for agent control"
        } else {
            "VB spawned; CDP not yet reachable — retry browser_status"
        },
    }))
}

pub async fn browser_navigate(args: Value, ctx: &McpContext) -> Result<Value, String> {
    let url = args
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing url".to_string())?;

    let session = read_vb_session().map_err(wallet_err)?.ok_or_else(|| {
        "browser_unavailable: no vb.session — run browser_open first with CDP enabled".to_string()
    })?;

    if !cdp_alive(&session.cdp_url).await {
        return Err(
            "browser_unavailable: VB CDP endpoint not reachable — reopen with browser_open"
                .to_string(),
        );
    }

    let suffixes = suffixes_for_nav(ctx, url, &session.allow_suffixes);
    check_url_allowed(url, &suffixes).map_err(wallet_err)?;

    cdp_open_url(&session.cdp_url, url)
        .await
        .map_err(wallet_err)?;

    Ok(json!({
        "status": "navigated",
        "url": url,
        "cdp_url": session.cdp_url,
    }))
}

pub async fn browser_status(_ctx: &McpContext) -> Result<Value, String> {
    let Some(session) = read_vb_session().map_err(wallet_err)? else {
        return Ok(json!({
            "available": false,
            "reason": "no_vb_session",
            "hint": "Call browser_open with VAUGHAN_DAPP_BROWSER_CDP_PORT set (default 9222)",
        }));
    };

    let alive = cdp_alive(&session.cdp_url).await;
    if !alive {
        return Ok(json!({
            "available": false,
            "reason": "cdp_unreachable",
            "cdp_url": session.cdp_url,
            "allow_suffixes_count": session.allow_suffixes.len(),
            "hint": "VB child not running or CDP port changed — browser_open again",
        }));
    }

    let pages = cdp_list_pages(&session.cdp_url).await.map_err(wallet_err)?;
    let rows: Vec<Value> = pages
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("page"))
        .map(|p| {
            json!({
                "url": p.get("url").and_then(|u| u.as_str()).unwrap_or(""),
                "title": p.get("title").and_then(|t| t.as_str()).unwrap_or(""),
            })
        })
        .collect();

    Ok(json!({
        "available": true,
        "cdp_url": session.cdp_url,
        "allow_suffixes_count": session.allow_suffixes.len(),
        "pages": rows,
    }))
}

async fn require_cdp_session() -> Result<String, String> {
    let session = read_vb_session().map_err(wallet_err)?.ok_or_else(|| {
        "browser_unavailable: no vb.session — run browser_open first with CDP enabled".to_string()
    })?;
    if !cdp_alive(&session.cdp_url).await {
        return Err(
            "browser_unavailable: VB CDP endpoint not reachable — reopen with browser_open"
                .to_string(),
        );
    }
    Ok(session.cdp_url)
}

pub async fn browser_snapshot(_ctx: &McpContext) -> Result<Value, String> {
    let cdp_url = require_cdp_session().await?;
    let snap = vb_cdp::cdp_snapshot(&cdp_url).await.map_err(wallet_err)?;
    Ok(json!({ "status": "snapshot", "cdp_url": cdp_url, "page": snap }))
}

pub async fn browser_click(args: Value, _ctx: &McpContext) -> Result<Value, String> {
    let ref_raw = args
        .get("ref")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing ref".to_string())?;
    let element_ref = ElementRef::parse(ref_raw).map_err(wallet_err)?;
    let cdp_url = require_cdp_session().await?;
    let result = vb_cdp::cdp_click(&cdp_url, element_ref)
        .await
        .map_err(wallet_err)?;
    Ok(json!({ "status": "clicked", "ref": ref_raw, "result": result }))
}

pub async fn browser_type(args: Value, _ctx: &McpContext) -> Result<Value, String> {
    let ref_raw = args
        .get("ref")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing ref".to_string())?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing text".to_string())?;
    let element_ref = ElementRef::parse(ref_raw).map_err(wallet_err)?;
    let cdp_url = require_cdp_session().await?;
    let result = vb_cdp::cdp_type(&cdp_url, element_ref, text)
        .await
        .map_err(wallet_err)?;
    Ok(json!({
        "status": "typed",
        "ref": ref_raw,
        "result": result,
    }))
}

pub async fn browser_press(args: Value, _ctx: &McpContext) -> Result<Value, String> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing key".to_string())?;
    let cdp_url = require_cdp_session().await?;
    let result = vb_cdp::cdp_press(&cdp_url, key).await.map_err(wallet_err)?;
    Ok(json!({ "status": "pressed", "key": key, "result": result }))
}

pub async fn browser_wait(args: Value, _ctx: &McpContext) -> Result<Value, String> {
    let text = args.get("text").and_then(|v| v.as_str());
    let selector = args.get("selector").and_then(|v| v.as_str());
    let url_contains = args.get("url_contains").and_then(|v| v.as_str());
    let timeout_ms = args
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(10_000);
    let cdp_url = require_cdp_session().await?;
    let result = vb_cdp::cdp_wait(
        &cdp_url,
        text,
        selector,
        url_contains,
        Duration::from_millis(timeout_ms),
    )
    .await
    .map_err(wallet_err)?;
    Ok(json!({ "status": "wait_met", "result": result }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_defs_count() {
        assert_eq!(browser_tool_definitions().len(), browser_tool_names().len());
    }
}
