/*
 * System-level model: Check->Pin Eviction Race (the bug fixed by
 * `evolve-dispatcher-dw`, commit 1d55b9c2)
 *
 * PURPOSE: demonstrate that Spin would have caught the race that the branch
 * fixes. The single #define BUGGY_DROP_FALLBACK selects between two builds of
 * `evict_one_clean`:
 *
 *   FIXED (default):  eviction never full-removes an entry. An unpersisted
 *                     victim is SKIPPED (stays resident in the memory-tier);
 *                     a persisted victim is DEMOTED to BlockDevice (still a
 *                     hit). Either way the key stays RESOLVABLE. If nothing is
 *                     demotable the caller gets AllocationFailed and serves
 *                     uncached -- a survivable local miss.
 *
 *   BUGGY (-DBUGGY_DROP_FALLBACK):  restores the pre-fix fallback in which
 *                     evict_one_clean does `dm.remove` on an unpersisted,
 *                     unpinned victim to free its DRAM slot. Because a
 *                     just-Checked-but-not-yet-Pinned entry has read_ref == 0,
 *                     the remove succeeds and silently DROPS the entry.
 *
 * THE RACE (from the fix commit message):
 *   A vLLM connector Checks a content-addressed key resident, then -- in a
 *   separate RPC -- Pins+Loads it. Between the Check and the Load's pin the
 *   entry has read_ref == 0. If a concurrent eviction full-removes it in that
 *   window the Load misses -> forwarded to remote-lookup -> the remote miss is
 *   a FATAL IoError in the connector (assert transfer_result.success ->
 *   EngineDeadError), crashing vLLM. `unstable` degrades gracefully;
 *   `evolve-dispatcher` (pre-fix) crashed 5/5 replay runs.
 *
 * Component wiring modeled (as wired in certus-server):
 *   Populator (populate)  --> memory-tier.insert (reserve, evict for space)
 *                         --> dispatch-map.create_memory_tier_entry (VISIBLE)
 *                         --> dispatch-map.downgrade_reference (writer read-ref)
 *                         --> bg_writer.enqueue(write-through)
 *   BgWriter              --> block-device write (may FAIL) -> release read-ref
 *                             (a FAILED write-through leaves the entry
 *                              unpersisted AND unpinned -- the victim state)
 *   Connector (vLLM)      --> Check: residency probe, NO pin
 *                         --> Load:  dispatch-map.lookup (pins on hit) / serve
 *   Evictor (evict_for_space -> evict_one_clean)
 *                         --> demote: dispatch-map.convert_to_storage (a HIT)
 *                         --> drop:   dispatch-map.remove  (BUGGY build only)
 *
 * Key modeling decisions matching the real code (components/dispatcher/src/lib.rs):
 *   - populate's linearization point is create_memory_tier_entry (lib.rs:3044):
 *     the key is visible in the dispatch-map before populate returns Ok.
 *   - downgrade_reference (lib.rs:3088) installs one read-ref held by the
 *     background writer, released when the write-through completes OR fails.
 *   - A FAILED write-through (IoError) releases the writer's ref while leaving
 *     `persisted` false -- the only way (within eviction scope) to reach an
 *     unpinned, unpersisted, pool-resident entry. This is exactly the
 *     "unpersisted victim" the drop fallback mishandled.
 *   - The connector's Check is a residency query distinct from the Load RPC and
 *     takes NO dispatch-map read-ref; the Load's dm.lookup (lib.rs:3676) pins.
 *   - evict_one_clean (lib.rs:920) demotes a persisted, unpinned, pool-resident
 *     victim (convert_to_storage, stays resolvable). The FIX removes the
 *     `dm.remove` fallback for unpersisted victims (see the diff in 1d55b9c2).
 *
 * Safety property (the essence of the fix):
 *   R-RESOLVE: once a connector has Checked a key resident, a later Load of that
 *              key never observes NotExist -- eviction may demote it but must
 *              never full-remove it. Modeled as `assert(loaded_ok)` after the
 *              Load. HOLDS in the fixed build; VIOLATED in the buggy build.
 *   R-REF:     removal/demotion only act on unpinned victims (read_refs == 0).
 *   R-FIN:     at quiescence no read-refs are leaked and pool accounting is
 *              consistent.
 *
 * Parameters tuned for full coverage (<10M states):
 *   N_KEYS=2, POOL_CAP=1 -> populates contend for the single slot, forcing
 *   eviction; N_CONN=2 concurrent connectors race Check vs Load against it.
 */

