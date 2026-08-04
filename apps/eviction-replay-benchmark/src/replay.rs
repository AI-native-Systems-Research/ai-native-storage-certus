//! Loading and normalizing a Qwen-Bailian anonymized usage trace (JSONL).
//!
//! Each line is one request, e.g.
//! `{"chat_id": 159, "parent_chat_id": 55, "timestamp": 61.1, "turn": 2,
//!   "type": "text", "hash_ids": [1089, 1090, 6326, …]}`.
//!
//! * `hash_ids` are already globally-shared, densely-remapped 16-token block
//!   ids: an identical integer means an identical cached block (shared prefixes
//!   across requests reuse the same ids). They are therefore used **directly**
//!   as [`CacheKey`]s — no hashing or interning needed.
//! * A request's `session_id` is its **conversation root**. `parent_chat_id`
//!   links a turn to the previous one (`-1` marks a root request); following it
//!   transitively groups every turn of one conversation under a single
//!   `session_id`, so the block chain carries the full multi-turn lineage.
//! * `type` (text / search / image / file / thinking / …) is retained as the
//!   op's `method` for reporting; the simulator treats every listed key as an
//!   access regardless.
//! * `timestamp`, `turn`, `input_length`, and `output_length` are ignored.
//!
//! A lineage-aware policy (e.g. `eviction-policy-session-lists`) uses the
//! `session_id` to protect a conversation's shared prefix from eviction;
//! recency-only policies ignore it.
//!
//! [`IEvictionPolicy`]: interfaces::IEvictionPolicy
//! [`CacheKey`]: interfaces::CacheKey

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use interfaces::{CacheKey, SessionId};
use serde::Deserialize;

/// `parent_chat_id` default for records that omit it: `-1` (a root request).
fn root_parent() -> i64 {
    -1
}

/// Raw JSON shape of one Qwen-Bailian trace line. Unused fields (`timestamp`,
/// `turn`, `input_length`, `output_length`) are ignored by serde's default
/// behaviour.
#[derive(Debug, Deserialize)]
struct RawRecord {
    #[serde(default)]
    chat_id: i64,
    #[serde(default = "root_parent")]
    parent_chat_id: i64,
    #[serde(default)]
    hash_ids: Vec<CacheKey>,
    #[serde(default, rename = "type")]
    request_type: String,
}

/// One replayed request, with block ids as [`CacheKey`]s and a derived
/// [`SessionId`].
#[derive(Debug, Clone)]
pub struct Op {
    /// Request `type` (`text` / `search` / `image` / `file` / `thinking` / …),
    /// retained for reporting. The simulator treats every listed key as an
    /// access regardless of type.
    pub method: String,
    /// Block ids referenced by this request, in prefix order.
    pub keys: Vec<CacheKey>,
    /// Session id for lineage-aware policies: the conversation root reached by
    /// following `parent_chat_id`.
    pub session_id: SessionId,
}

/// A parsed, normalized trace ready to drive [`crate::sim::simulate`].
#[derive(Debug, Clone, Default)]
pub struct Trace {
    /// Requests in trace order. Requests with no `hash_ids` are dropped on load.
    pub ops: Vec<Op>,
    /// Number of distinct block ids across the whole trace (the working-set
    /// size). A cache of this size or larger never evicts.
    pub distinct_keys: usize,
    /// Total block references across all requests. Equals the total number of
    /// cache accesses the simulator will perform.
    pub total_key_refs: usize,
}

/// Load and normalize a Qwen-Bailian JSONL trace from `path`.
///
/// Lines that are blank or carry an empty `hash_ids` array (e.g. image/file
/// requests with no cached token blocks) are skipped. Returns an error if the
/// file cannot be read or a line is not valid JSON in the expected shape.
pub fn load(path: &Path) -> io::Result<Trace> {
    let reader = BufReader::new(File::open(path)?);
    let mut ops = Vec::new();
    let mut total_key_refs = 0usize;
    let mut distinct: HashSet<CacheKey> = HashSet::new();
    // chat_id -> conversation root, so a turn's root can be resolved from its
    // parent in a single pass (parents always precede children in trace order).
    let mut root_of: HashMap<i64, i64> = HashMap::new();

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
        if raw.hash_ids.is_empty() {
            continue;
        }

        // Resolve the conversation root: a root request (`parent_chat_id < 0`)
        // is its own root; otherwise inherit the parent's resolved root, falling
        // back to the parent id itself if the parent was not seen (defensive).
        let root = if raw.parent_chat_id < 0 {
            raw.chat_id
        } else {
            *root_of
                .get(&raw.parent_chat_id)
                .unwrap_or(&raw.parent_chat_id)
        };
        root_of.insert(raw.chat_id, root);

        let keys = raw.hash_ids;
        for &k in &keys {
            distinct.insert(k);
        }
        total_key_refs += keys.len();
        let method = if raw.request_type.is_empty() {
            "request".to_string()
        } else {
            raw.request_type
        };
        ops.push(Op {
            method,
            keys,
            session_id: root as SessionId,
        });
    }

    Ok(Trace {
        ops,
        distinct_keys: distinct.len(),
        total_key_refs,
    })
}
