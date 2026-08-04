//! Per-pool session-lineage data structure.
//!
//! Each [`Pool`] tracks cache blocks grouped into per-session linear chains
//! (stacks). Registration links a new block as the child of its session's
//! current leaf, so the chain records lineage: the block pushed immediately
//! before block `B` in session `S` is `B`'s parent. Only *leaves* (blocks with
//! no tracked child) are eligible for eviction, and the victim is the
//! globally oldest-accessed leaf across every session in the pool. This
//! protects heads and interior blocks that still have descendants, improving on
//! plain recency-LRU by exploiting lineage.
//!
//! The structure is an index-based arena (`Vec<Node>` + free list) mirroring the
//! approach used by `eviction-policy-lru::LruList`: handles are `u32` arena
//! indices, not pointers, so nodes survive `Vec` reallocation and there are no
//! self-referential borrows. A `BTreeSet<(stamp, index)>` over the current
//! leaves gives O(log L) oldest-leaf selection, where `L` is the number of
//! sessions (each non-empty session has exactly one leaf).

use std::collections::{BTreeSet, HashMap};

use interfaces::{CacheKey, SessionId};

/// A single tracked cache block occupying one arena slot.
pub(crate) struct Node {
    /// The cache key this node tracks.
    pub key: CacheKey,
    /// Owning session. Every block belongs to exactly one session.
    pub session: SessionId,
    /// Arena index of the parent block, or `None` if this node is a chain head.
    pub parent: Option<u32>,
    /// Arena index of the single child block, or `None` if this node is a leaf.
    pub child: Option<u32>,
    /// Logical timestamp of the most recent access.
    pub stamp: u64,
    /// Slot occupancy flag; `false` slots live in the free list.
    pub active: bool,
}

/// Per-pool eviction bookkeeping over a set of session chains.
#[derive(Default)]
pub(crate) struct Pool {
    /// Arena of tracked blocks; index == `EvictionHandle.index`.
    nodes: Vec<Node>,
    /// Recycled slot indices available for reuse.
    free: Vec<u32>,
    /// key -> node index, enforcing idempotent re-registration.
    by_key: HashMap<CacheKey, u32>,
    /// session id -> current leaf node index.
    sessions: HashMap<SessionId, u32>,
    /// `(stamp, index)` for every current leaf, ordered oldest-first.
    leaves: BTreeSet<(u64, u32)>,
    /// Monotonically increasing logical access counter.
    clock: u64,
    /// Count of active nodes.
    len: usize,
}

impl Pool {
    /// Advance the logical clock and return the new value. Each call yields a
    /// strictly increasing stamp, so `(stamp, index)` leaf ordering is total
    /// and tie-breaks are deterministic.
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Place `node` into a free slot (reusing the free list) or append it.
    fn alloc(&mut self, node: Node) -> u32 {
        if let Some(idx) = self.free.pop() {
            self.nodes[idx as usize] = node;
            idx
        } else {
            let idx = self.nodes.len() as u32;
            self.nodes.push(node);
            idx
        }
    }

    /// Return `true` if `index` refers to a live (active) node.
    fn is_active(&self, index: u32) -> bool {
        matches!(self.nodes.get(index as usize), Some(n) if n.active)
    }

    /// Register `key` for `session`, returning the node's arena index.
    ///
    /// Idempotent: re-registering a key already tracked in this pool refreshes
    /// its recency and returns the existing index without allocating a node or
    /// altering lineage. Otherwise a new node is linked as the child (new leaf)
    /// of the session's current leaf, or becomes a head+leaf for a new session.
    pub fn register(&mut self, key: CacheKey, session: SessionId) -> u32 {
        if let Some(&idx) = self.by_key.get(&key) {
            self.touch(idx);
            return idx;
        }

        let stamp = self.tick();
        let parent = self.sessions.get(&session).copied();
        let idx = self.alloc(Node {
            key,
            session,
            parent,
            child: None,
            stamp,
            active: true,
        });

        if let Some(p) = parent {
            // The previous leaf gains a child and is no longer a leaf.
            let pstamp = self.nodes[p as usize].stamp;
            self.leaves.remove(&(pstamp, p));
            self.nodes[p as usize].child = Some(idx);
        }

        self.sessions.insert(session, idx);
        self.leaves.insert((stamp, idx));
        self.by_key.insert(key, idx);
        self.len += 1;
        idx
    }

