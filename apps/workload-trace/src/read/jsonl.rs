//! The JSONL container.
//!
//! One `Invocation` per line, and nothing container-specific past this module: the
//! parquet reader produces the same [`Invocation`] values, which is what makes
//! "the container is not information" a testable claim rather than an aspiration
//! (spec FR-021j).

use std::io::BufRead;
use std::path::Path;

use workload_model::trace::Invocation;

use super::{ReadError, Trace};

/// Read every invocation from a `.jsonl` file.
///
/// Blank lines are skipped; a malformed line fails the read with its line number
/// rather than being dropped, since a silently skipped record is a silently
/// truncated trace and FR-055e exists to refuse exactly that.
pub fn read_invocations(path: &Path) -> Result<Vec<Invocation>, ReadError> {
    let file =
        std::fs::File::open(path).map_err(|e| ReadError::Io(format!("{}: {e}", path.display())))?;
    let reader = std::io::BufReader::new(file);
    let mut out = Vec::new();
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| ReadError::Io(format!("{}:{}: {e}", path.display(), i + 1)))?;
        if line.trim().is_empty() {
            continue;
        }
        let inv: Invocation = serde_json::from_str(&line)
            .map_err(|e| ReadError::Io(format!("{}:{}: {e}", path.display(), i + 1)))?;
        out.push(inv);
    }
    Ok(out)
}

/// Read a whole JSONL trace: manifest, invocations, normalisation and ordering.
///
/// Prefer [`super::read_trace`], which picks the container. This stays public
/// because a caller that knows it has a `.jsonl` file — the round-trip test, for
/// one — should not have to route through a dispatch on the path's shape.
pub fn read_trace(
    path: &Path,
    allow_partial: bool,
    block_size: Option<u32>,
) -> Result<Trace, ReadError> {
    let manifest = super::read_manifest(path)?;
    let block_size = super::resolve_block_size(&manifest, block_size)?;
    let rows = read_invocations(path)?;
    super::assemble(manifest, rows, block_size, allow_partial)
}

#[cfg(test)]
mod tests {
    use super::*;
    use workload_model::trace::{BlockStats, TraceManifest};

    /// Write a trace to a temporary directory and read it back.
    fn round_trip(rows: &[Invocation], declared: u64) -> Result<Trace, ReadError> {
        let dir = std::env::temp_dir().join(format!(
            "certus-trace-test-{}-{}",
            std::process::id(),
            rows.len() * 7 + declared as usize
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let m = TraceManifest::synthetic(
            "t",
            16,
            BlockStats {
                sessions: 1,
                invocations: declared,
                unique_blocks: 3,
            },
        );
        std::fs::write(dir.join("manifest.json"), m.to_json().unwrap()).unwrap();
        let mut text = String::new();
        for r in rows {
            text.push_str(&serde_json::to_string(r).unwrap());
            text.push('\n');
        }
        let file = dir.join("trace.jsonl");
        std::fs::write(&file, text).unwrap();
        let out = read_trace(&file, false, None);
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn row(session: &str, index: i64, blocks: &[i64]) -> Invocation {
        Invocation {
            trace_id: "t".into(),
            session_id: Some(session.into()),
            invocation_index: index,
            parent_invocation: index - 1,
            parent_invocations: vec![],
            request_start: Some(index as f64),
            request_end: None,
            timestamp_kind: "start".into(),
            timestamp_is_synthetic: true,
            model: None,
            input_length: blocks.len() as i64 * 16,
            output_length: 0,
            reuse_from: vec![],
            new_input_blocks: vec![],
            new_output_blocks: vec![],
            full_input_blocks: blocks.to_vec(),
            full_output_blocks: vec![],
            partial_final_valid: 16,
        }
    }

    #[test]
    fn a_written_trace_reads_back_with_its_block_lists_intact() {
        let rows = vec![row("a", 0, &[1, 2, 3]), row("b", 0, &[1, 2, 9])];
        let t = round_trip(&rows, 2).expect("should read");
        assert_eq!(t.invocations.len(), 2);
        assert_eq!(t.references(), 6);
        assert_eq!(t.sessions(), 2);
        assert_eq!(t.invocations[0].blocks.len(), 3);
    }

    #[test]
    fn reading_fewer_rows_than_declared_is_refused() {
        let rows = vec![row("a", 0, &[1, 2, 3])];
        let e = round_trip(&rows, 500).expect_err("must refuse a sample");
        assert!(e.to_string().contains("1 of 500"), "{e}");
    }

    #[test]
    fn a_malformed_line_fails_with_its_line_number() {
        // A skipped record is a truncated trace, which is the thing FR-055e refuses.
        let dir = std::env::temp_dir().join(format!("certus-trace-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.jsonl");
        std::fs::write(&file, "{\"not\": \"an invocation\"}\n").unwrap();
        let e = read_invocations(&file).expect_err("must fail");
        assert!(e.to_string().contains(":1:"), "{e}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
