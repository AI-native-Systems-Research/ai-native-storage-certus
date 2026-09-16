//! Capture a **source** identity for FR-051's provenance check.
//!
//! FR-051 requires verifying that each daemon "was built from the same sources as the
//! generator". The generator and the agent are different binaries, so a digest of either
//! executable cannot answer that — their bytes differ by construction. What must match is
//! the tree they were built from.
//!
//! This is computed **once, here**, and both binaries obtain it by depending on this crate.
//! Two build scripts computing it independently could disagree — a stale cache on one, a
//! different working directory on the other — and a provenance check that can disagree with
//! itself is worse than none.
//!
//! The identity is the commit, plus the tracked diff, plus the porcelain status. The diff
//! covers uncommitted edits to tracked files and the status covers additions and deletions
//! by name. **The contents of untracked files are not covered**, which is a real hole: a new
//! file present on one node and absent on the other changes behaviour without changing this
//! id. It is stated rather than papered over.
//!
//! If git cannot answer, the identity is the literal `unknown`, and the handshake refuses a
//! peer whose identity is unknown rather than assuming the best. Provenance that cannot be
//! established must not be claimed.

use std::process::Command;

/// Run a git command and return its stdout, or `None` if git could not answer.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn main() {
    // A commit change is visible through these; an unstaged edit may not re-trigger the
    // script, which is why the agent is expected to be deployed from a committed tree.
    println!("cargo:rerun-if-changed=build.rs");
    for path in ["../../../../.git/HEAD", "../../../../.git/index"] {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rerun-if-env-changed=WORKLOAD_SOURCE_ID");

    if let Ok(forced) = std::env::var("WORKLOAD_SOURCE_ID") {
        println!("cargo:rustc-env=WORKLOAD_SOURCE_ID={forced}");
        return;
    }

    let id = match git(&["rev-parse", "HEAD"]) {
        Some(head) => {
            let diff = git(&["diff", "HEAD"]).unwrap_or_default();
            let status = git(&["status", "--porcelain"]).unwrap_or_default();
            // Lengths and a fold rather than the text itself: the value has to fit in an
            // environment variable, and the digest is taken over this string anyway.
            format!(
                "{}|diff:{}:{:x}|status:{}:{:x}",
                head.trim(),
                diff.len(),
                fold(diff.as_bytes()),
                status.len(),
                fold(status.as_bytes())
            )
        }
        None => "unknown".to_string(),
    };
    println!("cargo:rustc-env=WORKLOAD_SOURCE_ID={id}");
}

/// A cheap 64-bit fold, enough to distinguish two working trees.
///
/// Not cryptographic and does not need to be: the failure being prevented is an accident — a
/// node left running yesterday's build — and anyone able to replace the binary can replace
/// the identity it reports too.
fn fold(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0x9E37_79B9_7F4A_7C15;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x1000_0000_01B3);
        h = h.rotate_left(7);
    }
    h
}
