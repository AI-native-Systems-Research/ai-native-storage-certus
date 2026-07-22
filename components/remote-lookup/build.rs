//! Build script: embed an rpath to the pre-built zyre libraries so the crate's
//! test and bench binaries can load `libzyre`/`libczmq`/`libzmq` at runtime
//! without a manually-set `LD_LIBRARY_PATH`.
//!
//! `remote-lookup` links zyre only as a dev-dependency (for the `tests/mesh.rs`
//! multi-node harness), so its test/bench binaries pull in the zyre native
//! libraries transitively. The zyre crate's own `build.rs` embeds an rpath into
//! *its* binaries, but `cargo:rustc-link-arg` does not propagate to dependents,
//! so we re-emit the same rpath here for our binaries.
//!
//! We use `DT_RPATH` (via `--disable-new-dtags`) rather than the default
//! `DT_RUNPATH`: `DT_RUNPATH` is consulted only for an object's *direct*
//! dependencies, but `libzyre` pulls in `libczmq -> libzmq` transitively, and
//! only `DT_RPATH` is honored for those transitive lookups from the executable.

use std::env;
use std::path::PathBuf;

fn main() {
    // Resolve the zyre build directory the same way the `zyre` crate does:
    // an explicit `ZYRE_BUILD_DIR`, else `<workspace-root>/deps/zyre-build`.
    let zyre_build_dir = env::var("ZYRE_BUILD_DIR").unwrap_or_else(|_| {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let workspace_root = PathBuf::from(&manifest_dir)
            .parent()
            .and_then(|p| p.parent())
            .expect("workspace root is two levels above the crate manifest")
            .to_path_buf();
        workspace_root
            .join("deps")
            .join("zyre-build")
            .to_string_lossy()
            .into_owned()
    });

    let lib_dir = PathBuf::from(&zyre_build_dir).join("lib");
    let lib64_dir = PathBuf::from(&zyre_build_dir).join("lib64");

    // Applies to this crate's binary/test/bench/example targets (the test
    // binaries that link zyre transitively); the rlib itself ignores link args.
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib64_dir.display());

    println!("cargo:rerun-if-env-changed=ZYRE_BUILD_DIR");
}
