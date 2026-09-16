//! Loading a Weka conversation trace (JSONL, one conversation per line).
//!
//! Each line is a JSON object:
//! ```json
//! {
//!   "id": "002001296e8a...",
//!   "models": ["claude-opus-4-8"],
//!   "block_size": 64,
//!   "hash_id_scope": "local",
//!   "requests": [
//!     {"t": 0.0, "model": "claude-opus-4-8", "in": 640, "out": 20,
//!      "hash_ids": [0, 1, 2, ...], "type": "s", ...},
//!     ...
//!   ]
//! }
//! ```
//!
//! `hash_id_scope` is `"local"` (block IDs are per-conversation) or `"global"`
//! (block IDs are globally shared). When local, each block ID is remapped to a
//! globally unique [`CacheKey`] by hashing `(conversation_id, local_id)`.
//!
//! [`CacheKey`]: interfaces::CacheKey

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

use interfaces::{CacheKey, SessionId};
use serde::Deserialize;

use crate::replay::{Op, Trace};

#[derive(Debug, Deserialize)]
struct Conversation {
    id: String,
    #[serde(default)]
    hash_id_scope: String,
    requests: Vec<Request>,
}

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    hash_ids: Vec<u64>,
    #[serde(default, rename = "type")]
    request_type: String,
}

fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn remap_local_key(conv_id: &str, local_id: u64) -> CacheKey {
    let mut buf = conv_id.as_bytes().to_vec();
    buf.extend_from_slice(&local_id.to_le_bytes());
    fnv1a_64(&buf)
}

/// Load a Weka JSONL trace file and convert it to a [`Trace`].
///
/// Each line is one conversation; each request within a conversation becomes
/// one [`Op`]. When `hash_id_scope` is `"local"`, block IDs are remapped to
/// globally unique keys; when `"global"`, they are used directly.
pub fn load(path: &Path, max_conversations: Option<usize>) -> io::Result<Trace> {
    let reader = BufReader::new(File::open(path)?);
    let mut ops = Vec::new();
    let mut total_key_refs = 0usize;
    let mut distinct: HashSet<CacheKey> = HashSet::new();
    let mut conv_count = 0usize;

    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(limit) = max_conversations {
            if conv_count >= limit {
                break;
            }
        }

        let conv: Conversation = serde_json::from_str(line).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}:{}: malformed Weka trace line: {e}", path.display(), lineno + 1),
            )
        })?;

        let session_id = fnv1a_64(conv.id.as_bytes()) as SessionId;
        let is_local = conv.hash_id_scope != "global";

        for req in &conv.requests {
            if req.hash_ids.is_empty() {
                continue;
            }

            let keys: Vec<CacheKey> = if is_local {
                req.hash_ids
                    .iter()
                    .map(|&id| remap_local_key(&conv.id, id))
                    .collect()
            } else {
                req.hash_ids.clone()
            };

            for &k in &keys {
                distinct.insert(k);
            }
            total_key_refs += keys.len();

            let method = if req.request_type.is_empty() {
                "request".into()
            } else {
                req.request_type.clone()
            };

            ops.push(Op {
                method,
                keys,
                session_id,
            });
        }

        conv_count += 1;
    }

    Ok(Trace {
        ops,
        distinct_keys: distinct.len(),
        total_key_refs,
    })
}
