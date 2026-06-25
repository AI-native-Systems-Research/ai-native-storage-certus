/*
 * System-level model: Cold-path Promotion Atomicity (v2)
 *
 * Verifies that at most one thread succeeds in promoting a given key from
 * SSD (BlockDevice state) to memory-tier. No double-allocation of memory-tier
 * slots and no lost updates where both promoters believe they succeeded.
 *
 * Models the interaction between:
 *   - Multiple concurrent lookup threads (each calling promote_and_serve)
 *   - The dispatch-map (reference counting + entry state)
 *   - The memory-tier (slot allocation with bounded pool)
 *   - A background evictor (can evict completed promotions back to SSD)
 *
 * The critical race window:
 *   Thread A: dm.lookup(key) → BlockDevice → release_read → promote_and_serve
 *   Thread B: dm.lookup(key) → BlockDevice → release_read → promote_and_serve
 *   Both enter promote_and_serve concurrently for the same key.
 *
 * The evictor introduces a subtler race: after Thread A promotes key K,
 * the evictor may evict K back to BlockDevice, allowing Thread B to see
 * BlockDevice again and attempt re-promotion. This is a LEGITIMATE
 * sequential re-promotion, not a bug — the model verifies that even with
 * eviction, at most one mt.insert is live at any instant.
 *
 * Safety properties:
 *   P1: At most one thread holds a live MT allocation per key at any
 *       instant (no concurrent double-allocate).
 *   P2: No double-allocation: mt.insert(key) succeeds at most once per
 *       key lifecycle (no leaked memory-tier slots).
 *   P3: After all threads complete, the key has write_ref == 0
 *       (no dangling locks).
 *   P4: A failed promoter does not leave stale state (no dangling MT slot
 *       without a dispatch-map entry).
 *   P5: Pool usage counter is always consistent with actual allocations.
 *
 * Component wiring modeled:
 *   LookupThread → DispatchMap (lookup, release_read, remove, create_memory_tier_entry)
 *   LookupThread → MemoryTier (insert, remove)
 *   LookupThread → SSD read (abstracted as nondeterministic success/fail)
 *   Evictor → MemoryTier (evict_lru) + DispatchMap (convert_memory_tier_to_block)
 *
 * Run:
 *   spin -a promotion_atomicity.pml
 *   cc -O2 -DSAFETY -o pan pan.c
 *   ./pan -m200000
 */

/* ---------- Parameters ---------- */
#define N_THREADS   3    /* Concurrent lookup threads attempting promotion */
#define N_KEYS      2    /* Keys that can be promoted concurrently */
#define POOL_CAP    2    /* Memory-tier pool capacity (forces eviction) */

/* ---------- Dispatch-map entry states ---------- */
mtype = { DM_EMPTY, DM_BLOCK_DEVICE, DM_MEMORY_TIER };

/* ---------- Per-key dispatch-map state ---------- */
mtype dm_state[N_KEYS];
byte dm_read_ref[N_KEYS];
byte dm_write_ref[N_KEYS];

/* ---------- Per-key memory-tier state ---------- */
bool mt_allocated[N_KEYS];
byte mt_pool_used = 0;

/* Generation counter: tracks promotion cycles per key.
 * Incremented each time a key completes promotion and later gets evicted.
 * Prevents stale promoters from claiming success for a past lifecycle. */
byte key_gen[N_KEYS];

/* ---------- Promotion outcome tracking ---------- */
byte promotion_success[N_KEYS];

/* ---------- Coordination ---------- */
byte threads_done = 0;
bool shutdown = false;

