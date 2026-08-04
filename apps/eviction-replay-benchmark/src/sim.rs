//! A fixed-capacity cache simulator driven by an [`IEvictionPolicy`].
//!
//! The eviction policy component only *decides what to evict*; it does not
//! itself hold a cache. This simulator layers a bounded cache of `cache_size`
//! blocks on top of the policy and replays a [`Trace`] against it:
//!
//! * Every key reference in the trace is one **access**.
//! * A reference to a resident key is a **hit**; we call
//!   [`IEvictionPolicy::touch`] to refresh its ranking.
//! * A reference to an absent key is a **miss**; if the cache is full we call
//!   [`IEvictionPolicy::identify_next_to_evict`] (which removes and returns the
//!   victim) until there is room, then [`IEvictionPolicy::track`] the new key.
//!
//! The policy's internal size and the simulator's resident set stay in lock-step
//! (one `track` per insert, one `identify_next_to_evict` per eviction), so
//! `hits + misses == accesses` and `insertions - evictions == resident`.
//!
//! [`IEvictionPolicy`]: interfaces::IEvictionPolicy
//! [`IEvictionPolicy::touch`]: interfaces::IEvictionPolicy::touch
//! [`IEvictionPolicy::identify_next_to_evict`]: interfaces::IEvictionPolicy::identify_next_to_evict
//! [`IEvictionPolicy::track`]: interfaces::IEvictionPolicy::track

use std::collections::HashMap;
use std::time::{Duration, Instant};

use interfaces::{BlockSemantics, CacheKey, EvictionHandle, IEvictionPolicy};

use crate::replay::Trace;

/// Outcome of replaying a trace through one policy at one cache size.
///
/// Hit-rate is the *effectiveness* metric; the `mean_*_ns` accessors are the
/// *performance* metrics.
#[derive(Debug, Clone, Default)]
pub struct SimStats {
    /// Cache capacity in blocks used for this run.
    pub cache_size: usize,
    /// Total key references replayed (`== trace.total_key_refs`).
    pub accesses: u64,
    /// References that found the key resident.
    pub hits: u64,
    /// References that did not find the key resident.
    pub misses: u64,
    /// Keys inserted via `track` (`== misses`).
    pub insertions: u64,
    /// Keys removed via `identify_next_to_evict`.
    pub evictions: u64,
    /// Keys resident at the end of the replay.
    pub resident: usize,
    /// Wall-clock time for the whole replay (includes bookkeeping and the
    /// per-call timing overhead; use the `mean_*_ns` accessors for clean
    /// per-operation latency).
    pub wall: Duration,

    /// Number of `touch` calls timed (`== hits`).
    pub touch_calls: u64,
    /// Sum of `touch` call latencies, nanoseconds.
    pub touch_nanos: u128,
    /// Number of `identify_next_to_evict` calls timed.
    pub evict_calls: u64,
    /// Sum of `identify_next_to_evict` call latencies, nanoseconds.
    pub evict_nanos: u128,
    /// Number of `track` calls timed (`== insertions`).
    pub track_calls: u64,
    /// Sum of `track` call latencies, nanoseconds.
    pub track_nanos: u128,
}

fn mean(nanos: u128, calls: u64) -> f64 {
    if calls == 0 {
        0.0
    } else {
        nanos as f64 / calls as f64
    }
}

impl SimStats {
    /// Fraction of accesses that were hits, in `[0, 1]`.
    pub fn hit_rate(&self) -> f64 {
        if self.accesses == 0 {
            0.0
        } else {
            self.hits as f64 / self.accesses as f64
        }
    }

    /// Mean [`IEvictionPolicy::touch`] latency in nanoseconds.
    ///
    /// [`IEvictionPolicy::touch`]: interfaces::IEvictionPolicy::touch
    pub fn mean_touch_ns(&self) -> f64 {
        mean(self.touch_nanos, self.touch_calls)
    }

