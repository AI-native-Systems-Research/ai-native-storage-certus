// The REAL `LruList`, compiled in situ.
//
// `real` below is the production file `components/eviction-policy-optimized/src/lru_list.rs`
// pulled in textually with `include!` — not a copy, not a model. Editing the
// production file changes what every harness in `proofs_list.rs` checks, so these
// proofs cannot go stale against the code they are about.
//
// Why include it at all, when the component crate is already a dependency? Because
// `LruList` is `pub(crate)` to the component and its representation
// (`nodes`/`head`/`tail`/`free`/`len`, `Node::prev`/`next`/`active`) is private.
// Better than half the inventory's global invariants are statements ABOUT that
// representation — "the reported size equals the length of the eviction chain", "the
// forward and backward links agree", "each slot is either in use or free, never both"
// — and there is no way to observe a broken chain through `IEvictionPolicy` alone: a
// list whose `len` has drifted from its chain still answers `len()` with the drifted
// number. Compiling the real source inside this crate puts those fields in scope for
// the `inspect` child module, with the production source untouched.

pub mod real {
    // The production file, byte-for-byte except that its leading `//!` doc comments are
    // turned into plain `//` comments — see build.rs, which performs that one
    // transformation, proves it touched nothing but comments, and regenerates the copy
    // whenever `src/lru_list.rs` changes.
    include!(concat!(env!("OUT_DIR"), "/lru_list_real.rs"));

    /// Read-only view onto `LruList`'s private representation, for stating the
    /// structural invariants.
    ///
    /// `inspect` is a CHILD of the module holding `LruList`, so it may read private
    /// fields; that is the whole reason it exists, and it is why no accessor had to be
    /// added to the production source. Nothing here mutates the list. The one
    /// constructor, [`from_parts`], exists so an arbitrary (symbolic) representation
    /// can be built for the INDUCTIVE invariant harnesses — and every harness that
    /// uses it first does `kani::assume(inspect::invariant(&list, cap))`, so an
    /// unreachable representation can never witness a proof.
    #[cfg(kani)]
    pub(crate) mod inspect {
        use super::{LruList, Node};
        use interfaces::CacheKey;

        // -------------------------------------------------------------- accessors

        pub fn nodes_len(l: &LruList) -> usize {
            l.nodes.len()
        }
        pub fn head(l: &LruList) -> Option<u32> {
            l.head
        }
        pub fn tail(l: &LruList) -> Option<u32> {
            l.tail
        }
        pub fn len_field(l: &LruList) -> usize {
            l.len
        }
        pub fn free_len(l: &LruList) -> usize {
            l.free.len()
        }
        pub fn free_at(l: &LruList, i: usize) -> u32 {
            l.free[i]
        }
        pub fn node_key(l: &LruList, i: u32) -> CacheKey {
            l.nodes[i as usize].key
        }
        pub fn node_prev(l: &LruList, i: u32) -> Option<u32> {
            l.nodes[i as usize].prev
        }
        pub fn node_next(l: &LruList, i: u32) -> Option<u32> {
            l.nodes[i as usize].next
        }
        pub fn node_active(l: &LruList, i: u32) -> bool {
            l.nodes[i as usize].active
        }

        /// Number of slots marked live.
        pub fn active_count(l: &LruList) -> usize {
            let mut n = 0usize;
            let mut i = 0usize;
            while i < l.nodes.len() {
                if l.nodes[i].active {
                    n += 1;
                }
                i += 1;
            }
            n
        }

        /// How many times `idx` appears on the free list.
        pub fn free_occurrences(l: &LruList, idx: u32) -> usize {
            let mut n = 0usize;
            let mut i = 0usize;
            while i < l.free.len() {
                if l.free[i] == idx {
                    n += 1;
                }
                i += 1;
            }
            n
        }

        // ----------------------------------------------------------- constructors

        pub fn new_node(
            key: CacheKey,
            prev: Option<u32>,
            next: Option<u32>,
            active: bool,
        ) -> Node {
            Node {
                key,
                prev,
                next,
                active,
            }
        }

        /// Build an arbitrary list representation. ONLY for inductive harnesses, and
        /// only ever paired with `assume(invariant(..))`.
        pub fn from_parts(
            nodes: Vec<Node>,
            head: Option<u32>,
            tail: Option<u32>,
            free: Vec<u32>,
            len: usize,
        ) -> LruList {
            LruList {
                nodes,
                head,
                tail,
                free,
                len,
            }
        }

