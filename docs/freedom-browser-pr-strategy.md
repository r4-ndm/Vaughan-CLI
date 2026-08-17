# Freedom Browser Upstream PR Strategy — Maximizing Merge Acceptance

**Target Repository**: `solardev-xyz/freedom-browser` (MPL-2.0)  
**Objective**: Submit a clean, high-conviction Pull Request integrating Vaughan as a native `Signer` backend that maintainers can easily review, test, and merge without hesitation.

---

## 1. The Core Strategy: "The Zero-Friction Contribution"

Open-source maintainers reject or ignore PRs when they introduce:
- Heavy new dependencies / supply chain risks
- Invasive refactoring of existing core logic
- Missing test coverage or failing CI pipelines
- Unclear motivation or poor documentation

To guarantee the highest acceptance rate, our PR will follow the **"Invisible Footprint"** rule:
1. **Zero New npm Dependencies** (reusing existing `ws` and `ethers`).
2. **Exact Architectural Mirroring** (implementing `vaughan/` identically to their existing `ledger/` backend).
3. **100% Isolated & Opt-In** (zero code execution for users not using Vaughan).
4. **Self-Contained Mock Tests** (CI passes 100% green without needing Vaughan installed).

---

## 2. Step-by-Step Execution Plan

```
Phase 1: Pre-PR Engagement (Issue / RFC)
   │
   ▼
Phase 2: Code Implementation (Strict Ledger Mirroring)
   │
   ▼
Phase 3: Automated Unit & Mock Testing (CI Readiness)
   │
   ▼
Phase 4: PR Submission with Professional Template
   │
   ▼
Phase 5: Responsive Maintainer Collaboration
```

---

### Phase 1: Pre-PR Alignment (Opening the RFC Issue)

Before opening the PR, open a friendly GitHub Issue/Discussion on `solardev-xyz/freedom-browser` titled:  
`[RFC] Add Vaughan CLI as a Local Native Signer Backend (mirroring Ledger pattern)`

#### Issue Template Draft:
> **Hi Freedom Browser Team,**
>
> We love Freedom Browser’s security model and its modular `Signer` factory architecture. We have been building Vaughan-CLI (a local, memory-safe Rust terminal wallet) and would love to contribute a native signing backend to Freedom Browser.
>
> **Proposed Design:**
> - Follows the exact pattern established by `src/main/wallet/ledger/` (`transport.js`, `signer.js`, `errors.js`, `ipc.js`).
> - Connects to Vaughan over loopback WebSocket (`ws://127.0.0.1:8745`).
> - Reuses existing `package.json` dependencies (`ws` and `ethers`) — **zero new npm packages added**.
> - Strictly local: no hosted services, no clear-signing telemetry, keys remain completely outside the browser in a separate OS process.
>
> We already have a working, tested implementation ready. Would the maintainers be open to an upstream PR for this?

*Why this works*: It respects maintainer time and gives them ownership over the review process before seeing code.

---

### Phase 2: Implementation Guidelines (The "Invisible Footprint")

Implement `src/main/wallet/vaughan/` with surgical precision:

| File to Add / Modify | Responsibility | Strict Requirement |
|---|---|---|
| `src/main/wallet/vaughan/transport.js` | WebSocket client to `ws://127.0.0.1:8745` | Mirror `ledger/transport.js`. Handle reconnects, serialized queue, ping/pong, and error mapping. |
| `src/main/wallet/vaughan/signer.js` | Implements `Signer` interface | Implement `getAddress`, `signTransaction` (`vaughan_signTransaction`), `signMessage`, `signTypedData`. Check stored address on connect. |
| `src/main/wallet/vaughan/errors.js` | Error code mappings | Mirror `ledger/errors.js`. Map EIP-1193 codes (`4001` user rejected, `4100` unauthorized, `4900` disconnected) to clean browser errors. |
| `src/main/wallet/vaughan/ipc.js` | Account discovery IPC | Handle account query IPC without blocking renderer. |
| `src/main/wallet/identity-manager.js` | Wallet type registration | Add `WALLET_TYPES.VAUGHAN = 'vaughan'` and `addVaughanWallet(name, address)`. |
| `src/main/wallet/signers.js` | Signer factory dispatch | Add 3 lines: `record.type === WALLET_TYPES.VAUGHAN ? createVaughanBackend(record) : ...` |

#### Critical Rules:
- **No Linter Changes**: Match their ESLint / Prettier config down to the exact tab/space and semicolon style.
- **No Refactoring**: Do not clean up or touch unrelated files.
- **License Header**: Include their standard MPL-2.0 header comment on all new files.

---

### Phase 3: Self-Contained Unit & CI Tests

Maintainers love PRs with green CI tests that prove nothing is broken:
1. Add `test/wallet/vaughan-signer.test.js`.
2. Use a mock WebSocket server in the test (e.g. standard Node `http`/`ws` mock) so the test suite passes on GitHub Actions without needing `vaughan-cli` running on the test runner.
3. Test all 4 signing methods (`signTransaction`, `signMessage`, `signTypedData`, `getAddress`) plus rejection code `4001`.

---

### Phase 4: The Ultimate PR Submission Template

When creating the Pull Request on GitHub, use this exact description structure:

```markdown
### Summary
Adds native signer backend support for **Vaughan CLI** (a local Rust terminal wallet), allowing users to use Vaughan as their signing provider directly inside Freedom Browser.

### Architecture & Design
This implementation directly mirrors the existing **Ledger backend pattern** in `src/main/wallet/ledger/`:
- **`src/main/wallet/vaughan/`**: Transport, Signer contract implementation, error mapping, and IPC.
- **Zero New Dependencies**: Reuses existing `ws` and `ethers` dependencies already in `package.json`.
- **Hardware-Grade Isolation**: Signing requests are forwarded over loopback WebSocket (`ws://127.0.0.1:8745`). Private keys never enter the browser or JavaScript memory.
- **No Hosted Services**: All signing operations remain 100% local to the user's machine.

### Changes
- [x] Added `src/main/wallet/vaughan/` (`signer.js`, `transport.js`, `errors.js`, `ipc.js`)
- [x] Registered `WALLET_TYPES.VAUGHAN` in `identity-manager.js`
- [x] Added factory dispatch in `signers.js`
- [x] Added automated mock unit tests in `test/wallet/vaughan-signer.test.js`

### Verification & Testing
- [x] `npm test` passes 100% green
- [x] Tested with Uniswap & PulseX on Sepolia / PulseChain
- [x] Tested `signTransaction`, `personal_sign`, `eth_signTypedData_v4`
- [x] Tested user rejection and disconnection error handling

### License
Contributed under the Mozilla Public License 2.0 (MPL-2.0).
```

---

### Phase 5: Maintainer Review & Collaboration

Once the PR is open:
1. **Be Rapid on Feedback**: If a maintainer requests a naming change or style adjustment, address it within 24 hours.
2. **Be Flexible**: If they want the folder named differently or want to adjust how `addVaughanWallet` is exposed in the UI, accept their guidance cheerfully.
3. **Offer Live Demo Video / GIF**: Attach a short 15-second screencast showing Freedom Browser approving a Uniswap transaction via Vaughan CLI. Visual proof dramatically speeds up merges!

---

## 3. Probability Maximization Checklist

- [ ] Reused existing `ws` and `ethers` (0 new npm dependencies).
- [ ] Mirrored `ledger/` directory layout and coding conventions.
- [ ] Included comprehensive unit tests with a mock WebSocket server.
- [ ] Included MPL-2.0 license compliance.
- [ ] Prepared a 15-second visual demonstration GIF for the PR description.
