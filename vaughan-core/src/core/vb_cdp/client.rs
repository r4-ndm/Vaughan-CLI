//! CDP page WebSocket session (`CdpPage`) and cross-frame evaluation helpers.
//!
//! Uses `Runtime.evaluate` + `Input.*` over a page WebSocket — same approach as
//! `docs/spikes/cef-tauri` `cdp_ax_smoke`, without chromiumoxide.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

use super::js;
use crate::core::vb_browser::cdp_list_pages;
use crate::error::WalletError;

/// Parsed element ref from `browser_snapshot` (e.g. `e0`, `e12`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementRef(pub u32);

impl ElementRef {
    /// Parse `e{N}` refs returned by [`super::cdp_snapshot`].
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
pub(crate) struct CdpPage {
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    next_id: u64,
    domains_on: bool,
}

impl CdpPage {
    /// Attach to the agent's page target: the pinned tab from `browser_open`
    /// / `browser_navigate` when it still exists, else the first `type=page`
    /// target advertised by CDP HTTP `/json/list`.
    pub(crate) async fn connect_first_page(cdp_http_url: &str) -> Result<Self, WalletError> {
        let ws_url = cdp_page_ws_url(cdp_http_url).await?;
        let (ws, _) = connect_async(&ws_url)
            .await
            .map_err(|e| WalletError::NetworkError(format!("cdp ws connect: {e}")))?;
        Ok(Self {
            ws,
            next_id: 1,
            domains_on: false,
        })
    }

    /// Attach to a specific page target by CDP target id.
    pub(crate) async fn connect_target(
        cdp_http_url: &str,
        target_id: &str,
    ) -> Result<Self, WalletError> {
        let pages = cdp_list_pages(cdp_http_url).await?;
        let ws_url = pages
            .iter()
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("page"))
            .find(|p| p.get("id").and_then(|i| i.as_str()) == Some(target_id))
            .and_then(|p| p.get("webSocketDebuggerUrl"))
            .and_then(|u| u.as_str())
            .filter(|ws| !ws.is_empty())
            .ok_or_else(|| {
                WalletError::NetworkError(format!("cdp: page target `{target_id}` not found"))
            })?;
        let (ws, _) = connect_async(ws_url)
            .await
            .map_err(|e| WalletError::NetworkError(format!("cdp ws connect: {e}")))?;
        Ok(Self {
            ws,
            next_id: 1,
            domains_on: false,
        })
    }

    async fn ensure_domains(&mut self) -> Result<(), WalletError> {
        if self.domains_on {
            return Ok(());
        }
        let _ = self.call("Page.enable", json!({})).await;
        let _ = self.call("DOM.enable", json!({})).await;
        let _ = self.call("Runtime.enable", json!({})).await;
        self.domains_on = true;
        Ok(())
    }

    fn collect_frame_ids(node: &Value, out: &mut Vec<String>) {
        if let Some(id) = node
            .get("frame")
            .and_then(|f| f.get("id"))
            .and_then(|i| i.as_str())
        {
            out.push(id.to_string());
        }
        if let Some(children) = node.get("childFrames").and_then(|c| c.as_array()) {
            for child in children {
                Self::collect_frame_ids(child, out);
            }
        }
    }

    pub(crate) async fn frame_ids(&mut self) -> Result<Vec<String>, WalletError> {
        self.ensure_domains().await?;
        let tree = self.call("Page.getFrameTree", json!({})).await?;
        let mut ids = Vec::new();
        if let Some(root) = tree.get("frameTree") {
            Self::collect_frame_ids(root, &mut ids);
        }
        Ok(ids)
    }

