#!/usr/bin/env python3
"""Pairwise body proof for the app.rs spawn_job -> jobs/*.rs extraction.

Unlike the hl-tui split, this one is NOT a pure line move: each
`UiJob::Variant { .. } => { body }` match arm became a free function whose
signature names the same bindings the arm pattern bound, plus a delegating call
site in app.rs. So a whole-file multiset proof would drown in expected wrapper
noise.

Instead: for every job that moved, diff the ORIGINAL arm body against the NEW
function body, normalising only what the extraction is allowed to change:
  - `use ...;` lines hoisted to the module top
  - `&wallet`/`&handle` -> `wallet`/`handle` (params are already references)
  - fully-qualified paths that became plain imports
  - whitespace / rustfmt reflow
Anything left is a real behaviour-relevant edit and must be reviewed by hand.

Usage: scripts/split-proof-app-jobs.py [BASE_REV]
"""
import difflib
import re
import subprocess
import sys

BASE = sys.argv[1] if len(sys.argv) > 1 else "HEAD"
OLD_APP = "vaughan-tui/src/app.rs"

# variant -> (new_file, new_fn_name)
MOVED = {
    "Unlock": ("vaughan-tui/src/jobs/refresh.rs", "unlock"),
    "RefreshChrome": ("vaughan-tui/src/jobs/refresh.rs", "refresh_chrome"),
    "RefreshBalance": ("vaughan-tui/src/jobs/refresh.rs", "refresh_balance"),
    "RefreshAssets": ("vaughan-tui/src/jobs/refresh.rs", "refresh_assets"),
    "RefreshActivity": ("vaughan-tui/src/jobs/refresh.rs", "refresh_activity"),
    "RefreshAllowances": ("vaughan-tui/src/jobs/refresh.rs", "refresh_allowances"),
    "RefreshHexStakes": ("vaughan-tui/src/jobs/refresh.rs", "refresh_hex_stakes"),
    "PollTxStatus": ("vaughan-tui/src/jobs/refresh.rs", "poll_tx_status"),
    "RefreshBroadcastStatuses": ("vaughan-tui/src/jobs/refresh.rs", "refresh_broadcast_statuses"),
    "ReplaceBroadcast": ("vaughan-tui/src/jobs/refresh.rs", "replace_broadcast"),
    "EstimateFee": ("vaughan-tui/src/jobs/send.rs", "estimate_fee"),
    "EstimateTokenFee": ("vaughan-tui/src/jobs/send.rs", "estimate_token_fee"),
    "SendWithFee": ("vaughan-tui/src/jobs/send.rs", "send_with_fee"),
    "Send": ("vaughan-tui/src/jobs/send.rs", "send"),
    "SendToken": ("vaughan-tui/src/jobs/send.rs", "send_token"),
    "SendTokenWithFee": ("vaughan-tui/src/jobs/send.rs", "send_token_with_fee"),
    "SendStealth": ("vaughan-tui/src/jobs/send.rs", "send_stealth"),
    "SendEvm": ("vaughan-tui/src/jobs/send.rs", "send_evm"),
    "SendEvmWithFee": ("vaughan-tui/src/jobs/send.rs", "send_evm_with_fee"),
    "AssistBurnVerify": ("vaughan-tui/src/jobs/send.rs", "assist_burn_verify"),
    "EstimateEvmFee": ("vaughan-tui/src/jobs/send.rs", "estimate_evm_fee"),
    "DeployToken": ("vaughan-tui/src/jobs/send.rs", "deploy_token"),
    "DexSwapEstimateAfterApprove": ("vaughan-tui/src/jobs/dex.rs", "dex_swap_estimate_after_approve"),
    "DexAllowanceCheck": ("vaughan-tui/src/jobs/dex.rs", "dex_allowance_check"),
    "AggQuote": ("vaughan-tui/src/jobs/dex.rs", "agg_quote"),
    "AggCompareQuote": ("vaughan-tui/src/jobs/dex.rs", "agg_compare_quote"),
    "DexQuote": ("vaughan-tui/src/jobs/dex.rs", "dex_quote"),
    "BridgeQuote": ("vaughan-tui/src/jobs/dex.rs", "bridge_quote"),
    "McpQueuedApprove": ("vaughan-tui/src/jobs/mcp.rs", "mcp_queued_approve"),
}
# Free helpers that moved wholesale. `DcaSlice`'s arm was ALREADY a one-line
# delegation to a free fn in the original, so the meaningful comparison is
# fn-to-fn, not arm-to-fn.
MOVED_FNS = {
    "execute_sentient_transfer_job": "vaughan-tui/src/jobs/send.rs",
    "execute_sentient_evm_job": "vaughan-tui/src/jobs/send.rs",
    "parse_agg_quote_request": "vaughan-tui/src/jobs/dex.rs",
    "execute_dca_slice_job": "vaughan-tui/src/jobs/dca.rs",
}


