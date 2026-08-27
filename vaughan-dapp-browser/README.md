# VB (Vaughan Browser)

Optional allowlisted **Chromium-class** dApp shell for Vaughan-CLI.

User-facing name: **VB** (Vaughan Browser).  
Binary/crate: `vaughan-dapp-browser` (unchanged on disk).

## Engine (Phase 1)

System Chromium / Chrome / Brave / Edge + temporary unpacked MV3 extension:

- **MAIN** `inject.js` — EIP-1193 / EIP-6963 on `window.ethereum` + tamper watchdog
- **ISOLATED** `content_bridge.js` — `postMessage` ↔ extension port
- **background** — owns `WebSocket` to Vaughan; **read RPC proxy** via provider
- **declarativeNetRequest** — in-tab navigation gated by `allowlist.json`

Signing stays in the Vaughan TUI. Window stays open until you close it.

## Which browser should I install?

| Browser | Status | Notes |
|---------|--------|-------|
| **Chromium** | **Recommended** | Cleanest inject path; `sudo pacman -S chromium` on Arch/CachyOS |
| **Google Chrome** | **Recommended** | Same engine; widely installed |
| **Brave** | Supported | Auto-detected; pick Vaughan over Brave Wallet in dApps |
| **Microsoft Edge** | Supported | Chromium-based; auto-detected |
| **Firefox / Safari** | **Not supported** | Different extension model |

Override: `VAUGHAN_DAPP_BROWSER_CHROME=/usr/bin/brave`

Strategy: [`docs/dapp-browser-strategy.md`](../docs/dapp-browser-strategy.md)

## Usage

```bash
cargo run -p vaughan-cli                    # unlock first
cargo run -p vaughan-dapp-browser -- --self-check
cargo run -p vaughan-dapp-browser -- --url https://app.9inch.io/swap?chain=pulse
```

TUI **Web** → Enter opens VB when the binary is on `PATH`.

Env: `VAUGHAN_DAPP_BROWSER_CMD`, `VAUGHAN_DAPP_BROWSER_CHROME`, `VAUGHAN_DAPP_BROWSER_CDP_PORT`
