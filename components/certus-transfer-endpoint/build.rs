fn main() {
    println!("cargo:rerun-if-changed=src/wrapper.c");

    // Everything here is gated on `rdma`: the default build compiles no C,
    // probes no pkg-config entry and links nothing, so it needs no rdma-core
    // (constitution Principle VIII).
    if std::env::var("CARGO_FEATURE_RDMA").is_err() {
        return;
    }

    // Fail here, with the library's name, rather than at link time.
    let ibverbs = pkg_config::probe_library("libibverbs")
        .expect("feature `rdma` needs rdma-core's libibverbs (pkg-config libibverbs)");
    let rdmacm = pkg_config::probe_library("librdmacm")
        .expect("feature `rdma` needs rdma-core's librdmacm (pkg-config librdmacm)");

    // The shims, plus the `_Static_assert` half of the FFI layout check
    // (src/ffi.rs), so a header that disagrees with the Rust declarations fails
    // to compile.
    cc::Build::new()
        .file("src/wrapper.c")
        .includes(ibverbs.include_paths.iter().chain(&rdmacm.include_paths))
        .compile("certus_te_wrapper");
}
