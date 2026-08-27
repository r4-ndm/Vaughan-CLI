//! CDP WebSocket client for VB agent navigation (MCP B2).
//!
//! Uses `Runtime.evaluate` + `Input.*` over a page WebSocket — same approach as
//! `docs/spikes/cef-tauri` `cdp_ax_smoke`, without chromiumoxide.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::core::vb_browser::cdp_list_pages;
use crate::error::WalletError;

const INTERACTIVE_SELECTOR: &str = "a,button,input,textarea,select,[role=button],[role=link]";

/// Parsed element ref from `browser_snapshot` (e.g. `e0`, `e12`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementRef(pub u32);

impl ElementRef {
    /// Parse `e{N}` refs returned by [`cdp_snapshot`].
    pub fn parse(raw: &str) -> Result<Self, WalletError> {
        let t = raw.trim();
        let idx = t
            .strip_prefix('e')
            .or_else(|| t.strip_prefix('E'))
            .ok_or_else(|| WalletError::InvalidTransaction(format!("invalid ref `{raw}`")))?;
        let n: u32 = idx
            .parse()
            .map_err(|_| WalletError::InvalidTransaction(format!("invalid ref `{raw}`")))?;
        Ok(Self(n))
    }
}

/// Lightweight CDP session bound to one page target.
pub struct CdpPage {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next_id: u64,
}

impl CdpPage {
    /// Attach to the first `type=page` target advertised by CDP HTTP `/json/list`.
    pub async fn connect_first_page(cdp_http_url: &str) -> Result<Self, WalletError> {
        let ws_url = cdp_page_ws_url(cdp_http_url).await?;
        let (ws, _) = connect_async(&ws_url)
            .await
            .map_err(|e| WalletError::NetworkError(format!("cdp ws connect: {e}")))?;
        Ok(Self { ws, next_id: 1 })
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, WalletError> {
        let id = self.next_id;
        self.next_id += 1;
        let req = json!({ "id": id, "method": method, "params": params });
        self.ws
            .send(Message::Text(req.to_string().into()))
            .await
            .map_err(|e| WalletError::NetworkError(format!("cdp send: {e}")))?;

        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            let msg = tokio::time::timeout(Duration::from_secs(25), self.ws.next())
                .await
                .map_err(|_| WalletError::NetworkError("cdp recv timeout".into()))?
                .ok_or_else(|| WalletError::NetworkError("cdp ws closed".into()))?
                .map_err(|e| WalletError::NetworkError(format!("cdp recv: {e}")))?;

            let text = msg
                .into_text()
                .map_err(|e| WalletError::NetworkError(format!("cdp non-text: {e}")))?;
            let v: Value = serde_json::from_str(&text)
                .map_err(|e| WalletError::Serialization(format!("cdp json: {e}")))?;
            if v.get("id").and_then(|x| x.as_u64()) != Some(id) {
                continue;
            }
            if let Some(err) = v.get("error") {
                return Err(WalletError::NetworkError(format!("cdp {method}: {err}")));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
        Err(WalletError::NetworkError(format!(
            "cdp {method}: no response"
        )))
    }

    async fn evaluate(&mut self, expression: &str) -> Result<Value, WalletError> {
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        extract_eval_value(&result)
    }

    async fn insert_text(&mut self, text: &str) -> Result<(), WalletError> {
        self.call("Input.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    async fn dispatch_key(&mut self, key: &str, key_down: bool) -> Result<(), WalletError> {
        let (code, vk) = key_definition(key)?;
        let event_type = if key_down { "keyDown" } else { "keyUp" };
        self.call(
            "Input.dispatchKeyEvent",
            json!({
                "type": event_type,
                "key": key,
                "code": code,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
            }),
        )
        .await?;
        Ok(())
    }
}

/// WebSocket debugger URL for the first page target.
pub async fn cdp_page_ws_url(cdp_http_url: &str) -> Result<String, WalletError> {
    let pages = cdp_list_pages(cdp_http_url).await?;
    for page in &pages {
        if page.get("type").and_then(|t| t.as_str()) != Some("page") {
            continue;
        }
        if let Some(ws) = page.get("webSocketDebuggerUrl").and_then(|u| u.as_str()) {
            if !ws.is_empty() {
                return Ok(ws.to_string());
            }
        }
    }
    Err(WalletError::NetworkError(
        "cdp: no page target with webSocketDebuggerUrl".into(),
    ))
}

/// Interactive element snapshot for agents (`browser_snapshot`).
pub async fn cdp_snapshot(cdp_http_url: &str) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let expr = format!(
        r#"(() => {{
          const sel = '{INTERACTIVE_SELECTOR}';
          const refs = [...document.querySelectorAll(sel)].slice(0, 50).map((e, i) => ({{
            ref: `e${{i}}`,
            tag: e.tagName.toLowerCase(),
            role: e.getAttribute('role') || null,
            name: (e.innerText || e.getAttribute('aria-label') || e.value || e.href || '').trim().slice(0, 80)
          }}));
          return {{ title: document.title, url: location.href, refs }};
        }})()"#
    );
    page.evaluate(&expr).await
}

