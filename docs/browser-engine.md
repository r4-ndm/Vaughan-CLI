# Vaughan Browser Engine — a terminal contract browser

> The idea + requirements for Vaughan's contract browser (Phase 4 in `TASKS.md`).
> Written so any agent (or future-us) can pick this up and build it without
> re-deriving the design. Full cross-repo scope record: `wiz4rd-swap/docs/other-dexes-scope.md` (rev 5).

---

## 1. The one-line idea

**A terminal "browser" inside Vaughan: point at any contract address, and the
wallet discovers what the contract is (probe), shows you its functions (ABI),
lets you call them (read-only), and navigates the chain's DEXs/pairs/pools via
events — with zero custom code per protocol.**

It is *not* a DEX feature wearing a costume. Browsing and calling arbitrary
contracts is a general wallet capability — on par with Vaughan's provider,
chain-adapter, and signing layers. DEX browsing is just the first (most
compelling) use of it.

## 2. Why this design

| Rejected approach | Why |
|---|---|
| Per-DEX adapters (`V2ForkAdapter`, `V3ForkAdapter`, …) | N adapters for N DEXes; new DEX = new code. A browser handles any fork, original, or future protocol identically. |
| Shell out to `cast` (Foundry) | Vaughan is a wallet — it can't require the user's dev tools. Also, alloy *is* the library Foundry itself is built on, so linking it is the battle-tested option **as a library**. |
| Hand-rolled ABI codecs / crypto | Unnecessary risk. alloy-dyn-abi + alloy providers are battle-tested and already in the workspace. |

## 3. The three capabilities

1. **ABI resolution** — given any address:
   - **Verified contracts:** fetch the ABI from the explorer API
     (`api.scan.pulsechain.com/api?module=contract&action=getabi&address=…` —
     verified working 2026-08-18). Cache locally.
   - **Unverified contracts:** **selector probing** — `eth_call` against a
     capability library of known signatures (ERC20, V2 factory/pair, V3
     factory/pool, WETH, Multicall, …). This is a *knowledge library*, not
     per-DEX code: it matches any contract that speaks a standard interface.
2. **Generic calls** — encode/decode any function against the resolved ABI with
   alloy dyn-abi. Read-only in v0.1; write calls later through Vaughan's
   existing alloy signer layer (never subprocesses).
3. **Discovery by events** — find pairs/pools by scanning factory logs
   (`PairCreated` / `PoolCreated` topics) instead of deriving addresses from
   per-DEX init code hashes. Works identically for every fork — no hashes, no
   per-DEX constants.

## 4. Architecture

```
vaughan-core/                 ← THE ENGINE. Pure Rust, no UI, no DEX knowledge.
└─ browser/                   (new module)
   ├─ abi.rs                  explorer getabi fetch + local cache + parse
   ├─ probe.rs                selector-probe capability library → fingerprint
   ├─ selectors.rs            PUSH4 opcode extraction from getCode (~30 lines)
   ├─ sigdb.rs                4byte.directory lookup (reqwest)
   ├─ call.rs                 generic dyn-abi encode → eth_call → decode
   └─ events.rs               eth_getLogs topic scanning (PairCreated/PoolCreated)

vaughan-tui/
└─ views/browser.rs           ← THE INTERFACE. REPL pane (input line + output),
                                 reusing existing input.rs; ratatui + crossterm

wiz4rd-sdk/ (joins workspace at integration)   ← DEX VIEWS only (later phase)
   ├─ v2 view: price = getReserves ratio
   └─ v3 view: slot0 + wiz4rd-math tick math
```

Dependency direction: `vaughan-tui → vaughan-core::browser`; DEX views later
consume the engine's primitives + add protocol math. The engine never imports
DEX math.

## 5. REPL command surface

Stateful context is the point: `browse 0x…` sets the *current contract*, and
subsequent commands operate on it — navigation feels like browsing, not
one-shot subcommands.

```
browse 0x29eA…C523            # set context: show fingerprint + top functions
probe                         # re-run selector sniff → protocol fingerprint
info                          # name/ABI source (verified vs probed), bytecode size
call slot0()                  # call any function on current contract (read-only)
call getReserves()            # decoded output, typed
pairs                         # event-scan a factory: list pairs/pools (any fork)
token 0x95B3…90ab             # metadata probes: symbol, decimals, totalSupply
price PLSX/WPLS               # best-effort price across probed DEXes (later)
help                          # command list
history                       # scroll previous commands/output
```

Example session: `browse <pulsex-factory>` → `pairs` (186k pairs found) →
`browse <pair-address>` → `call getReserves()` → `token <token0>`.

