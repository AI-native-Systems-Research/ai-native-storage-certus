//! Offline tests for the Qwen-Bailian loader and the cache simulator.
//!
//! These build small synthetic traces (in-memory or a temp JSONL file) so they
//! run without network access; the real datasets are fetched on demand only by
//! the binary.

use std::fs;
use std::path::PathBuf;

use component_core::query_interface;
use interfaces::IEvictionPolicy;

use eviction_replay_benchmark::replay::{self, Op, Trace};
use eviction_replay_benchmark::sim::{simulate, SimStats};

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

fn write_tmp(tag: &str, contents: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("erb-test-{}-{}.jsonl", std::process::id(), tag));
    fs::write(&p, contents).expect("write temp trace");
    p
}

/// The loader parses the Qwen schema, uses `hash_ids` directly as keys, skips
/// empty-`hash_ids` records, and resolves conversation roots via
/// `parent_chat_id` so every turn of a conversation shares a `session_id`.
#[test]
fn loads_qwen_schema_and_resolves_conversation_root() {
    // chat 0: root turn 1. chat 1: turn 2, child of 0. chat 2: separate root.
    // chat 3: image request with no blocks -> dropped.
    let jsonl = concat!(
        r#"{"chat_id":0,"parent_chat_id":-1,"turn":1,"type":"text","hash_ids":[10,11,12]}"#,
        "\n",
        r#"{"chat_id":1,"parent_chat_id":0,"turn":2,"type":"text","hash_ids":[10,11,12,13,14]}"#,
        "\n",
        r#"{"chat_id":2,"parent_chat_id":-1,"turn":1,"type":"thinking","hash_ids":[20,21]}"#,
        "\n",
        r#"{"chat_id":3,"parent_chat_id":-1,"turn":1,"type":"image","hash_ids":[]}"#,
        "\n",
    );
    let path = write_tmp("root", jsonl);
    let t = replay::load(&path).expect("load synthetic qwen trace");
    let _ = fs::remove_file(&path);

    assert_eq!(t.ops.len(), 3, "empty-hash_ids record is dropped");
    assert_eq!(t.total_key_refs, 3 + 5 + 2);
    assert_eq!(t.distinct_keys, 7, "distinct block ids: 10..14, 20, 21");

    assert_eq!(
        t.ops[0].session_id, t.ops[1].session_id,
        "the two turns of one conversation share a session id"
    );
    assert_ne!(
        t.ops[0].session_id, t.ops[2].session_id,
        "a separate conversation has a distinct session id"
    );
    assert_eq!(t.ops[0].method, "text");
    assert_eq!(t.ops[2].method, "thinking");
}

/// With a cache at least as large as the working set nothing is ever evicted,
/// so every block after its first reference is a hit — an exact, policy-agnostic
/// count both policies must agree on.
#[test]
fn no_eviction_when_cache_holds_working_set() {
    let trace = Trace {
        ops: vec![
            Op {
                method: "text".into(),
                keys: vec![1, 2, 3],
                session_id: 1,
            },
            Op {
                method: "text".into(),
                keys: vec![1, 2, 3],
                session_id: 1,
            },
        ],
        distinct_keys: 3,
        total_key_refs: 6,
    };
    for stats in [run_lru(&trace, 3), run_session_lists(&trace, 3)] {
        assert_eq!(stats.accesses, 6);
        assert_eq!(stats.misses, 3);
        assert_eq!(stats.insertions, 3);
        assert_eq!(stats.evictions, 0, "no eviction at working-set size");
        assert_eq!(stats.hits, 3, "second pass is all hits");
        assert_eq!(stats.resident, 3);
    }
}

/// A trace of five interleaved conversations with rolling per-session locality,
/// small enough that a modest cache must evict.
fn synth_pressure_trace() -> Trace {
    let mut ops = Vec::new();
    let mut refs = 0usize;
    let mut distinct = std::collections::HashSet::new();
    for round in 0..20u64 {
        for session in 0..5u64 {
            let base = session * 10;
            let keys: Vec<u64> = (0..8u64).map(|i| base + ((round + i) % 10)).collect();
            for &k in &keys {
                distinct.insert(k);
            }
            refs += keys.len();
            ops.push(Op {
                method: "text".into(),
                keys,
                session_id: session,
            });
        }
    }
    Trace {
        ops,
        distinct_keys: distinct.len(),
        total_key_refs: refs,
    }
}

/// LRU has the stack (inclusion) property: a larger cache never yields fewer
/// hits. Sizes stay below the 50-block working set so eviction happens.
#[test]
fn lru_hit_count_is_monotonic_in_cache_size() {
    let t = synth_pressure_trace();
    let mut prev = 0u64;
    for w in [4usize, 8, 16, 32] {
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
    let t = synth_pressure_trace();
    let cache_size = 16;
    for stats in [run_lru(&t, cache_size), run_session_lists(&t, cache_size)] {
        assert_eq!(stats.accesses, t.total_key_refs as u64);
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
        assert!(stats.evictions > 0, "this trace must evict at size 16");
        assert!((0.0..=1.0).contains(&stats.hit_rate()));
    }
}

/// The performance metrics are actually populated: the hot-path operations are
/// exercised and their means are finite, non-negative numbers.
#[test]
fn latency_metrics_are_recorded() {
    let t = synth_pressure_trace();
    for stats in [run_lru(&t, 16), run_session_lists(&t, 16)] {
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
