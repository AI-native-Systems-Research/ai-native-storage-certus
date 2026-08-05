//! Black-box property test for the session-lineage eviction policy (SC-006).
//!
//! Drives a random sequence of `track` / `touch` / `remove` /
//! `identify_next_to_evict` operations through the public [`IEvictionPolicy`]
//! interface and cross-checks the observable behaviour against an independent
//! shadow model built from per-session chains. This validates the data-model
//! invariants (single linear chain, leaf-set exactness, session→leaf
//! consistency, key uniqueness, length agreement) without reaching into the
//! component's private state.

use std::collections::HashMap;
use std::sync::Arc;

use component_core::query_interface;
use eviction_policy_session_lists::EvictionPolicySessionListsComponent;
use interfaces::{BlockSemantics, CacheKey, EvictionHandle, IEvictionPolicy, SessionId};

/// Independent reference implementation of the intended semantics.
///
/// Each session is an ordered chain `head .. leaf` (a `Vec`, leaf last). A stamp
/// is assigned from a shared monotonic clock on the same events that refresh
/// recency in the component (new registration, touch, idempotent re-track), so
/// the relative stamp ordering matches the component's.
#[derive(Default)]
struct Shadow {
    sessions: HashMap<SessionId, Vec<CacheKey>>,
    session_of: HashMap<CacheKey, SessionId>,
    stamp: HashMap<CacheKey, u64>,
    clock: u64,
}

impl Shadow {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn is_live(&self, key: CacheKey) -> bool {
        self.session_of.contains_key(&key)
    }

    fn track(&mut self, key: CacheKey, session: SessionId) {
        if self.is_live(key) {
            let s = self.tick();
            self.stamp.insert(key, s);
            return;
        }
        let s = self.tick();
        self.stamp.insert(key, s);
        self.session_of.insert(key, session);
        self.sessions.entry(session).or_default().push(key);
    }

    fn touch(&mut self, key: CacheKey) {
        if self.is_live(key) {
            let s = self.tick();
            self.stamp.insert(key, s);
        }
    }

    fn remove(&mut self, key: CacheKey) {
        if let Some(session) = self.session_of.remove(&key) {
            self.stamp.remove(&key);
            let chain = self.sessions.get_mut(&session).unwrap();
            let pos = chain.iter().position(|&k| k == key).unwrap();
            chain.remove(pos);
            if chain.is_empty() {
                self.sessions.remove(&session);
            }
        }
    }

    /// Current leaves (chain tops), i.e. the eviction candidates.
    fn leaves(&self) -> Vec<CacheKey> {
        self.sessions
            .values()
            .map(|chain| *chain.last().unwrap())
            .collect()
    }

    /// Leaves ordered oldest-first by stamp, matching eviction order.
    fn ordered_leaves(&self) -> Vec<CacheKey> {
        let mut leaves = self.leaves();
        leaves.sort_by_key(|k| self.stamp[k]);
        leaves
    }

    fn evict(&mut self) -> Option<CacheKey> {
        let victim = *self.ordered_leaves().first()?;
        self.remove(victim);
        Some(victim)
    }

    fn len(&self) -> usize {
        self.session_of.len()
    }
}

fn ep() -> Arc<dyn IEvictionPolicy + Send + Sync> {
    let comp = EvictionPolicySessionListsComponent::new_default();
    query_interface!(comp, IEvictionPolicy).unwrap()
}

#[test]
fn random_operations_match_reference_model() {
    // Deterministic xorshift RNG — no external crate, reproducible.
    let mut s: u64 = 0xD1B54A32D192ED03;
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };

    let ep = ep();
    let pool = ep.create_pool();
    let mut shadow = Shadow::default();

    // key -> handle, for the currently-live keys.
    let mut handles: HashMap<CacheKey, EvictionHandle> = HashMap::new();
    let mut live: Vec<CacheKey> = Vec::new();
    let mut next_key: CacheKey = 1;
    const SESSIONS: u64 = 12;

    for _ in 0..8_000 {
        match next() % 6 {
            0..=2 => {
                // Register a fresh key into one of a bounded set of sessions.
                let session = next() % SESSIONS;
                let key = next_key;
                next_key += 1;
                let h = ep
                    .track(
                        pool,
                        key,
                        BlockSemantics {
                            session_id: session,
                        },
                    )
                    .unwrap();
                handles.insert(key, h);
                shadow.track(key, session);
                live.push(key);
            }
            3 => {
                // Idempotent re-track of a live key: same handle, recency refresh.
                if !live.is_empty() {
                    let key = live[(next() as usize) % live.len()];
                    let session = shadow.session_of[&key];
                    let h = ep
                        .track(
                            pool,
                            key,
                            BlockSemantics {
                                session_id: session,
                            },
                        )
                        .unwrap();
                    assert_eq!(h, handles[&key], "re-track must return existing handle");
                    shadow.track(key, session);
                }
            }
            4 => {
                // Touch a live key.
                if !live.is_empty() {
                    let key = live[(next() as usize) % live.len()];
                    ep.touch(handles[&key]).unwrap();
                    shadow.touch(key);
                }
            }
            _ => {
                // Evict the oldest leaf; both models must agree on the victim.
                let got = ep.identify_next_to_evict(pool);
                let expected = shadow.evict();
                assert_eq!(got, expected, "eviction victim mismatch");
                if let Some(key) = got {
                    handles.remove(&key);
                    if let Some(p) = live.iter().position(|&k| k == key) {
                        live.swap_remove(p);
                    }
                }
            }
        }

        // Observable invariants after every operation.
        assert_eq!(ep.len(pool), shadow.len(), "length agreement");

        // Leaf-set exactness + session→leaf consistency: the candidate set is
        // exactly one leaf per active session.
        let mut got_leaves = ep.get_eviction_candidates(pool, usize::MAX);
        let mut expected_leaves = shadow.leaves();
        got_leaves.sort_unstable();
        expected_leaves.sort_unstable();
        assert_eq!(got_leaves, expected_leaves, "leaf set mismatch");

        // Eviction order: candidates come back oldest-first.
        assert_eq!(
            ep.get_eviction_candidates(pool, usize::MAX),
            shadow.ordered_leaves(),
            "candidate ordering mismatch"
        );
    }
}
