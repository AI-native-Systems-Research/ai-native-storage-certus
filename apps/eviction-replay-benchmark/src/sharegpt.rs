//! Loading a ShareGPT conversation trace and converting it to cache block
//! access operations.
//!
//! A ShareGPT file is a JSON array of conversations:
//! ```json
//! [{"id": "abc", "conversations": [
//!     {"from": "human", "value": "..."},
//!     {"from": "gpt",   "value": "..."},
//!     ...
//! ]}, ...]
//! ```
//!
//! Each human/gpt turn pair becomes one cache access operation. The text is
//! accumulated across turns (modeling a growing KV-cache prefix), divided into
//! fixed-size character blocks, and hashed to produce [`CacheKey`]s. Identical
//! text produces identical block ids, so shared prefixes across conversations
//! naturally share cache keys.
//!
//! [`CacheKey`]: interfaces::CacheKey

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufReader};
use std::path::Path;

use interfaces::{CacheKey, SessionId};
use serde::Deserialize;

use crate::replay::{Op, Trace};

const DEFAULT_BLOCK_CHARS: usize = 64;

#[derive(Debug, Deserialize)]
struct Conversation {
    id: String,
    conversations: Vec<Turn>,
}

#[derive(Debug, Deserialize)]
struct Turn {
    #[allow(dead_code)]
    from: String,
    value: String,
}

fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in data {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn text_to_block_keys(text: &str, block_chars: usize) -> Vec<CacheKey> {
    let mut keys = Vec::new();
    let mut chunk = String::new();
    let mut chars_in_chunk = 0;
    for ch in text.chars() {
        chunk.push(ch);
        chars_in_chunk += 1;
        if chars_in_chunk == block_chars {
            keys.push(fnv1a_64(chunk.as_bytes()));
            chunk.clear();
            chars_in_chunk = 0;
        }
    }
    if !chunk.is_empty() {
        keys.push(fnv1a_64(chunk.as_bytes()));
    }
    keys
}

/// Load a ShareGPT-format JSON file and convert it to a [`Trace`].
///
/// Each human/gpt turn pair produces one [`Op`] whose keys are the cumulative
/// prefix blocks up to that turn (modeling a growing KV-cache). `block_chars`
/// controls how many characters map to one cache block (default 64, roughly
/// 16 tokens at ~4 chars/token).
pub fn load(path: &Path, block_chars: Option<usize>, max_conversations: Option<usize>) -> io::Result<Trace> {
    let block_chars = block_chars.unwrap_or(DEFAULT_BLOCK_CHARS);
    assert!(block_chars >= 1, "block_chars must be >= 1");

    let reader = BufReader::new(File::open(path)?);
    let convs: Vec<Conversation> = serde_json::from_reader(reader).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: ShareGPT parse error: {e}", path.display()),
        )
    })?;

    let limit = max_conversations.unwrap_or(convs.len());
    let mut ops = Vec::new();
    let mut total_key_refs = 0usize;
    let mut distinct: HashSet<CacheKey> = HashSet::new();

    for conv in convs.iter().take(limit) {
        let session_id = fnv1a_64(conv.id.as_bytes()) as SessionId;
        let turns = &conv.conversations;
        let mut cumulative = String::new();

        let pairs = turns.len() / 2;
        for pair in 0..pairs {
            let human = &turns[pair * 2];
            let gpt = &turns[pair * 2 + 1];

            cumulative.push_str(&human.value);

            let keys = text_to_block_keys(&cumulative, block_chars);
            for &k in &keys {
                distinct.insert(k);
            }
            total_key_refs += keys.len();

            ops.push(Op {
                method: "prefill".into(),
                keys,
                session_id,
            });

            cumulative.push_str(&gpt.value);
        }
    }

    Ok(Trace {
        ops,
        distinct_keys: distinct.len(),
        total_key_refs,
    })
}
