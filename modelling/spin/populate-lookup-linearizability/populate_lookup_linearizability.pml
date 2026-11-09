/*
 * System-level model: Populate-Lookup Linearizability
 *
 * Verifies the Certus dispatcher guarantee:
 *
 *   After populate(key) returns Ok, a concurrent lookup(key) NEVER observes
 *   KeyNotFound, UNLESS an explicit remove(key) or a memory-tier eviction
 *   (the "drop" path) has removed the entry from the dispatch-map in between.
 *
 * Component wiring modeled (as wired in certus-server):
 *
 *   client(populate) --> dispatcher.reserve_memory --> memory-tier.insert
 *                    --> dispatcher.copy_gpu_to_memory_completed
 *                        --> dispatch-map.create_memory_tier_entry   (VISIBLE here)
 *                        --> dispatch-map.downgrade_reference         (read-ref for writer)
 *                        --> bg_writer.enqueue(write-through)
 *   client(lookup)   --> dispatcher.lookup_async --> dispatch-map.lookup (pins read-ref)
 *   bg_writer        --> block-device write --> dispatch-map.convert_to_storage
 *   evictor          --> dispatcher.evict_one_clean
 *                        --> demote: dispatch-map.convert_to_storage (stays a HIT)
 *                        --> drop:   dispatch-map.remove             (becomes KeyNotFound)
 *
 * Key modeling decisions matching the real code (components/dispatcher/src/lib.rs):
 *   - The linearization point of populate is create_memory_tier_entry
 *     (lib.rs:3044): the key is inserted into the dispatch-map BEFORE populate
 *     returns Ok. `mt.insert` (phase 1) only reserves a pinned, write-ref'd slot
 *     that is NOT yet visible to dispatch-map lookups.
 *   - downgrade_reference (lib.rs:3088) installs one read-ref held by the
 *     background writer; it is released when the write-through completes. So a
 *     freshly-populated entry is pinned and cannot be evicted or removed until
 *     the writer runs.
 *   - dm.lookup (lib.rs:3676) increments read_ref for MemoryTier / BlockDevice
 *     hits (pinning the entry), and returns unpinned for NotExist / MismatchSize.
 *   - evict_one_clean (lib.rs:920) and remove (lib.rs:3810) reject entries with
 *     read_refs > 0. Demote (convert_to_storage, lib.rs:3730) is atomic under the
 *     DM mutex and keeps the entry resolvable (BlockDevice => cold-path promote).
 *
 * Safety properties verified:
 *   P-LIN:  A lookup that observes KeyNotFound implies the key was either never
 *           populated (in this generation) or a remove/eviction-drop departed it.
 *           (Enforced at the dispatch-map read in Looker.)
 *   P-PIN:  While a lookup holds its read-ref, the entry stays committed
 *           (no remove/drop under an in-flight load). (Assert during serve.)
 *   P-REF:  Removal paths (drop, remove) only fire at read_refs == 0.
 *   P-WJOB: A queued write-through job always finds its entry still committed,
 *           resident in the memory-tier, and pinned by the writer's read-ref.
 *   P-FIN:  At quiescence, every not-committed key has departed or was never
 *           populated, and no read-refs are leaked.
 *
 * Parameters tuned for full coverage:
 *   N_KEYS=2, POOL_CAP=1 -> populates contend for the single slot, forcing
 *   eviction (demote / drop) and exercising every departure path.
 */

/* ---------- Parameters ---------- */
#define N_KEYS      2
#define POOL_CAP    1
#define N_LOOK      2      /* number of concurrent Looker processes */
#define LOOKUPS     1      /* lookups performed by each Looker */
#define MAX_TRIES   4      /* max_eviction_attempts in reserve_memory */

/* ---------- Per-key state ---------- */
mtype = { MT, SSD };       /* dispatch-map location when committed */

bool committed[N_KEYS];    /* present in dispatch-map (a lookup would HIT) */
mtype location[N_KEYS];    /* MT = memory-tier, SSD = block-device (demoted) */
bool persisted[N_KEYS];    /* write-through complete (ssd_offset set) */
byte read_refs[N_KEYS];    /* dispatch-map read references (pins) */
bool write_ref[N_KEYS];    /* exclusive write ref held during populate phase 1-2 */
bool in_pool[N_KEYS];      /* occupies a memory-tier pool slot */
byte pool_used = 0;

