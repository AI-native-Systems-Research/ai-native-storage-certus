use std::path::PathBuf;
use std::process::Command;

const PROTOC_VERSION: &str = "25.1";

fn find_protoc() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("PROTOC") {
        let path = PathBuf::from(&p);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(output) = Command::new("which").arg("protoc").output() {
        if output.status.success() {
            let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
            if path.exists() {
                return Some(path);
            }
        }
    }
    None
}

fn download_protoc() -> PathBuf {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let protoc_dir = out_dir.join("protoc");
    let protoc_bin = protoc_dir.join("bin").join("protoc");

    if protoc_bin.exists() {
        return protoc_bin;
    }

    let url = format!(
        "https://github.com/protocolbuffers/protobuf/releases/download/v{}/protoc-{}-linux-x86_64.zip",
        PROTOC_VERSION, PROTOC_VERSION
    );

    let zip_path = out_dir.join("protoc.zip");

    let status = Command::new("curl")
        .args(["-sL", "-o"])
        .arg(&zip_path)
        .arg(&url)
        .status()
        .expect("failed to run curl");
    assert!(status.success(), "failed to download protoc from {url}");

    std::fs::create_dir_all(&protoc_dir).unwrap();
    let status = Command::new("unzip")
        .args(["-q", "-o"])
        .arg(&zip_path)
        .arg("-d")
        .arg(&protoc_dir)
        .status()
        .expect("failed to run unzip");
    assert!(status.success(), "failed to unzip protoc");

    std::fs::remove_file(&zip_path).ok();
    protoc_bin
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = find_protoc().unwrap_or_else(|| {
        eprintln!("cargo:warning=protoc not found, downloading v{PROTOC_VERSION}...");
        download_protoc()
    });

    std::env::set_var("PROTOC", &protoc);
    tonic_build::compile_protos("proto/dispatcher.proto")?;
    Ok(())
}
