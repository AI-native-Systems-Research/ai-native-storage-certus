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
//!
//! We emit TWO rpath entries: the absolute `deps/zyre-build/lib{,64}` derived
//! from the manifest path, AND an `$ORIGIN`-relative path (`$ORIGIN/../../../
//! deps/zyre-build/lib{,64}`, i.e. from `target/<profile>/deps/` back to the
//! workspace's `deps/`). The absolute path is stable when the target dir is
//! relocated; the `$ORIGIN`-relative one is stable when the *checkout* moves.
//! The latter matters because a git-worktree build that shares the parent
//! repo's `target/` bakes the worktree's (transient) absolute path into the
//! rpath, poisoning the shared artifact for the main checkout once the worktree
//! is removed — cargo treats the artifact as fresh (this script only reruns on
//! `ZYRE_BUILD_DIR`), so the stale absolute rpath is never regenerated and the
//! loader aborts with `libzyre.so.2: cannot open shared object file`. The
//! `$ORIGIN`-relative entry resolves regardless of which checkout produced the
//! binary, so the load succeeds even from a poisoned absolute path.

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
    // Absolute rpath (from the manifest path) — survives target-dir relocation.
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib64_dir.display());
    // `$ORIGIN`-relative rpath — survives checkout moves / worktree-shared
    // target dirs. Test/bench/example binaries live in `target/<profile>/deps`
    // (and `.../examples`), three levels below the workspace root, so `$ORIGIN/
    // ../../../deps/zyre-build/lib{,64}` reaches the same `deps/` the absolute
    // path names. The linker records `$ORIGIN` literally (no shell expansion);
    // a nonexistent entry is simply skipped by the loader, so listing both is
    // safe. See the module header for why the absolute entry can go stale.
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../../deps/zyre-build/lib");
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../../../deps/zyre-build/lib64");

    println!("cargo:rerun-if-env-changed=ZYRE_BUILD_DIR");
}