/*
 * Generation counter per key (incremented on each successful commit). Lets the
 * background writer detect a stale job after key reuse, mirroring the real
 * mt.peek generation check.
 */
byte gen[N_KEYS];

/*
 * pop_ok[k]   : populate(k) has returned Ok for the current generation.
 * departed[k] : a remove/eviction-drop has removed k from the dispatch-map
 *               since it was last committed. Reset when k is re-committed.
 * Together they let Looker distinguish a legitimate miss (never populated, or
 * departed) from a linearizability violation (visible-then-vanished).
 */
bool pop_ok[N_KEYS];
bool departed[N_KEYS];

/* ---------- Write-through job channel ---------- */
/* Carries (key_index, generation). */
chan write_q = [N_KEYS] of { byte, byte };

/* ---------- Coordination ---------- */
byte pop_done = 0;
byte look_done = 0;
bool shutdown = false;

/* ---------- Eviction: free one pin-safe victim ---------- */
/*
 * Models dispatcher::evict_one_clean. Scans for a committed, pool-resident,
 * unpinned victim. Preference: demote to SSD if persisted (keeps it a HIT);
 * otherwise drop it (becomes KeyNotFound). Pinned/unpersisted-and-write-ref'd
 * entries are skipped, never blind-freed. `freed` reports whether a slot was
 * reclaimed. The scan-and-act is atomic (real code holds the MT/DM locks).
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
            :: (committed[v] && in_pool[v] && read_refs[v] == 0 && !write_ref[v]) ->
                break
            :: else ->
                v++
            fi
        :: (v >= N_KEYS) ->
            break
        od;

        if
        :: (v < N_KEYS && committed[v] && in_pool[v]
            && read_refs[v] == 0 && !write_ref[v]) ->
            if
            :: persisted[v] ->
                /* Demote: convert_to_storage. Entry stays resolvable. */
                location[v] = SSD;
                in_pool[v] = false;
                pool_used--;
                freed = true
            :: else ->
                /* Drop: dm.remove. Write-through not done => block is lost. */
                assert(read_refs[v] == 0);        /* P-REF */
                committed[v] = false;
                departed[v] = true;
                in_pool[v] = false;
                pool_used--;
                freed = true
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

    /* Phase 1: reserve a memory-tier slot (mt.insert), evicting under pressure. */
    do
    :: !reserved ->
        atomic {
            if
            :: (!in_pool[k] && !committed[k] && pool_used < POOL_CAP) ->
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
            /* evict_for_space: try to free a pin-safe slot, bounded attempts. */
            evict_one_clean(freed);
            if
            :: freed ->
                skip
            :: !freed ->
                tries++;
                if
                :: (tries >= MAX_TRIES) ->
                    break            /* AllocationFailed: populate returns Err */
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
        /* Phase 2: async DMA copy GPU -> reserved slot (a schedulable point). */
        skip;

        /*
         * Phase 3: create_memory_tier_entry makes the key VISIBLE, then
         * downgrade_reference converts the write-ref into a read-ref held by the
         * background writer. populate returns Ok immediately after (pop_ok).
         * These share the dispatch-map mutex in the real code, so they are one
         * atomic step here. Setting pop_ok BEFORE committed would break P-LIN.
         */
        atomic {
            committed[k] = true;
            location[k] = MT;
            persisted[k] = false;
            departed[k] = false;
            gen[k]++;
            write_ref[k] = false;            /* downgrade_reference */
            read_refs[k]++;                  /* writer's read-ref */
            pop_ok[k] = true;                /* populate returned Ok */
        };

        /* bg_writer.enqueue — separate step so lookups can interleave. */
        write_q ! k, gen[k]
    :: !reserved ->
        skip                                 /* populate failed; P-LIN N/A */
    fi;

    pop_done++
}

/* ---------- Populator process (one per key) ---------- */
proctype Populator(byte k)
{
    do_populate(k)
}