    /// Refresh the recency of node `index`. Returns `false` if the handle is
    /// invalid or already removed.
    pub fn touch(&mut self, index: u32) -> bool {
        if !self.is_active(index) {
            return false;
        }
        let i = index as usize;
        let old_stamp = self.nodes[i].stamp;
        let is_leaf = self.nodes[i].child.is_none();
        let new_stamp = self.tick();
        self.nodes[i].stamp = new_stamp;
        if is_leaf {
            self.leaves.remove(&(old_stamp, index));
            self.leaves.insert((new_stamp, index));
        }
        true
    }

    /// Splice node `idx` out of its chain and free its slot, returning its key.
    ///
    /// Relinks `child.parent <-> parent.child`. If the node was a leaf, its
    /// parent (if any) becomes the session's new leaf, otherwise the session is
    /// dropped. Does not touch recency.
    fn unlink(&mut self, idx: u32) -> CacheKey {
        let i = idx as usize;
        let parent = self.nodes[i].parent;
        let child = self.nodes[i].child;
        let session = self.nodes[i].session;
        let key = self.nodes[i].key;
        let stamp = self.nodes[i].stamp;
        let was_leaf = child.is_none();

        // Relink neighbours around the removed node.
        if let Some(c) = child {
            self.nodes[c as usize].parent = parent;
        }
        if let Some(p) = parent {
            self.nodes[p as usize].child = child;
        }

        if was_leaf {
            self.leaves.remove(&(stamp, idx));
            match parent {
                Some(p) => {
                    // Parent is now childless -> it becomes the session leaf.
                    let pstamp = self.nodes[p as usize].stamp;
                    self.leaves.insert((pstamp, p));
                    self.sessions.insert(session, p);
                }
                None => {
                    // Singleton chain fully removed.
                    self.sessions.remove(&session);
                }
            }
        }
        // Interior/head removal leaves the session's leaf downstream unchanged.

        self.by_key.remove(&key);
        self.nodes[i].active = false;
        self.nodes[i].parent = None;
        self.nodes[i].child = None;
        self.free.push(idx);
        self.len -= 1;
        key
    }

    /// Select, remove, and return the oldest-accessed leaf's key across all
    /// sessions in the pool, or `None` if empty. The evicted block is not
    /// recency-refreshed.
    pub fn evict_oldest(&mut self) -> Option<CacheKey> {
        let (_, idx) = *self.leaves.iter().next()?;
        Some(self.unlink(idx))
    }

    /// Return up to `n` oldest-leaf keys in eviction order, removing none.
    pub fn candidates(&self, n: usize) -> Vec<CacheKey> {
        self.leaves
            .iter()
            .take(n)
            .map(|&(_, idx)| self.nodes[idx as usize].key)
            .collect()
    }

    /// Stop tracking node `index`, re-splicing its chain. Returns `false` if the
    /// handle is invalid or already removed.
    pub fn remove(&mut self, index: u32) -> bool {
        if !self.is_active(index) {
            return false;
        }
        self.unlink(index);
        true
    }

