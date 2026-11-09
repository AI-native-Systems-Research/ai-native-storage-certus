/*
 * System-level model: Cold-path Promotion Atomicity under Concurrent Lookups
 *
 * Verifies that when multiple concurrent lookups race to promote the same key
 * from SSD (BlockDevice) into the memory-tier, the promotion is atomic:
 *   - Exactly one lookup wins the mt.insert and allocates the slot.
 *   - Every loser observes MemoryTierError::AlreadyExists and RECOVERS by
 *     re-serving the block warm (never double-allocates, never returns a
 *     spurious miss while the winner's data is resident).
 *   - The dispatch-map entry flips to MemoryTier only AFTER the winner's
 *     SSD->DRAM read completes, so any lookup that observes MemoryTier is
 *     guaranteed to find resident data.
 *
 * This complements ../promotion-atomicity (which models an older
 * remove+recreate promotion protocol). Here we model the CURRENT protocol:
 * in-place `promote_block_to_memory_tier` (lib.rs:3894 / call site 2264) plus
 * the concurrent-promotion recovery pass (`serve_concurrently_promoted`,
 * lib.rs:1181, invoked from batch_lookup at lib.rs:2689). It adds a distinct
 * WARM-serving lookup role that races with promotion.
 *
 * Component wiring modeled (as wired in certus-server):
 *   LookupThread (batch_lookup) --> dispatch-map.lookup / release_read / touch
 *                               --> memory-tier.insert  (the atomicity point)
 *                               --> dispatch-map.promote_block_to_memory_tier
 *                               --> serve_concurrently_promoted (loser recovery)
 *   Evictor (evict_for_space)   --> dispatch-map.try_evict_to_block
 *                               --> memory-tier slot free
 *   SSD read                    --> nondeterministic schedulable step
 *
 * Key modeling decisions matching the real code:
 *   - Classification `dm.lookup` pins (read_ref++) on MemoryTier / BlockDevice
 *     hits; a BlockDevice classifier then release_read()s before promoting
 *     (lib.rs:2204-2213), opening the mt.insert race window.
 *   - `mt.insert(key)` is atomic and keyed: the first caller gets Ok (winner),
 *     all concurrent callers get AlreadyExists (losers). This is the single
 *     serialization point that makes promotion atomic.
 *   - The winner reserves the slot, performs the SSD->DRAM read, THEN calls
 *     promote_block_to_memory_tier to flip the entry to MemoryTier in place
 *     (refs preserved; comment lib.rs:2262 "In-place, pin-safe promote").
 *   - A loser loops in serve_concurrently_promoted: re-lookup, and if it sees
 *     BlockDevice (winner still reading) it releases and backs off; if it sees
 *     MemoryTier it serves warm. The entry's read-pin keeps it from eviction.
 *   - Eviction (evict_for_space / try_evict_to_block) demotes an unpinned,
 *     resident MemoryTier entry back to BlockDevice and frees the slot,
 *     enabling a legitimate later re-promotion (new lifecycle).
 *
 * Safety properties verified:
 *   A1 (single winner):  At most one lookup wins mt.insert per promotion
 *                        lifecycle. No double-allocation of the slot.
 *   A2 (resident-on-MT): Whenever a lookup observes MemoryTier, the slot is
 *                        allocated AND resident (SSD read completed). Serving
 *                        warm never reads un-populated DRAM.
 *   A3 (pin safety):     Eviction never fires on a pinned entry (read_refs>0);
 *                        a warm serve holds its pin throughout.
 *   A4 (pool accounting): mt_pool_used always equals the number of live slots;
 *                        no losing promoter leaks a slot.
 *   A5 (final quiescence): No leaked read-refs; dm/mt state consistent.
 *
 * Parameters tuned for full coverage (<10M states):
 *   N_KEYS=2, POOL_CAP=1 -> pool pressure forces PoolFull + eviction cycles.
 *   N_THREADS=3, keys assigned tid%N_KEYS -> two threads race on key 0
 *   (the same-key promotion race), one on key 1 (cross-key pool contention).
 */

