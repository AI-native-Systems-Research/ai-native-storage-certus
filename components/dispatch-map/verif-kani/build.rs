//! Derives includable copies of the production dispatch-map sources into `OUT_DIR`.
//!
//! WHY. `src/lib.rs` must compile the REAL `components/dispatch-map/src/lib.rs` inside this
//! crate so harnesses can see the `pub(crate)` entry representation. `include!` of the file
//! itself does not compile: it opens with inner doc comments (`//! ...`), and inner
//! attributes introduced by a macro expansion are rejected (rustc E0753).
//!
//! WHAT CHANGES, EXACTLY. One transformation on `lib.rs`: a line whose first non-blank
//! characters are `//!` becomes `//` + the rest. `entry.rs` and `state.rs` are copied
//! byte-for-byte (they are loaded as ordinary module files by `lib.rs`'s own `mod entry;` /
//! `mod state;`, which rustc resolves next to the included file, i.e. in OUT_DIR).
//! [`assert_code_identical`] fails the build unless every copy equals its production source
//! once comment-only lines are dropped, so the proofs can never check code that is not the
//! shipped code. The copies are regenerated on every change and never committed.

use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_dir = manifest.join("../src");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    for (name, dst, neutralize) in [
        ("lib.rs", "dm_lib.rs", true),
        ("entry.rs", "entry.rs", false),
        ("state.rs", "state.rs", false),
    ] {
        let src = src_dir.join(name);
        println!("cargo:rerun-if-changed={}", src.display());
        let original = std::fs::read_to_string(&src)
            .unwrap_or_else(|e| panic!("cannot read production source {}: {e}", src.display()));
        let derived = if neutralize { neutralize_inner_docs(&original) } else { original.clone() };
        assert_code_identical(&original, &derived, &src);
        std::fs::write(out.join(dst), derived).expect("cannot write derived source into OUT_DIR");
    }
}

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
    if strip(original) != strip(derived) {
        panic!(
            "derived copy of {} differs from the production source outside comments — \
             refusing to verify code that is not the shipped code",
            src.display()
        );
    }
}
