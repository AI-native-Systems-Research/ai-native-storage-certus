//! Integration tests driven by the committed in-repo ShareGPT trace.
//!
//! The trace lives at `benchmarks/kv-offload-replay/traces/sharegpt/`, two
//! levels up from this crate. Working-set facts used below (452 key-bearing
//! ops, 4555 key references, 442 distinct keys) are stable properties of that
//! committed file.

use std::path::{Path, PathBuf};

use component_core::query_interface;
use interfaces::IEvictionPolicy;

use eviction_replay_benchmark::replay::{self, Trace};
use eviction_replay_benchmark::sim::{simulate, SimStats};

const OPS: usize = 452;
const KEY_REFS: u64 = 4555;
const DISTINCT: usize = 442;

fn trace_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../benchmarks/kv-offload-replay/traces/sharegpt/199-prompts.mgr.jsonl")
}

fn load() -> Trace {
    replay::load(&trace_path()).expect("load committed in-repo trace")
}

fn run_lru(trace: &Trace, cache_size: usize) -> SimStats {
    let comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
    let ep = query_interface!(comp, IEvictionPolicy).unwrap();
    simulate(&*ep, trace, cache_size)
}

fn run_session_lists(trace: &Trace, cache_size: usize) -> SimStats {
    let comp = eviction_policy_session_lists::EvictionPolicySessionListsComponent::new_default();
    let ep = query_interface!(comp, IEvictionPolicy).unwrap();
    simulate(&*ep, trace, cache_size)
}

#[test]
fn trace_loads_with_expected_shape() {
    let t = load();
    assert_eq!(t.ops.len(), OPS, "key-bearing operation count");
    assert_eq!(t.total_key_refs as u64, KEY_REFS, "total key references");
    assert_eq!(t.distinct_keys, DISTINCT, "working-set size");
}

/// With a cache at least as large as the working set, nothing is ever evicted,
/// so every key after its first reference is a hit — an exact, policy-agnostic
/// count. Both policies must agree.
#[test]
fn exact_hits_when_cache_holds_working_set() {
    let t = load();
    let expected_hits = KEY_REFS - DISTINCT as u64; // 4113
    for stats in [run_lru(&t, DISTINCT), run_session_lists(&t, DISTINCT)] {
        assert_eq!(stats.accesses, KEY_REFS);
        assert_eq!(stats.evictions, 0, "no eviction at working-set size");
        assert_eq!(stats.insertions, DISTINCT as u64);
        assert_eq!(stats.misses, DISTINCT as u64);
        assert_eq!(stats.hits, expected_hits);
        assert_eq!(stats.resident, DISTINCT);
    }
}

/// LRU has the stack (inclusion) property: a larger cache never yields fewer
/// hits. Sizes are below the working set so eviction actually happens.
#[test]
fn lru_hit_count_is_monotonic_in_cache_size() {
    let t = load();
    let sizes = [16usize, 32, 64, 128, 256];
    let mut prev = 0u64;
    for w in sizes {
        let hits = run_lru(&t, w).hits;
        assert!(
            hits >= prev,
            "LRU hits must not decrease as cache grows: size {w} gave {hits}, previous {prev}"
        );
        prev = hits;
    }
}

/// Core bookkeeping invariants hold for both policies under eviction pressure.
#[test]
fn simulation_invariants_hold_under_pressure() {
    let t = load();
    let cache_size = 64;
    for stats in [run_lru(&t, cache_size), run_session_lists(&t, cache_size)] {
        assert_eq!(stats.accesses, KEY_REFS);
        assert_eq!(
            stats.hits + stats.misses,
            stats.accesses,
            "hits + misses == accesses"
        );
        assert_eq!(
            stats.misses, stats.insertions,
            "every miss inserts exactly once"
        );
        assert!(stats.insertions >= stats.evictions);
        assert_eq!(
            stats.insertions - stats.evictions,
            stats.resident as u64,
            "insertions - evictions == resident"
        );
        assert!(
            stats.resident <= cache_size,
            "resident set never exceeds capacity"
        );
        assert!(
            stats.evictions > 0,
            "a 64-block cache must evict on this trace"
        );
        // Effectiveness metric is well-formed.
        let hr = stats.hit_rate();
        assert!((0.0..=1.0).contains(&hr));
    }
}

/// The performance metrics are actually populated: the hot-path operations are
/// exercised and their means are finite, non-negative numbers.
#[test]
fn latency_metrics_are_recorded() {
    let t = load();
    for stats in [run_lru(&t, 64), run_session_lists(&t, 64)] {
        assert_eq!(stats.touch_calls, stats.hits, "one touch per hit");
        assert!(stats.evict_calls > 0, "eviction path exercised");
        assert!(stats.track_calls > 0, "track path exercised");
        for mean in [
            stats.mean_touch_ns(),
            stats.mean_evict_ns(),
            stats.mean_track_ns(),
        ] {
            assert!(
                mean.is_finite() && mean >= 0.0,
                "mean latency is a valid number"
            );
        }
    }
}