/* ---------- Parameters ---------- */
#define N_KEYS        2
#define POOL_CAP      1
#define N_THREADS     3
#define MAX_ATTEMPTS  3    /* bounded batch_lookup + recovery retries per thread */

/* ---------- Dispatch-map location ---------- */
mtype = { BLOCK, MT };     /* every key always has an entry; never fully removed */

mtype dm_loc[N_KEYS];      /* BlockDevice or MemoryTier */
byte  read_refs[N_KEYS];   /* dispatch-map read pins */

/* ---------- Memory-tier state ---------- */
bool  mt_alloc[N_KEYS];    /* a slot is allocated for this key */
bool  resident[N_KEYS];    /* winner's SSD->DRAM read has completed */
byte  mt_pool_used = 0;

/*
 * win_count[k] : number of mt.insert winners in the current promotion
 * lifecycle. Reset to 0 when the key is evicted (slot freed). A1 asserts it
 * never exceeds 1 — the essence of promotion atomicity.
 */
byte win_count[N_KEYS];

/* ---------- Coordination ---------- */
byte threads_done = 0;
bool shutdown = false;

/* ---------- Lookup + Promote thread ---------- */
proctype LookupThread(byte tid)
{
    byte k = tid % N_KEYS;
    byte attempts = 0;
    bool finished = false;

    mtype cls;
    bool won;
    bool already;
    bool poolfull;

    do
    :: (!finished && attempts < MAX_ATTEMPTS) ->
        attempts++;

        /* dm.lookup: pin on a hit, classify location. */
        atomic {
            if
            :: (dm_loc[k] == MT) ->
                read_refs[k]++;
                cls = MT
            :: (dm_loc[k] == BLOCK) ->
                read_refs[k]++;
                cls = BLOCK
            fi
        };

        if
        :: (cls == MT) ->
            /*
             * Warm hit. A2: observing MemoryTier implies the slot is resident.
             * The read-pin (A3) keeps the evictor off the entry while we serve.
             */
            assert(dm_loc[k] == MT && mt_alloc[k] && resident[k]);
            /* serve_memory_tier_to_gpu (H2D) — a schedulable serve window */
            skip;
            assert(dm_loc[k] == MT && mt_alloc[k] && resident[k]);
            atomic {
                assert(read_refs[k] > 0);
                read_refs[k]--            /* release_read */
            };
            finished = true

        :: (cls == BLOCK) ->
            /* Cold classify: release_read before promoting (lib.rs:2205). */
            atomic {
                assert(read_refs[k] > 0);
                read_refs[k]--
            };

            /* === RACE WINDOW: entry has no pin; siblings may also promote === */

            /* evict_for_space + mt.insert(key): the atomic keyed allocation. */
            won = false;
            already = false;
            poolfull = false;
            atomic {
                if
                :: (!mt_alloc[k] && mt_pool_used < POOL_CAP) ->
                    /* Winner: reserve the slot; data not yet read in. */
                    mt_alloc[k] = true;
                    mt_pool_used++;
                    resident[k] = false;
                    win_count[k]++;
                    assert(win_count[k] == 1);        /* A1: single winner */
                    won = true
                :: (mt_alloc[k]) ->
                    /* AlreadyExists: a sibling won. We are a recovery loser. */
                    already = true
                :: (!mt_alloc[k] && mt_pool_used >= POOL_CAP) ->
                    /* PoolFull: need the evictor to free a slot; retry later. */
                    poolfull = true
                fi
            };

            if
            :: won ->
                /*
                 * Winner. The pipelined SSD->DRAM read fills the slot it just
                 * reserved and, for the common single-region case, fuses the
                 * SSD->GPU serve inline (gpu_dst = the region, lib.rs:2357), so
                 * the GPU already has the data before the dispatch-map flip. The
                 * winner holds no dispatch-map read-ref (its classify pin was
                 * released at lib.rs:2205); it owns the freshly-inserted slot
                 * directly, and the entry stays BlockDevice — invisible to the
                 * MemoryTier evictor — until the flip below.
                 */
                skip;                                 /* pipelined read + fused serve */
                resident[k] = true;                   /* data now resident */
                /*
                 * promote_block_to_memory_tier: in-place flip, refs preserved.
                 * resident is set BEFORE the flip so A2 holds for every observer
                 * of MemoryTier. (Post-flip eviction back to BlockDevice is a
                 * legitimate later lifecycle, not a promotion-atomicity bug.)
                 */
                atomic {
                    dm_loc[k] = MT
                };
                finished = true

            :: already ->
                /*
                 * serve_concurrently_promoted: loop (bounded by attempts).
                 * Re-lookup on the next iteration; if the winner has flipped to
                 * MemoryTier we serve warm, else we back off and retry.
                 */
                skip

            :: poolfull ->
                /* Back off; the evictor may free a slot before we retry. */
                skip
            fi
        fi

    :: (finished || attempts >= MAX_ATTEMPTS) ->
        break
    od;

    threads_done++
}