def sh(*a):
    return subprocess.check_output(list(a), text=True)


def brace_block(lines, start, first_line_text=None):
    """End index (inclusive) of the braced block opening at/after `start`.

    `first_line_text` overrides what is scanned on the `start` line, which
    matters for a match arm whose pattern closes on the same line the body
    opens (`} => {` nets to depth 0 and would otherwise terminate instantly).
    """
    depth, started, j = 0, False, start
    while j < len(lines):
        seg = first_line_text if (j == start and first_line_text is not None) else lines[j]
        depth += seg.count("{") - seg.count("}")
        if "{" in seg:
            started = True
        if started and depth <= 0:
            return j
        j += 1
    raise AssertionError(f"unterminated block at {start}")


def old_arm_bodies(text):
    """variant -> body lines, for each `UiJob::V ... => {` arm in spawn_job."""
    lines = text.splitlines()
    sj = next(i for i, l in enumerate(lines) if re.match(r"\s*fn spawn_job\(", l))
    sj_end = brace_block(lines, sj)
    arms, i = {}, sj
    while i <= sj_end:
        m = re.match(r"\s*UiJob::(\w+)\s*(\{|=>)", lines[i])
        if not m:
            i += 1
            continue
        v = m.group(1)
        # find the `=>` that ends the pattern
        j = i
        while "=>" not in lines[j]:
            j += 1
        rhs = lines[j].split("=>", 1)[1]
        after = rhs.strip()
        if after.startswith("{"):
            # Scan only the post-`=>` text on this line so a pattern's closing
            # `}` does not cancel the body's opening `{`.
            end = brace_block(lines, j, first_line_text=rhs)
            inline = after[1:].strip()
            body = ([inline] if inline else []) + lines[j + 1:end]
        else:
            # Expression arm: run to the comma that closes it at depth 0.
            depth, k = 0, j
            while k < len(lines):
                seg = rhs if k == j else lines[k]
                depth += (
                    seg.count("(") + seg.count("{") + seg.count("[")
                    - seg.count(")") - seg.count("}") - seg.count("]")
                )
                if depth <= 0 and lines[k].rstrip().endswith(","):
                    break
                k += 1
            end = k
            body = [after] + lines[j + 1:end + 1]
        arms[v] = body
        i = end + 1
    return arms


def fn_body(text, name):
    lines = text.splitlines()
    for i, l in enumerate(lines):
        if re.match(rf"\s*(?:pub(?:\([^)]*\))?\s+)?fn {name}\(", l):
            # skip to the line containing the opening brace of the body
            j = i
            while "{" not in lines[j]:
                j += 1
            end = brace_block(lines, j)
            return lines[j + 1:end]
    raise AssertionError(f"fn {name} not found")


SUB = [
    (re.compile(r"&wallet\b"), "wallet"),
    (re.compile(r"&handle\b"), "handle"),
    (re.compile(r"\bvaughan_core::core::"), ""),
    (re.compile(r"\bvaughan_core::chains::"), ""),
    (re.compile(r"\bcrate::provider::"), ""),
    (re.compile(r"\balloy::primitives::"), ""),
    (re.compile(r"\bproposal::"), ""),
    (re.compile(r"\s+"), " "),
]


