# CEF / CDP spike (Phase 0.5)

Scratch area — **not** a Vaughan workspace member. Build only from here:

```bash
cd docs/spikes/cef-tauri
cargo run --bin cdp_ax_smoke
```

See [`../dapp-browser-strategy.md`](../../dapp-browser-strategy.md).

## Results (2026-08-26)

| Check | Result |
|-------|--------|
| `cargo build -p vaughan-cli` without CEF | **PASS** (spike not in workspace) |
| Localhost CDP + interactive page refs | **PASS** (`cdp_ax_smoke` → example.com link `e0`) |
| `Accessibility.getFullAXTree` via chromiumoxide 0.7 | **FAIL serde** vs Chrome 151 (`uninteresting`); use `Runtime.evaluate` refs for now / upgrade CDP types in Phase 2 |
| Tauri + `tauri-runtime-cef` window | **Deferred to Phase 1** — runtime **not on crates.io**; needs git `tauri-apps/tauri` `feat/cef` (or wrymium) |

### Runtime lock (for Phase 1)

1. Prefer **`tauri-runtime-cef` from git** (`feat/cef` / cef-rs) for the owned shell.
2. MCP/agent side uses **CDP on 127.0.0.1** (proven here with system Chromium + chromiumoxide).
3. Same CDP client should attach to CEF’s remote-debugging port once the shell exists.

## Checklist

1. [x] Disposable binary in this dir (not workspace member)
2. [x] CDP interactive refs on allowlisted-style HTTPS (`example.com`)
3. [x] Confirm default `vaughan-cli` does not fetch CEF
4. [ ] Tauri+CEF embed window (Phase 1)
5. [x] Winner notes in strategy Spike notes
