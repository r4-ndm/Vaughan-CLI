#!/usr/bin/env bash
# Vaughan-CLI mechanical audit gate.
#
# This script only covers what a machine can decide. It is the *first* half of an
# audit; the judgement half (money paths, custody invariants, claim-vs-reality) is
# docs/audit-playbook.md. A green run here is not an audit pass.
#
# Usage:
#   scripts/audit.sh                 # fmt, clippy (default + railgun), advisories,
#                                    # hygiene, secrets, default-build leak checks
#   scripts/audit.sh --with-tests    # adds `cargo test --workspace` (slow, anvil v1.2.1)
#   scripts/audit.sh --binary        # adds checks against ~/.cargo/bin/vaughan*
#   scripts/audit.sh --skip-railgun  # default-features clippy only; CI's railgun job
#                                    # owns the RAILGUN compile
#   scripts/audit.sh --no-audit      # skip cargo audit (advisory debt NOT checked)
#   scripts/audit.sh --history       # ALSO scan every reachable commit for secrets,
#                                    # not just the working tree (slow: walks all of
#                                    # `git rev-list --all`, O(commits x tree)); the
#                                    # default run stays working-tree-only
#
# `-history` is accepted as the same flag. Exit code 0 only when every selected
# check passes.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

RUN_TESTS=0
CHECK_BINARY=0
SKIP_RAILGUN=0
SKIP_AUDIT=0
HISTORY_SCAN=0
for arg in "$@"; do
    case "$arg" in
        --with-tests) RUN_TESTS=1 ;;
        --binary) CHECK_BINARY=1 ;;
        --skip-railgun) SKIP_RAILGUN=1 ;;
        --no-audit) SKIP_AUDIT=1 ;;
        -history|--history) HISTORY_SCAN=1 ;;
        -h|--help) sed -n '2,/^$/p' "${BASH_SOURCE[0]}"; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

FAILED=0
SKIPPED=0

pass() { printf '  ok    %s\n' "$1"; }
fail() { printf '  FAIL  %s\n' "$1"; FAILED=$((FAILED + 1)); }
note() { printf '  ..    %s\n' "$1"; }
section() { printf '\n== %s\n' "$1"; }

run_check() { # name, command...
    local name="$1"; shift
    local out
    if out=$("$@" 2>&1); then
        pass "$name"
    else
        fail "$name"
        printf '%s\n' "$out" | tail -25 | sed 's/^/        /'
    fi
}

# ---------------------------------------------------------------- build health
section "build health"

# `--locked` everywhere: the gate must judge the committed Cargo.lock, not a
# silently re-resolved graph.
run_check "cargo fmt --all -- --check" cargo fmt --all -- --check
run_check "clippy (default features, -D warnings)" \
    cargo clippy --workspace --all-targets --locked --quiet -- -D warnings
if [ "$SKIP_RAILGUN" -eq 1 ]; then
    note "railgun clippy skipped (--skip-railgun; the dedicated CI job covers it)"
    SKIPPED=$((SKIPPED + 1))
else
    run_check "clippy (railgun feature, -D warnings)" \
        cargo clippy -p vaughan-railgun -p vaughan-tui -p vaughan-cli \
        --features vaughan-railgun/enabled,vaughan-cli/railgun --all-targets --locked --quiet -- -D warnings
fi

if [ "$RUN_TESTS" -eq 1 ]; then
    if ! command -v anvil >/dev/null 2>&1; then
        fail "cargo test --workspace (anvil not on PATH — install Foundry v1.2.1)"
    else
        anvil_version=$(anvil --version 2>/dev/null | head -1)
        case "$anvil_version" in
            *1\.2\.1*)
                run_check "cargo test --workspace --locked" cargo test --workspace --locked --quiet
                ;;
            *)
                # Newer anvil replies to eth_sendTransaction before mining, which
                # races the send-then-read suites (docs/audit-playbook.md).
                fail "cargo test (anvil $anvil_version is not the pinned v1.2.1 — install Foundry v1.2.1)"
                ;;
        esac
    fi
else
    note "cargo test --workspace skipped (--with-tests)"
    SKIPPED=$((SKIPPED + 1))