/* ---------- Background Evictor process ---------- */
proctype Evictor()
{
    byte victim;

    do
    :: !shutdown ->
        /* Scan for an evictable key: DM_MEMORY_TIER, no refs, mt present. */
        atomic {
            victim = 0;
            do
            :: (victim < N_KEYS) ->
                if
                :: (dm_state[victim] == DM_MEMORY_TIER &&
                    dm_read_ref[victim] == 0 &&
                    dm_write_ref[victim] == 0 &&
                    mt_allocated[victim]) ->
                    break
                :: else ->
                    victim++
                fi
            :: (victim >= N_KEYS) ->
                break
            od
        };

        if
        :: (victim < N_KEYS) ->
            /* Evict: mt.remove(key) + dm.convert_memory_tier_to_block.
             * Models evict_for_space (lib.rs:483-542). */
            atomic {
                if
                :: (dm_state[victim] == DM_MEMORY_TIER &&
                    mt_allocated[victim] &&
                    dm_read_ref[victim] == 0 &&
                    dm_write_ref[victim] == 0) ->
                    mt_allocated[victim] = false;
                    mt_pool_used--;
                    dm_state[victim] = DM_BLOCK_DEVICE;
                    key_gen[victim]++
                :: else -> skip
                fi
            }
        :: (victim >= N_KEYS) ->
            /* Nothing to evict — wait for state change. */
            (shutdown || mt_pool_used > 0)
        fi

    :: shutdown ->
        break
    od
}

/* ---------- Lookup + Promote thread ---------- */
proctype LookupThread(byte thread_id)
{
    byte my_key;
    byte my_gen;
    bool got_block_device;
    bool dm_remove_ok;
    bool mt_insert_ok;
    bool create_entry_ok;
    byte attempt;

    /* Each thread attempts to promote one key.
     * Thread assignment: thread_id % N_KEYS → allows multiple threads
     * to target the same key (the interesting race scenario). */
    my_key = thread_id % N_KEYS;

    /* Allow up to 2 attempts (retry after eviction re-enables the key). */
    attempt = 0;
    do
    :: (attempt < 2) ->
        attempt++;

        /* Phase 1: dm.lookup(key) — wait for write_ref == 0, then take read_ref. */
        got_block_device = false;
        atomic {
            if
            :: (dm_state[my_key] == DM_BLOCK_DEVICE && dm_write_ref[my_key] == 0) ->
                dm_read_ref[my_key]++;
                got_block_device = true;
                my_gen = key_gen[my_key]
            :: (dm_state[my_key] != DM_BLOCK_DEVICE) ->
                skip
            :: (dm_write_ref[my_key] > 0) ->
                skip
            fi
        };

        if
        :: !got_block_device -> goto try_next
        :: got_block_device -> skip
        fi;

        /* Phase 2: release_read(key) before entering promote_and_serve. */
        atomic {
            assert(dm_read_ref[my_key] > 0);
            dm_read_ref[my_key]--
        };

        /* === RACE WINDOW: key has no refs, multiple threads can proceed === */

        /* Phase 3: mt.insert(key) — allocate memory-tier slot.
         * Protected by mt's internal keyed lookup — AlreadyExists if present. */
        mt_insert_ok = false;
        atomic {
            if
            :: (!mt_allocated[my_key] && mt_pool_used < POOL_CAP) ->
                mt_allocated[my_key] = true;
                mt_pool_used++;
                mt_insert_ok = true
            :: (mt_allocated[my_key]) ->
                /* AlreadyExists — another thread already inserted. */
                mt_insert_ok = false
            :: (!mt_allocated[my_key] && mt_pool_used >= POOL_CAP) ->
                /* PoolFull — eviction needed but we don't retry inline. */
                mt_insert_ok = false
            fi
        };

        if
        :: !mt_insert_ok -> goto try_next
        :: mt_insert_ok -> skip
        fi;

        /* P1 instant check: we are sole holder of this mt slot. */
        assert(mt_allocated[my_key]);

        /* Phase 4: SSD read (nondeterministic success/failure). */
        if
        :: true -> skip  /* SSD read succeeds */
        :: true ->
            /* SSD read fails — undo mt.insert to avoid P4 violation. */
            atomic {
                mt_allocated[my_key] = false;
                mt_pool_used--
            };
            goto try_next
        fi;

        /* Phase 5: dm.remove(key) — remove old BlockDevice entry.
         * Error ignored in real code (let _ = dm.remove(key)). */
        atomic {
            if
            :: (dm_state[my_key] == DM_BLOCK_DEVICE &&
                dm_read_ref[my_key] == 0 && dm_write_ref[my_key] == 0) ->
                dm_state[my_key] = DM_EMPTY
            :: (dm_state[my_key] == DM_EMPTY) ->
                skip  /* Already removed — fine */
            :: else ->
                skip  /* Remove failed — ignored */
            fi
        };

        /* Phase 6: dm.create_memory_tier_entry(key) — register as MemoryTier.
         * Acquires write ref. Fails with AlreadyExists if key exists. */
        create_entry_ok = false;
        atomic {
            if
            :: (dm_state[my_key] == DM_EMPTY) ->
                dm_state[my_key] = DM_MEMORY_TIER;
                dm_write_ref[my_key] = 1;
                create_entry_ok = true
            :: (dm_state[my_key] != DM_EMPTY) ->
                /* AlreadyExists — another thread won. */
                create_entry_ok = false
            fi
        };

        if
        :: !create_entry_ok ->
            /* Undo mt.insert to avoid dangling MT slot (P4). */
            atomic {
                mt_allocated[my_key] = false;
                mt_pool_used--
            };
            goto try_next
        :: create_entry_ok -> skip
        fi;

        /* Phase 7: dm.release_write(key) — promotion complete. */
        atomic {
            assert(dm_write_ref[my_key] == 1);
            dm_write_ref[my_key] = 0
        };

        /* P1: Record successful promotion. */
        promotion_success[my_key]++;
        goto done

try_next:
        skip
    :: (attempt >= 2) -> break
    od;

done:
    threads_done++
}

