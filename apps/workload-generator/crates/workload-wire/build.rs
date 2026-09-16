//! Capture a **source** identity for FR-051's provenance check.
//!
//! FR-051 requires verifying that each daemon "was built from the same sources as the
//! generator". The generator and the agent are different binaries, so a digest of either
//! executable cannot answer that — their bytes differ by construction. What must match is
//! the sources they were built from.
//!
//! # It hashes the sources, not the repository
//!
//! An earlier version of this script asked git: commit, plus a hash of the diff, plus the
//! porcelain status. That was the wrong instrument, for three reasons that only became
//! obvious once someone asked what happens to a build from a tarball:
//!
//! 1. **A tarball has no git.** The build still succeeded, but the identity fell back to
//!    `unknown` and every multi-node run from a release tarball would have been refused —
//!    a cost imposed by the mechanism rather than by the requirement.
//! 2. **Untracked files were invisible.** A new file present on one node and absent on the
//!    other changed behaviour without changing the identity, which is precisely the
//!    divergence the check exists to catch.
//! 3. **Cargo could not know when to re-run it.** `.git/index` moves on `git add`, not on a
//!    bare edit, so an uncommitted change could leave a stale identity compiled in.
//!
//! Hashing the source files themselves fixes all three: it needs no tooling, it sees every
//! file whether or not git has heard of it, and each file hashed is declared to cargo so an
//! edit re-runs this script. It is also a more literal reading of "the same sources".
//!
//! # What is covered
//!
//! Every `.rs` and `Cargo.toml` under `apps/workload-generator/crates/`, excluding `tests/`
//! and `benches/` — those compile into separate binaries that are never linked into the agent
//! or the generator, so an edit there cannot change either one's behaviour and including them
//! only forced a redeploy whenever a test was touched. Paths are sorted so the digest does not
//! depend on directory order, and each file contributes its path as well as its bytes, so
//! moving code between files changes the identity.
//!
//! Inline `#[cfg(test)]` modules inside `src/` are still covered, since a path cannot tell
//! them from the code around them. Editing one still invalidates a deployed agent — the
//! remaining friction, and a small one.
//!
//! Workspace dependencies outside this directory are **not** covered. A node running a
//! different `shmq-dispatcher` would not be caught here; that is a deliberate boundary
//! rather than an oversight, because hashing the whole repository would make the identity
//! change on every unrelated edit and the check would be ignored within a week.
//!
//! # What it costs, measured
//!
//! **1.2 ms** for 58 files and 948 KB — 0.5 ms walking the tree, 0.7 ms folding bytes — and
//! only when one of those files changes, since each is declared to cargo. A no-op rebuild
//! re-runs nothing; an edit that does trigger it already costs ~0.43 s of recompilation, so
//! the hash is about 0.3% of a cost the edit was going to pay anyway.
//!
//! For scale, and because it settles the boundary above rather than leaving it to taste:
//! every `.rs` and `Cargo.toml` in the whole repository is 410 files and 4.6 MB at **189
//! ms**, and including SPDK's C and the Python is 9 819 files and 153 MB at **447 ms**. Note
//! where that time goes — 181 of the 189 ms is the directory *walk*, not the hashing, so
//! widening the scope is paid for in `readdir` against `deps/` rather than in arithmetic.
//! None of it is prohibitive; the reason to stay narrow is the identity churn, not the
//! milliseconds.
//!
//! `WORKLOAD_SOURCE_ID` overrides everything, for a packager who has a better answer.

use std::fs;
use std::path::{Path, PathBuf};

/// Collect the files whose contents define this build's behaviour.
fn sources(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            // `target` holds build output, not sources. `tests` and `benches` compile into
            // separate binaries and are never linked into the agent or the generator, so an
            // edit there cannot change either one's behaviour — including them only forced a
            // redeploy every time a test was touched, which is friction with no safety in it.
            if !matches!(name.as_ref(), "target" | "tests" | "benches") && !name.starts_with('.') {
                sources(&path, out);
            }
        } else if name.ends_with(".rs") || name == "Cargo.toml" {
            out.push(path);
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=WORKLOAD_SOURCE_ID");

    if let Ok(forced) = std::env::var("WORKLOAD_SOURCE_ID") {
        println!("cargo:rustc-env=WORKLOAD_SOURCE_ID={forced}");
        return;
    }

    // The script runs with the crate root as its working directory, so `..` is the crates
    // directory holding every crate of this application.
    let root = PathBuf::from("..");
    let mut files = Vec::new();
    sources(&root, &mut files);
    // Sorted, so the digest does not depend on the order the filesystem returns entries.
    files.sort();

    let mut fold = Fold::new();
    let mut counted = 0usize;
    let mut bytes = 0usize;
    for path in &files {
        let Ok(content) = fs::read(path) else {
            continue;
        };
        // Declaring each file is what lets cargo re-run this script on an edit — the hole
        // the git version had.
        println!("cargo:rerun-if-changed={}", path.display());
        // The path as well as the bytes: moving code between files must change the identity.
        fold.write(path.to_string_lossy().as_bytes());
        fold.write(&content);
        counted += 1;
        bytes += content.len();
    }

    let id = if counted == 0 {
        // The sources could not be read at all, which is a real problem rather than a
        // portability case: the handshake refuses an unknown identity.
        "unknown".to_string()
    } else {
        format!("src:{counted}:{bytes}:{:016x}", fold.finish())
    };
    println!("cargo:rustc-env=WORKLOAD_SOURCE_ID={id}");
}

/// A cheap 64-bit fold over the source bytes.
///
/// Not cryptographic and does not need to be: the failure being prevented is an accident — a
/// node left running yesterday's build — and anyone able to replace the binary can replace
/// the identity it reports too. The file count and total size travel alongside it, so a
/// collision would have to match all three.
struct Fold(u64);

impl Fold {
    fn new() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x1000_0000_01B3);
            self.0 = self.0.rotate_left(7);
        }
        // A separator, so `["ab", "c"]` and `["a", "bc"]` do not fold alike.
        self.0 ^= 0xFFFF_FFFF_FFFF_FFFF;
    }

    fn finish(&self) -> u64 {
        self.0
    }
}
