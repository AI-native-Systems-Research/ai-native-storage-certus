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
//! The workload is a *manager* replay trace (`*.mgr.jsonl`): one JSON object
//! per line with a `method` (`touch` / `lookup` / `prepare_store` /
//! `complete_store`) and a `keys` array of block hashes. See [`replay`] for the
//! parsing and key-interning details and [`sim`] for the cache simulator.
//!
//! [`IEvictionPolicy`]: interfaces::IEvictionPolicy
//! [`IEvictionPolicy::touch`]: interfaces::IEvictionPolicy::touch
//! [`IEvictionPolicy::identify_next_to_evict`]: interfaces::IEvictionPolicy::identify_next_to_evict
//! [`IEvictionPolicy::track`]: interfaces::IEvictionPolicy::track

pub mod replay;
pub mod sim;