fi

# The default workspace must never pull the RAILGUN upstream stack: it is opt-in
# and it is what keeps `cargo build` fast (see the CI railgun job).
#
# NB: no `cargo tree | grep -q` — `grep -q` exits on the first match, SIGPIPEs
# `cargo tree`, and `pipefail` then reports a *hit* as a failure (and any cargo
# failure as a clean pass). Capture, then filter.
tree_out=$(cargo tree --workspace --locked -e normal 2>/dev/null)
tree_rc=$?
if [ "$tree_rc" -ne 0 ]; then
    fail "cargo tree --workspace failed (rc=$tree_rc) — default feature graph unverified"
elif printf '%s\n' "$tree_out" | grep -E 'railgun-wallet|broadcaster-core|wasmer' >/dev/null; then
    fail "default feature set leaks railgun-rust (opt-in only)"
else
    pass "default feature set excludes railgun-rust"
fi

# The inverse guarantee: a *default* build must not contain the private-panel
# literal at all. `cargo tree` proves the crate is absent, not that no literal
# shipped (vaughan-tui/src/views/receive.rs folds it behind cfg!(feature = ...)).
default_bin="${CARGO_TARGET_DIR:-target}/debug/vaughan"
if ! build_out=$(cargo build -p vaughan-cli --locked --quiet 2>&1); then
    fail "cargo build -p vaughan-cli failed — default build unverified"
    printf '%s\n' "$build_out" | tail -15 | sed 's/^/        /'
elif [ ! -x "$default_bin" ]; then
    fail "no default binary at $default_bin — cannot check for private-panel leakage"
else
    private_count=$(strings "$default_bin" | grep -c 'private (RAILGUN)' || true)
    if [ "$private_count" -eq 0 ]; then
        pass "default build omits the RAILGUN private panel"
    else
        fail "default build contains 'private (RAILGUN)' ($private_count hit(s)) — feature gate is broken"
    fi
fi

# ---------------------------------------------------------------- advisories
section "dependency advisories"

# cargo-audit is the one extra tool this gate needs. It must fail on advisories,
# and it must fail (not pass) when the advisory DB cannot be reached: an
# unreachable DB means "unchecked", never "clean".
if [ "$SKIP_AUDIT" -eq 1 ]; then
    note "cargo audit skipped (--no-audit) — advisory status UNCHECKED"
    SKIPPED=$((SKIPPED + 1))
elif ! command -v cargo-audit >/dev/null 2>&1; then
    fail "cargo-audit is not installed (cargo install cargo-audit) — advisories unchecked"
else
    audit_out=$(cargo audit 2>&1)
    audit_rc=$?
    if [ "$audit_rc" -eq 0 ]; then
        pass "cargo audit (no advisories)"
    elif printf '%s\n' "$audit_out" | grep -q 'vulnerabilities found'; then
        fail "cargo audit: advisories found"
        printf '%s\n' "$audit_out" | tail -30 | sed 's/^/        /'
    else
        fail "cargo audit could not complete (rc=$audit_rc) — advisory DB unreachable or tool error; advisory status UNKNOWN"
        printf '%s\n' "$audit_out" | tail -15 | sed 's/^/        /'
    fi
fi

# ---------------------------------------------------------------- tracked files
section "tracked file hygiene"

# `git ls-files` failing reads as "nothing tracked", i.e. a clean pass. Refuse.
if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    fail "not a git work tree — tracked-file hygiene cannot run"
fi

tracked() { git ls-files "$@" 2>/dev/null; }
hit() { # hit <regex> [pathspec...] — up to 5 tracked paths matching the regex
    local pat="$1"; shift
    tracked "$@" | grep -E "$pat" | head -5
}

# Filter first, truncate second: `head -5` on the raw file list made every check
# below inspect only the first five tracked paths.
if [ -n "$(hit 'node_modules/')" ]; then
    fail "node_modules is tracked"
else
    pass "no node_modules tracked"
fi

