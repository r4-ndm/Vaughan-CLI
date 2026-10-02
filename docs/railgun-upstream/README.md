# Upstream contributions to railgun-rust

Patches and issue drafts for [`triamazikamno/railgun-rust`](https://github.com/triamazikamno/railgun-rust)
and one dependency, prepared during RAILGUN Phase 0 (see `docs/railgun-go-no-go.md`).
These are contributions **to** upstream; nothing here is compiled into Vaughan.

All three patches are based on the pinned commit
`b61fca7e2476dc3e5f89463fcef97f53661890e6`, apply independently, and were
checked with `cargo check --workspace --all-targets`, `cargo test -p railgun-wallet`
(107 passed), `cargo clippy -p railgun-wallet --all-targets -D warnings` and
`cargo fmt --check`. Patch 01 was tested both with and without default features.

To send one: fork the repo, `git am docs/railgun-upstream/0N-*.patch` on a branch
from current `main` (rebase if needed), push to the fork, and open a PR with the
title/body below.

| # | What | Type | Vaughan impact |
|---|---|---|---|
| 01 | Wasmer Singlepass optional | Patch | **Release blocker** (BUSL-1.1) |
| 02 | Zeroize spending key on drop | Patch | Closes most of the key-residue risk |
| 03 | MIT licence in crate manifests | Patch | Clean licence scans |
| 04 | Optional prover feature | Issue | Lighter read-only builds |
| 05 | Sepolia deployment | Issue | Nice-to-have (Vaughan can pass its own) |
| 06 | `rs-poseidon` licence files | Issue (other repo) | Housekeeping |

---

## 01 — railgun-wallet: make Wasmer Singlepass an optional (default) feature

File: `01-optional-singlepass.patch`

> **Title:** railgun-wallet: make Wasmer Singlepass an optional (default) feature
>
> `wasmer-compiler-singlepass` 6.x is licensed BUSL-1.1: production use
> requires a paid Wasmer sponsorship. That makes `railgun-wallet` hard to adopt
> for wallets that ship to end users.
>
> Singlepass is only used for the 13×13 POI witness (`poi_witness_store`); every
> other witness already uses `Store::default()` (Cranelift, MIT). This PR adds a
> `singlepass` feature, **enabled by default so current behaviour is unchanged**.
> With `default-features = false`, the 13×13 POI witness also uses Cranelift, and
> `wasmer-compiler-singlepass` / `dynasm` / `dynasmrt` drop out of the graph.
>
> Tested: `cargo test -p railgun-wallet` with and without default features
> (107 passed each), clippy `-D warnings`, fmt, `cargo check --workspace`.
>
> Question: was Singlepass chosen for compile time/memory on the large circuit?
> If so, roughly how slow is Cranelift there? (We'll measure on our side too.)

## 02 — railgun-wallet: zeroize the spending key when WalletKeys is dropped

File: `02-zeroize-spending-key.patch`

> **Title:** railgun-wallet: zeroize the spending key when WalletKeys is dropped
>
> `WalletKeys` holds the spending private key as a plain `[u8; 32]` and is
> `Clone`, so each copy leaves the key in freed memory. This adds a `Drop` impl
> that wipes it with `zeroize` (already in the dependency graph via other crates).
>
> Deliberately narrow: `ViewingKeyData` and the intermediate `WalletNode` are
> `Copy`, so wiping those needs an API change (dropping `Copy`). Happy to do that
> as a follow-up if you're open to it.
>
> Tested: `cargo check --workspace --all-targets`, `cargo test -p railgun-wallet`
> (107 passed), clippy, fmt.

## 03 — Declare the MIT licence in every crate manifest

File: `03-license-metadata.patch`

> **Title:** Declare the MIT licence in every crate manifest
>
> The repo is MIT (`LICENSE`) but only `merkletree` sets `license`, so
> `cargo metadata`, cargo-deny and other licence scanners report the remaining
> crates as unlicensed. This sets `license = "MIT"` in `[workspace.package]` and
> inherits it in every crate. No code changes.

## 04 — Issue: optional prover feature for read-only consumers

> **Title:** Feature flag to build railgun-wallet without the prover (read-only use)
>
> We'd like to use `railgun-wallet` for read-only work first (key derivation,
> 0zk addresses, note decryption, `scan::parse_wallet_delta_from_logs`). Today
> that always compiles the proving stack (`ark-circom`, `wasmer` + Cranelift,
> artifacts), a large share of the ~380 crates `railgun-wallet` adds to our graph.
>
> Would you accept a `prover` feature (default on) that gates `prover`,
> `artifacts`, `zkey_cache`, `wasm_module_cache` and the plan-building parts of
> `tx` that need a `ProverService`? We can do the work; we'd like to agree on
> the module boundary first, since `tx::builder` mixes selection logic with
> proving.

## 05 — Issue: Sepolia deployment entry

> **Title:** Add Ethereum Sepolia (11155111) to RailgunDeployment / quick-sync
>
> For testnet-first development it'd help to have Sepolia built in. Values we
> verified on-chain (2026-10-02):
>
> - proxy `0xeCFCf3b4eC647c4Ca6D49108b311b7a7C9543fea` (Blockscout-verified
>   `PausableUpgradableProxy`, implementation `0x16dc2574cc30e4deecfb9ba22d95e5126a2fc0b3`)
> - RelayAdapt `0x7e3d929EbD5bDC84d02Bd3205c777578f33A214D` (verified `RelayAdapt`)
> - proxy created at block `5784774`
> - quick-sync: `https://rail-squid.squids.live/squid-railgun-eth-sepolia-v2/graphql`
>   (from a third-party Sepolia wallet config; not independently verified)
>
> Unknown to us: a Sepolia `relay_adapt_7702_contract`. Do you know whether one
> is deployed? If not, should the field become `Option<Address>`?

## 06 — Issue for `codex-storage/rs-poseidon`: add licence files

> **Title:** Add the LICENSE-MIT / LICENSE-APACHEv2 files referenced in the README
>
> The README says the crate is dual-licensed MIT / Apache-2.0 and links
> `LICENSE-MIT` and `LICENSE-APACHEv2`, but neither file is in the repo and
> `Cargo.toml` has no `license` field. Downstream licence scanners (e.g. via
> railgun-rust) flag it as unlicensed. Could you add the files and
> `license = "MIT OR Apache-2.0"`?
