//! Derives an includable copy of the production `src/lru_list.rs` into `OUT_DIR`.
//!
//! WHY THIS EXISTS. `src/lru_real.rs` needs the REAL `LruList` source compiled inside
//! this crate, so that a child module can read `LruList`'s and `Node`'s private fields
//! — better than half the inventory's global invariants are statements about that
//! representation and cannot be observed through `IEvictionPolicy` at all (a list whose
//! `len` has drifted from its chain still answers `len()` with the drifted number).
//! The natural spelling, `include!("../../src/lru_list.rs")`, does not compile: the
//! production file opens with an inner doc comment (`//! Index-based doubly-linked
//! list ...`), and inner attributes are only legal lexically at the start of a module
//! body, never introduced by a macro expansion (rustc E0753).
//!
//! WHAT THIS CHANGES, EXACTLY. One transformation, applied line by line: a line whose
//! first non-whitespace characters are `//!` becomes `//` + the rest. That turns inner
//! doc comments into ordinary comments and touches nothing else — no token of Rust code
//! is added, removed, or reordered, and [`assert_code_identical`] below enforces that
//! by comparing the two files with all comment-only lines removed. If the production
//! file is ever edited in a way this cannot handle, the build fails loudly rather than
//! verifying something subtly different from the shipped code.
//!
//! WHY IT IS STILL DRIFT-PROOF. The copy is regenerated from the production file on
//! every build (`rerun-if-changed` below), lives only in `OUT_DIR`, and is never
//! committed. Editing `components/eviction-policy-optimized/src/lru_list.rs` therefore
//! changes what every harness in `proofs_list.rs` checks, which is the property that
//! makes `fidelity: real-type-bounded` an honest label for those proofs.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../src/lru_list.rs");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("lru_list_real.rs");

    println!("cargo:rerun-if-changed={}", src.display());

    let original = std::fs::read_to_string(&src)
        .unwrap_or_else(|e| panic!("cannot read production source {}: {e}", src.display()));

    let derived = neutralize_inner_docs(&original);
    assert_code_identical(&original, &derived, &src);

    std::fs::write(&out, derived).expect("cannot write derived source into OUT_DIR");
}

/// `//!` at the start of a line (after optional whitespace) becomes `//`.
fn neutralize_inner_docs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for line in s.split_inclusive('\n') {
        let indent_len = line.len() - line.trim_start().len();
        let (indent, rest) = line.split_at(indent_len);
        if let Some(tail) = rest.strip_prefix("//!") {
            out.push_str(indent);
            out.push_str("//");
            out.push_str(tail);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Fail the build unless the two texts agree once every comment-only line is dropped —
/// i.e. unless the transformation really was confined to comments.
fn assert_code_identical(original: &str, derived: &str, src: &Path) {
    let strip = |s: &str| -> Vec<String> {
        s.lines()
            .map(|l| l.trim_end())
            .filter(|l| {
                let t = l.trim_start();
                !t.is_empty() && !t.starts_with("//")
            })
            .map(|l| l.to_string())
            .collect()
    };
    let a = strip(original);
    let b = strip(derived);
    if a != b {
        panic!(
            "derived copy of {} differs from the production source outside comments — \
             refusing to verify code that is not the shipped code",
            src.display()
        );
    }
}
