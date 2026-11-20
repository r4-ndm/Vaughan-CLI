# RAILGUN — roadmap for a slow, rock-solid build

**Status:** planning record, 2026-11-09. No shipping date. Sepolia only. The principle is
correctness over speed, and every irreversible step (a real note, a real proof, a real
unshield) is preceded by a reversible version of it.

Two independent reviews produced this: a technical path measured on this machine, and a
strategy/risk pass against the upstream project and the licence text. Facts marked
**verified** were checked directly (GitHub API, our pin, the lockfile); measurements are
from `docs`-less scratch runs described in §2; everything else is judgement and marked so.

---

## 1. The three findings that order everything

### 1.1 Our pin leaks the sender address in every note — verified, and it outranks the licence

We pin `railgun-wallet` at `b61fca7e` (**2026-10-01**). Upstream merged PR #2 on
**2026-10-21**: *"`try_from_note` fixed `sender_random` to `MEMO_SENDER_RANDOM_NULL`, so
every note encoded the sender's master public key. Any receiver could recover the sender's
0zk address, including broadcasters through their fee notes."*

Consequences, in order of severity:

- Every note our code writes today is a permanent privacy leak. You cannot un-leak a note;
  it is in the chain's history forever.
- Broadcasters can link all transactions they relay for one wallet.
- Sepolia testing is not "safe practice" here: it teaches the flow using code that writes
  the wrong ciphertext, and any test note we create is a leak we keep.

**Therefore: re-pin before any further shield work.** This is a one-commit change with a
real review cost (read `keys.rs`, `crypto/`, `notes`, `scan.rs`, `prover.rs`; then
`scripts/railgun-parity.sh` + full suite + `cargo audit` + licence re-check).

### 1.2 The Singlepass licence blocker is measured, not argued — it is a non-problem on desktop

`wasmer-compiler-singlepass` is BUSL-1.1: non-production use free, production needs a paid
Wasmer sponsorship, converts to MPL-2.0 ~2029–2030. It is used in exactly one place —
`railgun-wallet/src/prover.rs`, the 13×13 POI witness (`use_singlepass_poi_witness` returns
true only for 13 in / 13 out). Everything else already uses Cranelift (MIT).

Measured on this machine, POI 13×13 witness, release build, same lockfile:

| Metric | Singlepass (BUSL) | Cranelift (MIT) | Delta |
|---|---|---|---|
| Module JIT compile | 67.41 ms | 330.40 ms | +263 ms |
| Witness evaluation | 375.43 ms | **255.10 ms** | **Cranelift 32% faster** |
| Total in-process | 443.34 ms | 586.01 ms | +143 ms |
| Process wall time | 0.466 s | 0.611 s | +145 ms |
| Peak RSS | 95.4 MB | 295.4 MB | +200 MB |
| Licence | BUSL-1.1 | MIT | — |

A 145 ms difference is imperceptible in a TUI, Cranelift produces *faster* witness code,
and 295 MB peak is ordinary for a desktop wallet. **Patch 01 (already written and tested:
Singlepass behind a default-on feature, Cranelift fallback) is a clean win**, and the
licence issue disappears the moment we build downstream with `default-features = false`.

Caveat to keep honest: this measures the one circuit that uses Singlepass, on one machine,
with a warm cache. Re-measure if the circuit set or the pin changes.

### 1.3 Upstream is a one-maintainer project — patch, don't fork, and don't drift

**Verified 2026-11-09:** 1 star, 3 forks, **0 open issues**, last push 2026-11-04, MIT,
one maintainer. PR #2 merged 22 minutes after opening; PR #1 got maintainer-guided review
then closed. `main` still has `wasmer = { features = ["singlepass"] }` with **no
`[features]` table** — our patch has never been proposed. `keys.rs` still derives `Clone`
with no `Drop` zeroize. Our patches 02 and 03 are also not upstream.

Also upstream since our pin: a Sepolia chain config, `relay_adapt` made optional, and the
sender-leak fix. Implications:

- **Send the patches as PRs, not issues.** With zero open issues and one maintainer, a
  tested PR is a merge; an issue is a wish.
- **Fork only for the licence, and keep the delta to one commit.** The cost is not the
  patch, it is rebasing a 1,300-line `prover.rs` that upstream touches most weeks.
- **Re-pin regularly, in isolated commits.** Every week of drift makes the next re-pin a
  merge instead of a diff.
- Upstream is actively building `railgun-broadcaster` and `waku` crates — **do not build
  our own broadcaster** (Phase 4 territory).

---

## 2. Sequencing (each step reversible before the irreversible one)

**S0 — Freeze scope in writing.** Sepolia only. Read-only balances + shield are the only
user-facing RAILGUN features. No mainnet send, no mainnet 0zk receive beyond display.
Restate the licence blocker as *"first real-funds use"*, not *"release"*: the trigger is
using it on real value, which happens before any release.

**S1 — Re-pin upstream (privacy).** `b61fca7e` → current `main`. One commit. Gains the
sender-leak fix, Sepolia config, optional `relay_adapt`. Gates: parity script, full suite,
`cargo audit`, licence re-check. Record old→new in `TASKS.md`.