/* ---------- Build selector ----------
 * Leave undefined for the FIXED dispatcher (verification passes).
 * Compile pan with -DBUGGY_DROP_FALLBACK to model the pre-fix dispatcher
 * (verification finds the Check->Pin race). See the Makefile `buggy` target.
 */
/* #define BUGGY_DROP_FALLBACK */

/* ---------- Parameters ---------- */
#define N_KEYS      2
#define POOL_CAP    1
#define N_CONN      2      /* concurrent vLLM connectors (Check then Load) */
#define MAX_TRIES   4      /* max_eviction_attempts in reserve_memory */

/* ---------- Per-key state ---------- */
mtype = { MT, BLOCK };     /* dispatch-map location when present */

bool  dm_present[N_KEYS];  /* entry exists in dispatch-map (a lookup would HIT) */
mtype dm_loc[N_KEYS];      /* MT = memory-tier, BLOCK = demoted to block-device */
bool  persisted[N_KEYS];   /* write-through complete (ssd_offset set) */
byte  read_refs[N_KEYS];   /* dispatch-map read references (pins) */
bool  write_ref[N_KEYS];   /* exclusive write-ref during populate phase 1-2 */
bool  in_pool[N_KEYS];     /* occupies a memory-tier pool slot */
byte  pool_used = 0;

/* Generation counter per key: lets BgWriter detect a stale job after reuse. */
byte  gen[N_KEYS];

/* pop_ok[k]: populate(k) returned Ok for the current generation. */
bool  pop_ok[N_KEYS];

/* ---------- Write-through job channel: (key_index, generation) ---------- */
chan write_q = [N_KEYS] of { byte, byte };

/* ---------- Coordination ---------- */
byte pop_done = 0;
byte conn_done = 0;
bool shutdown = false;

/* ---------- Eviction: free one pin-safe victim ---------- */
/*
 * Models dispatcher::evict_one_clean (lib.rs:920). Scans for an unpinned,
 * pool-resident victim.
 *   - Persisted victim  -> DEMOTE (convert_to_storage): flip to BlockDevice,
 *     free the slot. The entry stays RESOLVABLE (cold-path promote can re-read
 *     it), so a concurrent Load still hits.
 *   - Unpersisted victim -> cannot be demoted (no ssd_offset yet).
 *       FIXED build:  SKIP it. The scan only ever stops on a demotable
 *                     (persisted) candidate; an all-unpersisted tier yields
 *                     freed=false -> AllocationFailed -> uncached serve.
 *       BUGGY build:  the pre-fix `dm.remove` fallback FULL-REMOVES it (it is
 *                     unpinned so remove succeeds) -> the key becomes NotExist.
 * The scan-and-act is atomic (the real code holds the MT / DM locks).
 */