    /// Mean [`IEvictionPolicy::identify_next_to_evict`] latency in nanoseconds.
    ///
    /// [`IEvictionPolicy::identify_next_to_evict`]: interfaces::IEvictionPolicy::identify_next_to_evict
    pub fn mean_evict_ns(&self) -> f64 {
        mean(self.evict_nanos, self.evict_calls)
    }

    /// Mean [`IEvictionPolicy::track`] latency in nanoseconds.
    ///
    /// [`IEvictionPolicy::track`]: interfaces::IEvictionPolicy::track
    pub fn mean_track_ns(&self) -> f64 {
        mean(self.track_nanos, self.track_calls)
    }

    /// Overall replay throughput in accesses per second.
    pub fn ops_per_sec(&self) -> f64 {
        let secs = self.wall.as_secs_f64();
        if secs == 0.0 {
            0.0
        } else {
            self.accesses as f64 / secs
        }
    }
}

/// Replay `trace` through `ep` with a cache holding at most `cache_size` blocks.
///
/// A fresh pool is created on `ep`, so the same component instance may be reused
/// across calls. Panics if `cache_size == 0`.
///
/// ```
/// use component_core::query_interface;
/// use interfaces::IEvictionPolicy;
/// use eviction_replay_benchmark::replay::{Op, Trace};
/// use eviction_replay_benchmark::sim::simulate;
///
/// // A tiny two-op trace: store three blocks, then reference them again.
/// let trace = Trace {
///     ops: vec![
///         Op { method: "store".into(), keys: vec![1, 2, 3], session_id: 1 },
///         Op { method: "lookup".into(), keys: vec![1, 2, 3], session_id: 1 },
///     ],
///     distinct_keys: 3,
///     total_key_refs: 6,
/// };
///
/// let comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
/// let ep = query_interface!(comp, IEvictionPolicy).unwrap();
/// let stats = simulate(&*ep, &trace, 8); // capacity comfortably fits all keys
///
/// assert_eq!(stats.accesses, 6);
/// assert_eq!(stats.hits, 3);       // second pass is all hits
/// assert_eq!(stats.evictions, 0);  // nothing evicted at this size
/// ```
pub fn simulate(ep: &dyn IEvictionPolicy, trace: &Trace, cache_size: usize) -> SimStats {
    assert!(cache_size >= 1, "cache_size must be >= 1");

    let pool = ep.create_pool();
    let mut resident: HashMap<CacheKey, EvictionHandle> = HashMap::with_capacity(cache_size + 1);
    let mut s = SimStats {
        cache_size,
        ..Default::default()
    };

    let wall_start = Instant::now();
    for op in &trace.ops {
        for &key in &op.keys {
            s.accesses += 1;

            if let Some(&handle) = resident.get(&key) {
                // Hit: refresh the entry's eviction ranking.
                s.hits += 1;
                let t = Instant::now();
                let _ = ep.touch(handle);
                s.touch_nanos += t.elapsed().as_nanos();
                s.touch_calls += 1;
            } else {
                // Miss: make room, then admit the new key.
                s.misses += 1;
                while resident.len() >= cache_size {
                    let t = Instant::now();
                    let victim = ep.identify_next_to_evict(pool);
                    s.evict_nanos += t.elapsed().as_nanos();
                    s.evict_calls += 1;
                    match victim {
                        Some(v) if resident.remove(&v).is_some() => s.evictions += 1,
                        // None (empty pool) or a victim we don't hold would loop
                        // forever; neither can happen while the resident set and
                        // the policy stay in lock-step, but guard defensively.
                        _ => break,
                    }
                }
                let t = Instant::now();
                let handle = ep
                    .track(
                        pool,
                        key,
                        BlockSemantics {
                            session_id: op.session_id,
                        },
                    )
                    .expect("track on a freshly created pool cannot fail");
                s.track_nanos += t.elapsed().as_nanos();
                s.track_calls += 1;
                resident.insert(key, handle);
                s.insertions += 1;
            }
        }
    }
    s.wall = wall_start.elapsed();
    s.resident = resident.len();
    s
}