        // -------------------------------------------------- symbolic state builders
        //
        // These live here rather than in the harness modules because `Node` is private
        // to the production source and so cannot even be NAMED outside this module.

        /// A symbolic slot number that exists, or `None`.
        pub fn any_opt_idx(n: usize) -> Option<u32> {
            if kani::any::<bool>() {
                let i: u32 = kani::any();
                kani::assume((i as usize) < n);
                Some(i)
            } else {
                None
            }
        }

        /// A symbolic slot number that exists. Requires `n > 0`.
        pub fn any_idx(n: usize) -> u32 {
            let i: u32 = kani::any();
            kani::assume((i as usize) < n);
            i
        }

        /// A completely arbitrary `n`-slot representation: every key, link, live flag,
        /// both ends, the free list and the stored length are symbolic. Almost all of
        /// these are NOT valid lists; callers pair this with `assume(invariant(..))`.
        pub fn any_list(n: usize) -> LruList {
            let mut nodes: Vec<Node> = Vec::with_capacity(n);
            let mut i = 0usize;
            while i < n {
                let key: CacheKey = kani::any();
                let prev = any_opt_idx(n);
                let next = any_opt_idx(n);
                let active: bool = kani::any();
                nodes.push(Node {
                    key,
                    prev,
                    next,
                    active,
                });
                i += 1;
            }
            let head = any_opt_idx(n);
            let tail = any_opt_idx(n);

            let flen: usize = kani::any();
            kani::assume(flen <= n);
            let mut free: Vec<u32> = Vec::with_capacity(n);
            let mut j = 0usize;
            while j < flen {
                let x: u32 = kani::any();
                kani::assume((x as usize) < n);
                free.push(x);
                j += 1;
            }

            let len: usize = kani::any();
            kani::assume(len <= n);

            LruList {
                nodes,
                head,
                tail,
                free,
                len,
            }
        }

        /// An arbitrary VALID `n`-slot list: [`any_list`] constrained by the full
        /// structural invariant. This is the pre-state of every inductive harness.
        ///
        /// The risk with an assumed pre-state is vacuity — if the conjunction were
        /// unsatisfiable, every assertion after it would pass for free. That is exactly
        /// what the `__mutant` twin of each inductive harness rules out: the twin
        /// asserts something FALSE of any real list from the same assumed pre-state, and
        /// the scorer requires it to FAIL. A mutant that passed would mean this
        /// pre-state is empty, not that the code is correct.
        pub fn valid_list(n: usize, cap: usize) -> LruList {
            let l = any_list(n);
            kani::assume(invariant(&l, cap));
            l
        }

        // --------------------------------------------------------------- chain ops
        //
        // Every walk below is bounded by `cap` and also bails out on an
        // out-of-range slot number, so a corrupt representation makes the walk
        // REPORT failure rather than panic — that is what lets the invariant be
        // evaluated on a state that may violate it.

        /// Steps taken walking forward from `head`, and whether the walk ended at a
        /// `None` `next` (`false` means it ran out of budget or left the slot array,
        /// i.e. a cycle or a dangling link).
        pub fn walk_forward(l: &LruList, cap: usize) -> (usize, bool) {
            let mut steps = 0usize;
            let mut cur = l.head;
            while let Some(i) = cur {
                if steps >= cap || i as usize >= l.nodes.len() {
                    return (steps, false);
                }
                steps += 1;
                cur = l.nodes[i as usize].next;
            }
            (steps, true)
        }

        /// Steps taken walking backward from `tail`.
        pub fn walk_backward(l: &LruList, cap: usize) -> (usize, bool) {
            let mut steps = 0usize;
            let mut cur = l.tail;
            while let Some(i) = cur {
                if steps >= cap || i as usize >= l.nodes.len() {
                    return (steps, false);
                }
                steps += 1;
                cur = l.nodes[i as usize].prev;
            }
            (steps, true)
        }