inline evict_one_clean(freed)
{
    byte v;
    freed = false;
    atomic {
        v = 0;
        do
        :: (v < N_KEYS) ->
            if
            :: (dm_present[v] && in_pool[v] && read_refs[v] == 0
                && !write_ref[v] && persisted[v]) ->
                /* Demotable candidate: demotion always keeps it resolvable. */
                break
#ifdef BUGGY_DROP_FALLBACK
            :: (dm_present[v] && in_pool[v] && read_refs[v] == 0
                && !write_ref[v] && !persisted[v]) ->
                /*
                 * PRE-FIX: an unpersisted, unpinned entry is also treated as a
                 * reclaimable victim (the removed `dm.remove` fallback). The
                 * fixed build skips it -- this scan arm does not exist there,
                 * so the drop branch below is unreachable and pruned.
                 */
                break
#endif
            :: else ->
                v++
            fi
        :: (v >= N_KEYS) ->
            break
        od;

        if
        :: (v < N_KEYS && dm_present[v] && in_pool[v]
            && read_refs[v] == 0 && !write_ref[v]) ->
            assert(read_refs[v] == 0);            /* R-REF */
            if
            :: persisted[v] ->
                /* convert_to_storage: demote, entry stays resolvable. */
                dm_loc[v] = BLOCK;
                in_pool[v] = false;
                pool_used--;
                freed = true
#ifdef BUGGY_DROP_FALLBACK
            :: else ->
                /*
                 * PRE-FIX FALLBACK (removed by commit 1d55b9c2): dm.remove
                 * full-removes the unpersisted victim to free the DRAM slot.
                 * read_ref==0 so it succeeds -> the key silently vanishes.
                 * A connector that Checked it resident now Loads a miss.
                 */
                dm_present[v] = false;
                in_pool[v] = false;
                pool_used--;
                freed = true
#endif
            fi
        :: else ->
            skip
        fi
    }
}

/* ---------- Populate: reserve -> DMA -> commit -> enqueue ---------- */
inline do_populate(k)
{
    bool reserved = false;
    byte tries = 0;
    bool freed;

    /* Phase 1: reserve a memory-tier slot (mt.insert), evict under pressure. */
    do
    :: !reserved ->
        atomic {
            if
            :: (!in_pool[k] && !dm_present[k] && pool_used < POOL_CAP) ->
                in_pool[k] = true;
                pool_used++;
                write_ref[k] = true;         /* exclusive, not yet in dispatch-map */
                reserved = true
            :: else ->
                skip
            fi
        };
        if
        :: reserved ->
            skip
        :: !reserved ->
            evict_one_clean(freed);
            if
            :: freed ->
                skip
            :: !freed ->
                tries++;
                if
                :: (tries >= MAX_TRIES) ->
                    break            /* AllocationFailed: populate serves uncached */
                :: else ->
                    skip             /* retry: another proc may free a slot */
                fi
            fi
        fi
    :: reserved ->
        break
    od;

    if
    :: reserved ->
        /* Phase 2: async DMA GPU -> reserved slot (a schedulable point). */
        skip;

        /*
         * Phase 3: create_memory_tier_entry makes the key VISIBLE, then
         * downgrade_reference converts the write-ref into the writer's read-ref.
         * populate returns Ok right after (pop_ok). One atomic step (shared DM
         * mutex in the real code).
         */
        atomic {
            dm_present[k] = true;
            dm_loc[k] = MT;
            persisted[k] = false;
            gen[k]++;
            write_ref[k] = false;            /* downgrade_reference */
            read_refs[k]++;                  /* writer's read-ref */
            pop_ok[k] = true;
        };

        write_q ! k, gen[k]                  /* bg_writer.enqueue */
    :: !reserved ->
        skip                                 /* populate degraded (uncached) */
    fi;

    pop_done++
}

/* ---------- Populator process (one per key) ---------- */
proctype Populator(byte k)
{
    do_populate(k)
}

/* ---------- Background writer ---------- */
/*
 * Dequeues a write-through job; the entry is pinned by the writer's read-ref so
 * it is still present, resident and this generation's. The write-through either
 * completes (persisted -> later demotable) or FAILS (IoError: ref released,
 * persisted stays false -> the unpinned, unpersisted victim the drop fallback
 * mishandled). Either way it releases the writer's read-ref.
 */
