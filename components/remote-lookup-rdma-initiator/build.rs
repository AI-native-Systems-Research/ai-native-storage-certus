fn main() {
    println!("cargo:rerun-if-changed=src/wrapper.c");

    // The real rdma-core path is gated behind the `rdma` feature. Without it the
    // crate builds over the in-process mock transport and must not link any
    // external RDMA library or compile the C shim, so a box with no rdma-core
    // still builds.
    if std::env::var("CARGO_FEATURE_RDMA").is_err() {
        return;
    }

    // Link RDMA libraries (rdma-core: libibverbs + librdmacm).
    if let Ok(lib) = pkg_config::probe_library("libibverbs") {
        for path in &lib.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    } else {
        println!("cargo:rustc-link-search=native=/usr/lib64");
    }
    println!("cargo:rustc-link-lib=ibverbs");

    if let Ok(lib) = pkg_config::probe_library("librdmacm") {
        for path in &lib.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    } else {
        println!("cargo:rustc-link-search=native=/usr/lib64");
    }
    println!("cargo:rustc-link-lib=rdmacm");

    // Compile the C wrapper for inline ibverbs functions.
    cc::Build::new()
        .file("src/wrapper.c")
        .include("/usr/include")
        .compile("rdma_wrapper");
}