        /// Is `idx` on the chain (walking forward from `head`, within `cap` steps)?
        pub fn in_chain(l: &LruList, idx: u32, cap: usize) -> bool {
            let mut steps = 0usize;
            let mut cur = l.head;
            while let Some(i) = cur {
                if steps >= cap || i as usize >= l.nodes.len() {
                    return false;
                }
                if i == idx {
                    return true;
                }
                steps += 1;
                cur = l.nodes[i as usize].next;
            }
            false
        }

        /// The chain's keys, front (next victim) to back, at most `cap`.
        pub fn chain_keys(l: &LruList, cap: usize) -> Vec<CacheKey> {
            let mut out = Vec::new();
            let mut cur = l.head;
            while let Some(i) = cur {
                if out.len() >= cap || i as usize >= l.nodes.len() {
                    break;
                }
                out.push(l.nodes[i as usize].key);
                cur = l.nodes[i as usize].next;
            }
            out
        }

        /// The chain's slot indices, front to back, at most `cap`.
        pub fn chain_indices(l: &LruList, cap: usize) -> Vec<u32> {
            let mut out = Vec::new();
            let mut cur = l.head;
            while let Some(i) = cur {
                if out.len() >= cap || i as usize >= l.nodes.len() {
                    break;
                }
                out.push(i);
                cur = l.nodes[i as usize].next;
            }
            out
        }

        /// `chain_indices` with `idx` deleted — the "relative order of the others"
        /// that frame properties talk about.
        pub fn chain_indices_without(l: &LruList, idx: u32, cap: usize) -> Vec<u32> {
            let mut out = Vec::new();
            let all = chain_indices(l, cap);
            let mut k = 0usize;
            while k < all.len() {
                if all[k] != idx {
                    out.push(all[k]);
                }
                k += 1;
            }
            out
        }

        // ------------------------------------------- allocation-free chain projection
        //
        // WHY THESE EXIST. `chain_indices` and `snapshot` return `Vec`s, so a harness that
        // builds two of them and compares them pays for two heap allocations, two
        // deallocations and an allocator-dependent compare path. At CBMC level that is what
        // forced `verify_epo_touch_frame_relative_order_others` and
        // `verify_epo_remove_frame_relative_order_others` up to an unwind bound of 20 — at
        // which, measured, they no longer finish inside the gate's 300 s cap at all. The
        // fixed-size array form keeps the allocator out of the picture and lets those
        // harnesses run at the file's usual bound of 8. `SLOTS = 8` is comfortably above
        // `CAP = NS + 2 = 5`, and the projection asserts it never overflows, so a longer
        // chain would be a failed check rather than a silently truncated comparison.

        pub const SLOTS: usize = 8;

        /// The chain's slot indices, front to back, as `(array, length)`.
        ///
        /// Bails out on an out-of-range link exactly as `chain_indices` does, so a corrupt
        /// representation makes the projection stop rather than panic.
        pub fn chain_slots(l: &LruList, cap: usize) -> ([u32; SLOTS], usize) {
            let mut out = [u32::MAX; SLOTS];
            let mut n = 0usize;
            let mut cur = l.head;
            while let Some(i) = cur {
                if n >= cap || i as usize >= l.nodes.len() {
                    break;
                }
                assert!(n < SLOTS, "chain longer than the projection can hold");
                out[n] = i;
                n += 1;
                cur = l.nodes[i as usize].next;
            }
            (out, n)
        }

        /// `chain_slots` with `idx` deleted — the "relative order of the others" the frame
        /// properties talk about, without a heap allocation.
        pub fn chain_slots_without(
            l: &LruList,
            idx: u32,
            cap: usize,
        ) -> ([u32; SLOTS], usize) {
            let (all, k) = chain_slots(l, cap);
            let mut out = [u32::MAX; SLOTS];
            let mut n = 0usize;
            let mut j = 0usize;
            while j < k {
                if all[j] != idx {
                    out[n] = all[j];
                    n += 1;
                }
                j += 1;
            }
            (out, n)
        }

        /// Element-by-element equality of two `(array, length)` projections.
        pub fn slots_eq(a: &([u32; SLOTS], usize), b: &([u32; SLOTS], usize)) -> bool {
            if a.1 != b.1 {
                return false;
            }
            let mut i = 0usize;
            while i < a.1 {
                if a.0[i] != b.0[i] {
                    return false;
                }
                i += 1;
            }
            true
        }

