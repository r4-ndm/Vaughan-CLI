#!/usr/bin/env python3
"""Wallet-mutex discipline ratchet: keep RPC and signing off the wallet lock.

`vaughan-tui/src/app.rs`'s `spawn_job` states the rule this enforces:

    Never hold the wallet mutex across RPC / signing awaits — that freezes the
    TUI on `[busy]` / `(busy)`.

It was prose, and six jobs violated it without anything noticing. This turns it
into a check. Existing violations are frozen in scripts/lock-budget.txt; a new
one fails the gate. Same shape as scripts/size_budget.py: the baseline may
shrink, never grow.

Two violation shapes, because they read differently and only one is lintable:

  A  a guard is bound (`let w = wallet.lock()...`) and is still in scope at a
     `block_on` call. Clippy cannot see this one — `block_on` is not `.await`.

  B  the lock is taken *inside* an `async` block that then `.await`s. This is
     what `clippy::await_holding_lock` catches, so these carry an explicit
     `#[allow(...)]` and are visible in review.

Scope is deliberately the *wallet* mutex (a `.lock()` whose receiver mentions
`wallet`), not every mutex: the invariant is about wallet contention freezing
the UI, and widening it would drown the signal in unrelated locks.

Usage: scripts/lock_discipline.py [--update] [--root DIR]
Exit 0 = within budget, 1 = violation, 2 = tool error (never a silent pass).
"""
import argparse
import re
import subprocess
import sys
from pathlib import Path

BASELINE = "scripts/lock-budget.txt"
FN_SIG = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)"
)
# `let <name> = <something mentioning wallet>.lock()`
GUARD = re.compile(r"\blet\s+(?:mut\s+)?\w+\s*=\s*[^;]*\bwallet\b[^;]*\.\s*lock\s*\(")
BLOCK_ON = re.compile(r"\bblock_on\s*\(")
ASYNC_BLOCK = re.compile(r"\bblock_on\s*\(\s*async\s*(?:move\s*)?\{")


def tracked_rs(root: Path):
    out = subprocess.run(
        ["git", "ls-files", "*.rs"], cwd=root, capture_output=True, text=True
    )
    if out.returncode != 0 or not out.stdout.strip():
        print("lock-discipline: git ls-files failed or returned nothing", file=sys.stderr)
        sys.exit(2)
    return [
        p for p in out.stdout.split("\n") if p and "/src/" in p and "/tests/" not in p
    ]


def functions(lines):
    """Yield (name, start, end_exclusive) for each brace-matched fn."""
    i = 0
    while i < len(lines):
        m = FN_SIG.match(lines[i])
        if not m:
            i += 1
            continue
        depth, started, j = 0, False, i
        while j < len(lines):
            depth += lines[j].count("{") - lines[j].count("}")
            if "{" in lines[j]:
                started = True
            if started and depth <= 0:
                break
            if not started and lines[j].rstrip().endswith(";"):
                break  # trait method declaration, no body
            j += 1
        if started:
            yield m.group(1), i, j + 1
            i = j + 1
        else:
            i += 1


def shape_a(body):
    """Guard bound, then block_on reached while its enclosing scope is open."""
    depth, guard_depth = 0, None
    for raw in body:
        s = raw.strip()
        before = depth
        depth += s.count("{") - s.count("}")
        if GUARD.search(s):
            guard_depth = before
        elif guard_depth is not None and before < guard_depth:
            guard_depth = None
        if guard_depth is not None and BLOCK_ON.search(s):
            return True
    return False


def shape_b(body):
    """Lock taken inside a block_on(async { .. }) that also awaits."""
    joined = " ".join(l.strip() for l in body)
    for m in ASYNC_BLOCK.finditer(joined):
        seg, depth, i = joined[m.end():], 1, 0
        while i < len(seg) and depth > 0:
            if seg[i] == "{":
                depth += 1
            elif seg[i] == "}":
                depth -= 1
            i += 1
        inner = seg[:i]
        if GUARD.search(inner) and ".await" in inner:
            return True
    return False


def measure(root: Path):
    found = {}
    for rel in tracked_rs(root):
        try:
            text = (root / rel).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as e:
            print(f"lock-discipline: cannot read {rel}: {e}", file=sys.stderr)
            sys.exit(2)
        lines = text.split("\n")
        for name, start, end in functions(lines):
            body = lines[start:end]
            shapes = ("A" if shape_a(body) else "") + ("B" if shape_b(body) else "")
            if shapes:
                found[f"{rel}::{name}"] = shapes
    return found


def read_baseline(root: Path):
    path = root / BASELINE
    if not path.exists():
        print(f"lock-discipline: baseline {BASELINE} missing (run --update)", file=sys.stderr)
        sys.exit(2)
    out = {}
    for raw in path.read_text().split("\n"):
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        parts = s.split(None, 1)
        if len(parts) != 2 or not set(parts[0]) <= {"A", "B"}:
            print(f"lock-discipline: malformed baseline line: {raw!r}", file=sys.stderr)
            sys.exit(2)
        out[parts[1]] = parts[0]
    return out


def write_baseline(root: Path, found):
    head = (
        "# Wallet-mutex discipline baseline (see scripts/lock_discipline.py).\n"
        "# Existing violations only: entries may be REMOVED, never added without a\n"
        "# reason in the PR. `A` = guard held across block_on; `B` = lock taken inside\n"
        "# an async block that awaits (clippy::await_holding_lock).\n"
        "# The rule being frozen here is stated in vaughan-tui/src/app.rs's spawn_job:\n"
        "# never hold the wallet mutex across RPC / signing awaits.\n"
    )
    body = "".join(f"{s} {k}\n" for k, s in sorted(found.items()))
    (root / BASELINE).write_text(head + body)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--root", default=".")
    a = ap.parse_args()
    root = Path(a.root).resolve()
    found = measure(root)
    if a.update:
        write_baseline(root, found)
        print(f"lock-discipline: baseline written ({len(found)} held-lock site(s))")
        return 0
    base = read_baseline(root)
    bad, notes = [], []
    for k, shapes in sorted(found.items()):
        if k not in base:
            bad.append(
                f"NEW held-lock site (shape {shapes}): {k}; snapshot what you need "
                f"and drop the guard before the await"
            )
        elif shapes != base[k]:
            bad.append(f"CHANGED shape: {k} {base[k]} -> {shapes}; re-check it by hand")
    for k in sorted(set(base) - set(found)):
        notes.append(f"fixed: {k} no longer holds the lock (run --update to drop it)")
    for n in notes:
        print(f"  ..    {n}")
    for b in bad:
        print(f"  FAIL  {b}")
    print(f"lock-discipline: {len(found)} held-lock site(s), {len(bad)} violation(s)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
