fn main() {
    if cfg!(feature = "gpu") {
        // Link against CUDA runtime library.
        // Standard CUDA install locations.
        println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
        println!("cargo:rustc-link-search=native=/usr/lib64");
        println!("cargo:rustc-link-lib=dylib=cudart");
    }
}
