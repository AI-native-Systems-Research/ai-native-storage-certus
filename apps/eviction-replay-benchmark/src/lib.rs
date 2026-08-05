//! Replay-driven cache-hit and latency harness for any [`IEvictionPolicy`]
//! implementation.
//!
//! The benchmark answers two questions about an eviction policy under a real,
//! captured workload:
//!
//! 1. **Effectiveness** — for a *fixed cache size*, how many of the trace's key
//!    references are cache **hits**? A policy that keeps the *important* (i.e.
//!    frequently re-referenced) blocks resident longest converts more repeat
//!    references into hits, so a higher hit rate at the same cache size is the
//!    direct measure of "keeping the most important block around longest".
//! 2. **Performance** — what is the mean per-call latency of the hot-path
//!    operations [`IEvictionPolicy::touch`] and
//!    [`IEvictionPolicy::identify_next_to_evict`] (plus
//!    [`IEvictionPolicy::track`] for context)?
//!
//! The workload is a [Qwen-Bailian anonymized usage trace][qwen]: one JSON
//! object per line describing a request, with a `hash_ids` array of shared
//! 16-token block ids and `chat_id` / `parent_chat_id` conversation lineage.
//! Traces are named by short id (`chat` / `api` / `thinking` / `coder`) and
//! fetched on demand to `/tmp` — see [`dataset`]. See [`replay`] for parsing and
//! conversation-root derivation, and [`sim`] for the cache simulator.
//!
//! [qwen]: https://github.com/alibaba-edu/qwen-bailian-usagetraces-anon
//! [`IEvictionPolicy`]: interfaces::IEvictionPolicy
//! [`IEvictionPolicy::touch`]: interfaces::IEvictionPolicy::touch
//! [`IEvictionPolicy::identify_next_to_evict`]: interfaces::IEvictionPolicy::identify_next_to_evict
//! [`IEvictionPolicy::track`]: interfaces::IEvictionPolicy::track

pub mod dataset;
pub mod replay;
pub mod sim;