/* ---------- Background writer ---------- */
proctype BgWriter()
{
    byte jk, jg;

    do
    :: write_q ? jk, jg ->
        /*
         * P-WJOB: while the job was queued, the entry could not be evicted or
         * removed (the writer's read-ref pins it), so it is still committed,
         * resident in the memory-tier, and this generation's.
         *
         * The write-through either completes (persisted => later evictable by
         * demote-to-SSD) or fails (IoError: ref released but persisted stays
         * false => the entry is now an unpinned, unpersisted candidate that
         * evict_one_clean must DROP rather than demote).
         */
        atomic {
            assert(committed[jk] && gen[jk] == jg && location[jk] == MT);
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

/* ---------- Explicit remove (one bounded attempt) ---------- */
proctype Remover()
{
    byte k;

    select(k : 0 .. N_KEYS - 1);
    atomic {
        if
        :: (committed[k] && read_refs[k] == 0 && !write_ref[k]) ->
            /* dm.remove succeeds only when unpinned (P-REF). */
            assert(read_refs[k] == 0);
            committed[k] = false;
            departed[k] = true;
            if
            :: in_pool[k] ->
                in_pool[k] = false;
                pool_used--
            :: else ->
                skip
            fi
        :: else ->
            skip                             /* ActiveReferences / KeyNotFound */
        fi
    }
}

/* ---------- Looker process (concurrent lookups) ---------- */
proctype Looker()
{
    byte n = 0;
    byte k;
    bool hit;
    bool saw_pop_ok;
    bool saw_departed;

    do
    :: (n < LOOKUPS) ->
        n++;
        select(k : 0 .. N_KEYS - 1);

        /*
         * dm.lookup — the lookup's linearization point. Snapshot committed,
         * pop_ok, and departed together under the dispatch-map mutex: the P-LIN
         * check must reason about the state AT this instant, not a later one
         * (a miss now is legitimate if populate has not yet returned Ok now,
         * even if it commits a moment afterwards). Pin the entry on a hit.
         */
        atomic {
            hit = committed[k];
            saw_pop_ok = pop_ok[k];
            saw_departed = departed[k];
            if
            :: committed[k] ->
                read_refs[k]++               /* MemoryTier/BlockDevice => pinned */
            :: else ->
                skip                         /* NotExist => unpinned */
            fi
        };

        if
        :: !hit ->
            /*
             * P-LIN: a KeyNotFound is legitimate only if, at the lookup instant,
             * the key was never populated in this generation (!saw_pop_ok), or a
             * remove/eviction-drop had already departed it (saw_departed). A
             * "visible-then-vanished" entry (populated, not departed, yet
             * missing) would violate linearizability.
             */
            assert(!saw_pop_ok || saw_departed)
        :: hit ->
            /*
             * Serve phase (H2D DMA / cold-path promote-and-serve). Interleaves
             * with everything else. P-PIN: the read-ref we hold forbids any
             * remove/drop, so the entry must remain committed throughout.
             */
            assert(committed[k]);
            atomic {
                assert(read_refs[k] > 0);
                read_refs[k]--               /* release_read */
            }
        fi
    :: (n >= LOOKUPS) ->
        break
    od;

    look_done++
}

/* ---------- Initialization ---------- */
init
{
    byte k;

    d_step {
        k = 0;
        do
        :: (k < N_KEYS) ->
            committed[k] = false;
            location[k] = MT;
            persisted[k] = false;
            read_refs[k] = 0;
            write_ref[k] = false;
            in_pool[k] = false;
            gen[k] = 0;
            pop_ok[k] = false;
            departed[k] = false;
            k++
        :: (k >= N_KEYS) ->
            break
        od
    };

    run BgWriter();
    run Evictor();
    run Remover();

    /* One Populator per key. */
    k = 0;
    do
    :: (k < N_KEYS) ->
        run Populator(k);
        k++
    :: (k >= N_KEYS) ->
        break
    od;

    /* N_LOOK concurrent Lookers. */
    k = 0;
    do
    :: (k < N_LOOK) ->
        run Looker();
        k++
    :: (k >= N_LOOK) ->
        break
    od;

    /* Wait for all populates and lookups to finish. */
    (pop_done == N_KEYS && look_done == N_LOOK);

    /* Signal shutdown and let background processes drain. */
    shutdown = true;
    timeout;

    /* Final invariants (P-FIN). */
    d_step {
        k = 0;
        do
        :: (k < N_KEYS) ->
            /* Not committed => departed or never populated. */
            assert(committed[k] || departed[k] || !pop_ok[k]);
            /* No leaked read-refs at quiescence. */
            assert(read_refs[k] == 0);
            /* Pool accounting is consistent. */
            assert(!in_pool[k] || committed[k]);
            k++
        :: (k >= N_KEYS) ->
            break
        od
    }
}