def norm(body):
    """Collapse a body to a comparable token stream.

    Drops module-level noise a split may legitimately move: comments and
    `use` items (including multi-line ones, which get hoisted to the new
    module's top).
    """
    joined = []
    in_use = False
    for raw in body:
        s = raw.strip()
        if in_use:
            if s.endswith(";"):
                in_use = False
            continue
        if not s or s.startswith("//"):
            continue
        if s.startswith("use "):
            if not s.endswith(";"):
                in_use = True
            continue
        joined.append(s)
    text = " ".join(joined)
    for pat, rep in SUB:
        text = pat.sub(rep, text)
    text = text.replace("( ", "(").replace(" )", ")").replace(" ,", ",")
    text = re.sub(r",(\s*[)\]}])", r"\1", text)
    return text.strip().rstrip(",").strip()


def norm_braceless(text):
    """Tier-2: additionally blind to brace placement and all whitespace.

    `|_| { expr }` and `|_| expr` are the same program; rustfmt will keep
    whichever the input had, so a pure-restyle edit shows up in tier 1. This
    tier proves nothing but brace/whitespace placement moved — every
    identifier, literal, operator and call must still match exactly.
    """
    return re.sub(r"[{}\s]", "", text)


def norm_punctless(text):
    """Tier-3: also blind to commas.

    A block match arm (`=> { Some(x?) }`) rewritten as an expression arm
    (`=> Some(x?),`) swaps a brace pair for a trailing comma. Only `dex_quote`
    needed this. Identifiers, literals, operators, calls and argument ORDER
    must still match exactly, so a transposed argument would still fail.
    """
    return re.sub(r"[{}\s,]", "", text)


old_app = sh("git", "show", f"{BASE}:{OLD_APP}")
old_arms = old_arm_bodies(old_app)
new_cache = {}


def new_text(path):
    if path not in new_cache:
        with open(path, encoding="utf-8") as f:
            new_cache[path] = f.read()
    return new_cache[path]


pairs = []
for variant, (path, fn) in sorted(MOVED.items()):
    if variant not in old_arms:
        pairs.append((variant, f"{path}::{fn}", None, None))
        continue
    pairs.append((variant, f"{path}::{fn}",
                  norm(old_arms[variant]), norm(fn_body(new_text(path), fn))))
for fn, path in sorted(MOVED_FNS.items()):
    pairs.append((fn, f"{path}::{fn}",
                  norm(fn_body(old_app, fn)), norm(fn_body(new_text(path), fn))))

exact, restyled, repunct, fails = [], [], [], []
for name, where, a, b in pairs:
    if a is None:
        fails.append((name, where, "", "", "arm not found in original spawn_job"))
    elif a == b:
        exact.append((name, where))
    elif norm_braceless(a) == norm_braceless(b):
        restyled.append((name, where))
    elif norm_punctless(a) == norm_punctless(b):
        repunct.append((name, where))
    else:
        fails.append((name, where, a, b, "token stream differs"))

total = len(pairs)
print(f"=== tier 1 — token-identical: {len(exact)}/{total} ===")
for name, where in exact:
    print(f"  ok    {name} -> {where}")

if restyled:
    print(f"\n=== tier 2 — identical modulo brace/whitespace style: {len(restyled)}/{total} ===")
    print("    (every identifier, literal, operator and call matches exactly;")
    print("     only closure-brace placement moved — e.g. `|_| {{ e }}` -> `|_| e`)")
    for name, where in restyled:
        print(f"  ok*   {name} -> {where}")

if repunct:
    print(f"\n=== tier 3 — identical modulo brace/whitespace/comma: {len(repunct)}/{total} ===")
    print("    (block match arm rewritten as an expression arm: `=> {{ Some(x?) }}`")
    print("     -> `=> Some(x?),`. Argument order still compared exactly.)")
    for name, where in repunct:
        print(f"  ok**  {name} -> {where}")

if fails:
    print(f"\n=== FAIL — real body differences ({len(fails)}) ===")
    for name, where, a, b, why in fails:
        print(f"\n--- {name} -> {where}  [{why}]")
        for d in difflib.unified_diff(
            a.split(" "), b.split(" "), fromfile="old", tofile="new", lineterm="", n=4
        ):
            print(f"    {d}")
    sys.exit(1)

print(f"\nPASS: all {total} moved bodies preserve the original token stream "
      f"({len(exact)} exact, {len(restyled)} brace-restyle, {len(repunct)} arm-restyle).")
