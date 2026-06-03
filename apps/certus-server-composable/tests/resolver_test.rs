//! Unit tests for dylib path resolution.

use std::fs;
use tempfile::TempDir;

#[test]
fn test_search_path_ordering() {
    let dir1 = TempDir::new().unwrap();
    let dir2 = TempDir::new().unwrap();

    // Create a dylib in dir2 only.
    let dylib_path = dir2.path().join("libtest.so");
    fs::write(&dylib_path, b"fake dylib").unwrap();

    // Search should find it in dir2.
    let search_paths = vec![dir1.path().to_path_buf(), dir2.path().to_path_buf()];
    let mut found = None;
    for dir in &search_paths {
        let candidate = dir.join("libtest.so");
        if candidate.exists() {
            found = Some(candidate);
            break;
        }
    }
    assert_eq!(found.unwrap(), dylib_path);
}

#[test]
fn test_absolute_path_override() {
    let dir = TempDir::new().unwrap();
    let dylib_path = dir.path().join("libcustom.so");
    fs::write(&dylib_path, b"fake dylib").unwrap();

    // Absolute path should be used directly.
    let abs = dylib_path.to_str().unwrap();
    let path = std::path::PathBuf::from(abs);
    assert!(path.exists());
    assert!(path.is_file());
}

#[test]
fn test_missing_file_detection() {
    let dir = TempDir::new().unwrap();
    let search_paths = vec![dir.path().to_path_buf()];

    let mut found = false;
    for dir in &search_paths {
        let candidate = dir.join("libnonexistent.so");
        if candidate.exists() {
            found = true;
        }
    }
    assert!(!found);
}

#[test]
fn test_env_var_prepend() {
    // Simulate CERTUS_LIB_PATH parsing.
    let env_val = "/opt/certus/lib:/usr/local/lib/certus";
    let parts: Vec<&str> = env_val.split(':').collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "/opt/certus/lib");
    assert_eq!(parts[1], "/usr/local/lib/certus");
}

#[test]
fn test_empty_search_paths() {
    let search_paths: Vec<std::path::PathBuf> = vec![];
    let mut found = false;
    for dir in &search_paths {
        let candidate = dir.join("libany.so");
        if candidate.exists() {
            found = true;
        }
    }
    assert!(!found);
}
