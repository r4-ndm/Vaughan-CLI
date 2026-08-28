# VB MCP smoke — operator checklist

> **Purpose:** Verify Vaughan MCP → `vaughan-dapp-browser` (VB) → CDP agent navigation
> (B1 + B2) on your machine. **No signing** in this checklist.
>
> **When to run:** After pulling `main` with commits `5f5e3af` (B1) and `dca40e6` (B2),
> or any later branch that includes `vb_browser`, `vb_cdp`, and `browser_bridge`.
>
> **Time:** ~15 minutes first run; ~5 minutes once green.

---

## Prerequisites

| # | Check | Command / action |
|---|--------|------------------|
| 1 | Repo up to date | `cd ~/Desktop/Vaughan-CLI && git pull` |
| 2 | Builds clean | `cargo build -p vaughan-cli -p vaughan-dapp-browser` |
| 3 | VB on PATH | `which vaughan-dapp-browser` — if empty: `cargo install --path vaughan-dapp-browser` |
| 4 | Chromium/Chrome | `which chromium` or `which google-chrome` — note full path |
| 5 | Port 9222 free | `ss -tlnp \| grep 9222` — should be empty before smoke |

---

## One-time Cursor MCP setup

Edit [`.cursor/mcp.json`](../.cursor/mcp.json) — add `env` to the `vaughan` server:

```json
{
  "mcpServers": {
    "vaughan": {
      "command": "cargo",
      "args": ["run", "-q", "-p", "vaughan-cli", "--", "mcp", "--profile", "default"],
      "env": {
        "VAUGHAN_DAPP_BROWSER_CDP_PORT": "9222",
        "VAUGHAN_DAPP_BROWSER_CHROME": "/usr/bin/chromium"
      }
    }
  }
}
```

Replace `VAUGHAN_DAPP_BROWSER_CHROME` with your browser path if different.

**Restart Cursor MCP** (Settings → MCP → toggle `vaughan`, or restart Cursor).

Confirm tools appear: `browser_open`, `browser_navigate`, `browser_status`,
`browser_snapshot`, `browser_click`, `browser_type`, `browser_press`, `browser_wait`.

---

## Session layout

| Terminal | Role |
|----------|------|
| **A** | Vaughan TUI unlocked (optional but recommended for full MCP stack) |
| **Cursor chat** | Agent calls MCP tools (paste prompt below) |

Terminal A:

```bash
cd ~/Desktop/Vaughan-CLI
cargo run -p vaughan-cli
# unlock wallet; leave running
```

---

## Phase 0 — VB alone (no MCP)

Proves Chromium + CDP before involving Cursor.

```bash
export VAUGHAN_DAPP_BROWSER_CDP_PORT=9222
vaughan-dapp-browser --url https://example.com --cdp-port 9222 --allow-host example.com
```

Second terminal:

```bash
curl -s http://127.0.0.1:9222/json/version
curl -s http://127.0.0.1:9222/json/list | head -20
```

- [ ] Browser window opens on example.com
- [ ] `/json/version` returns JSON (not connection refused)
- [ ] `/json/list` shows at least one `"type": "page"` entry

Kill VB when done (close window or `pkill vaughan-dapp-browser`).

---

## Phase 1 — MCP B1 (open / status / navigate)

Paste into **Cursor chat** (Vaughan MCP connected):

```
VB MCP smoke Phase 1 — run in order, paste each tool result:

1. browser_status {}
2. browser_open { "url": "https://example.com" }
3. browser_status {}
4. browser_navigate { "url": "https://example.com/" }
5. browser_status {}
```

### Pass criteria

| Step | Expected |
|------|----------|
| 1 | `"available": false`, `"reason": "no_vb_session"` |
| 2 | `"status": "opened"`, `"cdp_alive": true` |
| 3 | `"available": true`, `"pages"` non-empty |
| 4 | `"status": "navigated"` |
| 5 | Still `"available": true` |

- [ ] Phase 1 pass

---

## Phase 2 — MCP B2 (snapshot / wait / click)

Same session (VB still running from Phase 1). Paste:

```
VB MCP smoke Phase 2 — run in order:

1. browser_snapshot {}
2. browser_wait { "text": "Example Domain", "timeout_ms": 15000 }
3. browser_click { "ref": "e0" }   # only if e0 exists; skip if refs empty
4. browser_snapshot {}
```

### Pass criteria

| Step | Expected |
|------|----------|
| 1 | `"page"` with `"title"`, `"url"`, `"refs"` array (`e0`, …) |
| 2 | `"status": "wait_met"` |
| 3 | `"status": "clicked"` or `"ok": true` in result (or skip if no refs) |
| 4 | Snapshot returns again without error |

- [ ] Phase 2 pass

---

## Phase 3 — MCP B2 (type / press) — optional

Use a page with an input if you have one allowlisted. Skip on example.com if no inputs.

```
browser_type { "ref": "eN", "text": "hello" }
browser_press { "key": "Enter" }
```

Only run on a **test page you control** — not a live dApp with real funds.

- [ ] Phase 3 pass or N/A

---

## Phase 4 — Allowlist gate (negative test)

Confirms server-side nav block works:

```
browser_navigate { "url": "https://evil.example.not-allowlisted.test/" }
```

- [ ] Tool returns error containing `not in VB allowlist` (or similar)

---

## Automated CI (no VB binary required)

Run before/after smoke to catch regressions:

```bash
cargo test -p vaughan-mcp --test conformance
cargo fmt --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

---

## Troubleshooting

| Symptom | Fix |
|---------|-----|
| `vaughan-dapp-browser not found` | `cargo install --path vaughan-dapp-browser`; restart MCP |
| `cdp_alive: false` | Check port 9222; ensure `VAUGHAN_DAPP_BROWSER_CDP_PORT` in `mcp.json` env; restart Cursor |
| No browser window | Set `VAUGHAN_DAPP_BROWSER_CHROME` to full path in `mcp.json` |
| `no vb.session` | Run `browser_open` first |
| MCP tools missing | Open repo root in Cursor; rebuild `vaughan-cli`; restart MCP |
| `browser_unavailable: CDP not reachable` | VB crashed — `browser_open` again |

---

## Prompt for a fresh agent session

Copy-paste this when you return:

```
Read docs/vb-mcp-smoke.md and run the full VB MCP smoke (Phase 0–2 minimum).
I'm on Linux, repo at ~/Desktop/Vaughan-CLI. Report pass/fail per checkbox.
If blocked, tell me exactly which prerequisite failed.
```

---

## Related docs

- [mcp.md](mcp.md) — MCP setup
- [mcp-smoke.md](mcp-smoke.md) — general MCP smoke (wallet propose/approve)
- [browserless-pulse-demo.md](browserless-pulse-demo.md) — product demo (broader than VB smoke)
- [dapp-browser-strategy.md](dapp-browser-strategy.md) — VB architecture (B1/B2)