/// Click an element by snapshot ref (`browser_click`).
pub async fn cdp_click(cdp_http_url: &str, element_ref: ElementRef) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let idx = element_ref.0;
    let expr = format!(
        r#"(() => {{
          const sel = '{INTERACTIVE_SELECTOR}';
          const els = [...document.querySelectorAll(sel)];
          const e = els[{idx}];
          if (!e) return {{ ok: false, error: 'ref not found' }};
          e.click();
          return {{ ok: true, ref: 'e{idx}' }};
        }})()"#
    );
    page.evaluate(&expr).await
}

/// Focus ref and type text (`browser_type`).
pub async fn cdp_type(
    cdp_http_url: &str,
    element_ref: ElementRef,
    text: &str,
) -> Result<Value, WalletError> {
    let mut page = CdpPage::connect_first_page(cdp_http_url).await?;
    let idx = element_ref.0;
    let expr = format!(
        r#"(() => {{
          const sel = '{INTERACTIVE_SELECTOR}';
          const els = [...document.querySelectorAll(sel)];
          const e = els[{idx}];
          if (!e) return {{ ok: false, error: 'ref not found' }};
          e.focus();
          return {{ ok: true, ref: 'e{idx}' }};
        }})()"#
    );
    let focus = page.evaluate(&expr).await?;
    if focus.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        return Ok(focus);
    }
    page.insert_text(text).await?;
    Ok(json!({ "ok": true, "ref": format!("e{idx}"), "typed_len": text.chars().count() }))
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
    format!(
        r#"(() => {{
          const text = {text_lit};
          const selector = {sel_lit};
          const urlPart = {url_lit};
          if (text && document.body && document.body.innerText.includes(text)) {{
            return {{ ok: true, matched: 'text', url: location.href }};
          }}
          if (selector && document.querySelector(selector)) {{
            return {{ ok: true, matched: 'selector', url: location.href }};
          }}
          if (urlPart && location.href.includes(urlPart)) {{
            return {{ ok: true, matched: 'url', url: location.href }};
          }}
          return {{ ok: false }};
        }})()"#
    )
}

fn extract_eval_value(result: &Value) -> Result<Value, WalletError> {
    if result.get("exceptionDetails").is_some() {
        return Err(WalletError::NetworkError(format!(
            "cdp evaluate exception: {result}"
        )));
    }
    Ok(result
        .get("result")
        .and_then(|r| r.get("value"))
        .cloned()
        .unwrap_or(Value::Null))
}

fn key_definition(key: &str) -> Result<(&'static str, u32), WalletError> {
    match key {
        "Enter" => Ok(("Enter", 13)),
        "Tab" => Ok(("Tab", 9)),
        "Escape" => Ok(("Escape", 27)),
        "Backspace" => Ok(("Backspace", 8)),
        "Delete" => Ok(("Delete", 46)),
        "ArrowUp" => Ok(("ArrowUp", 38)),
        "ArrowDown" => Ok(("ArrowDown", 40)),
        "ArrowLeft" => Ok(("ArrowLeft", 37)),
        "ArrowRight" => Ok(("ArrowRight", 39)),
        "Space" | " " => Ok(("Space", 32)),
        other => Err(WalletError::InvalidTransaction(format!(
            "unsupported key `{other}` — use Enter, Tab, Escape, Arrow*, Space, Backspace, Delete"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_ref_parse() {
        assert_eq!(ElementRef::parse("e0").unwrap(), ElementRef(0));
        assert_eq!(ElementRef::parse("e12").unwrap(), ElementRef(12));
        assert!(ElementRef::parse("x1").is_err());
    }

    #[test]
    fn key_definition_enter() {
        let (code, vk) = key_definition("Enter").unwrap();
        assert_eq!(code, "Enter");
        assert_eq!(vk, 13);
    }
}
