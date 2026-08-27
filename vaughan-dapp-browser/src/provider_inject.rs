//! Minimal EIP-1193 / EIP-6963 page shim (MAIN world).
//!
//! Talks to the isolated extension bridge via `postMessage` (not a page-level
//! `WebSocket`). That keeps us working on dApps whose CSP `connect-src` allows
//! only `https:` / `wss:` (e.g. 9inch) and blocks `ws://127.0.0.1`.
//!
//! Shows a short on-page banner so humans can confirm inject worked (dApps often
//! label unknown providers as “Injected”). Signing stays in the Vaughan TUI.

/// MAIN-world inject script (no provider URL baked in — background owns the WS).
pub fn script() -> String {
    SCRIPT.to_string()
}

const SCRIPT: &str = r##"(function() {
  if (window.__VAUGHAN_ETH_INJECTED__) return;
  window.__VAUGHAN_ETH_INJECTED__ = true;
  const PAGE = "vaughan-page";
  const EXT = "vaughan-ext";
  let nextId = 1;
  const pending = new Map();
  const listeners = {};
  let selectedAddress = null;
  let chainId = null;

  function emit(event, data) {
    (listeners[event] || []).forEach(fn => { try { fn(data); } catch (_) {} });
  }

  function showBanner(text, kind) {
    try {
      const id = "__vaughan_inject_banner";
      let el = document.getElementById(id);
      if (!el) {
        el = document.createElement("div");
        el.id = id;
        el.setAttribute("data-vaughan", "inject-ok");
        Object.assign(el.style, {
          position: "fixed", top: "0", left: "0", right: "0", zIndex: "2147483647",
          padding: "10px 14px", fontFamily: "system-ui,sans-serif", fontSize: "13px",
          fontWeight: "600", textAlign: "center", boxShadow: "0 2px 8px rgba(0,0,0,.25)",
          cursor: "pointer",
        });
        el.title = "Click to dismiss";
        el.addEventListener("click", () => el.remove());
        const mount = () => {
          if (document.documentElement) document.documentElement.appendChild(el);
          else document.addEventListener("DOMContentLoaded", () => document.documentElement.appendChild(el), { once: true });
        };
        mount();
      }
      el.style.background = kind === "wait" ? "#1d4ed8" : kind === "err" ? "#b91c1c" : "#065f46";
      el.style.color = "#fff";
      el.textContent = text;
    } catch (_) {}
  }

  window.addEventListener("message", (ev) => {
    if (ev.source !== window) return;
    const msg = ev.data;
    if (!msg || msg.source !== EXT) return;
    if (msg.type === "bridge-down") {
      showBanner("Vaughan bridge offline - unlock Vaughan. (click to dismiss)", "err");
      return;
    }
    if (msg.type === "event") {
      if (msg.method === "accountsChanged") {
        const accounts = msg.params || [];
        selectedAddress = accounts[0] || null;
        emit("accountsChanged", accounts);
      }
      if (msg.method === "chainChanged") {
        chainId = (msg.params && msg.params[0]) || msg.params;
        emit("chainChanged", chainId);
      }
      return;
    }
    if (msg.type === "rpc-result" && msg.id != null && pending.has(msg.id)) {
      const entry = pending.get(msg.id);
      pending.delete(msg.id);
      const { resolve, reject, sensitive } = entry;
      if (msg.error) {
        if (sensitive) showBanner("Vaughan denied/error - check the Vaughan TUI. (click to dismiss)", "err");
        reject(Object.assign(new Error(msg.error.message || "provider error"), msg.error));
      } else {
        if (Array.isArray(msg.result) && msg.result[0] && typeof msg.result[0] === "string" && msg.result[0].indexOf("0x") === 0) {
          selectedAddress = msg.result[0];
        }
        if (typeof msg.result === "string" && /^0x[0-9a-fA-F]+$/.test(msg.result) && msg.result.length <= 18) {
          chainId = msg.result;
        }
        if (sensitive) showBanner("Vaughan: request ok (approve sign/send in TUI). (click to dismiss)", "ok");
        resolve(msg.result);
      }
    }
  });

  function isSensitive(method) {
    return method === "eth_requestAccounts" || method === "wallet_requestPermissions" || method === "personal_sign" || method === "eth_sendTransaction" || method === "eth_signTypedData_v4" || method === "vaughan_signTransaction";
  }

  function rpc(method, params) {
    return new Promise((resolve, reject) => {
      const id = nextId++;
      const sensitive = isSensitive(method);
      if (sensitive) {
        showBanner("Confirm in the Vaughan TUI terminal (not this browser). (click to dismiss)", "wait");
      }
      pending.set(id, { resolve, reject, sensitive });
      // No page origin here: the background worker derives it from
      // Chrome-attested `port.sender.url`; anything the page posts is forgeable.
      window.postMessage({ source: PAGE, type: "rpc", id, method, params: params || [] }, "*");
      setTimeout(() => {
        if (pending.has(id)) {
          pending.delete(id);
          if (sensitive) showBanner("Vaughan bridge timeout - unlock Vaughan / check Web list. (click to dismiss)", "err");
          reject(Object.assign(new Error("Vaughan provider timeout"), { code: 4900 }));
        }
      }, 120000);
    });
  }

  const provider = {
    isVaughan: true,
    // MetaMask-family convenience (EIP interop): many Pulse dApps gate on this.
    // EIP-6963 still announces as "Vaughan" / rdns wallet.vaughan.
    isMetaMask: true,
    get selectedAddress() { return selectedAddress; },
    get chainId() { return chainId; },
    get networkVersion() {
      if (!chainId) return null;
      try { return String(parseInt(chainId, 16)); } catch (_) { return null; }
    },
    isConnected: () => true,
    request: ({ method, params }) => {
      if (method === "wallet_requestPermissions") {
        return rpc("eth_requestAccounts", []).then((accounts) => ([{
          parentCapability: "eth_accounts",
          caveats: [{ type: "restrictReturnedAccounts", value: accounts }],
        }]));
      }
      if (method === "wallet_getPermissions") {
        return Promise.resolve(selectedAddress ? [{
          parentCapability: "eth_accounts",
          caveats: [{ type: "restrictReturnedAccounts", value: [selectedAddress] }],
        }] : []);
      }
      return rpc(method, params);
    },
    send(payloadOrMethod, callbackOrParams) {
      if (typeof payloadOrMethod === "string") {
        const method = payloadOrMethod;
        if (typeof callbackOrParams === "function") {
          return rpc(method, []).then(
            (result) => callbackOrParams(null, { id: undefined, jsonrpc: "2.0", result }),
            (error) => callbackOrParams(error, null)
          );
        }
        const params = Array.isArray(callbackOrParams) ? callbackOrParams : [];
        return rpc(method, params);
      }
      const payload = payloadOrMethod;
      const p = rpc(payload.method, payload.params);
      if (typeof callbackOrParams === "function") {
        p.then((result) => callbackOrParams(null, { id: payload.id, jsonrpc: "2.0", result }))
          .catch((error) => callbackOrParams(error, null));
        return;
      }
      return p;
    },
    sendAsync(payload, callback) {
      return provider.send(payload, callback);
    },
    on: (event, fn) => { (listeners[event] ||= []).push(fn); return provider; },
    removeListener: (event, fn) => {
      listeners[event] = (listeners[event] || []).filter(f => f !== fn);
      return provider;
    },
    enable: () => rpc("eth_requestAccounts", []),
  };
  provider.providers = [provider];
  const SEAL = Symbol.for("wallet.vaughan.vb.seal");
  provider[SEAL] = 1;

  function announce6963() {
    try {
      const info = {
        uuid: "vaughan-dapp-browser-0000-0000-0000-000000000001",
        name: "Vaughan",
        icon: "data:image/svg+xml,<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 32 32'><rect fill='%23065f46' width='32' height='32' rx='6'/><text x='16' y='22' text-anchor='middle' fill='white' font-size='14' font-family='sans-serif'>V</text></svg>",
        rdns: "wallet.vaughan",
      };
      const detail = Object.freeze({ info, provider });
      window.dispatchEvent(new CustomEvent("eip6963:announceProvider", { detail }));
    } catch (_) {}
  }

  try {
    Object.defineProperty(window, "ethereum", {
      configurable: false,
      enumerable: true,
      get: () => provider,
    });
  } catch (_) {
    try {
      Object.defineProperty(window, "ethereum", {
        configurable: true,
        enumerable: true,
        get: () => provider,
        set: () => {},
      });
    } catch (_2) {
      window.ethereum = provider;
    }
  }

  try {
    window.addEventListener("eip6963:requestProvider", () => announce6963());
    announce6963();
  } catch (_) {}

  function providerIntact() {
    try {
      const eth = window.ethereum;
      return !!(eth && eth.isVaughan && eth[SEAL] === 1);
    } catch (_) {
      return false;
    }
  }

  setInterval(() => {
    if (providerIntact()) return;
    showBanner("VB wallet tampered — pick Vaughan in the wallet list or reload. (click to dismiss)", "err");
    announce6963();
    try {
      Object.defineProperty(window, "ethereum", {
        configurable: false,
        enumerable: true,
        get: () => provider,
      });
    } catch (_) {}
  }, 4000);

  console.info("[Vaughan VB] window.ethereum injected (isVaughan=true). Approve in Vaughan TUI.");
  const paint = () => showBanner("VB injected — Connect here, approve sign/send in Vaughan TUI. (click to dismiss)", "ok");
  if (document.documentElement) paint();
  else document.addEventListener("DOMContentLoaded", paint, { once: true });
  rpc("eth_chainId", []).then((id) => { chainId = id; }).catch(() => {});
})();"##;