        /// The complete representation as FIXED-SIZE arrays — the allocation-free twin of
        /// [`Snapshot`], for the same reason `chain_slots` is the twin of `chain_indices`.
        /// `u32::MAX` is the `None` sentinel for the two ends and for every link; no real
        /// slot number can collide with it, because `SLOTS` bounds the slot count and the
        /// projection asserts that bound.
        #[derive(Clone, Copy)]
        pub struct FixedSnapshot {
            pub keys: [CacheKey; SLOTS],
            pub prev: [u32; SLOTS],
            pub next: [u32; SLOTS],
            pub active: [bool; SLOTS],
            pub nodes_len: usize,
            pub head: u32,
            pub tail: u32,
            pub free: [u32; SLOTS],
            pub free_len: usize,
            pub len: usize,
        }

        fn opt_to_sentinel(o: Option<u32>) -> u32 {
            match o {
                Some(v) => v,
                None => u32::MAX,
            }
        }

        pub fn fixed_snapshot(l: &LruList) -> FixedSnapshot {
            assert!(l.nodes.len() <= SLOTS, "more slots than the projection can hold");
            assert!(l.free.len() <= SLOTS, "free list longer than the projection can hold");
            let mut keys = [0u64; SLOTS];
            let mut prev = [u32::MAX; SLOTS];
            let mut next = [u32::MAX; SLOTS];
            let mut active = [false; SLOTS];
            let mut i = 0usize;
            while i < l.nodes.len() {
                keys[i] = l.nodes[i].key;
                prev[i] = opt_to_sentinel(l.nodes[i].prev);
                next[i] = opt_to_sentinel(l.nodes[i].next);
                active[i] = l.nodes[i].active;
                i += 1;
            }
            let mut free = [u32::MAX; SLOTS];
            let mut j = 0usize;
            while j < l.free.len() {
                free[j] = l.free[j];
                j += 1;
            }
            FixedSnapshot {
                keys,
                prev,
                next,
                active,
                nodes_len: l.nodes.len(),
                head: opt_to_sentinel(l.head),
                tail: opt_to_sentinel(l.tail),
                free,
                free_len: l.free.len(),
                len: l.len,
            }
        }

        /// Field-by-field, slot-by-slot equality over the WHOLE representation, so
        /// "changes nothing" stays a claim about all of it rather than about a subset.
        pub fn fixed_snapshot_eq(a: &FixedSnapshot, b: &FixedSnapshot) -> bool {
            if a.nodes_len != b.nodes_len
                || a.head != b.head
                || a.tail != b.tail
                || a.len != b.len
                || a.free_len != b.free_len
            {
                return false;
            }
            let mut i = 0usize;
            while i < a.nodes_len {
                if a.keys[i] != b.keys[i]
                    || a.prev[i] != b.prev[i]
                    || a.next[i] != b.next[i]
                    || a.active[i] != b.active[i]
                {
                    return false;
                }
                i += 1;
            }
            let mut j = 0usize;
            while j < a.free_len {
                if a.free[j] != b.free[j] {
                    return false;
                }
                j += 1;
            }
            true
        }

        // ---------------------------------------------------------------- snapshots

        /// The complete observable representation of a list, captured so that a
        /// "changes nothing" property can be stated as one equality instead of a pile
        /// of field comparisons that might quietly omit a field.
        #[derive(PartialEq, Eq, Clone, Debug)]
        pub struct Snapshot {
            pub nodes: Vec<(CacheKey, Option<u32>, Option<u32>, bool)>,
            pub head: Option<u32>,
            pub tail: Option<u32>,
            pub free: Vec<u32>,
            pub len: usize,
        }

        // Sequence comparison, element by element.
        //
        // NOT `==` on the `Vec`s and NOT the derived `PartialEq` on `Snapshot`: slice equality
        // lowers to `memcmp`, whose byte loop CBMC has to unwind over the whole buffer, so a
        // `Snapshot` of four 24-byte tuples needs an unwind bound near 100. Every harness that
        // compared one under this file's small bound failed with "unwinding assertion loop 0"
        // in `<builtin-library-memcmp>` — a bound artefact, not a property failure. Comparing
        // field by field removes the memcmp entirely and is more explicit about what
        // "unchanged" means.
        pub fn u32_seq_eq(a: &Vec<u32>, b: &Vec<u32>) -> bool {
            if a.len() != b.len() {
                return false;
            }
            let mut i = 0usize;
            while i < a.len() {
                if a[i] != b[i] {
                    return false;
                }
                i += 1;
            }
            true
        }

