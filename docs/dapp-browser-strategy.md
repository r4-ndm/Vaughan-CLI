# Vaughan dApp browser strategy

Optional **Chromium agent browser** side door for Vaughan-CLI. Browserless Pulse
(Ag / Dex / Browse / MCP) stays the **default** product path. This document is
the north star for `vaughan-dapp-browser` and related MCP tools.

## Plain-language: CEF

**CEF (Chromium Embedded Framework)** means we ship Chrome’s engine inside our
optional browser binary, instead of borrowing the OS webview (WebKitGTK on
Linux). That gives:

- Chrome-class dApp compatibility on every platform
- **CDP (Chrome DevTools Protocol)** — the remote-control API agents need for
  accessibility snapshots, clicks, and typing (same family as Playwright)

Stock Tauri + Wry (OS webview) is **rejected** for this product: no CDP, weak
Linux dApp fit.

## North star

1. **Best agent control** — CDP-class navigate / snapshot / click / type on
   allowlisted pages.
2. **Securable** — host allowlist, thin-proxy wallet bridge, **never auto-sign**
   (every sign/send hits Vaughan TUI approve/deny).
3. **Modular / disconnectable** — CEF only in an optional crate+binary; core
   Vaughan builds and runs without it.
4. **Multi-chain EVM** — not Pulse-only; chain comes from Vaughan’s active
   network + EIP-1193 `wallet_switchEthereumChain`.
5. Browserless Pulse remains default; this is a side door.

```
MCP agent → browser_* tools → localhost CDP → vaughan-dapp-browser (CEF)
                                              ↓
                                         allowlisted dApp
                                              ↓
                                    vaughan-provider (EIP-1193)
                                              ↓
                                    Vaughan TUI approve
```

## Verdict