    pub(crate) async fn evaluate_in_frame(
        &mut self,
        frame_id: &str,
        expression: &str,
    ) -> Result<Value, WalletError> {
        self.ensure_domains().await?;
        let world = self
            .call(
                "Page.createIsolatedWorld",
                json!({
                    "frameId": frame_id,
                    "worldName": "vaughanAgent",
                    "grantUniveralAccess": true,
                }),
            )
            .await?;
        let ctx = world
            .get("executionContextId")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| WalletError::NetworkError("cdp: no executionContextId".into()))?;
        let result = self
            .call(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "contextId": ctx,
                    "returnByValue": true,
                    "awaitPromise": true,
                }),
            )
            .await?;
        extract_eval_value(&result)
    }

    async fn click_by_dom_search(&mut self, text: &str) -> Result<Value, WalletError> {
        self.ensure_domains().await?;
        let lower = text.to_lowercase();
        let query = format!(
            "//*[contains(translate(normalize-space(.), \
             'ABCDEFGHIJKLMNOPQRSTUVWXYZ', 'abcdefghijklmnopqrstuvwxyz'), {})]",
            xpath_literal(&lower)
        );
        let search = self
            .call(
                "DOM.performSearch",
                json!({
                    "query": query,
                    "includeUserAgentShadowDOM": true,
                }),
            )
            .await?;
        let search_id = search
            .get("searchId")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| WalletError::NetworkError("cdp: performSearch failed".into()))?;
        let count = search
            .get("resultCount")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        if count == 0 {
            let _ = self
                .call("DOM.discardSearchResults", json!({ "searchId": search_id }))
                .await;
            return Ok(json!({ "ok": false, "error": "dom search empty", "needle": lower }));
        }
        let results = self
            .call(
                "DOM.getSearchResults",
                json!({
                    "searchId": search_id,
                    "fromIndex": 0,
                    "toIndex": count.min(20),
                }),
            )
            .await?;
        let _ = self
            .call("DOM.discardSearchResults", json!({ "searchId": search_id }))
            .await;

        let node_ids = results
            .get("nodeIds")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();

        for node_id in node_ids {
            let Some(nid) = node_id.as_i64() else {
                continue;
            };
            let desc = self
                .call("DOM.describeNode", json!({ "nodeId": nid }))
                .await
                .unwrap_or(Value::Null);
            let node_text = desc
                .get("node")
                .and_then(|n| n.get("nodeValue"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let combined = format!(
                "{node_text} {}",
                desc.get("node")
                    .and_then(|n| n.get("attributes"))
                    .and_then(|a| a.as_array())
                    .map(|attrs| {
                        attrs
                            .iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default()
            )
            .to_lowercase();
            if combined.contains("click to dismiss") || combined.contains("vb injected") {
                continue;
            }
            if !combined.contains(&lower) {
                continue;
            }
            let Ok(box_model) = self.call("DOM.getBoxModel", json!({ "nodeId": nid })).await else {
                continue;
            };
            let Some(quad) = box_model
                .get("model")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
            else {
                continue;
            };
            if quad.len() < 8 {
                continue;
            }
            let cx = (quad[0].as_f64().unwrap_or(0.0)
                + quad[2].as_f64().unwrap_or(0.0)
                + quad[4].as_f64().unwrap_or(0.0)
                + quad[6].as_f64().unwrap_or(0.0))
                / 4.0;
            let cy = (quad[1].as_f64().unwrap_or(0.0)
                + quad[3].as_f64().unwrap_or(0.0)
                + quad[5].as_f64().unwrap_or(0.0)
                + quad[7].as_f64().unwrap_or(0.0))
                / 4.0;
            for event_type in ["mousePressed", "mouseReleased"] {
                self.call(
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": event_type,
                        "x": cx,
                        "y": cy,
                        "button": "left",
                        "clickCount": 1,
                    }),
                )
                .await?;
            }
            return Ok(json!({
                "ok": true,
                "method": "dom-search",
                "label": text,
                "x": cx,
                "y": cy,
            }));
        }
        Ok(json!({ "ok": false, "error": "dom nodes not clickable", "needle": lower }))
    }

    /// Click an interactive ref from [`js::INTERACTIVE_ELS`] whose label matches `needle`.
    async fn click_by_snapshot_ref(&mut self, needle: &str) -> Result<Value, WalletError> {
        let needle_json = serde_json::to_string(&needle.to_lowercase())
            .map_err(|e| WalletError::Serialization(e.to_string()))?;
        self.evaluate(&js::click_snapshot_ref(&needle_json)).await
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

    pub(crate) async fn evaluate(&mut self, expression: &str) -> Result<Value, WalletError> {
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

    pub(crate) async fn dispatch_key(
        &mut self,
        key: &str,
        key_down: bool,
    ) -> Result<(), WalletError> {
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

    /// Insert text at the focused element through the browser's real input
    /// pipeline (`Input.insertText` — same path as IME/paste). Masked React
    /// inputs apply their own formatting to this, unlike a foreign
    /// `value =` set which two venues were observed mangling (÷1000).
    pub(crate) async fn insert_text(&mut self, text: &str) -> Result<(), WalletError> {
        self.call("Input.insertText", json!({ "text": text }))
            .await?;
        Ok(())
    }

    /// Type a single character as a real key press (keyDown carrying `text`,
    /// then keyUp) — indistinguishable from human typing, so masked inputs
    /// must treat it exactly like a user's keystroke.
    pub(crate) async fn type_char(&mut self, ch: char) -> Result<(), WalletError> {
        let (code, vk) = char_key_definition(ch)?;
        let key = ch.to_string();
        self.call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyDown",
                "key": key,
                "code": code,
                "text": key,
                "windowsVirtualKeyCode": vk,
                "nativeVirtualKeyCode": vk,
            }),
        )
        .await?;
        self.call(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
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

/// WebSocket debugger URL for the agent's page target.
///
/// Target stickiness: when the MCP side pinned a tab (`vb.target`, written by
/// `browser_open` / `browser_navigate`) and that target still exists on this
/// CDP endpoint, it wins — otherwise the first page target is the fallback.
/// Without the pin, a second tab (e.g. opened by the dApp itself) could
/// silently become "first" and receive the agent's clicks/keystrokes.
pub async fn cdp_page_ws_url(cdp_http_url: &str) -> Result<String, WalletError> {
    let pages = cdp_list_pages(cdp_http_url).await?;
    let pin = crate::core::vb_browser::read_target_pin()
        .filter(|p| p.cdp_url.trim_end_matches('/') == cdp_http_url.trim_end_matches('/'));
    let mut first: Option<&str> = None;
    for page in &pages {
        if page.get("type").and_then(|t| t.as_str()) != Some("page") {
            continue;
        }
        let ws = page
            .get("webSocketDebuggerUrl")
            .and_then(|u| u.as_str())
            .filter(|ws| !ws.is_empty());
        let Some(ws) = ws else { continue };
        if first.is_none() {
            first = Some(ws);
        }
        if let Some(pin) = &pin {
            if page.get("id").and_then(|i| i.as_str()) == Some(pin.target_id.as_str()) {
                return Ok(ws.to_string());
            }
        }
    }
    if let Some(ws) = first {
        return Ok(ws.to_string());
    }
    Err(WalletError::NetworkError(
        "cdp: no page target with webSocketDebuggerUrl".into(),
    ))
}

/// Navigate an existing page target to `url` and focus it. Backs
/// [`crate::core::vb_browser::cdp_open_or_reuse`]: same-origin tabs are
/// reused instead of accumulating one tab per agent navigation.
pub async fn cdp_navigate_target(
    cdp_http_url: &str,
    target_id: &str,
    url: &str,
) -> Result<(), WalletError> {
    let mut page = CdpPage::connect_target(cdp_http_url, target_id).await?;
    page.call("Page.navigate", json!({ "url": url })).await?;
    let _ = page.call("Page.bringToFront", json!({})).await;
    Ok(())
}

/// Focus an existing page target without reloading (preserves dApp wallet state).
pub async fn cdp_focus_target(cdp_http_url: &str, target_id: &str) -> Result<(), WalletError> {
    let mut page = CdpPage::connect_target(cdp_http_url, target_id).await?;
    let _ = page.call("Page.bringToFront", json!({})).await;
    Ok(())
}

/// XPath 1.0 string literal: no escape mechanism exists, so quote with `'` —
/// or `"` when the text contains `'` — and fall back to `concat()` when both
/// quote kinds appear. Agent-supplied text reaches this query raw, so without
/// this a quote in `browser_click_text` breaks or bends the search.
fn xpath_literal(s: &str) -> String {
    if !s.contains('\'') {
        return format!("'{s}'");
    }
    if !s.contains('"') {
        return format!("\"{s}\"");
    }
    let parts: Vec<String> = s.split('\'').map(|p| format!("'{p}'")).collect();
    format!("concat({})", parts.join(", \"'\", "))
}

/// Whether a click payload indicates success (top-level or nested `result.ok`).
pub(crate) fn click_result_ok(r: &Value) -> bool {
    r.get("ok").and_then(|v| v.as_bool()) == Some(true)
        || r.pointer("/result/ok").and_then(|v| v.as_bool()) == Some(true)
}

/// CDP frame id from an `evaluate_in_all_frames` / picker step payload.
pub(crate) fn step_frame_id(v: &Value) -> Option<&str> {
    v.get("frame").and_then(|f| f.as_str())
}

fn picker_inner(v: &Value) -> &Value {
    v.get("result").unwrap_or(v)
}

fn picker_result_score(v: &Value) -> i32 {
    let r = picker_inner(v);
    if r.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        if r.get("matched_address").and_then(|x| x.as_bool()) == Some(true) {
            return 10_000;
        }
        return 5_000;
    }
    let rows = r.get("rows").and_then(|x| x.as_u64()).unwrap_or(0) as i32;
    let vis = r
        .get("visible_rows")
        .and_then(|x| x.as_array())
        .map(|a| a.len())
        .unwrap_or(0) as i32;
    rows * 100 + vis + (r.get("modal_len").and_then(|x| x.as_u64()).unwrap_or(0) as i32)
}

/// Picker eval: prefer `prefer_frame` (from a prior search/open step), else the
/// frame whose result scores highest (row count / matched address).
pub(crate) async fn evaluate_picker_step(
    page: &mut CdpPage,
    expression: &str,
    prefer_frame: Option<&str>,
) -> Result<Value, WalletError> {
    let wrapped = format!("(() => {expression})()");
    let mut best: Option<Value> = None;
    let mut best_score = -1i32;

    let mut consider = |fid: &str, r: Value| {
        let payload = json!({ "frame": fid, "result": r });
        let score = picker_result_score(&payload);
        if score > best_score {
            best_score = score;
            best = Some(payload);
        }
    };

    if let Some(fid) = prefer_frame {
        if let Ok(r) = page.evaluate_in_frame(fid, &wrapped).await {
            consider(fid, r);
        }
    }

    if let Ok(frame_ids) = page.frame_ids().await {
        for fid in frame_ids {
            if prefer_frame == Some(fid.as_str()) {
                continue;
            }
            if let Ok(r) = page.evaluate_in_frame(&fid, &wrapped).await {
                consider(&fid, r);
            }
        }
    }

    if let Some(b) = best {
        return Ok(b);
    }

    if let Ok(r) = page.evaluate(&wrapped).await {
        return Ok(json!({ "frame": "main", "result": r }));
    }
    Ok(json!({ "ok": false, "error": "picker evaluate failed in all frames" }))
}

/// Run a JS IIFE in every frame; return first `{ ok: true }` payload.
pub(crate) async fn evaluate_in_all_frames(
    page: &mut CdpPage,
    expression: &str,
) -> Result<Value, WalletError> {
    let wrapped = format!("(() => {expression})()");
    if let Ok(frame_ids) = page.frame_ids().await {
        for fid in &frame_ids {
            if let Ok(r) = page.evaluate_in_frame(fid, &wrapped).await {
                if r.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    return Ok(json!({ "ok": true, "frame": fid, "result": r }));
                }
            }
        }
    }
    if let Ok(r) = page.evaluate(&wrapped).await {
        if r.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(json!({ "ok": true, "frame": "main", "result": r }));
        }
        return Ok(r);
    }
    Ok(json!({ "ok": false, "error": "evaluate failed in all frames" }))
}

