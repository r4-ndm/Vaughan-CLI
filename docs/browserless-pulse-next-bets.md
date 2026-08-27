# Browserless Pulse — next bets (implementation plan)

> **Status:** Implemented (2026-08-27). See [TASKS.md](../TASKS.md) Browserless Pulse / Phase 7.

## 1. VB post-load navigation allowlist (MCP server-side)

**Already shipped in extension:** MV3 `declarativeNetRequest` in
[`vaughan-dapp-browser/src/extension_assets.rs`](../vaughan-dapp-browser/src/extension_assets.rs)
(`installNavGate` + `allowlist.json`).

**Remaining gap:** MCP `browser_navigate` must validate URLs server-side; persist
allowlist in `vb.session` for agents.

### Changes

| File | Work |
|------|------|
| [`vaughan-dapp-browser/src/launch.rs`](../vaughan-dapp-browser/src/launch.rs) | Extend `write_vb_session` with `allow_suffixes` from `Allowlist` |
| **New** `vaughan-core/src/core/vb_browser.rs` | `read_vb_session`, `check_url_allowed`, `cdp_alive`, `cdp_open_url`, `cdp_list_pages`, `spawn_dapp_browser` |
| [`vaughan-core/src/core/mod.rs`](../vaughan-core/src/core/mod.rs) | `pub mod vb_browser` |
| [`docs/dapp-browser-strategy.md`](dapp-browser-strategy.md) | Mark in-tab nav gate **done**; note MCP server-side check |
| [`TASKS.md`](../TASKS.md) | Check off nav allowlist item |

### Tests

- `vb_browser`: suffix/subdomain allow, ephemeral URLs
- `extension_assets`: existing `background_installs_nav_gate` (keep)

---

## 2. Local EIP-712 → Approve view

**Exists today:** Provider path `eth_signTypedData_v4` → `ApprovalKind::SignTypedData` →
[`vaughan-core/src/security/signing.rs`](../vaughan-core/src/security/signing.rs).

**Missing:** Browserless entry — paste JSON without a dApp.

### Changes

| File | Work |
|------|------|
| [`vaughan-cli/src/main.rs`](../vaughan-cli/src/main.rs) | `vaughan sign-typed-data --data JSON \| --file path` + unlock + confirm + `--json` |
| [`vaughan-tui/src/app.rs`](../vaughan-tui/src/app.rs) | `KeyOutcome::SignTypedData(Value)` → Approve gate |
| [`vaughan-tui/src/views/browser.rs`](../vaughan-tui/src/views/browser.rs) | REPL command `sign-typed <json\|@file>` |
| [`vaughan-tui/src/intent.rs`](../vaughan-tui/src/intent.rs) | Optional `/sign` macro (paste flow) |
| [`TASKS.md`](../TASKS.md) | Check off P3 EIP-712 item |

### UX

1. User pastes EIP-712 JSON (types, domain, primaryType, message)
2. Full Approve card (same as dApp) — `y` / `n`
3. Print `0x…` signature (CLI `--json` or browser REPL status line)

---

## 3. Ambire AA polish

**Already shipped:** TUI `b`, 7702 self-pay, Anvil + fork E2E.

### Quick wins (minimal diff)

| File | Work |
|------|------|
| [`vaughan-tui/src/views/aa_send.rs`](../vaughan-tui/src/views/aa_send.rs) | Confirm screen: show active network name; testnet reminder; clearer bootstrap/delegation note |
| [`docs/ambire-aa.md`](ambire-aa.md) | One paragraph: TUI path = 7702 self-pay; 4337 deferred |

No new AA crypto — UX copy only.

---

## 4. MCP browser B1 (`browser_open` / `navigate` / `status`)

### Changes

| File | Work |
|------|------|
| **New** `vaughan-mcp/src/browser_bridge.rs` | Tool defs + handlers |
| [`vaughan-mcp/src/dispatch.rs`](../vaughan-mcp/src/dispatch.rs) | Route three tools; use `vb_browser` + profile allow hosts |
| [`vaughan-mcp/src/lib.rs`](../vaughan-mcp/src/lib.rs) | `pub mod browser_bridge` |
| [`docs/mcp.md`](mcp.md) | Document tools + `UNAVAILABLE` when binary/session absent |
| [`vaughan-mcp/tests/conformance.rs`](../vaughan-mcp/tests/conformance.rs) | Assert tools appear in `tools/list` |

### Tool behaviour

```text
browser_open(url)
  → check_url_allowed(url, profile suffixes)
  → spawn vaughan-dapp-browser --cdp-port $VAUGHAN_DAPP_BROWSER_CDP_PORT|9222
  → poll vb.session + cdp_alive (≤5s)

browser_navigate(url)
  → require vb.session + allowlist check
  → cdp_open_url(cdp_url, url)

browser_status()
  → { available, cdp_url, pages: [{url,title}], allow_suffixes_count }
  → structured unavailable if no session / dead CDP
```

**Never auto-sign.** Signing stays TUI/provider only.

---

## 5. Browserless Pulse demo reel script

**Cannot record video in-repo** — ship a operator script for you to record.

| File | Work |
|------|------|
| **New** [`docs/browserless-pulse-demo.md`](browserless-pulse-demo.md) | Step-by-step ~3 min demo; no Chrome/Freedom in frame |
| [`TASKS.md`](../TASKS.md) | Check off demo reel item (script = done; video optional) |
| [`docs/browserless-pulse.md`](browserless-pulse.md) | Link to demo script |

### Demo script outline

1. Unlock PulseChain testnet v4 (943)
2. **Ag (`g`):** quote → swap → approve once
3. **Browse (`c`):** probe contract / `call name`
4. **MCP:** `propose_transfer` small amount → approve in TUI
5. **Receive (`v`):** stealth URI copy
6. *(Optional)* VB (`w`) on allowlisted URL — show green inject banner; approve one read-only call only

---

## Suggested implementation order

1. `vb_browser` + `vb.session` suffixes (unblocks MCP + docs)
2. MCP `browser_*` B1
3. CLI + TUI EIP-712
4. AA confirm copy polish
5. Demo script + TASKS sync

## Verification gate

```bash
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## Unpushed commit reminder

`c32d8ea` — Blunt plan parked (local only until push).