| Piece | Choice |
|-------|--------|
| Shell | `vaughan-dapp-browser` — Phase 1: system Chromium + CDP; later Tauri+CEF |
| Prefer | Official `tauri-runtime-cef` + [cef-rs](https://github.com/tauri-apps/cef-rs); wrymium if blocked |
| Agent API | MCP → localhost CDP; Settings toggle default **off** |
| Wallet | Thin-proxy → `vaughan-provider` → TUI; **no agent auto-sign** |
| Chains | Multi-chain **EVM** (allowlist hosts; network from Vaughan) |
| Freedom | **Parked** — upstream [PR #195](https://github.com/solardev-xyz/freedom-browser/pull/195) pending ([freedom-browser-status.md](freedom-browser-status.md)) |
| Ladybird / Servo | Long-horizon / reject for now |
| Vaughan-Dioxus | Architectural guide only — never copy/vendor |

## Supported system browsers (Phase 1)

Phase 1 **does not ship a browser**. **VB** spawns a **Chromium-class** binary on
the user’s machine with an isolated profile + unpacked MV3 extension.

### Recommendation

| Tier | Browsers | Why |
|------|----------|-----|
| **Recommended** | **Chromium**, **Google Chrome** | Same engine we test against; no built-in wallet competing with Vaughan’s inject; predictable extension + CDP flags |
| **Supported** | **Brave**, **Microsoft Edge** | Chromium-based; auto-detected on `PATH`. Extra UX friction possible (see below) |
| **Not supported** | Firefox, Safari, Ladybird | Different extension / automation stack |

**Default install guidance for new users:** install **Chromium** (or Chrome if
they already use it). On Arch/CachyOS: `pacman -S chromium`.

### Why not Brave-first?

Brave works and is auto-detected, but it is **not** our top recommendation:

1. **Brave Wallet** — Brave injects its own Ethereum provider. Vaughan also
   injects via EIP-1193 / EIP-6963. dApps may show multiple wallets; the user
   must pick **Injected** / MetaMask-family / Vaughan, not Brave Wallet.
2. **Shields** — tracker/cookie blocking can break fragile DeFi frontends.
   Users may need to lower Shields per site.
3. **Support matrix** — we smoke-test against distro **Chromium** first.

Brave-only machines are fine: VB finds `/usr/bin/brave` automatically, or
set `VAUGHAN_DAPP_BROWSER_CHROME=/usr/bin/brave`.

### Auto-detect order

When `--chrome` is omitted, `vaughan-dapp-browser` tries (first hit wins):

Chromium → Chrome → Brave → Edge (plus common `/usr/bin/…` paths).

Override: `--chrome`, or env `VAUGHAN_DAPP_BROWSER_CHROME`.

### Future (Phase 3+)

Bundled **CEF** removes the “install Chromium” step and pins engine version for
agents — at the cost of download size and manual security updates. Until then,
system Chromium is the best balance of compatibility and maintenance.

### Frameworks considered and rejected as primary shell

| Option | Why not |
|--------|---------|
| Stock Tauri + Wry | No CDP; Linux WebKit fragile for DeFi |
| System Chrome + attach CDP | Not Vaughan-owned; weak hard allowlist; profile bleed |
| chromiumoxide / zendriver as the UI | Great CDP *clients* for our port; not an owned wallet shell |
| Electron (second product) | Freedom already covers interim Chromium + Node |
| Playwright-as-UI | Dual stack; not a wallet browser |
| wew / kurogane alone | Fallback only if Tauri+CEF spike fails |

## Modularity and kill-switch

Treat the browser as a **plugin binary**, not part of the wallet heart.

| Rule | Detail |
|------|--------|
| Separate crate | `vaughan-dapp-browser` — Tauri/CEF deps **only here** |
| No CEF in core | `vaughan-core`, `vaughan-provider`, `vaughan-mcp` never link `libcef` |
| Soft launch | TUI `w` / MCP discover binary via `PATH` or config; missing → Freedom dev fallback or “not installed” |
| Narrow seams | (1) spawn CLI (`--url`, allowlist, provider WS) (2) EIP-1193 to Vaughan (3) optional CDP port/token for agents |
| MCP degrade | `browser_*` tools return structured unavailable if child absent; Ag/Dex/MCP wallet tools keep working |
| Default build | `cargo build -p vaughan-cli` must **not** require CEF download |
| Kill-switch day | Drop/disable crate + soft-launch + MCP browser tools; Browserless Pulse + provider remain; Freedom stays parked |

## Multi-chain (not Pulse-only)

- Shell is a chain-agnostic HTTPS + wallet bridge. Product copy may still lead
  with Pulse; the binary must not hardcode chain id 369/943.
- Chain of record = Vaughan’s active network (`eth_chainId` /
  `wallet_switchEthereumChain` on the existing provider).
- Allowlist = trusted **hosts/origins**, not “Pulse dApps only.”
- v1 scope = **multi-chain EVM**. Non-EVM web wallets are out until a bridge exists.
- Phase 1 smoke: one Pulse allowlisted dApp **and** one non-Pulse EVM origin.

## Security rules (non-negotiable)

- Navigation allowlist only (no open-internet general browser).
- CDP binds **127.0.0.1 only**; off unless agent-browser toggle is on.
- Agent tools require unlock + toggle + live child process.
- Allowlist every CDP/`browser_navigate` (agents must not bypass UI chrome).
- Page never sees vault secrets / bearer tokens (thin-proxy).
- Honest EIP-6963 as Vaughan; MetaMask-family `isMetaMask` convenience flag
  only for Pulse dApp interop (documented; not a full MetaMask spoof).
- Timeout on approve = deny.

## Risks we must not underestimate

1. **Chromium CVEs** — bundled CEF does not auto-update like Chrome; pin + upgrade cadence.
2. **Size** — often >100MB installer, >300MB on disk; CI must fetch CEF artifacts.
3. **`tauri-runtime-cef` is early (0.1.x)** — spike before dating MCP delivery.
4. Sandbox / helper processes; Wayland vs X11 on Linux.
5. CDP is privileged — default off; consider random port + token.
6. Dual allowlist enforcement (navigation + agent navigate).
7. Provider inject + multi-chain switch approve path on CEF.
8. Never sneak CEF types into `vaughan-core`.
9. Keep Freedom bridge tests warm; integration **parked** until PR #195 merges.

## Phased delivery

### Phase 0 — Docs (this file + TASKS / REQUIREMENTS)

Done when strategy, Browserless Pulse side-door wording, and TASKS checkboxes exist.

### Phase 0.5 — CEF spike (gate)

On Linux: Tauri + `tauri-runtime-cef` loads allowlisted HTTPS; localhost CDP
accessibility snapshot returns usable refs. If blocked → wrymium → wew/kurogane.
Lock runtime in a short note under this doc’s “Spike notes” section. Still no
CEF link from default `vaughan-cli` build.

### Phase 1 — Modular shell + wallet

- Crate + binary; soft-launch from `w`; Freedom fallback.
- Host allowlist; thin-proxy → TUI approve.
- CDP only when toggle on.
- Smoke: Pulse + second EVM origin; `switchEthereumChain` if applicable.

### Phase 2 — MCP B1

`browser_open` / `browser_navigate` / `browser_status` (CDP; allowlisted).
Unavailable if binary missing.

### Phase 3 — MCP B2

`browser_snapshot` / `browser_click` / `browser_type` / `browser_press` /
`browser_wait` via CDP WebSocket (`Runtime.evaluate` refs + `Input.*`). Never auto-sign.
Chain changes still Vaughan-approved.

Optional later (B3): screenshots, multi-tab — one at a time.

## Explicit non-goals

- Replacing Browserless Pulse with “live in the browser”
- General-purpose Chrome replacement / arbitrary URLs / extensions
- Agent auto-sign or silent broadcast
- CEF inside `vaughan-core`
- Pulse-only browser lock-in
- Depending on the user’s personal Chrome profile
- Non-EVM web wallets in v1
- Ladybird as mid-term default
- Copying Vaughan-Dioxus source

## Ladybird (appendix)

Ladybird remains a long-horizon curiosity (Alpha, C++ primary). Revisit only if
it ships a stable embeddable engine with an agent-control story comparable to
CDP. Do not fork or schedule mid-term work.

## Spike notes

_Phase 0.5 complete (agent CDP path); Phase 1 Chromium shell landed (CEF embed still later)._

- **Agent CDP:** **PASS** — `docs/spikes/cef-tauri` `cdp_ax_smoke`.
- **Phase 1 binary:** `vaughan-dapp-browser` — system Chromium + MV3 extension
  (background WS → `ws://127.0.0.1:8745`, CSP-safe); `--cdp-port` advertises
  agent CDP (**off** unless set). Soft-launch from TUI `w`.
- **Phase 1 gaps:** MetaMask-family `isMetaMask` convenience flag for Pulse dApp interop while
  EIP-6963 still announces as Vaughan. **In-tab navigation gate:** shipped (MV3
  `declarativeNetRequest` + `allowlist.json`); MCP validates URLs server-side via
  `vb.session` allow suffixes (`browser_navigate`).
- **CEF/Tauri:** still git-only; not linked. Kill-switch: remove crate + soft-launch.
- **Modularity:** CEF not in core; Chromium shell only in this package (no
  chromiumoxide).

## Related

- [browserless-pulse.md](browserless-pulse.md) — default product thesis
- [freedom-browser-integration.md](freedom-browser-integration.md) — interim Chromium door
- [mcp.md](mcp.md) — MCP surface (browser tools land here later)
- [TASKS.md](../TASKS.md) — phased checkboxes
