//! Unpacked Chromium extension assets for the Vaughan provider bridge.
//!
//! Architecture (CSP-safe):
//! - **MAIN** world `inject.js` — EIP-1193 / EIP-6963 on `window.ethereum`
//! - **ISOLATED** `content_bridge.js` — `postMessage` ↔ extension port
//! - **background** service worker — owns `WebSocket` to Vaughan (not subject
//!   to the page `connect-src`, which blocks `ws://` on sites like 9inch)
//!
//! A fixed manifest `key` keeps the extension id (and Origin) stable so the
//! Vaughan TUI can always allowlist [`EXTENSION_ORIGIN`].

/// Stable public key → Chromium extension id `cneeaoilhnioopaiaidjadinahpgacpn`.
pub const EXTENSION_PUBLIC_KEY: &str = "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEApt1Xf9PmN2tF2s2/nf6a/b0ACZ8EQiBDUdejGUjJvS63KEa12WgTs9MAPKyWdqaW3ElZwz7xHWrCe8ZH8dQlDYIyRWY4e8EVUdCrKBWkoYKNDQo/jZ62GED/DeoqRXVJxASPFn+HVOy+AYSyM9139cJ400SsoYsNioPxgYyLax20zYAbkLY5cFiTYd+G4jiA1v3MyNkrM2mgo7B1hJPEX4Q9nXvq0IK+n6y8lldmrVrdMSfzvL7dr9I6uY5ATFSeBYBEg69k58nOlAVh90Bx/gaYuSG+5wLJJNUr6/+RsG0XaM+SdmoaMaId+BsBoGrP+FvL/Nc1j4PkJ0PrQuv8gQIDAQAB";

/// Chromium extension id derived from [`EXTENSION_PUBLIC_KEY`].
pub const EXTENSION_ID: &str = "cneeaoilhnioopaiaidjadinahpgacpn";

/// WebSocket handshake Origin for this extension (must match TUI allowlist).
/// Keep in sync with `vaughan_tui::provider::DAPP_BROWSER_PROVIDER_ORIGIN`.
pub const EXTENSION_ORIGIN: &str = "chrome-extension://cneeaoilhnioopaiaidjadinahpgacpn";

/// Build `manifest.json` for the unpacked extension.
pub fn manifest_json() -> String {
    format!(
        r#"{{
  "manifest_version": 3,
  "name": "Vaughan Wallet Provider",
  "version": "0.3.0",
  "description": "Injects window.ethereum → Vaughan TUI (CSP-safe background WebSocket).",
  "key": "{key}",
  "permissions": [
    "declarativeNetRequest"
  ],
  "background": {{
    "service_worker": "background.js"
  }},
  "host_permissions": [
    "<all_urls>",
    "ws://127.0.0.1/*",
    "ws://localhost/*",
    "http://127.0.0.1/*",
    "http://localhost/*"
  ],
  "content_scripts": [
    {{
      "matches": ["https://*/*", "http://localhost/*", "http://127.0.0.1/*"],
      "js": ["content_bridge.js"],
      "run_at": "document_start",
      "all_frames": false
    }},
    {{
      "matches": ["https://*/*", "http://localhost/*", "http://127.0.0.1/*"],
      "js": ["inject.js"],
      "run_at": "document_start",
      "world": "MAIN",
      "all_frames": false
    }}
  ]
}}"#,
        key = EXTENSION_PUBLIC_KEY
    )
}

/// Isolated-world bridge: page postMessage ↔ extension background port.
pub fn content_bridge_js() -> &'static str {
    r#"(function () {
  const PAGE = "vaughan-page";
  const EXT = "vaughan-ext";
  let port = null;

  function connect() {
    try {
      port = chrome.runtime.connect({ name: "vaughan-provider" });
    } catch (_) {
      port = null;
      return;
    }
    port.onMessage.addListener((msg) => {
      window.postMessage({ source: EXT, ...msg }, "*");
    });
    port.onDisconnect.addListener(() => {
      port = null;
      window.postMessage({ source: EXT, type: "bridge-down" }, "*");
      setTimeout(connect, 500);
    });
  }

  window.addEventListener("message", (ev) => {
    if (ev.source !== window) return;
    const data = ev.data;
    if (!data || data.source !== PAGE) return;
    if (!port) connect();
    if (!port) {
      window.postMessage({
        source: EXT,
        type: "rpc-result",
        id: data.id,
        error: { code: 4900, message: "Vaughan extension bridge offline" },
      }, "*");
      return;
    }
    try {
      port.postMessage(data);
    } catch (_) {
      port = null;
      connect();
    }
  });

  connect();
  // Keep the MV3 service worker alive while a tab is open.
  setInterval(() => {
    if (!port) connect();
    try { port && port.postMessage({ source: PAGE, type: "ping" }); } catch (_) { port = null; }
  }, 20000);
})();"#
}