/// Deep-dom text click across all frames (token rows in embedded swap iframes).
pub(crate) async fn deep_click_in_all_frames(
    page: &mut CdpPage,
    text: &str,
) -> Result<Value, WalletError> {
    let needle = text.to_lowercase();
    let needle_json =
        serde_json::to_string(&needle).map_err(|e| WalletError::Serialization(e.to_string()))?;
    let click_expr = format!(
        "({})({needle_json}, /click to dismiss|vb injected|approve sign\\/send in vaughan tui|connect wallet|search wallet/i)",
        js::DEEP_CLICK_BY_TEXT
    );
    if let Ok(frame_ids) = page.frame_ids().await {
        for fid in &frame_ids {
            if let Ok(r) = page.evaluate_in_frame(fid, &click_expr).await {
                if r.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    return Ok(json!({ "ok": true, "frame": fid, "result": r }));
                }
            }
        }
    }
    if let Ok(r) = page.evaluate(&format!("(() => {click_expr})()")).await {
        if r.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(json!({ "ok": true, "frame": "main", "result": r }));
        }
    }
    Ok(json!({ "ok": false, "error": "token text not found in any frame", "symbol": text }))
}

/// Full click-by-text cascade: frames → main eval → snapshot refs → DOM search.
pub(crate) async fn click_text_cascade(
    page: &mut CdpPage,
    text: &str,
) -> Result<Value, WalletError> {
    let needle = text.to_lowercase();
    let needle_json =
        serde_json::to_string(&needle).map_err(|e| WalletError::Serialization(e.to_string()))?;
    page.ensure_domains().await?;

    let click_expr = format!(
        "({})({needle_json}, /click to dismiss|vb injected|approve sign\\/send in vaughan tui/i)",
        js::DEEP_CLICK_BY_TEXT
    );

    // Try every frame (main + embedded / OOPIF child frames).
    if let Ok(frame_ids) = page.frame_ids().await {
        for fid in &frame_ids {
            if let Ok(r) = page.evaluate_in_frame(fid, &click_expr).await {
                if r.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                    return Ok(json!({ "frame": fid, "result": r }));
                }
            }
        }
    }

    // Main document evaluate (isolated world not required).
    if let Ok(r) = page.evaluate(&format!("(() => {click_expr})()")).await {
        if r.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(json!({ "frame": "main-eval", "result": r }));
        }
    }

    // Snapshot ref click (shadow DOM — wallet modals, token lists).
    if let Ok(r) = page.click_by_snapshot_ref(text).await {
        if r.get("ok").and_then(|v| v.as_bool()) == Some(true) {
            return Ok(json!({ "frame": "snapshot-ref", "result": r }));
        }
    }

    // Coordinate click via DOM.performSearch (shadow + pseudo pierce).
    let dom = page
        .click_by_dom_search(text)
        .await
        .unwrap_or_else(|e| json!({ "ok": false, "error": format!("dom-search: {e}") }));
    if dom.get("ok").and_then(|v| v.as_bool()) == Some(true) {
        return Ok(json!({ "frame": "dom-search", "result": dom }));
    }

    Ok(json!({
        "ok": false,
        "error": "text not found in any frame",
        "needle": needle,
        "dom": dom,
    }))
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