Batch mode for scripting: `vaughan browser -c "browse 0x…; call slot0()"`.

## 6. Building blocks (battle-tested only)

| Concern | Crate | Status |
|---|---|---|
| RPC / calls / logs / code | alloy (provider) | workspace dep |
| Dynamic ABI encode/decode | `alloy-dyn-abi` | **already a vaughan-core dep** |
| ABI JSON parse | `alloy-json-abi` (via alloy) | available |
| HTTP (explorer, 4byte) | reqwest | add |
| JSON | serde / serde_json | workspace deps |
| TUI | ratatui + crossterm | workspace deps |
| Input line/history | existing `vaughan-tui/src/input.rs` (+ `tui-input` if wanted) | exists |

**We write only glue**: the probe library, PUSH4 parser, ABI cache, REPL view —
all unit-testable, zero crypto/encoding of our own. If a capability already
exists battle-tested, use it; do not hand-roll.

## 7. Requirements

### Functional
- [ ] **ABI resolution** — fetch verified ABIs from `api.scan.pulsechain.com`; parse to a callable form; local disk cache keyed by address (+ network).
- [ ] **Probe** — capability library (ERC20, V2 factory/pair, V3 factory/pool, WETH, Multicall3, …) probed via `eth_call`; returns a protocol fingerprint (e.g. `v2-factory`, `erc20`, `unknown`).
- [ ] **PUSH4 extraction** — derive candidate selectors from bytecode for unverified contracts (correctly skipping PUSH-data; PUSH0 handled).
- [ ] **Signature lookup** — resolve selectors to signatures via 4byte.directory; offline fallback = raw selector hex.
- [ ] **Generic call** — `call <sig or selector> [args]` on the current contract, typed decode of the result, clear error on revert.
- [ ] **Event scan** — `pairs <factory>` via `PairCreated`/`PoolCreated` topic0 logs (paginated), without init code hashes.
- [ ] **REPL view** — input line + scrolling output; stateful current-contract context; `help`; history (persisted); tab completion of commands/selectors.
- [ ] **Batch mode** — `-c "cmd1; cmd2"` non-interactive, same engine.
- [ ] **Tests** — probe fingerprints against known contracts (PulseX factory `0x29eA…C523`, Multicall3 `0xcA11…`), PUSH4 parser fixtures, ABI cache round-trip, call encode/decode vectors.

### Non-functional
- [ ] **Pure Rust, no cast/foundry at runtime** — the engine must work with zero external binaries. `cast` is a dev-time cross-check only.
- [ ] **Read-only on other DEXes in v0.1** — no write calls through the browser. Money-moving flows only via Vaughan's existing approval + signer path.
- [ ] **Graceful degradation** — unverified + no explorer access → probe-only support; never crash, always explain.
- [ ] **Network-aware** — ABI cache and probe results keyed by chain; works on PulseChain testnet 943 and mainnet 369.
- [ ] **Quality gate** — `cargo build`, `cargo test`, `cargo fmt --check`, `cargo clippy -D warnings` all clean (existing workspace standard).

## 8. Verified facts (2026-08-18)

- `getabi` endpoint works on `api.scan.pulsechain.com` (tested on Multicall3 `0xcA11bde05977b3631167028862bE2a173976CA11`).
- PulseX Router `0x165C…552d9` → Factory `0x29eA…C523`, PLSX `0x95B3…90ab`, **186,244 live pairs** (V2-style `allPairsLength()`).
- PulseChain: chain IDs 369 (mainnet) / 943 (testnet); EIP-1559 supported on both.

## 9. Scope boundaries (explicitly deferred)

- Write calls / swaps on other DEXes (v0.1 is read-only)
- Cross-DEX routing / aggregation (a `price` aggregation view is optional-later)
- StableSwap / exotic curve math (renders as a contract; views attach only where standard interfaces match)
- Tx replay / tracing (`cast run` equivalent needs an EVM — **revm**, battle-tested, used by Foundry; a later power feature)
- DEX *views* (V2/V3 price) — from `wiz4rd-sdk` at workspace integration, not the engine

## 10. Cross-references

- Scope record (all revs): `wiz4rd-swap/docs/other-dexes-scope.md`
- Implementation tasks: Vaughan-CLI `TASKS.md` → Phase 4
- Plan entry: Vaughan-CLI `PLAN.md` → Phase 4
- The engine it plugs into: `vaughan-core/src/chains/evm/adapter.rs`, `vaughan-provider` (RPC), `vaughan-tui/src/input.rs`