        pub fn key_seq_eq(a: &Vec<CacheKey>, b: &Vec<CacheKey>) -> bool {
            if a.len() != b.len() {
                return false;
            }
            let mut i = 0usize;
            while i < a.len() {
                if a[i] != b[i] {
                    return false;
                }
                i += 1;
            }
            true
        }

        /// Full-representation equality, field by field — see `u32_seq_eq` for why this is not
        /// `==`. Every component of `Snapshot` is compared, so "changes nothing" stays a claim
        /// about the whole representation rather than about a subset someone forgot to list.
        pub fn snapshot_eq(a: &Snapshot, b: &Snapshot) -> bool {
            if a.head != b.head || a.tail != b.tail || a.len != b.len {
                return false;
            }
            if a.nodes.len() != b.nodes.len() {
                return false;
            }
            let mut i = 0usize;
            while i < a.nodes.len() {
                let (ka, pa, na, aa) = a.nodes[i];
                let (kb, pb, nb, ab) = b.nodes[i];
                if ka != kb || pa != pb || na != nb || aa != ab {
                    return false;
                }
                i += 1;
            }
            u32_seq_eq(&a.free, &b.free)
        }

        pub fn snapshot(l: &LruList) -> Snapshot {
            let mut nodes = Vec::with_capacity(l.nodes.len());
            let mut i = 0usize;
            while i < l.nodes.len() {
                let nd = &l.nodes[i];
                nodes.push((nd.key, nd.prev, nd.next, nd.active));
                i += 1;
            }
            Snapshot {
                nodes,
                head: l.head,
                tail: l.tail,
                free: l.free.clone(),
                len: l.len,
            }
        }

        /// Constrain every slot's key to be different from every other's.
        ///
        /// Needed only by the properties phrased about a KEY rather than a slot ("once
        /// an entry has been removed, no later eviction returns that entry again"): with
        /// duplicate keys in the list such a statement is not even well posed, and the
        /// production code deliberately permits duplicates
        /// (EPO-TRACK-POST-NON-IDEMPOTENT-REREGISTRATION). Harnesses that assume this
        /// say so in their doc comment and in the advisory note.
        pub fn assume_distinct_keys(l: &LruList) {
            let n = l.nodes.len();
            let mut i = 0usize;
            while i < n {
                let mut j = i + 1;
                while j < n {
                    kani::assume(l.nodes[i].key != l.nodes[j].key);
                    j += 1;
                }
                i += 1;
            }
        }

        /// Does the chain contain `key`?
        pub fn chain_contains_key(l: &LruList, key: CacheKey, cap: usize) -> bool {
            let keys = chain_keys(l, cap);
            let mut i = 0usize;
            while i < keys.len() {
                if keys[i] == key {
                    return true;
                }
                i += 1;
            }
            false
        }

        // --------------------------------------------------------------- invariant

        /// `EPO-INV-INTERNAL-INDEX-IN-RANGE`: every slot number the list stores —
        /// both ends, every link, every free-list entry — designates an allocated
        /// slot.
        pub fn inv_index_in_range(l: &LruList) -> bool {
            let n = l.nodes.len();
            if let Some(h) = l.head {
                if h as usize >= n {
                    return false;
                }
            }
            if let Some(t) = l.tail {
                if t as usize >= n {
                    return false;
                }
            }
            let mut i = 0usize;
            while i < n {
                if let Some(p) = l.nodes[i].prev {
                    if p as usize >= n {
                        return false;
                    }
                }
                if let Some(x) = l.nodes[i].next {
                    if x as usize >= n {
                        return false;
                    }
                }
                i += 1;
            }
            let mut i = 0usize;
            while i < l.free.len() {
                if l.free[i] as usize >= n {
                    return false;
                }
                i += 1;
            }
            true
        }

        /// `EPO-INV-LIST-EMPTY-IFF-NO-ENDS`: both ends and a non-zero length, or
        /// neither end and zero length. Never one end without the other.
        pub fn inv_empty_iff_no_ends(l: &LruList) -> bool {
            match (l.head, l.tail) {
                (None, None) => l.len == 0,
                (Some(_), Some(_)) => l.len > 0,
                _ => false,
            }
        }

