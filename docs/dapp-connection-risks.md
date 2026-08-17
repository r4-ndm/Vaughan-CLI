# Vaughan + Freedom Browser dApp Connection — Risk Analysis & Edge Cases

**Document Status**: Technical Risk Assessment & Mitigation Plan  
**Target Subsystems**: `vaughan-provider`, `vaughan-tui`, Freedom Browser Signer Backend (`src/main/wallet/vaughan/`)

---

## 1. Executive Summary

While the **Vaughan + Freedom Browser** architecture (OS-level process separation via loopback IPC) offers massive security and architectural advantages over browser extension wallets and cloud relays, integrating an external terminal signer with a web browser introduces subtle edge cases.

This document details the **five areas of highest uncertainty / lowest confidence** in the connection pipeline, their failure modes, and concrete mitigation strategies.

---

## 2. Deep-Dive Risk Analysis

### Risk 1: The Localhost "Loopback Blindspot" & Origin Spoofing (High Security Risk)

#### Problem
`vaughan-provider` binds to `127.0.0.1:8745`. Because loopback sockets accept any local connection:
1. **Web Browser Cross-Origin WebSockets**: WebSockets initiated from standard browsers (e.g. Chrome, Firefox, or Brave) do **not** enforce standard CORS before opening a connection. A malicious website visited in an external browser could open `ws://127.0.0.1:8745`, query `eth_accounts`, and track the user's wallet address.
2. **Local Processes & Scripts**: Any script running in user space (Python, Node.js, curl) sends **no `Origin` header** (`origin: None`). Vaughan currently has no mechanism to cryptographically verify whether a connection originates from Freedom Browser's main process or a rogue local daemon.

#### Failure Scenario
A user visits `evil-drainer.com` on Chrome. The site silently probes `ws://127.0.0.1:8745`, discovers Vaughan is active, retrieves the user's active address via `eth_accounts`, and attempts to flood Vaughan with `personal_sign` or `eth_sendTransaction` requests.

#### Mitigation Strategy
- **One-Time Handshake Token (App Pairing Secret)**:
  Generate a persistent or session-based API secret (`~/.vaughan/provider.secret` with `0o600` permissions) shared between Vaughan and Freedom Browser, or require a handshake header (`Authorization: Bearer <token>`).
- **Strict Origin Allowlist Enforcement (FR-2.4)**:
  In `vaughan-provider`, automatically reject WebSocket connections carrying unauthorized `Origin` headers before they can invoke any RPC methods.

---

### Risk 2: TUI Focus Stealing & "Accidental Keypress" Approvals (High UX/Safety Risk)

#### Problem
In terminal UIs, keyboard input is continuous and unbuffered by GUI window managers. If an incoming dApp signing request pops up a modal dialog over the active view while the user is typing, a buffered keystroke can trigger an accidental approval.

#### Failure Scenario
1. The user is on the `Send` view in `vaughan-tui`, entering a recipient address and pressing `Enter` to proceed.
2. At that exact millisecond, a dApp in Freedom Browser dispatches an `eth_sendTransaction` request.
3. The TUI switches to the Approval Modal; the pending `Enter` keystroke immediately satisfies the modal's confirmation, signing and broadcasting the dApp's transaction without visual review.

#### Mitigation Strategy
- **Deliberate Confirmation Pattern**:
  Never use a bare `Enter` or single-key press for dApp signing approvals. Require a two-step gesture (e.g. typing `yes` + `Enter`, or requiring a specific combination like `Ctrl+Y`).
- **Input Debounce Window**:
  When an approval modal is rendered, discard all keyboard input for the first **300ms–500ms** to ensure buffered key events from previous screens cannot execute the approval.
- **Request Queue Isolation**:
  Display an unobtrusive notification pill ("1 Pending dApp Request — Press F2 to Review") rather than hijacking the active screen.

---

### Risk 3: Missing Read-Call Handlers (`eth_call`, `eth_estimateGas`, etc.)

