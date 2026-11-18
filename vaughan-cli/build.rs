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

use std::process::Command;

fn main() {
    // A new commit (or a branch switch) has to re-stamp the binary. These two
    // files cover commit and checkout; a tree that only has dirty edits is
    // handled by cargo rebuilding the crate for the source change anyway.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");

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
    // `--quiet` makes diff exit non-zero when there is a difference; the `!`
    // success check below therefore means "tree is dirty".
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