        /// `EPO-INV-LIST-ACYCLIC-AND-LENGTH` + `EPO-INV-LEN-MATCHES-CHAIN`: the
        /// forward walk terminates in exactly `len` steps, and so does the backward
        /// walk.
        pub fn inv_len_matches_chain(l: &LruList, cap: usize) -> bool {
            let (steps, ok) = walk_forward(l, cap);
            if !ok || steps != l.len {
                return false;
            }
            let (bsteps, bok) = walk_backward(l, cap);
            bok && bsteps == l.len
        }

        /// `EPO-INV-LIST-LINKS-SYMMETRIC`: a forward link is mirrored by a backward
        /// link, the front has no backward link and the back has no forward link.
        pub fn inv_links_symmetric(l: &LruList, cap: usize) -> bool {
            if let Some(h) = l.head {
                if h as usize >= l.nodes.len() || l.nodes[h as usize].prev.is_some() {
                    return false;
                }
            }
            if let Some(t) = l.tail {
                if t as usize >= l.nodes.len() || l.nodes[t as usize].next.is_some() {
                    return false;
                }
            }
            let chain = chain_indices(l, cap);
            let mut k = 0usize;
            while k < chain.len() {
                let idx = chain[k];
                match l.nodes[idx as usize].next {
                    Some(j) => {
                        if j as usize >= l.nodes.len()
                            || l.nodes[j as usize].prev != Some(idx)
                        {
                            return false;
                        }
                    }
                    None => {}
                }
                k += 1;
            }
            true
        }

        /// `EPO-INV-LIST-TAIL-IS-ONLY-NODE-WITHOUT-NEXT`: among live slots, only the
        /// recorded back has no forward link.
        pub fn inv_tail_only_node_without_next(l: &LruList) -> bool {
            let mut i = 0usize;
            while i < l.nodes.len() {
                if l.nodes[i].active && l.nodes[i].next.is_none() && l.tail != Some(i as u32) {
                    return false;
                }
                i += 1;
            }
            true
        }

        /// `EPO-INV-LEN-MATCHES-ACTIVE-SLOTS`: a slot is live exactly while it is on
        /// the chain, and the live count is the reported size.
        pub fn inv_len_matches_active_slots(l: &LruList, cap: usize) -> bool {
            if active_count(l) != l.len {
                return false;
            }
            let mut i = 0usize;
            while i < l.nodes.len() {
                if l.nodes[i].active != in_chain(l, i as u32, cap) {
                    return false;
                }
                i += 1;
            }
            true
        }

        /// `EPO-INV-SLOT-OWNERSHIP-DISJOINT`: the free list is exactly the not-live
        /// slots, each appearing exactly once — so a live slot is never handed out and
        /// two inserts can never be given the same slot.
        pub fn inv_slot_ownership_disjoint(l: &LruList) -> bool {
            let mut i = 0usize;
            while i < l.nodes.len() {
                let occ = free_occurrences(l, i as u32);
                if l.nodes[i].active {
                    if occ != 0 {
                        return false;
                    }
                } else if occ != 1 {
                    return false;
                }
                i += 1;
            }
            true
        }

        /// `EPO-INV-SLOTS-ACCOUNTED`: allocated slots = tracked entries + free slots.
        pub fn inv_slots_accounted(l: &LruList) -> bool {
            l.nodes.len() == l.len + l.free.len()
        }

        /// The whole structural invariant, as one predicate: the conjunction the
        /// inductive harnesses assume of the pre-state and assert of the post-state.
        /// Each conjunct also stands alone above, so a failure is attributable to the
        /// one inventory property it belongs to.
        ///
        /// `cap` bounds the chain walks; it must be at least `nodes.len()`.
        pub fn invariant(l: &LruList, cap: usize) -> bool {
            inv_index_in_range(l)
                && inv_empty_iff_no_ends(l)
                && inv_len_matches_chain(l, cap)
                && inv_links_symmetric(l, cap)
                && inv_tail_only_node_without_next(l)
                && inv_len_matches_active_slots(l, cap)
                && inv_slot_ownership_disjoint(l)
                && inv_slots_accounted(l)
        }
    }

}

pub(crate) use real::LruList;

#[cfg(kani)]
pub(crate) use real::inspect;
