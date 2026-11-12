#!/usr/bin/env bash
# Live mission-control view of the coding agents working in this repo.
#
# Read-only: it mirrors each agent's tmux pane, so watching it cannot disturb the
# agent's composer, draft or approvals. Close the pane to stop watching; the
# agents keep running (tmux sessions are independent).
#
#   bash scripts/agent-watch.sh            # all known agent sessions
#   bash scripts/agent-watch.sh agy        # one session
#
# Run it inside a herdr tab (`herdr tab create --label agents` then
# `herdr pane run <pane> "bash scripts/agent-watch.sh"`) or any terminal.

set -uo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.." || exit 1

REFRESH=${REFRESH:-5}
want=("$@")
[ "${#want[@]}" -eq 0 ] && want=(agy fb)

if [ -t 1 ]; then clear; fi

while :; do
    if [ -t 1 ]; then printf '\033[H\033[2J'; fi
    printf 'Vaughan-CLI · agent watch · %s\n' "$(date +%H:%M:%S)"
    printf 'branch %s · %s uncommitted · HEAD %s\n' \
        "$(git branch --show-current 2>/dev/null)" \
        "$(git status --porcelain 2>/dev/null | wc -l)" \
        "$(git log --oneline -1 2>/dev/null | cut -d' ' -f1)"
    printf '%s\n' '--------------------------------------------------------------------------'

    for s in "${want[@]}"; do
        if tmux has-session -t "$s" 2>/dev/null; then
            # Only markers that exist exclusively during a live turn — generic words
            # like "working" also appear in the agents' sponsorship/ad panels.
            state=$(tmux capture-pane -p -t "$s" 2>/dev/null | grep -cE 'esc to cancel|Thought for [0-9]|Running \(|Requesting permission for' || true)
            [ "$state" -gt 0 ] && state="busy" || state="idle"
            printf '\n=== tmux:%s (%s) ===\n' "$s" "$state"
            # Bottom of the pane: that is where a live turn, an approval dialog
            # or the composer sits. Full-screen scrollback would show stale prompts.
            tmux capture-pane -p -t "$s" -S -14 2>/dev/null | sed -e 's/[[:space:]]*$//' | tail -12
        else
            printf '\n=== tmux:%s (not running) ===\n' "$s"
        fi
    done

    printf '\n%s\n' '--------------------------------------------------------------------------'
    printf 'approvals: hermes handles them · attach to drive manually: tmux attach -t agy\n'
    printf 'refreshing every %ss · close the pane to stop watching\n' "$REFRESH"
    sleep "$REFRESH"
done
