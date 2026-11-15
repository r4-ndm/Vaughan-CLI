#!/usr/bin/env python3
"""Size-budget ratchet: keep modules small enough for the next agent to work on.

Existing debt is frozen in scripts/size-budget.txt; new debt is refused.

Rules (production code only, i.e. lines before the first column-0 `#[cfg(test)]`;
tracked `*.rs` files under a `src/` directory):
  1. A file over MAX_FILE_LINES must be listed in the baseline.
  2. A listed file may not grow past its recorded line count.
  3. A function of MAX_FN_LINES or more must be listed in the baseline.
  4. A listed function may not grow past its recorded length.
A file/function that SHRANK is reported as a note: run `--update` to lower the
number so the ratchet tightens. Baseline lines:  `file <lines> <path>`  and
`fn <lines> <path>::<name>#<n>` (n = nth fn of that name in the file).

Usage: scripts/size_budget.py [--update] [--root DIR]
Exit 0 = within budget, 1 = violation, 2 = tool error (never a silent pass).
"""
import argparse
import re
import subprocess
import sys
from pathlib import Path

MAX_FILE_LINES = 800
MAX_FN_LINES = 150
BASELINE = "scripts/size-budget.txt"
FN_SIG = re.compile(
    r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)"
)


def tracked_rs(root: Path):
    out = subprocess.run(
        ["git", "ls-files", "*.rs"], cwd=root, capture_output=True, text=True
    )
    if out.returncode != 0 or not out.stdout.strip():
        # `git ls-files` failing reads as "nothing tracked" == a clean pass. Refuse.
        print("size-budget: git ls-files failed or returned nothing", file=sys.stderr)
        sys.exit(2)
    return [
        p for p in out.stdout.split("\n") if p and "/src/" in p and "/tests/" not in p
    ]


def prod_lines(text: str):
    lines = text.split("\n")
    for i, l in enumerate(lines):
        if l.startswith("#[cfg(test)]"):
            return lines[:i]
    return lines


def fn_lengths(lines):
    """Yield (name#n, length) for every fn whose body can be brace-matched."""
    seen = {}
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
            seen[m.group(1)] = seen.get(m.group(1), 0) + 1
            yield f"{m.group(1)}#{seen[m.group(1)]}", j - i + 1
            i = j + 1
        else:
            i += 1


def measure(root: Path):
    files, fns = {}, {}
    for rel in tracked_rs(root):
        try:
            text = (root / rel).read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as e:
            print(f"size-budget: cannot read {rel}: {e}", file=sys.stderr)
            sys.exit(2)
        lines = prod_lines(text)
        if len(lines) > MAX_FILE_LINES:
            files[rel] = len(lines)
        for key, n in fn_lengths(lines):
            if n >= MAX_FN_LINES:
                fns[f"{rel}::{key}"] = n
    return files, fns


def read_baseline(root: Path):
    path = root / BASELINE
    files, fns = {}, {}
    if not path.exists():
        print(f"size-budget: baseline {BASELINE} missing (run --update)", file=sys.stderr)
        sys.exit(2)
    for raw in path.read_text().split("\n"):
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        parts = s.split(None, 2)
        if len(parts) != 3 or parts[0] not in ("file", "fn") or not parts[1].isdigit():
            print(f"size-budget: malformed baseline line: {raw!r}", file=sys.stderr)
            sys.exit(2)
        (files if parts[0] == "file" else fns)[parts[2]] = int(parts[1])
    return files, fns


def write_baseline(root: Path, files, fns):
    head = (
        "# Size-budget baseline (see scripts/size_budget.py). Existing debt only:\n"
        "# numbers may go DOWN, never up. A new entry here needs a reason in the PR.\n"
        f"# file = production lines > {MAX_FILE_LINES}; fn = length >= {MAX_FN_LINES}.\n"
    )
    body = "".join(f"file {n} {p}\n" for p, n in sorted(files.items()))
    body += "".join(f"fn {n} {p}\n" for p, n in sorted(fns.items()))
    (root / BASELINE).write_text(head + body)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--update", action="store_true")
    ap.add_argument("--root", default=".")
    a = ap.parse_args()
    root = Path(a.root).resolve()
    files, fns = measure(root)
    if a.update:
        write_baseline(root, files, fns)
        print(f"size-budget: baseline written ({len(files)} files, {len(fns)} fns)")
        return 0
    bfiles, bfns = read_baseline(root)
    bad, notes = [], []
    for kind, cur, base, limit in (
        ("file", files, bfiles, MAX_FILE_LINES),
        ("fn", fns, bfns, MAX_FN_LINES),
    ):
        for k, n in sorted(cur.items()):
            if k not in base:
                bad.append(f"NEW over-budget {kind}: {k} = {n} (limit {limit}); split it")
            elif n > base[k]:
                bad.append(f"GREW {kind}: {k} {base[k]} -> {n}; new code goes in a new module")
            elif n < base[k]:
                notes.append(f"shrank {kind}: {k} {base[k]} -> {n} (run --update to lock it in)")
        for k in sorted(set(base) - set(cur)):
            notes.append(f"now within budget {kind}: {k} (run --update to drop it)")
    for n in notes:
        print(f"  ..    {n}")
    for b in bad:
        print(f"  FAIL  {b}")
    print(
        f"size-budget: {len(files)} files / {len(fns)} fns over budget, "
        f"{len(bad)} violation(s)"
    )
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
