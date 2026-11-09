#!/usr/bin/env bash
# Auto-answer Antigravity (agy) permission dialogs in a herdr pane.
#
#   scripts/approve-herdr.sh <pane-id> [max-iterations]
#
# Why a loop: agy blocks on every command until answered, so an unattended agent
# stalls at the first prompt. The user should never be the one clicking these.
#
# Rules that make it safe:
#   - read ONLY the bottom of the pane; stale prompts linger above and would route
#     the key to the next, different dialog
#   - match the two-option file-edit dialog FIRST (option 1); a "3" there is ignored
#     and the follow-up Enter does nothing, which stalls the agent silently
#   - never send anything when no dialog is up: a stray digit lands in the agent's
#     composer and gets submitted as a message
#
# Only run this against an agent whose UI does this — doing it to freebuff presses
# digits into its composer.

set -uo pipefail

pane=${1:-}
iters=${2:-0}
[ -n "$pane" ] || { echo "usage: $0 <pane-id> [max-iterations]" >&2; exit 2; }

n=0
while :; do
    n=$((n + 1))
    [ "$iters" -gt 0 ] && [ "$n" -gt "$iters" ] && break

    bottom=$(herdr pane read "$pane" --lines 14 2>/dev/null | grep -v '^[[:space:]]*$')
    [ -n "$bottom" ] || { sleep 2; continue; }

    if printf '%s\n' "$bottom" | grep -qE 'Accept this file edit|Allow creation of this file|Create file'; then
        herdr pane run "$pane" "1" >/dev/null 2>&1
        echo "[$(date +%H:%M:%S)] edit -> 1"
    elif printf '%s\n' "$bottom" | grep -qE 'Requesting permission for|Run this command\?'; then
        herdr pane run "$pane" "3" >/dev/null 2>&1
        echo "[$(date +%H:%M:%S)] command -> 3"
    fi

    sleep 1
done
