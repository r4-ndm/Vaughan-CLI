#!/usr/bin/env python3
"""Move-only proof for the vaughan-hl/src/tui.rs -> tui/{mod,render}.rs split.

Same technique as private/split-proof.sh: normalise away the things a split is
*allowed* to change (module docs, `use`/`mod` lines, the extra `impl HlTuiApp {`
wrapper, `pub(super)` visibility bumps) and assert the remaining line multiset is
conserved 1-to-1. A non-empty diff means a body changed, which a move-only commit
must not do.

Usage: scripts/split-proof-hl-tui.py [BASE_REV]
"""
import collections
import difflib
import subprocess
import sys

BASE = sys.argv[1] if len(sys.argv) > 1 else "HEAD"
OLD_PATH = "vaughan-hl/src/tui.rs"
NEW_PATHS = ["vaughan-hl/src/tui/mod.rs", "vaughan-hl/src/tui/render.rs"]


def read_git(rev: str, path: str) -> str:
    return subprocess.check_output(["git", "show", f"{rev}:{path}"], text=True)


def read_worktree(path: str) -> str:
    with open(path, encoding="utf-8") as f:
        return f.read()


def normalise(text: str, label: str):
    """Yield (normalised_line, provenance) skipping split-permitted noise."""
    out = []
    in_multiline_use = False
    impl_depths = []
    depth = 0
    for lineno, raw in enumerate(text.splitlines(), 1):
        s = raw.strip()
        if not s or s.startswith("//!"):
            continue
        if in_multiline_use:
            if s.endswith(";"):
                in_multiline_use = False
            continue
        if depth == 0 and (
            s.startswith("use ") or s.startswith("pub use ") or s.startswith("mod ")
            or s.startswith("pub mod ") or s.startswith("pub(crate) mod ")
        ):
            if not s.endswith(";"):
                in_multiline_use = True
            continue
        # The split adds a second `impl HlTuiApp {` wrapper in render.rs; the
        # original had exactly one. Elide the wrapper (and its closing brace)
        # so method bodies compare directly.
        if depth == 0 and s == "impl HlTuiApp {":
            impl_depths.append(depth)
            depth += s.count("{") - s.count("}")
            continue
        next_depth = depth + s.count("{") - s.count("}")
        if s == "}" and impl_depths and next_depth == impl_depths[-1]:
            impl_depths.pop()
            depth = next_depth
            continue
        depth = next_depth
        out.append((s.replace("pub(super) ", ""), f"{label}:{lineno}: {raw}"))
    assert not impl_depths, f"unclosed impl HlTuiApp in {label}"
    return out


old_items = normalise(read_git(BASE, OLD_PATH), f"{BASE[:8]}:tui.rs")
new_items = []
for p in NEW_PATHS:
    new_items.extend(normalise(read_worktree(p), p))

old_sorted = sorted(n for n, _ in old_items)
new_sorted = sorted(n for n, _ in new_items)

diff = list(
    difflib.unified_diff(
        old_sorted, new_sorted,
        fromfile=f"old/{OLD_PATH}(normalised)",
        tofile="new/tui/{mod,render}.rs(normalised)",
        lineterm="",
    )
)

if diff:
    print("FAIL: normalised line multiset differs — this is NOT move-only.", file=sys.stderr)
    oc = collections.Counter(n for n, _ in old_items)
    nc = collections.Counter(n for n, _ in new_items)
    only_old, only_new = oc - nc, nc - oc
    if only_old:
        print("\nLines in OLD missing from NEW:", file=sys.stderr)
        for n, loc in old_items:
            if only_old[n] > 0:
                print(f"  - {loc}", file=sys.stderr)
                only_old[n] -= 1
    if only_new:
        print("\nExtra/modified lines in NEW:", file=sys.stderr)
        for n, loc in new_items:
            if only_new[n] > 0:
                print(f"  + {loc}", file=sys.stderr)
                only_new[n] -= 1
    sys.exit(1)

print(f"PASS: {len(old_sorted)} normalised lines conserved 1-to-1 (move-only).")