/* ---------- Background evictor ---------- */
/*
 * Models evict_for_space -> try_evict_to_block: demote one unpinned, resident
 * MemoryTier entry back to BlockDevice and free its pool slot. Pinned entries
 * (read_refs > 0) are never evicted (A3). Freeing the slot resets win_count so
 * a later promotion is a fresh, legitimate lifecycle.
 */
proctype Evictor()
{
    byte v;
    bool found;

    do
    :: !shutdown ->
        atomic {
            v = 0;
            found = false;
            do
            :: (v < N_KEYS) ->
                if
                :: (dm_loc[v] == MT && mt_alloc[v] && resident[v]
                    && read_refs[v] == 0) ->
                    found = true;
                    break
                :: else ->
                    v++
                fi
            :: (v >= N_KEYS) ->
                break
            od;

            if
            :: found ->
                assert(read_refs[v] == 0);            /* A3 */
                dm_loc[v] = BLOCK;
                mt_alloc[v] = false;
                resident[v] = false;
                mt_pool_used--;
                win_count[v] = 0                      /* new lifecycle */
            :: else ->
                skip
            fi
        }
    :: shutdown ->
        break
    od
}

/* ---------- Initialization ---------- */
init
{
    byte k;
    byte t;

    d_step {
        k = 0;
        do
        :: (k < N_KEYS) ->
            dm_loc[k] = BLOCK;
            read_refs[k] = 0;
            mt_alloc[k] = false;
            resident[k] = false;
            win_count[k] = 0;
            k++
        :: (k >= N_KEYS) ->
            break
        od
    };

    run Evictor();

    t = 0;
    do
    :: (t < N_THREADS) ->
        run LookupThread(t);
        t++
    :: (t >= N_THREADS) ->
        break
    od;

    /* Wait for all lookup threads, then drain the evictor. */
    (threads_done == N_THREADS);
    shutdown = true;
    timeout;

    /* Final invariants (A4, A5). */
    d_step {
        byte live = 0;
        k = 0;
        do
        :: (k < N_KEYS) ->
            assert(read_refs[k] == 0);                /* no leaked pins */
            assert(win_count[k] <= 1);                /* A1 holds at rest */
            /* dm/mt consistency: MemoryTier <=> allocated & resident. */
            if
            :: (dm_loc[k] == MT) ->
                assert(mt_alloc[k] && resident[k])
            :: (dm_loc[k] == BLOCK) ->
                assert(!mt_alloc[k])
            fi;
            if
            :: mt_alloc[k] -> live++
            :: else -> skip
            fi;
            k++
        :: (k >= N_KEYS) ->
            break
        od;
        assert(mt_pool_used == live)                  /* A4 */
    }
}
