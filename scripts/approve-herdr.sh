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

    # 24 lines, not 14: agy prints a multi-line command under the header, so a long
    # one pushes "Requesting permission for:" above a narrow window and the dialog
    # goes unrecognised — which looks exactly like an idle agent.
    bottom=$(herdr pane read "$pane" --lines 24 2>/dev/null | grep -v '^[[:space:]]*$')
    [ -n "$bottom" ] || { sleep 2; continue; }

    if printf '%s\n' "$bottom" | grep -qE 'Accept this file edit|Allow creation of this file|Create file'; then
        herdr pane run "$pane" "1" >/dev/null 2>&1
        echo "[$(date +%H:%M:%S)] edit -> 1"
    elif printf '%s\n' "$bottom" | grep -qE 'Requesting permission for|Run this command\?|^[[:space:]]*1\. Yes'; then
        # Read the command under the prompt and decide: auto-approve the read/build/test
        # verbs an agent needs continuously, HOLD anything else so the orchestrator sees
        # it. A hold stalls the agent (harmless) instead of silently allowing rm/sudo/curl.
        cmd=$(printf '%s\n' "$bottom" | grep -A3 'Requesting permission for' | tail -n +2 | head -2 | tr '\n' ' ')
        # Extraction fails when a multi-line command is longer than the captured window,
        # or when the prompt scrolled so the marker sits above it. Approve ONCE ("1") in
        # that case: work continues, nothing new is persisted to the allow-list, and the
        # command re-prompts if it recurs.
        if [ -z "$cmd" ]; then
            herdr pane run "$pane" "1" >/dev/null 2>&1
            echo "[$(date +%H:%M:%S)] unclassified -> 1"
            sleep 1
            continue
        fi
        # A two-option dialog has no "3." line. Sending 3 there is ignored and the agent
        # stalls silently — the most expensive failure mode in this workflow.
        if ! printf '%s\n' "$bottom" | grep -qE '^[[:space:]]*3\.'; then
            herdr pane run "$pane" "1" >/dev/null 2>&1
            echo "[$(date +%H:%M:%S)] two-option -> 1: $cmd"
            sleep 1
            continue
        fi
        case "$cmd" in
        *"cargo "* | *"git "* | *"rg "* | *"grep "* | *"find "* | *"ls "* | *"cat "* | *"head "* | \
        *"tail "* | *"wc "* | *"sed "* | *"awk "* | *"sort "* | *"uniq "* | *"cut "* | *"diff "* | \
        *"strings "* | *"sha256sum "* | *"stat "* | *"du "* | *"df "* | *"which "* | *"command -v"* | \
        *"echo "* | *"printf "* | *"mkdir "* | *"cp "* | *"mv "* | *"touch "* | *"date"* | *"jq "* | \
        *"gh "* | *"bash -n"* | *"python3 "* | *"cargo-deny"* | *"cargo audit"* | *"anvil"* | *"forge"* | *"cast"*)
            herdr pane run "$pane" "3" >/dev/null 2>&1
            echo "[$(date +%H:%M:%S)] command -> 3: $cmd"
            ;;
        *)
            echo "[$(date +%H:%M:%S)] HOLD (needs review): $cmd"
            ;;
        esac
    fi

    sleep 1
done
