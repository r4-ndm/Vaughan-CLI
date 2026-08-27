# Browserless Pulse — demo recording script

Use this to record a ~3 minute reel. **No Freedom Browser, no general Chrome
browsing** — only Vaughan TUI + optional VB inject banner if you show the web
side door.

## Before recording

- Wallet unlocked on **PulseChain testnet v4 (943)**
- Small tPLS balance for one Ag swap + one MCP micro-transfer
- Terminal: full screen, readable font (`vaughan` or `cargo run -p vaughan-cli`)
- Optional: Cursor with MCP connected for step 4

## Scene 1 — Unlock & dashboard (15s)

1. Launch Vaughan → unlock
2. Dashboard shows address + balance
3. Narration hook: *"The wallet that doesn't need Chrome."*

## Scene 2 — Ag swap (45s)

1. Press **`g`** (Aggregator)
2. Enter a small swap (e.g. tPLS → known testnet token)
3. Route preview → **approve once** → Done with tx hash
4. Point out: no website, no WalletConnect

## Scene 3 — Contract browser (30s)

1. Press **`c`** (Browse)
2. `browse 0x…` (known testnet contract) or paste from history
3. `call name` or `probe`
4. Narration: *"Inspect any contract without an explorer tab."*

## Scene 4 — MCP propose (45s)

1. In Cursor (or `vaughan propose transfer …`): agent proposes tiny transfer
2. Switch to Vaughan TUI → **Approve** card shows full request
3. **`y`** once → tx hash
4. Narration: *"Agents propose; you approve. Keys never leave Vaughan."*

## Scene 5 — Stealth receive (20s)

1. Press **`v`** (Receive)
2. Show public address + **stealth URI** (`st:…`)
3. Narration: *"Receive without linking deposits."*

## Optional scene 6 — VB side door (20s)

Only if `vaughan-dapp-browser` is installed:

1. Press **`w`** → open allowlisted dApp (e.g. 9inch / Squirrel on 943)
2. Show **green inject banner** in Chromium
3. **Do not** complete a sign on camera unless you want to show the TUI approve card
4. Narration: *"Optional web side door — signing still happens here in the terminal."*

## Do not show

- Freedom Browser (`VAUGHAN_FREEDOM_CMD` fallback)
- Random URLs / open internet
- Auto-sign / Sentient mode unless explicitly demoing policy

## After recording

- Export 1080p; keep Vaughan window legible
- Upload with title: *Vaughan — Browserless Pulse*
- Link [browserless-pulse.md](browserless-pulse.md) in description
