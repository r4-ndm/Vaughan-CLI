#!/usr/bin/env bash
# Vaughan-CLI mechanical audit gate.
#
# This script only covers what a machine can decide. It is the *first* half of an
# audit; the judgement half (money paths, custody invariants, claim-vs-reality) is
# docs/audit-playbook.md. A green run here is not an audit pass.
#
# Usage:
#   scripts/audit.sh            # fmt, clippy (default + railgun), hygiene, secrets
#   scripts/audit.sh --with-tests   # adds `cargo test --workspace` (slow)
#   scripts/audit.sh --binary      # adds checks against ~/.cargo/bin/vaughan*
#
# Exit code 0 only when every selected check passes.

set -uo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

RUN_TESTS=0
CHECK_BINARY=0
for arg in "$@"; do
    case "$arg" in
        --with-tests) RUN_TESTS=1 ;;
        --binary) CHECK_BINARY=1 ;;
        -h|--help) sed -n '2,16p' "${BASH_SOURCE[0]}"; exit 0 ;;
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

run_check "cargo fmt --all -- --check" cargo fmt --all -- --check
run_check "clippy (default features, -D warnings)" \
    cargo clippy --workspace --all-targets --quiet -- -D warnings
run_check "clippy (railgun feature, -D warnings)" \
    cargo clippy -p vaughan-railgun -p vaughan-tui -p vaughan-cli \
    --features vaughan-railgun/enabled,vaughan-cli/railgun --all-targets --quiet -- -D warnings

if [ "$RUN_TESTS" -eq 1 ]; then
    if ! command -v anvil >/dev/null 2>&1; then
        fail "cargo test --workspace (anvil not on PATH — install Foundry v1.2.1)"
    else
        run_check "cargo test --workspace" cargo test --workspace --quiet
    fi
else
    note "cargo test --workspace skipped (--with-tests)"
    SKIPPED=$((SKIPPED + 1))
fi

# The default workspace must never pull the RAILGUN upstream stack: it is opt-in
# and it is what keeps `cargo build` fast (see the CI railgun job).
if cargo tree --workspace -e normal 2>/dev/null \
    | grep -qE 'railgun-wallet|broadcaster-core|wasmer'; then
    fail "default feature set leaks railgun-rust (opt-in only)"
else
    pass "default feature set excludes railgun-rust"
fi

# ---------------------------------------------------------------- tracked files
section "tracked file hygiene"

tracked_hits() { git ls-files "$@" 2>/dev/null | head -5; }

if [ -n "$(tracked_hits | grep -E 'node_modules/')" ]; then
    fail "node_modules is tracked"
else
    pass "no node_modules tracked"
fi

if [ -n "$(tracked_hits | grep -E '(^|/)(target|\.env|\.env\.[a-z]+|wallet\.json|agent\.key\.json|.*\.keystore|vaughan-backup-.*)$')" ]; then
    fail "build artifacts or wallet state tracked"
    tracked_hits | grep -E '(^|/)(target|\.env|wallet\.json|agent\.key\.json|.*\.keystore)' | head -5 | sed 's/^/        /'
else
    pass "no build artifacts or wallet state tracked"
fi

if [ -n "$(git ls-files private/ 2>/dev/null | head -1)" ]; then
    fail "private/ (local-only planning) is tracked"
else
    pass "private/ untracked"
fi

# strategies are gitignored by design; only the public example may be committed
unexpected_strats=$(git ls-files 'vaughan-hl/strategies/*.toml' 2>/dev/null \
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
api_pat='sk-[A-Za-z0-9]{20,}|sk-or-v1-[A-Za-z0-9]{20,}|AIza[0-9A-Za-z_-]{30,}|-----BEGIN [A-Z ]*PRIVATE KEY-----'
api_hits=$(git grep -nIE "$api_pat" -- '*.rs' '*.toml' '*.json' '*.md' '*.sh' '*.yml' \
        ':!test-vectors/**' 2>/dev/null | head -10)

hex_pat='[0-9a-fA-F]{64}'
hex_hits=$(git grep -nIE "$hex_pat" -- '*.rs' '*.toml' '*.json' '*.md' '*.sh' '*.yml' \
        ':!*package-lock.json' ':!*Cargo.lock' ':!test-vectors/**' 2>/dev/null \
    | grep -iE '(priv|secret|mnemonic|signer|api[_-]?key)' \
    | grep -viE 'anvil|ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80|59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d|b256!|SALT=|/tx/|topic|selector|0xaaaa|deadbeef|000000000000000000000000' \
    | head -10)

if [ -n "$api_hits" ] || [ -n "$hex_hits" ]; then
    note "review each hit — a real key here is a P0:"
    [ -n "$api_hits" ] && printf '%s\n' "$api_hits" | sed 's/^/        /'
    [ -n "$hex_hits" ] && printf '%s\n' "$hex_hits" | sed 's/^/        /'
else
    pass "no obvious live key material"
fi

# ---------------------------------------------------------------- installed binary
section "installed binary"

if [ "$CHECK_BINARY" -eq 1 ]; then
    BIN="${HOME}/.cargo/bin/vaughan"
    if [ ! -x "$BIN" ]; then
        fail "no installed binary at $BIN"
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
EOF

[ "$FAILED" -eq 0 ] || exit 1
exit 0