state_hits=$(hit '(^|/)(target|\.env|\.env\.[a-z]+|wallet\.json|agent\.key\.json|.*\.keystore|vaughan-backup-.*)$')
if [ -n "$state_hits" ]; then
    fail "build artifacts or wallet state tracked"
    printf '%s\n' "$state_hits" | sed 's/^/        /'
else
    pass "no build artifacts or wallet state tracked"
fi

if [ -n "$(tracked private/ | head -1)" ]; then
    fail "private/ (local-only planning) is tracked"
else
    pass "private/ untracked"
fi

# strategies are gitignored by design; only the public example may be committed
unexpected_strats=$(tracked 'vaughan-hl/strategies/*.toml' \
    | grep -vE 'example_.*\.toml$' | head -5)
if [ -n "$unexpected_strats" ]; then
    fail "non-example strategy TOMLs tracked (proprietary IP)"
    printf '%s\n' "$unexpected_strats" | sed 's/^/        /'
else
    pass "only example strategies tracked"
fi

# ---------------------------------------------------------------- secrets
section "secrets scan (tracked text files)"

# Real key material only. Two tiers so the output stays readable:
#   1. unmistakable formats (API key prefixes, PEM blocks) — always reported
#   2. bare 64-hex — reported only when the same line names a secret, minus the
#      well-known anvil dev keys, CREATE2 salts, tx hashes and event topics
#
# Any hit fails the gate: a committed key is a P0, not a note.
scan_paths=('*.rs' '*.toml' '*.json' '*.md' '*.sh' '*.yml' '*.yaml' '*.txt'
    '*.mjs' '*.py' '*.service' '.env*')
scan_skip=(':!test-vectors/**')

# stderr is left visible on purpose: a git failure must not look like "no hits".
git_grep() { # git_grep <pattern> [pathspec...]
    local pat="$1"; shift
    git grep -nIE "$pat" -- "$@"
}

api_pat='sk-[A-Za-z0-9]{20,}|sk-or-v1-[A-Za-z0-9]{20,}|AIza[0-9A-Za-z_-]{30,}|-----BEGIN [A-Z ]*PRIVATE KEY-----'
api_hits=$(git_grep "$api_pat" "${scan_paths[@]}" "${scan_skip[@]}")
api_rc=$?

hex_pat='[0-9a-fA-F]{64}'

# Tier-2 filters, shared by the working-tree and history tiers so both judge the
# same evidence by the same rules.
hex_context='(priv|secret|mnemonic|signer|api[_-]?key)'
hex_allow='anvil|ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80|59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d|b256!|SALT=|/tx/|topic|selector|0xaaaa|deadbeef|000000000000000000000000'
hex_tier() { # hex_tier <raw tier-2 output> — filtered, truncated hits
    printf '%s\n' "$1" \
        | grep -iE "$hex_context" \
        | grep -viE "$hex_allow" \
        | head -10
}

hex_raw=$(git_grep "$hex_pat" "${scan_paths[@]}" ':!*package-lock.json' ':!*Cargo.lock' "${scan_skip[@]}")
hex_rc=$?
hex_hits=$(hex_tier "$hex_raw")

scan_broken=0
[ "$api_rc" -gt 1 ] && scan_broken=1
[ "$hex_rc" -gt 1 ] && scan_broken=1

if [ "$scan_broken" -eq 1 ]; then
    fail "secrets scan did not run (git grep rc=$api_rc/$hex_rc) — unchecked, not clean"
elif [ -n "$api_hits" ] || [ -n "$hex_hits" ]; then
    fail "secrets scan: hit(s) require review — a real key here is a P0"
    [ -n "$api_hits" ] && printf '%s\n' "$api_hits" | sed 's/^/        /'
    [ -n "$hex_hits" ] && printf '%s\n' "$hex_hits" | sed 's/^/        /'
else
    pass "no obvious live key material"
fi

# -------------------------------------------------------- secrets (history)
section "secrets scan (git history, all refs)"

# A key that was committed and later deleted is still public: the blob sits in
# every clone and every `git rev-list --all` until history is rewritten. Same two
# tiers and the same pathspecs as above, but judged over every reachable commit
# instead of the working tree. Opt-in (--history) because it walks the whole graph.
if [ "$HISTORY_SCAN" -eq 0 ]; then
    note "git history skipped (--history) — committed-then-deleted keys unseen"
    SKIPPED=$((SKIPPED + 1))