    /// Number of active tracked nodes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Reset the pool to empty.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.free.clear();
        self.by_key.clear();
        self.sessions.clear();
        self.leaves.clear();
        self.clock = 0;
        self.len = 0;
    }

    /// Assert every data-model invariant holds. Test-only; used by the internal
    /// property test (SC-006).
    #[cfg(test)]
    pub(crate) fn check_invariants(&self) {
        use std::collections::HashSet;

        // Invariant 6: length agreement.
        let active_count = self.nodes.iter().filter(|n| n.active).count();
        assert_eq!(active_count, self.len, "len disagrees with active count");
        assert_eq!(
            active_count,
            self.nodes.len() - self.free.len(),
            "active count disagrees with arena occupancy"
        );

        // Invariant 5: key uniqueness.
        assert_eq!(
            self.by_key.len(),
            self.len,
            "by_key size disagrees with len"
        );
        for (k, &idx) in &self.by_key {
            let n = &self.nodes[idx as usize];
            assert!(n.active, "by_key points at inactive node");
            assert_eq!(n.key, *k, "by_key key mismatch");
        }

        // Invariant 2: leaf-set exactness.
        let leaf_indices: HashSet<u32> = self.leaves.iter().map(|&(_, i)| i).collect();
        assert_eq!(
            leaf_indices.len(),
            self.leaves.len(),
            "duplicate leaf index"
        );
        for (i, n) in self.nodes.iter().enumerate() {
            if n.active && n.child.is_none() {
                assert!(
                    self.leaves.contains(&(n.stamp, i as u32)),
                    "active leaf {i} missing from leaves set"
                );
            }
        }
        for &(stamp, idx) in &self.leaves {
            let n = &self.nodes[idx as usize];
            assert!(n.active, "leaves set references inactive node");
            assert!(n.child.is_none(), "leaves set references non-leaf");
            assert_eq!(n.stamp, stamp, "stale stamp in leaves set");
        }

        // Invariants 1 & 4: single linear chain, no orphans, no cycles.
        for (i, n) in self.nodes.iter().enumerate() {
            if !n.active {
                continue;
            }
            if let Some(p) = n.parent {
                let pn = &self.nodes[p as usize];
                assert!(pn.active, "parent of active node is inactive");
                assert_eq!(pn.child, Some(i as u32), "parent.child does not point back");
            }
            if let Some(c) = n.child {
                let cn = &self.nodes[c as usize];
                assert!(cn.active, "child of active node is inactive");
                assert_eq!(
                    cn.parent,
                    Some(i as u32),
                    "child.parent does not point back"
                );
            }
            let mut steps = 0usize;
            let mut cur = n.parent;
            while let Some(p) = cur {
                steps += 1;
                assert!(steps <= self.len, "cycle detected walking to head");
                cur = self.nodes[p as usize].parent;
            }
        }

        // Invariant 3: session -> leaf consistency.
        for (sid, &idx) in &self.sessions {
            let n = &self.nodes[idx as usize];
            assert!(n.active, "session points at inactive node");
            assert_eq!(n.session, *sid, "session id mismatch on leaf");
            assert!(n.child.is_none(), "session leaf has a child");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Look up the arena index of a live key (test helper).
    fn idx_of(pool: &Pool, key: CacheKey) -> u32 {
        *pool.by_key.get(&key).expect("key not tracked")
    }

    // ---- User Story 2: registration builds correct lineage --------------

    #[test]
    fn first_block_is_head_and_leaf() {
        let mut p = Pool::default();
        let i = p.register(10, 1);
        assert_eq!(p.nodes[i as usize].parent, None, "head has no parent");
        assert_eq!(p.nodes[i as usize].child, None, "sole block is a leaf");
        assert_eq!(p.candidates(4), vec![10]);
        p.check_invariants();
    }

    #[test]
    fn second_block_becomes_child_and_leaf() {
        let mut p = Pool::default();
        let a = p.register(10, 1);
        let b = p.register(20, 1);
        assert_eq!(p.nodes[b as usize].parent, Some(a), "B's parent is A");
        assert_eq!(p.nodes[a as usize].child, Some(b), "A's child is B");
        assert_eq!(p.nodes[b as usize].child, None, "B is the new leaf");
        // Only the leaf B is an eviction candidate, not head A.
        assert_eq!(p.candidates(4), vec![20]);
        p.check_invariants();
    }

    #[test]
    fn distinct_sessions_form_independent_chains() {
        let mut p = Pool::default();
        p.register(10, 1);
        p.register(11, 1);
        p.register(20, 2);
        // Each session has exactly one leaf; neither appears in the other's chain.
        let a1 = idx_of(&p, 10);
        let b1 = idx_of(&p, 11);
        let a2 = idx_of(&p, 20);
        assert_eq!(p.nodes[b1 as usize].parent, Some(a1));
        assert_eq!(p.nodes[a2 as usize].parent, None, "session 2 head");
        assert_ne!(p.nodes[a2 as usize].session, p.nodes[a1 as usize].session);
        let mut cands = p.candidates(8);
        cands.sort_unstable();
        assert_eq!(cands, vec![11, 20], "leaves are the two session tops");
        p.check_invariants();
    }

    #[test]
    fn reregister_is_idempotent_recency_refresh() {
        let mut p = Pool::default();
        let a = p.register(10, 1);
        p.register(20, 1);
        let before_len = p.len();
        // Re-register the head key: no new node, same handle, refreshed recency.
        let again = p.register(10, 1);
        assert_eq!(again, a, "same handle returned");
        assert_eq!(p.len(), before_len, "no new node allocated");
        // Lineage unchanged: A still head, its child still the leaf.
        assert_eq!(p.nodes[a as usize].parent, None);
        assert_eq!(p.nodes[a as usize].child, Some(idx_of(&p, 20)));
        p.check_invariants();
    }

    #[test]
    fn remove_interior_relinks_chain() {
        let mut p = Pool::default();
        let a = p.register(10, 1);
        let b = p.register(20, 1);
        let c = p.register(30, 1);
        assert!(p.remove(b), "interior removal succeeds");
        assert_eq!(p.nodes[c as usize].parent, Some(a), "C reparented to A");
        assert_eq!(p.nodes[a as usize].child, Some(c), "A's child is now C");
        assert_eq!(p.candidates(4), vec![30], "C remains the only leaf");
        p.check_invariants();
    }

    #[test]
    fn remove_invalid_handle_fails() {
        let mut p = Pool::default();
        p.register(10, 1);
        assert!(!p.remove(999), "out-of-range handle rejected");
        let a = idx_of(&p, 10);
        assert!(p.remove(a));
        assert!(!p.remove(a), "double remove rejected");
        p.check_invariants();
    }

    // ---- User Story 1: lineage-preserving victim selection --------------

    #[test]
    fn evicts_oldest_leaf_across_sessions() {
        let mut p = Pool::default();
        p.register(10, 1); // stamp 1 (leaf of session 1)
        p.register(20, 2); // stamp 2 (leaf of session 2)
                           // Session 1's leaf (key 10) is oldest -> evicted first.
        assert_eq!(p.evict_oldest(), Some(10));
        assert_eq!(p.evict_oldest(), Some(20));
        assert_eq!(p.evict_oldest(), None, "empty domain yields None");
        p.check_invariants();
    }

    #[test]
    fn eviction_promotes_parent_then_continues() {
        let mut p = Pool::default();
        let a = p.register(10, 1);
        let _b = p.register(20, 1);
        let _c = p.register(30, 1);
        // Chain A -> B -> C; only C (leaf) is evictable.
        assert_eq!(p.evict_oldest(), Some(30), "leaf C first");
        // Now B is the leaf and becomes eligible.
        assert_eq!(p.candidates(4), vec![20]);
        assert_eq!(p.evict_oldest(), Some(20), "B promoted then evicted");
        assert_eq!(p.candidates(4), vec![10], "A is now the leaf");
        assert_eq!(p.nodes[a as usize].child, None, "A childless after B gone");
        p.check_invariants();
    }

    #[test]
    fn never_evicts_a_node_with_a_tracked_child() {
        let mut p = Pool::default();
        p.register(10, 1);
        p.register(20, 1);
        // The head (key 10) has a child, so it is never a candidate/victim.
        for _ in 0..1 {
            let victim = p.evict_oldest().unwrap();
            assert_ne!(victim, 10, "head with a child must not be evicted first");
        }
        p.check_invariants();
    }

    #[test]
    fn candidates_returns_ordered_leaves_without_removing() {
        let mut p = Pool::default();
        p.register(10, 1); // stamp 1
        p.register(20, 2); // stamp 2
        p.register(30, 3); // stamp 3
        assert_eq!(p.candidates(2), vec![10, 20], "oldest-first, capped at n");
        assert_eq!(p.len(), 3, "candidates does not remove");
        assert_eq!(p.candidates(10), vec![10, 20, 30]);
        p.check_invariants();
    }

    // ---- User Story 3: recency refresh ----------------------------------

    #[test]
    fn touch_changes_the_victim() {
        let mut p = Pool::default();
        p.register(10, 1); // stamp 1
        p.register(20, 2); // stamp 2
                           // Touch the older leaf so the other becomes the victim.
        assert!(p.touch(idx_of(&p, 10)));
        assert_eq!(p.evict_oldest(), Some(20), "untouched leaf now oldest");
        p.check_invariants();
    }

    #[test]
    fn touch_invalid_handle_fails() {
        let mut p = Pool::default();
        p.register(10, 1);
        assert!(!p.touch(999), "invalid handle rejected");
        p.check_invariants();
    }

    #[test]
    fn eviction_does_not_refresh_recency() {
        let mut p = Pool::default();
        p.register(10, 1); // stamp 1, session 1
        p.register(20, 2); // stamp 2, session 2
                           // Evicting the oldest does not bump the survivor's recency, and the
                           // evicted key does not linger with a refreshed stamp.
        assert_eq!(p.evict_oldest(), Some(10));
        assert_eq!(p.candidates(4), vec![20]);
        p.check_invariants();
    }

    #[test]
    fn clear_resets_pool() {
        let mut p = Pool::default();
        p.register(10, 1);
        p.register(20, 1);
        p.clear();
        assert_eq!(p.len(), 0);
        assert_eq!(p.evict_oldest(), None);
        // Reusable after clear.
        let i = p.register(30, 5);
        assert_eq!(p.nodes[i as usize].parent, None);
        p.check_invariants();
    }

    // ---- Internal invariant property test (SC-006) ----------------------

    #[test]
    fn randomized_operations_preserve_invariants() {
        // Deterministic xorshift RNG (no external crate).
        let mut s: u64 = 0x9E3779B97F4A7C15;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };

        let mut p = Pool::default();
        let mut live: Vec<(CacheKey, u32)> = Vec::new(); // (key, handle)
        let mut next_key: CacheKey = 1;

        for _ in 0..5_000 {
            match next() % 5 {
                0..=1 => {
                    // register a fresh key into one of a few sessions
                    let session = next() % 8;
                    let key = next_key;
                    next_key += 1;
                    let h = p.register(key, session);
                    live.push((key, h));
                }
                2 => {
                    // touch a live handle (or a random invalid one)
                    if let Some(&(_, h)) = live.get((next() as usize) % live.len().max(1)) {
                        p.touch(h);
                    }
                }
                3 => {
                    // remove a live handle
                    if !live.is_empty() {
                        let pos = (next() as usize) % live.len();
                        let (_, h) = live.swap_remove(pos);
                        p.remove(h);
                    }
                }
                _ => {
                    // evict oldest leaf; drop it from the live set
                    if let Some(key) = p.evict_oldest() {
                        if let Some(pos) = live.iter().position(|&(k, _)| k == key) {
                            live.swap_remove(pos);
                        }
                    }
                }
            }
            p.check_invariants();
        }
    }
}
