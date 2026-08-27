# Freedom Browser — parked (pending upstream PR)

**Status:** **Parked** — no active Freedom integration work in Vaughan until upstream
accepts our signer-backend PR.

| Item | State |
|------|--------|
| Vaughan provider bridge (`vaughan-provider`, TUI approve flow) | **Shipped** |
| Upstream Freedom signer backend | **Open PR** — [solardev-xyz/freedom-browser#195](https://github.com/solardev-xyz/freedom-browser/pull/195) |
| Product focus | **Browserless Pulse** (Ag / Dex / Browse / MCP) + **VB** (`vaughan-dapp-browser`) |

## What “parked” means

1. **Default path is not Freedom.** Users swap, inspect, and approve in the TUI
   without opening Electron. Agents use MCP. Allowlisted dApps use **VB** when
   installed.
2. **Freedom is a fallback only.** The Web screen (`w`) tries VB first, then a
   local Freedom checkout if `VAUGHAN_FREEDOM_CMD` (or a known binary path) is
   set. There is no expectation that end users install Freedom today.
3. **Upstream merge unblocks the integration.** Until [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195)
   lands on `solardev-xyz/freedom-browser` `main`, we do not treat Freedom as a
   supported product surface — only maintain the Vaughan-side bridge and smoke
   tests for when the PR merges.

## Active web paths (use these)

| Path | When |
|------|------|
| **Browserless Pulse** | Always — Ag (`g`), Dex (`d`), Browse (`c`), MCP |
| **VB** (`vaughan-dapp-browser`) | Optional — system Chromium + extension; signing still TUI-only ([dapp-browser-strategy.md](dapp-browser-strategy.md)) |
| **Freedom** | Dev fallback only — local checkout + env; **parked** until PR #195 merges |

## Vaughan-side artifacts (kept warm)

- `vaughan-provider` — loopback EIP-1193 WebSocket (`127.0.0.1:8745`)
- TUI Web screen + trusted dApp list → VB / Freedom launch
- Tests: `freedom_bridge_smoke`, optional `freedom_node_bridge_e2e` (`FREEDOM_REPO`)
- Research: [freedom-browser-integration.md](freedom-browser-integration.md)

## After PR #195 merges

1. Document install steps for released Freedom + Vaughan.
2. Re-enable Freedom as a documented optional side door (still not the default).
3. Close parked status here; track any follow-up parity items in
   [native-parity-tricks.md](native-parity-tricks.md).

## Related

- [browserless-pulse.md](browserless-pulse.md) — product thesis
- [dapp-browser-strategy.md](dapp-browser-strategy.md) — VB (owned Chromium shell)
- [Security-Table.md](Security-Table.md) — Freedom column marked parked
