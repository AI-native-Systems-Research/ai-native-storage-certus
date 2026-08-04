//! Loading and normalizing a manager replay trace (`*.mgr.jsonl`).
//!
//! Each line is a JSON object such as
//! `{"ts": 2.45, "method": "lookup", "keys": ["41176c…", "594304…"]}`.
//! Only `method` and `keys` are used; `ts` and `success` are ignored.
//!
//! Block hashes are opaque 64-hex-char SHA-256 strings, but [`IEvictionPolicy`]
//! keys are `u64` ([`CacheKey`]). We *intern* each distinct hash to a dense,
//! sequential `u64` so the mapping is collision-free (parsing a 16-hex prefix
//! could alias) and stable within a run.
//!
//! Each operation is also assigned a `session_id`: the interned id of the
//! operation's **first** block hash. In prefix-cached LLM serving the first
//! block identifies the shared conversation root, so every request that extends
//! the same prefix shares a `session_id`. A lineage-aware policy (e.g.
//! `eviction-policy-session-lists`) uses this to protect a conversation's
//! shared prefix from eviction; recency-only policies ignore it.
//!
//! [`IEvictionPolicy`]: interfaces::IEvictionPolicy

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use interfaces::{CacheKey, SessionId};
use serde::Deserialize;

/// Raw JSON shape of one trace line. Unknown fields (`ts`, `success`) are
/// ignored by serde's default behaviour.
#[derive(Debug, Deserialize)]
struct RawRecord {
    #[serde(default)]
    method: String,
    #[serde(default)]
    keys: Vec<String>,
}

/// One replayed manager operation, with block hashes interned to [`CacheKey`]s
/// and a derived [`SessionId`].
#[derive(Debug, Clone)]
pub struct Op {
    /// Trace method (`touch` / `lookup` / `prepare_store` / `complete_store`).
    /// Retained for reporting; the simulator treats every listed key as an
    /// access regardless of method.
    pub method: String,
    /// Block keys referenced by this operation, in trace (prefix) order.
    pub keys: Vec<CacheKey>,
    /// Session id for lineage-aware policies: the interned id of `keys[0]`.
    pub session_id: SessionId,
}

/// A parsed, normalized trace ready to drive [`crate::sim::simulate`].
#[derive(Debug, Clone, Default)]
pub struct Trace {
    /// Operations in trace order. Operations with no keys are dropped on load.
    pub ops: Vec<Op>,
    /// Number of distinct block keys across the whole trace (the working-set
    /// size). A cache of this size or larger never evicts.
    pub distinct_keys: usize,
    /// Total key references across all operations. Equals the total number of
    /// cache accesses the simulator will perform.
    pub total_key_refs: usize,
}

/// Assigns a dense, sequential [`CacheKey`] to each distinct block hash.
#[derive(Default)]
struct Interner {
    map: HashMap<String, CacheKey>,
    next: CacheKey,
}

impl Interner {
    fn intern(&mut self, hex: &str) -> CacheKey {
        if let Some(&k) = self.map.get(hex) {
            return k;
        }
        let k = self.next;
        self.next += 1;
        self.map.insert(hex.to_string(), k);
        k
    }
}

/// Load and normalize a `*.mgr.jsonl` trace from `path`.
///
/// Lines that are blank or have an empty `keys` array (e.g. the trace's
/// `touch []` no-ops) are skipped. Returns an error if the file cannot be read
/// or a line is not valid JSON in the expected shape.
pub fn load(path: &Path) -> io::Result<Trace> {
    let reader = BufReader::new(File::open(path)?);
    let mut interner = Interner::default();
    let mut ops = Vec::new();
    let mut total_key_refs = 0usize;

    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let raw: RawRecord = serde_json::from_str(line).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}:{}: malformed trace line: {e}",
                    path.display(),
                    lineno + 1
                ),
            )
        })?;
        if raw.keys.is_empty() {
            continue;
        }
        let keys: Vec<CacheKey> = raw.keys.iter().map(|h| interner.intern(h)).collect();
        let session_id = keys[0];
        total_key_refs += keys.len();
        ops.push(Op {
            method: raw.method,
            keys,
            session_id,
        });
    }

    Ok(Trace {
        ops,
        distinct_keys: interner.map.len(),
        total_key_refs,
    })
}
