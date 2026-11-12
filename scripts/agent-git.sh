#!/usr/bin/env bash
# Git operations that are safe when several agents share a repository.
#
# The failure this prevents: one agent's `git add -A` (or a scratch branch, or a
# `git stash`) swallowing another agent's uncommitted work. Prose rules did not
# stop it, so the dangerous verbs are refused here.
#
# Usage:
#   scripts/agent-git.sh status
#       Dirty files, split into staged / unstaged / untracked, so it is obvious
#       which of them are not yours.
#
#   scripts/agent-git.sh stage <path>...
#       Stage explicit paths only. Refuses `.`, `-A`, `--all`, globs that expand
#       to the whole tree, and paths outside the repo.
#
#   scripts/agent-git.sh commit "<message>" <path>...
#       Stage exactly those paths and commit them. Aborts if anything else is
#       already staged (someone else's work would ride along). `--force` overrides.
#
#   scripts/agent-git.sh scratch <name>
#       Create a throwaway worktree at HEAD in $TMPDIR/scratch-<name>-<sha>.
#       Worktrees have their own index and working tree, so negative tests and
#       experiment commits cannot touch the shared checkout. Prints the path.
#
#   scripts/agent-git.sh drop <path>
#       Remove a scratch worktree (refuses a plain checkout).
#
#   scripts/agent-git.sh recover <ref> <path>...
#       Restore paths from a commit that was deleted or swept away.
#
#   scripts/agent-git.sh undo
#       Undo the last commit keeping its content staged, then print the staged
#       set so it can be re-split deliberately.

set -uo pipefail

die() { printf 'agent-git: %s\n' "$*" >&2; exit 1; }
note() { printf 'agent-git: %s\n' "$*" >&2; }

git rev-parse --is-inside-work-tree >/dev/null 2>&1 || die "not inside a git work tree"

cmd=${1:-}
[ -n "$cmd" ] || die "no command — see the header of this script"
shift || true

case "$cmd" in
status)
    printf '== staged (these WILL be committed)\n'
    git diff --cached --name-status || true
    printf '\n== unstaged\n'
    git diff --name-status || true
    printf '\n== untracked\n'
    git ls-files --others --exclude-standard || true
    printf '\n%s\n' "any file above you did not edit belongs to another agent: do not stage it,"
    printf '%s\n' "do not stash it, and do not create branches around it"
    ;;

stage)
    [ "$#" -gt 0 ] || die "stage needs explicit paths"
    for p in "$@"; do
        case "$p" in
        .|./|-A|--all|'*') die "refusing broad staging ($p) — name the files" ;;
        esac
    done
    git add -- "$@" || die "git add failed"
    printf 'staged:\n'
    git diff --cached --name-only
    ;;

commit)
    msg=${1:-}
    [ -n "$msg" ] || die 'commit needs a message: commit "<msg>" <path>...'
    shift || true
    force=0
    paths=()
    for a in "$@"; do
        case "$a" in
        --force) force=1 ;;
        *) paths+=("$a") ;;
        esac
    done
    [ "${#paths[@]}" -gt 0 ] || die "commit needs explicit paths (never 'git commit -a' here)"

    # Anything already staged that is not in the declared set would ride along.
    if [ "$force" -eq 0 ]; then
        staged=$(git diff --cached --name-only)
        if [ -n "$staged" ]; then
            foreign=""
            while IFS= read -r f; do
                [ -n "$f" ] || continue
                keep=0
                for p in "${paths[@]}"; do [ "$f" = "$p" ] && keep=1; done
                [ "$keep" -eq 0 ] && foreign="$foreign$f"$'\n'
            done <<<"$staged"
            if [ -n "$foreign" ]; then
                note "refusing to commit: the index already holds paths outside your list:"
                printf '%s' "$foreign" >&2
                note "commit them separately, or pass --force if they really are yours"
                exit 3
            fi
        fi
    fi

    git add -- "${paths[@]}" || die "git add failed"
    git commit -m "$msg" -- "${paths[@]}"
    ;;

scratch)
    name=${1:-} || true
    [ -n "${name:-}" ] || die "scratch needs a name"
    sha=$(git rev-parse --short HEAD) || die "cannot resolve HEAD"
    dest="${TMPDIR:-/tmp}/scratch-${name}-${sha}"
    [ -e "$dest" ] && die "$dest already exists"
    git worktree add --detach "$dest" HEAD >/dev/null || die "worktree add failed"
    printf '%s\n' "$dest"
    note "isolated index — negative tests and throwaway commits belong here"
    ;;

drop)
    dest=${1:-}
    [ -n "$dest" ] || die "drop needs the worktree path"
    case "$dest" in
    */scratch-*) ;;
    *) die "refusing: $dest is not a scratch-<name>-<sha> worktree path" ;;
    esac
    git worktree remove --force "$dest" || die "worktree remove failed"
    note "removed $dest"
    ;;

recover)
    ref=${1:-}
    [ -n "$ref" ] || die "recover needs a ref"
    shift || true
    [ "$#" -gt 0 ] || die "recover needs explicit paths"
    git show --stat --oneline "$ref" | head -15 >&2
    git checkout "$ref" -- "$@" || die "restore failed"
    note "restored $* from $ref (staged — review before committing)"
    ;;

undo)
    git reset --soft HEAD~1 || die "undo needs at least one commit"
    note "last commit undone; these paths are staged:"
    git diff --cached --name-only >&2
    ;;

*)
    sed -n '2,40p' "${BASH_SOURCE[0]}"
    exit 2
    ;;
esac
