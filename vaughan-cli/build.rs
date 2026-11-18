//! Stamps the binary with the git revision it was built from.
//!
//! Why: the crate version alone cannot answer "am I running my build or the
//! release?". Every commit between two releases carries the same
//! `CARGO_PKG_VERSION`, so a locally built `vaughan` and a months-old release
//! download both print `0.2.11` and look identical — which is exactly how a
//! stale binary goes unnoticed while the repo moves on. `vaughan --version` now
//! prints the revision, and a `-dirty` marker when the tree had uncommitted
//! changes at build time.
//!
//! Degrades quietly: a build from a published tarball or a vendored source tree
//! has no `.git`, so the revision is reported as `unknown` rather than failing
//! the build.
//!
//! Known limit, since this stamp exists to be trustworthy: cargo decides when
//! to re-run a build script, and the rerun triggers below are the git files
//! that record a commit, a checkout, and the index. Those cover the cases that
//! change the revision. They do NOT cover editing a file in another workspace
//! crate and rebuilding without committing — cargo may reuse this script's
//! output, so `-dirty` can lag by one build. Treat the revision as exact and
//! the dirty marker as a hint. `git rev-parse --git-path` is used rather than
//! `../.git/...` so this works in a linked worktree, where `.git` is a file.

use std::process::Command;

fn main() {
    for file in ["HEAD", "index"] {
        if let Some(path) = git(&["rev-parse", "--git-path", file]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    // HEAD on a branch is a symref; the commit lands in the ref file it names,
    // which is what actually changes on `git commit`.
    if let Some(head_ref) = git(&["symbolic-ref", "-q", "HEAD"]) {
        if let Some(path) = git(&["rev-parse", "--git-path", &head_ref]) {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let full = match git_rev() {
        Some(rev) => format!("{version} ({rev})"),
        None => format!("{version} (unknown rev)"),
    };
    println!("cargo:rustc-env=VAUGHAN_VERSION_FULL={full}");
}

/// Short HEAD revision plus a `-dirty` suffix when the tree has local changes.
fn git_rev() -> Option<String> {
    let rev = git(&["rev-parse", "--short=9", "HEAD"])?;
    // `--quiet` makes diff exit non-zero when there is a difference, so a
    // non-success status here means the tracked tree is dirty. Untracked files
    // are deliberately ignored: a new file does not change what was built.
    let clean = Command::new("git")
        .args(["diff", "--quiet", "HEAD"])
        .status()
        .map(|s| s.success())
        .unwrap_or(true);
    Some(if clean { rev } else { format!("{rev}-dirty") })
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