/// Local self-check HTML (served on loopback) — proves inject without a dApp.
pub fn self_check_html() -> &'static str {
    r##"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8"/>
  <title>Vaughan inject check</title>
  <style>
    body { font-family: system-ui, sans-serif; max-width: 40rem; margin: 3rem auto; padding: 0 1rem; }
    .ok { color: #065f46; font-weight: 700; font-size: 1.4rem; }
    .bad { color: #b91c1c; font-weight: 700; font-size: 1.4rem; }
    button { margin-top: 1rem; padding: 0.6rem 1rem; font-size: 1rem; cursor: pointer; }
    pre { background: #f3f4f6; padding: 0.75rem; overflow: auto; }
  </style>
</head>
<body>
  <h1>Vaughan inject check</h1>
  <p id="status" class="bad">Checking for window.ethereum.isVaughan…</p>
  <p id="bridge" class="bad">Bridge: waiting…</p>
  <pre id="detail"></pre>
  <button id="btn" type="button" disabled>Request accounts (eth_requestAccounts)</button>
  <p>Unlock Vaughan TUI first. Inject PASS alone is not enough — Bridge should show chainId. Approve sign/send in Vaughan (dApps may say “Injected”).</p>
  <script>
    async function refresh() {
      const eth = window.ethereum;
      const ok = !!(eth && eth.isVaughan);
      const status = document.getElementById("status");
      const bridge = document.getElementById("bridge");
      const detail = document.getElementById("detail");
      const btn = document.getElementById("btn");
      status.className = ok ? "ok" : "bad";
      status.textContent = ok
        ? "PASS — Vaughan provider is injected"
        : "FAIL — Vaughan provider not found (extension not loaded?)";
      btn.disabled = !ok;
      let chainId = null;
      let bridgeErr = null;
      if (ok) {
        try {
          chainId = await eth.request({ method: "eth_chainId" });
          bridge.className = "ok";
          bridge.textContent = "PASS — bridge ok (eth_chainId " + chainId + ")";
        } catch (e) {
          bridgeErr = e && e.message ? e.message : String(e);
          bridge.className = "bad";
          bridge.textContent = "FAIL — bridge (unlock Vaughan?): " + bridgeErr;
        }
      } else {
        bridge.className = "bad";
        bridge.textContent = "Bridge: n/a until inject PASS";
      }
      detail.textContent = JSON.stringify({
        hasEthereum: !!eth,
        isVaughan: !!(eth && eth.isVaughan),
        isMetaMask: !!(eth && eth.isMetaMask),
        flag: !!window.__VAUGHAN_ETH_INJECTED__,
        chainId,
        bridgeErr,
      }, null, 2);
    }
    document.getElementById("btn").onclick = async () => {
      const detail = document.getElementById("detail");
      try {
        const accounts = await window.ethereum.request({ method: "eth_requestAccounts" });
        detail.textContent = "accounts: " + JSON.stringify(accounts, null, 2);
      } catch (e) {
        detail.textContent = "error: " + (e && e.message ? e.message : String(e));
      }
    };
    refresh();
    setInterval(refresh, 1500);
  </script>
</body>
</html>"##
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_has_vaughan_flag_and_postmessage_bridge() {
        let s = script();
        assert!(s.contains("isVaughan: true"));
        assert!(s.contains("eip6963:announceProvider"));
        assert!(s.contains("rdns: \"wallet.vaughan\""));
        assert!(s.contains("VB injected"));
        assert!(s.contains("wallet.vaughan.vb.seal"));
        assert!(s.contains("providerIntact"));
        assert!(s.contains("vaughan-page"));
        assert!(s.contains("wallet_requestPermissions"));
        assert!(s.contains("isSensitive"));
        assert!(!s.contains("new WebSocket"));
    }

    #[test]
    fn self_check_html_mentions_pass() {
        assert!(self_check_html().contains("isVaughan"));
        assert!(self_check_html().contains("eth_requestAccounts"));
    }
}