/// Background service worker: owns the Vaughan WebSocket.
///
/// Two trust rules live here:
/// - **Origin attestation:** the page origin comes from `port.sender.url`
///   (Chrome-attested), never from a page-supplied field — anything arriving
///   over `postMessage` is forgeable by the page.
/// - **Response routing:** the worker assigns the JSON-RPC wire id itself and
///   maps it back to the originating port, so two tabs cannot collide on ids
///   or steal each other's (signature-bearing) responses.
pub fn background_js(provider_ws: &str) -> String {
    let ws =
        serde_json::to_string(provider_ws).unwrap_or_else(|_| "\"ws://127.0.0.1:8745\"".into());
    format!(
        r#"(function () {{
  const WS_URL = {ws};
  /** @type {{WebSocket|null}} */
  let socket = null;
  /** @type {{Map<number, {{port: chrome.runtime.Port, clientId: *, method: string, pageOrigin: string}}>}} */
  const pending = new Map();
  /** @type {{Set<chrome.runtime.Port>}} */
  const ports = new Set();
  /** Port → Chrome-attested page origin (for event scoping). */
  const portOrigins = new Map();
  /**
   * Origins that actually hold a Connect grant this WS session. Learned from
   * non-empty eth_accounts / eth_requestAccounts results; accountsChanged is
   * forwarded only to these (an address is not broadcast to every open tab).
   */
  const connectedOrigins = new Set();
  let nextWireId = 1;

  function ensureSocket() {{
    if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) {{
      return socket;
    }}
    socket = new WebSocket(WS_URL);
    socket.addEventListener("message", (ev) => {{
      let msg;
      try {{ msg = JSON.parse(ev.data); }} catch (_) {{ return; }}
      if (msg && msg.id != null && pending.has(msg.id)) {{
        const entry = pending.get(msg.id);
        pending.delete(msg.id);
        if (
          entry.pageOrigin &&
          (entry.method === "eth_requestAccounts" || entry.method === "eth_accounts") &&
          Array.isArray(msg.result) &&
          msg.result.length > 0
        ) {{
          connectedOrigins.add(entry.pageOrigin);
        }}
        try {{
          entry.port.postMessage({{
            type: "rpc-result",
            id: entry.clientId,
            result: msg.result,
            error: msg.error,
          }});
        }} catch (_) {{}}
        return;
      }}
      if (msg && msg.method === "chainChanged") {{
        // Chain id is not account-sensitive: broadcast to every tab.
        for (const port of ports) {{
          try {{
            port.postMessage({{ type: "event", method: msg.method, params: msg.params }});
          }} catch (_) {{}}
        }}
        return;
      }}
      if (msg && msg.method === "accountsChanged") {{
        const accounts = Array.isArray(msg.params) ? msg.params : [];
        for (const port of ports) {{
          if (!connectedOrigins.has(portOrigins.get(port) || "")) continue;
          try {{
            port.postMessage({{ type: "event", method: msg.method, params: msg.params }});
          }} catch (_) {{}}
        }}
        // Empty list = lock/disconnect: nobody holds a grant any more.
        if (accounts.length === 0) connectedOrigins.clear();
        return;
      }}
    }});
    socket.addEventListener("close", () => {{
      // A new Vaughan process = fresh grant set; relearn on next session.
      socket = null;
      connectedOrigins.clear();
    }});
    socket.addEventListener("error", () => {{
      for (const [, entry] of pending) {{
        try {{
          entry.port.postMessage({{
            type: "rpc-result",
            id: entry.clientId,
            error: {{ code: 4900, message: "Vaughan provider WebSocket error — is Vaughan unlocked?" }},
          }});
        }} catch (_) {{}}
      }}
      pending.clear();
    }});
    return socket;
  }}

  function sendRpc(port, clientId, method, params, pageOrigin) {{
    const wireId = nextWireId++;
    pending.set(wireId, {{ port, clientId, method, pageOrigin }});
    const payload = JSON.stringify({{
      jsonrpc: "2.0",
      id: wireId,
      method,
      params: params || [],
      vaughan_page_origin: pageOrigin || undefined,
    }});
    const s = ensureSocket();
    const go = () => {{
      try {{ s.send(payload); }}
      catch (e) {{
        pending.delete(wireId);
        try {{
          port.postMessage({{
            type: "rpc-result",
            id: clientId,
            error: {{ code: 4900, message: String(e && e.message || e) }},
          }});
        }} catch (_) {{}}
      }}
    }};
    if (s.readyState === WebSocket.OPEN) go();
    else s.addEventListener("open", go, {{ once: true }});
  }}

  chrome.runtime.onConnect.addListener((port) => {{
    if (port.name !== "vaughan-provider") return;
    // Chrome attests sender.url for content-script ports; the page cannot
    // forge it. Derived once per port (all_frames=false → top-level page).
    let pageOrigin = "";
    try {{
      if (port.sender && port.sender.url) {{
        pageOrigin = new URL(port.sender.url).origin;
      }}
    }} catch (_) {{}}
    ports.add(port);
    portOrigins.set(port, pageOrigin);
    port.onDisconnect.addListener(() => {{
      ports.delete(port);
      portOrigins.delete(port);
      for (const [wireId, entry] of pending) {{
        if (entry.port === port) pending.delete(wireId);
      }}
    }});
    port.onMessage.addListener((data) => {{
      if (!data || data.source !== "vaughan-page") return;
      if (data.type === "ping") return;
      // Requests without an id are notifications; EIP-1193 pages always send
      // one, and the server answers notifications with silence — drop them.
      if (data.type === "rpc" && data.id != null) {{
        sendRpc(port, data.id, data.method, data.params, pageOrigin);
      }}
    }});
    ensureSocket();
  }});

  // In-tab navigation gate (MV3 declarativeNetRequest): block main_frame loads
  // to hosts outside allowlist.json (seeded by vaughan-dapp-browser at launch).
  const NAV_GATE_RULE_ID = 9001;
  async function installNavGate() {{
    let suffixes = [];
    try {{
      const resp = await fetch(chrome.runtime.getURL("allowlist.json"));
      const data = await resp.json();
      suffixes = Array.isArray(data.suffixes) ? data.suffixes : [];
    }} catch (_) {{
      return;
    }}
    const domains = suffixes.map((s) => String(s || "").trim().toLowerCase()).filter(Boolean);
    for (const h of ["localhost", "127.0.0.1"]) {{
      if (!domains.includes(h)) domains.push(h);
    }}
    if (domains.length === 0) return;
    await chrome.declarativeNetRequest.updateDynamicRules({{
      removeRuleIds: [NAV_GATE_RULE_ID],
      addRules: [{{
        id: NAV_GATE_RULE_ID,
        priority: 1,
        action: {{ type: "block" }},
        condition: {{
          resourceTypes: ["main_frame"],
          excludedRequestDomains: domains,
        }},
      }}],
    }});
  }}
  installNavGate().catch(() => {{}});
}})();"#,
        ws = ws
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_origin_matches_stable_id() {
        assert_eq!(
            EXTENSION_ORIGIN,
            format!("chrome-extension://{EXTENSION_ID}")
        );
        // Non-test binary path also references the id via eprintln / docs.
        assert!(manifest_json().contains("Vaughan Wallet Provider"));
        let _ = EXTENSION_ID;
    }

    #[test]
    fn manifest_embeds_key_and_service_worker() {
        let m = manifest_json();
        assert!(m.contains(EXTENSION_PUBLIC_KEY));
        assert!(m.contains("background.js"));
        assert!(m.contains("content_bridge.js"));
        assert!(m.contains("\"world\": \"MAIN\""));
    }

    #[test]
    fn background_embeds_loopback_ws() {
        let js = background_js("ws://127.0.0.1:8745");
        assert!(js.contains("ws://127.0.0.1:8745"));
        assert!(js.contains("WebSocket"));
    }

    #[test]
    fn background_attests_origin_and_namespaces_wire_ids() {
        let js = background_js("ws://127.0.0.1:8745");
        // H1: page origin must come from Chrome-attested sender.url…
        assert!(js.contains("port.sender.url"));
        // …never from a page-supplied postMessage field.
        assert!(!js.contains("data.pageOrigin"));
        // H2: the worker assigns wire ids and maps them back per port.
        assert!(js.contains("nextWireId++"));
        assert!(js.contains("clientId"));
    }

    #[test]
    fn background_scopes_accounts_changed_to_connected_origins() {
        let js = background_js("ws://127.0.0.1:8745");
        // M3: account events only reach origins that completed a Connect.
        assert!(js.contains("connectedOrigins"));
        assert!(js.contains("portOrigins"));
        // Grants are learned from non-empty accounts results and die on lock.
        assert!(js.contains("eth_requestAccounts"));
        assert!(js.contains("connectedOrigins.clear()"));
    }

    #[test]
    fn manifest_includes_nav_gate_permissions() {
        let m = manifest_json();
        assert!(m.contains("declarativeNetRequest"));
        assert!(m.contains("<all_urls>"));
    }

    #[test]
    fn background_installs_nav_gate() {
        let js = background_js("ws://127.0.0.1:8745");
        assert!(js.contains("installNavGate"));
        assert!(js.contains("excludedRequestDomains"));
        assert!(js.contains("allowlist.json"));
    }
}
