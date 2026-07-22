fn main() {
    if cfg!(feature = "gpu") {
        println!("cargo:rustc-link-search=native=/usr/local/cuda/lib64");
        println!("cargo:rustc-link-search=native=/usr/lib64");
        if let Ok(path) = std::env::var("CUDA_RUNTIME_LIB_PATH") {
            println!("cargo:rustc-link-search=native={path}");
        }
        // Python nvidia-cuda-runtime-cu12 package location (pip install).
        if let Ok(site) = std::env::var("HOME") {
            let pip_path = format!(
                "{}/.local/lib/python3.9/site-packages/nvidia/cuda_runtime/lib",
                site
            );
            if std::path::Path::new(&pip_path).exists() {
                println!("cargo:rustc-link-search=native={pip_path}");
            }
        }
        println!("cargo:rustc-link-lib=dylib=cudart");

        // GDRCopy library (required for GPU BAR1 mapping with SPDK).
        if cfg!(feature = "spdk") {
            if let Ok(path) = std::env::var("GDRCOPY_LIB_PATH") {
                println!("cargo:rustc-link-search=native={path}");
            } else {
                // Project-local GDRCopy build (kernel/modules/gdrcopy/src/).
                let manifest_dir =
                    std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
                let gdrcopy_lib = manifest_dir.join("../../kernel/modules/gdrcopy/src");
                if gdrcopy_lib.exists() {
                    println!(
                        "cargo:rustc-link-search=native={}",
                        gdrcopy_lib.canonicalize().unwrap().display()
                    );
                } else {
                    println!("cargo:rustc-link-search=native=/usr/local/lib");
                    println!("cargo:rustc-link-search=native=/usr/local/gdrcopy/lib");
                    println!("cargo:rustc-link-search=native=/usr/lib64");
                }
            }
            println!("cargo:rustc-link-lib=dylib=gdrapi");
        }
    }
}
