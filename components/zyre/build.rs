use std::env;
use std::path::PathBuf;

fn main() {
    let zyre_build_dir = env::var("ZYRE_BUILD_DIR").unwrap_or_else(|_| {
        let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
        let workspace_root = PathBuf::from(&manifest_dir)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        workspace_root
            .join("deps")
            .join("zyre-build")
            .to_string_lossy()
            .to_string()
    });

    let include_dir = PathBuf::from(&zyre_build_dir).join("include");
    let lib_dir = PathBuf::from(&zyre_build_dir).join("lib");
    let lib64_dir = PathBuf::from(&zyre_build_dir).join("lib64");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-search=native={}", lib64_dir.display());
    // Embed the library directories as rpath so the dynamic loader can find
    // libzyre/libczmq/libzmq at runtime without requiring LD_LIBRARY_PATH.
    // Use DT_RPATH (--disable-new-dtags) rather than the default DT_RUNPATH:
    // DT_RUNPATH is only consulted for an object's *direct* dependencies, but
    // libzyre pulls in libczmq -> libzmq transitively, and only DT_RPATH is
    // honored for those transitive lookups from the executable.
    println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib_dir.display());
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", lib64_dir.display());
    println!("cargo:rustc-link-lib=zyre");
    println!("cargo:rustc-link-lib=czmq");
    println!("cargo:rustc-link-lib=zmq");

    println!("cargo:rerun-if-env-changed=ZYRE_BUILD_DIR");
    println!(
        "cargo:rerun-if-changed={}",
        include_dir.join("zyre.h").display()
    );

    let bindings = bindgen::Builder::default()
        .header(include_dir.join("zyre.h").to_string_lossy())
        .clang_arg(format!("-I{}", include_dir.display()))
        .allowlist_function("zyre_.*")
        .allowlist_function("zyre_event_.*")
        .allowlist_function("zmsg_.*")
        .allowlist_function("zframe_.*")
        .allowlist_function("zhash_.*")
        .allowlist_function("zsock_.*")
        .allowlist_function("zlist_.*")
        .allowlist_function("zpoller_.*")
        .allowlist_type("zyre_t")
        .allowlist_type("zyre_event_t")
        .allowlist_type("zmsg_t")
        .allowlist_type("zframe_t")
        .allowlist_type("zhash_t")
        .allowlist_type("zsock_t")
        .allowlist_type("zlist_t")
        .allowlist_type("zpoller_t")
        .generate_comments(true)
        .derive_debug(true)
        .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
        .generate()
        .expect("Unable to generate zyre bindings");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings");
}