/// CDP `code` + Windows virtual-key code for a typed character. ASCII
/// alphanumerics double as their own VK codes ('0'=0x30 … 'A'=0x41).
fn char_key_definition(ch: char) -> Result<(String, u32), WalletError> {
    match ch {
        '0'..='9' => Ok((format!("Digit{ch}"), ch as u32)),
        'a'..='z' => Ok((
            format!("Key{}", ch.to_ascii_uppercase()),
            ch.to_ascii_uppercase() as u32,
        )),
        'A'..='Z' => Ok((format!("Key{ch}"), ch as u32)),
        '.' => Ok(("Period".into(), 190)),
        ',' => Ok(("Comma".into(), 188)),
        '-' => Ok(("Minus".into(), 189)),
        '_' => Ok(("Minus".into(), 189)),
        ' ' => Ok(("Space".into(), 32)),
        other => Err(WalletError::InvalidTransaction(format!(
            "browser_type: unsupported character `{other}`"
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

    #[test]
    fn xpath_literal_quoting() {
        assert_eq!(xpath_literal("connect wallet"), "'connect wallet'");
        assert_eq!(xpath_literal("it's"), "\"it's\"");
        assert_eq!(xpath_literal("a'b\"c"), "concat('a', \"'\", 'b\"c')");
    }
}