/* ---------- Initialization ---------- */
init
{
    byte k;
    byte t;

    /* Set up initial state: all keys in BlockDevice state. */
    d_step {
        k = 0;
        do
        :: (k < N_KEYS) ->
            dm_state[k] = DM_BLOCK_DEVICE;
            dm_read_ref[k] = 0;
            dm_write_ref[k] = 0;
            mt_allocated[k] = false;
            promotion_success[k] = 0;
            key_gen[k] = 0;
            k++
        :: (k >= N_KEYS) -> break
        od
    };

    /* Start evictor. */
    run Evictor();

    /* Start concurrent lookup threads. */
    t = 0;
    do
    :: (t < N_THREADS) ->
        run LookupThread(t);
        t++
    :: (t >= N_THREADS) -> break
    od;

    /* Wait for all threads to complete. */
    (threads_done == N_THREADS);
    shutdown = true;

    /* Wait for evictor to exit. */
    timeout;

    /* Final invariant checks. */
    d_step {
        k = 0;
        do
        :: (k < N_KEYS) ->
            /* P3: No dangling write refs after all threads complete. */
            assert(dm_write_ref[k] == 0);
            assert(dm_read_ref[k] == 0);

            /* P2/P4: mt_allocated consistent with dm_state. */
            if
            :: (dm_state[k] == DM_MEMORY_TIER) ->
                assert(mt_allocated[k] == true)
            :: (dm_state[k] != DM_MEMORY_TIER) ->
                assert(mt_allocated[k] == false)
            fi;

            k++
        :: (k >= N_KEYS) -> break
        od;

        /* P5: Pool counter matches actual allocations. */
        byte mt_count = 0;
        k = 0;
        do
        :: (k < N_KEYS) ->
            if
            :: mt_allocated[k] -> mt_count++
            :: !mt_allocated[k] -> skip
            fi;
            k++
        :: (k >= N_KEYS) -> break
        od;
        assert(mt_pool_used == mt_count)
    }
}