**S2 — Send the upstream PRs in parallel with S1's waiting time.** PR 01 (Singlepass
optional, Cranelift default), 02 (zeroize `WalletKeys`), 03 (licence metadata), 07
(shield npk check). Issues for 04–06, but convert 05 to a verification note since Sepolia
landed upstream. Internal review date: 3 weeks. If 01 is declined or ignored → one-commit
fork on a named upstream SHA.

**S3 — Close Phase 2's evidence honestly.** Manual Sepolia shield → rescan → note appears
with the expected amount after the 25 bp fee. Answer the POI-on-Sepolia question (§3.1).
If Sepolia POI is unavailable, that is a recorded limitation, not a "Gate 2 pending".

**S4 — Spendability before spend (highest-value new feature).** Merkle-root history
verification (exists — harden it) plus a read-only "would this note be spendable, and why
not" panel fed by POI status. Zero signing. Turns "I hope this worked" into evidence.

**S5 — Prover worker, offline first.** Build the Groth16 proof for an unshield in a
worker thread on the Cranelift path; verify it on-chain on Sepolia with a reviewable dump
of the inputs. Instrument proof time, memory, failure modes, artifact fetch/hash check,
first-run-offline behaviour. UI must never freeze; proof progress must be cancellable.

**S6 — One unshield, self-broadcast, to our own address.** Single token, two-step
approval, linkage warning, and `Broadcast { tx_hash }` only when the hash is real. Then
POI submission — staggered, isolated, documented.

**S7 — Broadcasters, as their own go/no-go.** Prefer upstream's `railgun-broadcaster` +
`waku` crates over anything homegrown. This is a counterparty decision, not a coding task.

**S8 — Then, and only then:** restore drills, external audit of the delta, small-value
mainnet unshield to self, and finally a decision about anyone else's money.

### Longer-horizon (optional hardening, not a licence fix)

Native witness generation (no Wasm at all) is worth roughly 24–35 weeks by the technical
review: codegen from the same `.circom` sources via `witnesscalc`/`circom --c`, a
differential suite running both calculators over random vectors plus historical on-chain
events, then dropping `wasmer` + `ark-circom`. It removes ~240 crates and the two
unfixable advisories (`ruzstd`, `memmap2`), but it makes us the crypto implementer and we
would own circuit parity forever. **Do it only if proof latency or dependency weight
actually bites** — the licence does not require it (see §1.2). Pilot on one circuit behind
a feature flag, with the differential test written first.

---

## 3. Risks, ranked by how expensively they fail late

1. **Key/recovery divergence discovered at restore time.** Funds sit in 0zk; the day you
   need them, the keys don't reproduce and nobody — including us — can recover. Fails
   *after* the money is in the pool. Mitigation: parity fixtures at every gate, restore
   drills on clean hardware, two independent implementations agreeing, enumerated account
   derivation.
2. **Notes that can never be spent (POI).** Balances display fine; only POI list status
   decides whether a note can leave. Surfaces when you try to unshield. Mitigation: the S4
   spendability panel, never calling a shielded balance "money" without status, only
   shielding what you can afford to have stuck.
3. **Prover reality discovered at the end.** Compile cost, memory, artifact fetch, first-run
   offline — all measured only in S5 today. Mitigation: S5 before any UI polish, pin
   artifact hashes, proof-latency budget with cancel.
4. **Metadata leakage baked into history.** Permanent, non-refundable. The sender leak
   (§1.1) is a live example; self-broadcast patterns, shield→unshield timing and quick-sync
   choices compound into a deanonymisation set. Mitigation: re-pin first; treat every new
   metadata source as a design decision, not a default.
5. **Fork drift.** A fork with Vaughan-only changes makes every upstream security fix a
   manual merge. Mitigation: one-commit delta, parity script green on every bump.

---

## 4. Explicitly not building yet

Mainnet send/unshield or mainnet 0zk receive beyond display · our own broadcaster or Waku
relay client · cross-chain RAILGUN swaps or arbitrary `RelayAdapt` calls · 7702
stealth-account shields, gas sponsorship, auto top-up · ERC-4337 paymaster · running a POI
node in production · RAILGUN tools on the MCP/agent surface · batch/multi-token unshield ·
"private swap" · native witness generation before §1.2's numbers are re-confirmed · any
`railgun`-enabled "beta" build that invites real funds.

---

## 5. Decisions this document asks for

1. **Re-pin now, or finish current RAILGUN UI work first?** Recommendation: re-pin now —
   every shield written before it is a permanent leak.
2. **Who owns the upstream PRs?** They need a GitHub identity and a 3-week follow-up.
3. **Fork-if-stalled: yes or no, and who rebases it?** Decide before sending PR 01.
4. **POI on Sepolia: available or not?** This decides whether Phase 2 evidence is possible
   at all on testnet.
5. **Is a paid Wasmer sponsorship ever on the table?** If yes it is a fifth option; if no,
   delete it from the discussion.

## Appendix — what to verify rather than trust

- Singlepass/Cranelift numbers: one machine, one circuit, warm cache. Re-measure on the
  target laptop before making any UI latency promise.
- Circuit inventory ("36 transact shapes + 2 POI shapes") comes from the technical review
  and is **not** independently verified here; count them from the artifact manifests before
  scoping any native-witness work.
- The upstream repo state above is a 2026-11-09 snapshot; re-check before acting on it.
