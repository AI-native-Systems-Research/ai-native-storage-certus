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
//! Every `.rs` and `Cargo.toml` under `apps/workload-generator/crates/`, which is the
//! generator, the agent, the wire and the model — the code whose divergence between two
//! nodes would change a measurement. Paths are sorted so the digest does not depend on
//! directory order, and each file contributes its path as well as its bytes, so moving code
//! between files changes the identity.
//!
//! Workspace dependencies outside this directory are **not** covered. A node running a
//! different `shmq-dispatcher` would not be caught here; that is a deliberate boundary
//! rather than an oversight, because hashing the whole repository would make the identity
//! change on every unrelated edit and the check would be ignored within a week.
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
            // `target` holds build output, not sources, and including it would make the
            // identity depend on whether the tree had been built.
            if name != "target" && !name.starts_with('.') {
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