elif ! hist_revs_raw=$(git rev-list --all 2>&1); then
    fail "git rev-list --all failed — history scan UNCHECKED"
    printf '%s\n' "$hist_revs_raw" | tail -5 | sed 's/^/        /'
elif [ -z "$hist_revs_raw" ]; then
    # Empty rev list would make `git grep` read the working tree and call it history.
    fail "git rev-list --all returned no commits — history scan UNCHECKED"
else
    mapfile -t hist_revs <<< "$hist_revs_raw"

    hist_grep() { # hist_grep <pattern> [pathspec...] — hits are `rev:path:line:text`
        local pat="$1"; shift
        git grep -nIE "$pat" "${hist_revs[@]}" -- "$@"
    }

    hist_api=$(hist_grep "$api_pat" "${scan_paths[@]}" "${scan_skip[@]}")
    hist_api_rc=$?
    hist_hex_raw=$(hist_grep "$hex_pat" "${scan_paths[@]}" ':!*package-lock.json' ':!*Cargo.lock' "${scan_skip[@]}")
    hist_hex_rc=$?
    hist_hex_hits=$(hex_tier "$hist_hex_raw")

    hist_broken=0
    [ "$hist_api_rc" -gt 1 ] && hist_broken=1
    [ "$hist_hex_rc" -gt 1 ] && hist_broken=1

    if [ "$hist_broken" -eq 1 ]; then
        fail "history secrets scan did not run (git grep rc=$hist_api_rc/$hist_hex_rc) — UNCHECKED, not clean"
    elif [ -n "$hist_api" ] || [ -n "$hist_hex_hits" ]; then
        fail "history secrets scan: key material in committed history (${#hist_revs[@]} commits) — rotate first, then rewrite history"
        [ -n "$hist_api" ] && printf '%s\n' "$hist_api" | head -5 | sed 's/^/        /'
        [ -n "$hist_hex_hits" ] && printf '%s\n' "$hist_hex_hits" | sed 's/^/        /'
    else
        pass "no obvious live key material in history (${#hist_revs[@]} commits)"
    fi
fi

# ---------------------------------------------------------------- installed binary
section "installed binary"

if [ "$CHECK_BINARY" -eq 1 ]; then
    BIN="${HOME}/.cargo/bin/vaughan"
    if [ ! -x "$BIN" ]; then
        note "no installed binary at $BIN (CI has none) — build with scripts/install-vaughan.sh to check it"
        SKIPPED=$((SKIPPED + 1))
    else
        # NB: no `grep -q` here — it exits on first match, SIGPIPEs `strings`, and
        # `pipefail` would turn a hit into a failure.
        if strings "$BIN" | grep 'private (RAILGUN)' >/dev/null; then
            pass "installed binary has the RAILGUN private panel"
        else
            fail "installed binary lacks the railgun feature (build with --features railgun)"
        fi
        if [ -L "${HOME}/.cargo/bin/vaughan-p" ] || [ -x "${HOME}/.cargo/bin/vaughan-p" ]; then
            pass "vaughan-p alias present"
        else
            note "vaughan-p alias missing (scripts/install-vaughan.sh creates it)"
        fi
    fi
else
    note "installed binary checks skipped (--binary)"
    SKIPPED=$((SKIPPED + 1))
fi

# ---------------------------------------------------------------- summary
section "summary"
printf '  failed: %s   skipped: %s\n' "$FAILED" "$SKIPPED"
cat <<'EOF'

  This gate decides mechanics only. Still to do by hand (docs/audit-playbook.md):
    - read every diff hunk on a money path, not just the green checkmarks
    - confirm no fabricated tx hash / guessed constant / unbound recipient
    - confirm the claim in docs matches what the binary actually does
    - re-run with --history before a release: the working-tree scan cannot see a
      key that was committed and deleted
EOF

[ "$FAILED" -eq 0 ] || exit 1
exit 0