#### Problem
Standard Web3 dApps often issue *read requests* (`eth_call`, `eth_estimateGas`, `eth_getBalance`, `eth_blockNumber`) directly through `window.ethereum.request(...)` rather than using their own RPC providers.

`vaughan-provider` currently only handles 8 methods in `methods.rs` and returns error code `4200` (`UnsupportedMethod`) for all other JSON-RPC methods.

#### Failure Scenario
A dApp calls `window.ethereum.request({ method: 'eth_estimateGas', params: [...] })`. If Freedom Browser forwards this call directly to Vaughan instead of its internal RPC pool, Vaughan rejects the call with code `4200`, causing the dApp frontend to crash.

#### Mitigation Strategy
- **Browser-Side Read Interception (Recommended)**:
  In `src/main/wallet/vaughan/signer.js`, ensure the backend intercepts read methods and resolves them using Freedom Browser's `rpc-manager.js` pool, routing **only** signing and account methods to Vaughan.
- **Core RPC Fallback Proxy**:
  Implement an optional transparent RPC proxy in `vaughan-provider` that forwards unhandled `eth_*` read methods directly to the active chain's RPC endpoint.

---

### Risk 4: Dynamic EIP-712 (`eth_signTypedData_v4`) Parsing Complexity in Rust

#### Problem
In JavaScript, EIP-712 structured data signing is forgiving because `ethers` handles dynamic JSON objects natively. In Rust, Alloy requires constructing typed domains and ABI encoders.

dApps across the ecosystem (e.g. OpenSea Seaport, Uniswap Permit2, Snapshot) use highly complex, deeply nested, or non-standard EIP-712 payload shapes (e.g., custom array types, mixed integer representations, stringified JSON strings).

#### Failure Scenario
A dApp sends a valid EIP-712 typed data payload containing complex nested structs. `vaughan-core` fails to parse or hash the dynamic schema, resulting in an internal parsing error (`-32603`) or generating an invalid signature hash.

#### Mitigation Strategy
- Implement dynamic EIP-712 hashing using `alloy_dyn_abi` and `alloy_primitives`.
- Add integration test suites in `vaughan-core` covering canonical test vectors from OpenSea Seaport and Uniswap Permit2.

---

### Risk 5: In-Flight Timeouts & Connection Drops

#### Problem
Web3 dApps typically enforce strict 30-to-60-second timeouts on signing requests. If a user steps away while an approval prompt is active:
- The dApp cancels the request on the browser side.
- If the user returns later and approves the prompt in the terminal, Vaughan signs and broadcasts a stale transaction that the dApp is no longer tracking.

#### Failure Scenario
A user initiates a DEX swap. The swap fee quote expires after 45 seconds. The user reviews the terminal prompt 3 minutes later and hits approve; the transaction executes with outdated slippage parameters or fails on-chain.

#### Mitigation Strategy
- **Prompt Auto-Expiration**:
  Attach a 60-second countdown timer to all pending approval modals in `vaughan-tui`. If not approved within the window, auto-reject with code `4001` (`UserRejected`).
- **Connection Lifecycle Synchronization**:
  If the WebSocket client disconnects or aborts the HTTP frame while a prompt is pending, immediately cancel and purge the prompt from the TUI queue.

---

## 3. Priority Hardening Roadmap

| Priority | Task | Target Component |
|---|---|---|
| **P0** | Shared Handshake Token / Pairing Secret | `vaughan-provider` & Freedom Browser |
| **P0** | Debounced & Deliberate Confirmation Pattern in TUI | `vaughan-tui` |
| **P1** | Filter Read Calls to Browser RPC Pool | Freedom Browser `signer.js` |
| **P1** | 60-Second Prompt Expiration & Disconnect Cleanup | `vaughan-tui` & `vaughan-provider` |
| **P2** | Dynamic EIP-712 Permit2/Seaport Test Vectors | `vaughan-core` |
