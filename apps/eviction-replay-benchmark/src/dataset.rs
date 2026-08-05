//! On-demand fetch of the Qwen-Bailian anonymized usage traces.
//!
//! The traces live in a git-LFS repository; each is tens to ~130 MB. To avoid
//! committing that data, the benchmark downloads the requested trace to `/tmp`
//! on first use (via `curl`) and reuses the cached copy thereafter.
//!
//! Datasets are named by short identifiers:
//!
//! | id | file | workload |
//! |----|------|----------|
//! | `chat` | `qwen_traceA_blksz_16.jsonl` | To-C interactive chat (multi-turn) |
//! | `api` | `qwen_traceB_blksz_16.jsonl` | To-B API-driven task automation |
//! | `thinking` | `qwen_thinking_blksz_16.jsonl` | reasoning-intensive chat |
//! | `coder` | `qwen_coder_blksz_16.jsonl` | code generation |
//!
//! Source: <https://github.com/alibaba-edu/qwen-bailian-usagetraces-anon>.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

/// git-LFS media endpoint that serves the raw trace bytes for the public repo.
const BASE_URL: &str =
    "https://media.githubusercontent.com/media/alibaba-edu/qwen-bailian-usagetraces-anon/main/";

/// Directory the downloaded traces are cached in.
const CACHE_DIR: &str = "/tmp";

/// `(id, remote filename, one-line description)` for each supported dataset.
pub const DATASETS: &[(&str, &str, &str)] = &[
    (
        "chat",
        "qwen_traceA_blksz_16.jsonl",
        "To-C interactive chat (multi-turn)",
    ),
    (
        "api",
        "qwen_traceB_blksz_16.jsonl",
        "To-B API-driven task automation",
    ),
    (
        "thinking",
        "qwen_thinking_blksz_16.jsonl",
        "reasoning-intensive chat",
    ),
    ("coder", "qwen_coder_blksz_16.jsonl", "code generation"),
];

/// Remote filename for a dataset id, or `None` if the id is unknown.
pub fn filename(id: &str) -> Option<&'static str> {
    DATASETS
        .iter()
        .find(|(k, _, _)| *k == id)
        .map(|(_, f, _)| *f)
}

/// Human-readable description for a dataset id.
pub fn describe(id: &str) -> Option<&'static str> {
    DATASETS
        .iter()
        .find(|(k, _, _)| *k == id)
        .map(|(_, _, d)| *d)
}

/// Ensure the named dataset is present in `/tmp`, downloading it if the cached
/// copy is missing or empty. Returns the local path.
pub fn ensure(id: &str) -> io::Result<PathBuf> {
    let file = filename(id).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unknown dataset '{id}' (expected one of chat, api, thinking, coder)"),
        )
    })?;
    let path = Path::new(CACHE_DIR).join(file);
    if path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(path);
    }
    download(file, &path)?;
    Ok(path)
}

/// Fetch `file` from the media endpoint to `dest` via `curl`, using a `.part`
/// staging file so an interrupted download never leaves a truncated trace that
/// a later run would treat as complete.
fn download(file: &str, dest: &Path) -> io::Result<()> {
    let url = format!("{BASE_URL}{file}");
    let part = dest.with_extension("part");
    eprintln!("downloading {url}\n         -> {} ...", dest.display());
    let status = Command::new("curl")
        .args(["-fL", "--retry", "3", "--progress-bar", "-o"])
        .arg(&part)
        .arg(&url)
        .status()
        .map_err(|e| io::Error::other(format!("failed to run curl (is it installed?): {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        return Err(io::Error::other(format!(
            "curl failed to download {url} ({status})"
        )));
    }
    std::fs::rename(&part, dest)
}
