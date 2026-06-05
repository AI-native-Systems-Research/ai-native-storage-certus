fn main() {
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

    // Compile C wrapper for inline ibverbs functions
    cc::Build::new()
        .file("src/wrapper.c")
        .include("/usr/include")
        .compile("rdma_wrapper");
}
