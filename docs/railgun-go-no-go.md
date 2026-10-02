# RAILGUN — go/no-go

> Recorded 2026-10-02 (Phases 0–1; Gates 0–1 passed). **Status: GO for Phases 0–2 on Sepolia,
> behind the opt-in `railgun` feature. User-facing release with `railgun` is
> BLOCKED** until the Wasmer Singlepass licence issue below is resolved.
> Supersedes the RAILGUN half of `docs/kohaku-go-no-go.md` (FR-3.4).

## TL;DR

- Source: upstream [`triamazikamno/railgun-rust`](https://github.com/triamazikamno/railgun-rust)
  (MIT), pinned to commit `b61fca7e2476dc3e5f89463fcef97f53661890e6` as a git
  dependency. No code is copied into Vaughan (CLAUDE.md engineering rule 5).
- Isolation: only `vaughan-railgun` depends on it, and only with its `enabled`
  feature. Frontends expose that as `railgun` (`vaughan-tui`, `vaughan-cli`).
  The default `cargo build --workspace` compiles none of it (checked in CI).
- Why it's viable now: railgun-rust derives keys the way the official engine
  does (`HMAC-SHA512("babyjubjub seed")`, `m/44'/1984'/0'/0'/i'` spend,
  `m/420'/1984'/0'/0'/i'` view) and its JS-parity fixtures pass. That
  incompatibility is what blocked Kohaku-rs.
- Not on PulseChain. Supported: Ethereum 1, BNB 56, Polygon 137, Arbitrum
  42161, plus Ethereum Sepolia 11155111 for testnet-first work.

## Phase 0 verification (2026-10-02)

| Check | Result |
|---|---|
| Default `--workspace` build excludes railgun-rust | Yes: 0 matches for `railgun-wallet` / `broadcaster-core` / `wasmer` in `cargo tree --workspace -e normal` |
| Existing dependency versions changed by adding it | None (lockfile only gained entries) |
| Upstream JS-parity fixtures (address, spending key, Poseidon sig, bound-params hash, witness inputs) | 5/5 pass |
| Full `railgun-wallet` + `broadcaster-core` suites under **Vaughan's** `Cargo.lock` (`alloy` 2.4.1; upstream tested 2.2.0) | 148 passed, 0 failed, 1 ignored (`scripts/railgun-parity.sh`) |
| `cargo audit` | No vulnerabilities. New informational warnings, all via the RAILGUN stack: `proc-macro-error2` unmaintained (RUSTSEC-2026-0173, via `dynasm` ← Wasmer Singlepass), `memmap2` 0.6.2 unsound (RUSTSEC-2026-0186, via `wasmer` ← `shared-buffer`). Pre-existing on `main`: `derivative`, `paste`, `chacha20` (yanked). |
| Crates added by `--features railgun` (vaughan-cli graph) | 242 |
| `unsafe` in railgun-rust | One block: `railgun-wallet/src/wasm_module_cache.rs:148` (`Module::deserialize` of cached compiled Wasm). See threat model. |

### Licences

All added crates are permissive (MIT / Apache-2.0 / BSD / ISC / Zlib / CC0 /
0BSD / BSL-1.0 Boost / Apache-2.0 WITH LLVM-exception), except:

- **`wasmer-compiler-singlepass` 6.1.0: Business Source License 1.1. RELEASE
  BLOCKER.** Production use requires a paid Wasmer sponsorship; each version
  converts to MPL-2.0 four years after release. railgun-rust uses Singlepass in
  exactly one place, the 13×13 POI witness store (`railgun-wallet/src/prover.rs`,
  `poi_witness_store`); everything else uses `Store::default()` (Cranelift,
  MIT). Phases 1–2 never run the prover, but the crate is still linked.
  Development and testing are non-production use and allowed. Fix: upstream PR
  to put Singlepass behind an off-by-default feature with a Cranelift fallback;
  otherwise fork (or Wasmer 5.x / native witness generation). No release build
  with `railgun` until resolved.
- `dynasm` / `dynasmrt`: MPL-2.0 (file-level copyleft; fine as unmodified
  dependency). Pulled only by Singlepass, so it disappears with the fix above.
- `self_cell`: Apache-2.0 OR GPL-2.0-only; we take Apache-2.0.
- railgun-rust's own crates declare no `license` field, but the repo is MIT
  (`LICENSE`). Upstream PR: add `license = "MIT"`.
- `rs-poseidon` (transitive git dep, see below): README declares MIT OR
  Apache-2.0, but the referenced licence files are missing from the repo.
  Acceptable; ask upstream to add the files.

### Transitive git dependencies (pin-and-review like the main pin)

| Crate | Source |
|---|---|
| `railgun-wallet`, `broadcaster-core`, `local-db`, `merkletree`, `poi`, `trustless-artifacts` | `triamazikamno/railgun-rust` @ `b61fca7e…` |
| `rs-poseidon` | `codex-storage/rs-poseidon` @ `bdbb8ba2735406e6e7b747abd3bc6711d55370aa` (Poseidon hash used by `broadcaster-core::crypto::poseidon`) |

### Sepolia deployment (verified on-chain)

| Item | Value | Evidence |
|---|---|---|
| Proxy | `0xeCFCf3b4eC647c4Ca6D49108b311b7a7C9543fea` | Blockscout-verified source `PausableUpgradableProxy` (RAILGUN's proxy contract) |
| Implementation (EIP-1967 slot) | `0x16dc2574cc30e4deecfb9ba22d95e5126a2fc0b3` | Code hash `0xd9ed4f83c7592b5a366858ae7a4f6c764543373bf7465ab457b80edc3d74b016` matches the `expectedBytecodeHash` in an independent RAILGUN Sepolia wallet config |
| Proxy admin (EIP-1967 slot) | `0x3d1171ccb5ab40ba5ab887c1527a16ec39a8f1ae` | on-chain storage |
| RelayAdapt | `0x7e3d929EbD5bDC84d02Bd3205c777578f33A214D` | Blockscout-verified source `RelayAdapt`; same deployer as the proxy |
| Deployer | `0x496F4da065012Eca6d65337217DF7F1e7a6B5928` | Blockscout |
| Scan start block | **`5784774`** (proxy created via internal tx; first events at 5784776) | Blockscout internal-tx + log index |

The published `deploymentBlock` (5784866) is about 90 blocks after creation;
Vaughan scans from the creation block. Sepolia has no legacy history, so
`v2_start_block` = `legacy_shield_block` = scan start.

## Phase 1: read-only (2026-10-02)

Receive screen (`v`) → `0` opens "Private (RAILGUN)" in builds with
`--features railgun`. Shows per-token shielded balances with scan progress;
the 0zk receive address (copy with `z`) on Sepolia only. Hidden behind one
line on chains without RAILGUN.

| Item | Result |
|---|---|
| Key parity with the official engine | `test-vectors/railgun/vectors.json` from `@railgun-community/engine` 9.8.0 (dev-only generator): HD accounts 0, 1 and 7 of the test mnemonic give identical 0zk addresses, master and viewing public keys |
| Seed access | `AccountManager::with_hd_seed` / `WalletState::with_hd_seed`: active HD account only (hardware and imported refused), seed zeroized after the closure |
| Keys kept | Viewing keys only (wiped on drop, no `Debug`); the spending key is zeroed right after derivation |
| Scanner | `eth_getLogs` over the 7 RAILGUN events, 5 000-block chunks (halved on errors), 6 confirmations, checkpoint every 20 chunks; timestamps fetched only for blocks holding our notes; alloy retry/backoff on rate limits |
| Full Sepolia scan (deployment → head, Tenderly) | ~4 min 46 s for ~6 M blocks; later runs resume from the cache |
| Real data | 407 live Sepolia events decode; a full scan finds 4 unspent notes for the public test mnemonic's account 0: 0.648375 WETH (`0xfff9976782d46cc05630d1f6ebab18b2324d6b14`) at block 11 830 615. Cross-check target for Railway (public mnemonic, so anyone can spend it) |
| Cache | `<profile dir>/railgun/<chain>-<index>.cache`, AES-256-GCM, chain/index/contract bound as associated data, `0600` file in a `0700` dir, atomic writes. Key: Argon2id (vault costs) over the account's viewing key, not the seed: the viewing key already decrypts every cached note from chain data, it avoids a second seed access, and the KDF runs off the UI thread |
| RPC history | Some RPCs answer old `eth_getLogs` with `[]` instead of an error (PublicNode Sepolia), which would read as a zero balance. Every scan first checks the deployment's first 50 blocks hold proxy events and fails loudly otherwise; the panel then tries the network's next RPC. Probe verified on Sepolia, Ethereum, Polygon and Arbitrum (BNB not checked: free RPCs refuse archive queries). Tenderly added as a Sepolia fallback RPC |
| Mainnet scans | Same scanner; from deployment that's ~10 M blocks on Ethereum (much slower, resumable). Quick-sync (squid) stays opt-in and is not built yet |
| POI / network calls | None besides the RPC; no telemetry |

## Threat model

| Risk | Mitigation |
|---|---|
| Key incompatibility with the official engine (lost funds on restore) | Parity fixtures + a Vaughan test vector + manual Railway cross-check at Gate 1; `scripts/railgun-parity.sh` on every pin bump |
| Malicious / broken upstream update | Exact `rev` pin + `Cargo.lock`; bump procedure below |
| Upstream disappears | Lockfile pin; fork fallback is a one-line `Cargo.toml` change |
| Seed / spending-key leakage | Seed only via a zeroizing `with_hd_seed` closure in `vaughan-core`; zeroize newtypes, no `Debug`; nothing persisted except an encrypted cache. **Residual:** upstream `WalletKeys` doesn't zeroize (upstream PR; fork trigger if refused) |
| Cached native code (`wasm_module_cache.rs` `unsafe` `Module::deserialize`) | Phase 3: cache only under Vaughan's private data dir with owner-only permissions, or disable the cache. Never a shared/temp dir |
| Tampered proving artifacts | railgun-rust `trustless-artifacts` hash verification; pin hashes in Vaughan |
| Lying RPC | Phase 1 balance is display-only; Phase 3 verifies Merkle roots on-chain before any spend |
| RPC without history returns empty logs | Deployment-block history probe before every scan; next RPC tried |
| Cache tampering / replay onto another account | AES-GCM with chain id, account index and contract as associated data; unreadable cache → full rescan |
| Third parties (RPC, squid, POI, IPFS) see metadata | Squid opt-in; endpoints disclosed and configurable; no telemetry |
| Funds stuck before unshield exists | Mainnet shield and mainnet 0zk receive address hidden until Gate 3; recovery = same mnemonic in Railway wallet |
| Hidden protocol fee | Contract basis-points fee shown on every shield/unshield card |
| Licence (Singlepass BUSL) | Release blocked until removed (see Licences) |

## How "signing requires explicit approval" applies (CLAUDE.md rule 5)

railgun-rust creates the RAILGUN EdDSA spend signature and zk proof while
**building** a send/unshield plan, before there's a final transaction to show.
So Phase 3 uses two steps:

1. **Prepare confirm:** recipient, token, amount, protocol fee and chain shown;
   on confirm, derive the spending key (inside `with_hd_seed`), build the plan
   and prove, all locally. Nothing leaves the machine; cancel discards it.
2. **Final approval:** the resulting `TransactionCall` goes through the
   existing approval gate (humanizer card, gas, linkage warning) and only then
   is the EVM transaction signed and broadcast.

The Phase 2 `RAILGUN_SHIELD` message signature is shown on the shield approval
card and never auto-signed or cached.

## Pin-bump procedure

1. Read the upstream diff `old_rev..new_rev` (focus: `keys.rs`, `crypto/`,
   `notes`, `scan.rs`, `prover.rs`, `artifacts.rs`, `Cargo.toml` deps).
2. Update both `rev`s in the workspace `Cargo.toml`; `cargo update -p railgun-wallet -p broadcaster-core`.
3. `./scripts/railgun-parity.sh` (must be all green).
4. `cargo audit` plus a licence re-check of new crates (watch for Singlepass/BUSL).
5. `cargo clippy` / `cargo test` with `--features vaughan-cli/railgun`.
6. Record old → new rev, date and findings in `TASKS.md`.

## Upstream PRs to send (non-blocking except where noted)

Tested patches for 1, 3 and 5 plus issue texts for 2, 4 and 6 are in
`docs/railgun-upstream/` (not yet sent).

1. **Singlepass behind an off-by-default feature, Cranelift fallback.** Release
   blocker for Vaughan (licence).
2. Optional `prover` feature so read-only builds skip `wasmer` / `ark-circom`.
3. `zeroize` for `WalletKeys` / spending key on drop.
4. Sepolia `RailgunDeployment` (values above) + quick-sync entry.
5. `license = "MIT"` in workspace crate manifests.
6. (to `codex-storage/rs-poseidon`) add the `LICENSE-MIT` / `LICENSE-APACHEv2`
   files its README references.

## Phases and gates

Plan details: `TASKS.md` → "RAILGUN via railgun-rust".

- **Gate 0 (passed 2026-10-02):** allowlist approved in CLAUDE.md; audit +
  licences reviewed; Sepolia verified; default build excludes railgun-rust.
- **Phase 1:** read-only 0zk address + shielded balance (Sepolia receive
  address only; mainnet balances display-only). **Gate 1 (passed
  2026-10-02):** official-engine parity vector; Railway 5.24.21 shows the same
  0zk address for the test mnemonic; the Vaughan TUI scan shows the expected
  Sepolia balance.
- **Phase 2:** Sepolia-only shield. **Gate 2:** Sepolia shield → scan → balance;
  POI aggregator confirmed on Sepolia.
- **Phase 3:** Merkle verification, prover + POI, two-step send/unshield.
  **Gate 3:** full Sepolia lifecycle + licence blocker resolved + your sign-off
  for mainnet.
- **Phase 4:** Waku broadcasters, separate go/no-go.