proctype BgWriter()
{
    byte jk, jg;

    do
    :: write_q ? jk, jg ->
        atomic {
            assert(dm_present[jk] && gen[jk] == jg && dm_loc[jk] == MT);
            assert(read_refs[jk] > 0);
            if
            :: true  -> persisted[jk] = true /* write-through completes */
            :: true  -> skip                 /* write-through fails (IoError) */
            fi;
            read_refs[jk]--                  /* release writer's read-ref */
        }
    :: (shutdown && empty(write_q)) ->
        break
    od
}

/* ---------- Background evictor ---------- */
proctype Evictor()
{
    bool freed;

    do
    :: !shutdown ->
        evict_one_clean(freed)
    :: shutdown ->
        break
    od
}

/* ---------- vLLM connector: Check (no pin) then Load (pins) ---------- */
proctype Connector()
{
    byte k;
    bool checked_present;
    bool loaded_ok;

    select(k : 0 .. N_KEYS - 1);

    /* --- Check phase: residency probe, takes NO dispatch-map read-ref. --- */
    atomic {
        checked_present = dm_present[k]
    };

    if
    :: !checked_present ->
        /* Never observed resident here; a later miss is legitimate. */
        skip
    :: checked_present ->
        /* === CHECK -> PIN WINDOW: no pin held; eviction may act here. === */

        /* --- Load phase: dm.lookup pins on hit; a miss -> remote-forward. --- */
        atomic {
            if
            :: dm_present[k] ->
                read_refs[k]++;
                loaded_ok = true
            :: else ->
                loaded_ok = false            /* NotExist -> remote-lookup -> FATAL */
            fi
        };

        /*
         * R-RESOLVE (the fix): a key this connector Checked resident MUST still
         * be resolvable at Load. Eviction may DEMOTE (BlockDevice, still a hit)
         * but must never full-REMOVE. A miss here is forwarded to remote-lookup,
         * whose miss is a fatal IoError in the vLLM connector (EngineDeadError).
         * Holds in the fixed build; the buggy build's drop fallback violates it.
         */
        assert(loaded_ok);

        if
        :: loaded_ok ->
            /* serve (H2D / cold-path promote), then release the pin. */
            assert(dm_present[k]);
            atomic {
                assert(read_refs[k] > 0);
                read_refs[k]--
            }
        :: else ->
            skip
        fi
    fi;

    conn_done++
}

/* ---------- Initialization ---------- */
init
{
    byte k;

    d_step {
        k = 0;
        do
        :: (k < N_KEYS) ->
            dm_present[k] = false;
            dm_loc[k] = MT;
            persisted[k] = false;
            read_refs[k] = 0;
            write_ref[k] = false;
            in_pool[k] = false;
            gen[k] = 0;
            pop_ok[k] = false;
            k++
        :: (k >= N_KEYS) ->
            break
        od
    };

    run BgWriter();
    run Evictor();

    /* One Populator per key. */
    k = 0;
    do
    :: (k < N_KEYS) ->
        run Populator(k);
        k++
    :: (k >= N_KEYS) ->
        break
    od;

    /* N_CONN concurrent connectors. */
    k = 0;
    do
    :: (k < N_CONN) ->
        run Connector();
        k++
    :: (k >= N_CONN) ->
        break
    od;

    /* Wait for populates and connectors, then drain background processes. */
    (pop_done == N_KEYS && conn_done == N_CONN);
    shutdown = true;
    timeout;

    /* Final invariants (R-REF / R-FIN). */
    d_step {
        byte live = 0;
        k = 0;
        do
        :: (k < N_KEYS) ->
            assert(read_refs[k] == 0);            /* no leaked pins */
            /* dm/mt consistency: a pool slot implies a present MemoryTier entry. */
            assert(!in_pool[k] || (dm_present[k] && dm_loc[k] == MT));
            if
            :: in_pool[k] -> live++
            :: else -> skip
            fi;
            k++
        :: (k >= N_KEYS) ->
            break
        od;
        assert(pool_used == live)                 /* pool accounting */
    }
}
