/*
 * System-level model: Cold-path Promotion Atomicity
 *
 * Verifies that at most one thread succeeds in promoting a given key from
 * SSD (BlockDevice state) to memory-tier. No double-allocation of memory-tier
 * slots and no lost updates where both promoters believe they succeeded.
 *
 * Models the interaction between:
 *   - Multiple concurrent lookup threads (each calling promote_and_serve)
 *   - The dispatch-map (reference counting + entry state)
 *   - The memory-tier (slot allocation)
 *
 * The critical race window:
 *   Thread A: dm.lookup(key) → BlockDevice → release_read → promote_and_serve
 *   Thread B: dm.lookup(key) → BlockDevice → release_read → promote_and_serve
 *   Both enter promote_and_serve concurrently for the same key.
 *
 * Real-code resolution (components/dispatch-map/src/lib.rs:336-354):
 *   - dm.remove(key) fails with KeyNotFound if entry already removed
 *   - create_memory_tier_entry(key) fails with AlreadyExists if key exists
 *   - mt.insert(key) may fail with AlreadyExists at the memory-tier level
 *
 * Safety properties:
 *   P1: At most one thread successfully completes promotion for a given key
 *       (exactly one create_memory_tier_entry succeeds).
 *   P2: No double-allocation: mt.insert(key) succeeds at most once per
 *       key lifecycle (no leaked memory-tier slots).
 *   P3: After promotion completes, the key is in MemoryTier state in the
 *       dispatch-map with write_ref released.
 *   P4: A failed promoter does not leave stale state (no dangling MT slot
 *       without a dispatch-map entry).
 *
 * Component wiring modeled:
 *   LookupThread → DispatchMap (lookup, release_read, remove, create_memory_tier_entry)
 *   LookupThread → MemoryTier (insert, remove)
 *   LookupThread → SSD read (abstracted as nondeterministic success/fail)
 *
 * Run:
 *   spin -a promotion_atomicity.pml
 *   cc -O2 -DSAFETY -o pan pan.c
 *   ./pan -m200000
 */

/* ---------- Parameters ---------- */
#define N_THREADS   3    /* Concurrent lookup threads attempting promotion */
#define N_KEYS      2    /* Keys that can be promoted concurrently */

/* ---------- Dispatch-map entry states ---------- */
mtype = { DM_EMPTY, DM_BLOCK_DEVICE, DM_MEMORY_TIER };

/* ---------- Per-key dispatch-map state ---------- */
mtype dm_state[N_KEYS];
byte dm_read_ref[N_KEYS];
byte dm_write_ref[N_KEYS];

/* ---------- Per-key memory-tier state ---------- */
bool mt_allocated[N_KEYS];

/* ---------- Promotion outcome tracking ---------- */
/* Counts how many threads successfully promoted each key. */
byte promotion_success[N_KEYS];

/* ---------- Coordination ---------- */
byte threads_done = 0;

/* ---------- Lookup + Promote thread ---------- */
proctype LookupThread(byte thread_id)
{
    byte my_key;
    bool got_block_device;
    bool dm_remove_ok;
    bool mt_insert_ok;
    bool create_entry_ok;

    /* Each thread attempts to promote one key.
     * Thread assignment: thread_id % N_KEYS → allows multiple threads
     * to target the same key (the interesting race scenario). */
    my_key = thread_id % N_KEYS;

    /* Phase 1: dm.lookup(key) — wait for write_ref == 0, then take read_ref.
     * Models dispatch-map/src/lib.rs:143-183. */
    got_block_device = false;
    atomic {
        if
        :: (dm_state[my_key] == DM_BLOCK_DEVICE && dm_write_ref[my_key] == 0) ->
            dm_read_ref[my_key]++;
            got_block_device = true
        :: (dm_state[my_key] != DM_BLOCK_DEVICE) ->
            skip  /* Not in BlockDevice state — nothing to promote */
        :: (dm_write_ref[my_key] > 0) ->
            skip  /* Writer active — would block/timeout in real code */
        fi
    };

    if
    :: !got_block_device -> goto done
    :: got_block_device -> skip
    fi;

    /* Phase 2: release_read(key) before entering promote_and_serve.
     * Models lib.rs:1324 (single lookup) or batch_lookup cold-path release. */
    atomic {
        assert(dm_read_ref[my_key] > 0);
        dm_read_ref[my_key]--
    };

    /* === RACE WINDOW: key has no refs, multiple threads can proceed === */

    /* Phase 3: promote_and_serve (lib.rs:194-288)
     * Step 3a: mt.insert(key) — allocate memory-tier slot. */
    mt_insert_ok = false;
    atomic {
        if
        :: (!mt_allocated[my_key]) ->
            mt_allocated[my_key] = true;
            mt_insert_ok = true
        :: (mt_allocated[my_key]) ->
            /* AlreadyExists — another thread already inserted. */
            mt_insert_ok = false
        fi
    };

    if
    :: !mt_insert_ok -> goto done  /* Promotion fails gracefully */
    :: mt_insert_ok -> skip
    fi;

    /* Step 3b: SSD read into memory-tier slot (abstracted as nondeterministic). */
    if
    :: true -> skip  /* SSD read succeeds */
    :: true ->
        /* SSD read fails — must undo mt.insert to avoid P4 violation. */
        atomic {
            mt_allocated[my_key] = false
        };
        goto done
    fi;

    /* Step 3c: dm.remove(key) — remove old BlockDevice entry.
     * Fails if entry already removed or has active refs. */
    dm_remove_ok = false;
    atomic {
        if
        :: (dm_state[my_key] == DM_BLOCK_DEVICE &&
            dm_read_ref[my_key] == 0 && dm_write_ref[my_key] == 0) ->
            dm_state[my_key] = DM_EMPTY;
            dm_remove_ok = true
        :: (dm_state[my_key] == DM_EMPTY) ->
            /* Already removed by another promoter — this is fine. */
            dm_remove_ok = true
        :: (dm_state[my_key] == DM_MEMORY_TIER) ->
            /* Another thread already completed promotion. */
            dm_remove_ok = false
        :: (dm_read_ref[my_key] > 0 || dm_write_ref[my_key] > 0) ->
            /* Active references — remove fails. */
            dm_remove_ok = false
        fi
    };

    if
    :: !dm_remove_ok ->
        /* Undo mt.insert to avoid dangling MT slot (P4). */
        atomic { mt_allocated[my_key] = false };
        goto done
    :: dm_remove_ok -> skip
    fi;

    /* Step 3d: dm.create_memory_tier_entry(key) — register as MemoryTier.
     * Fails with AlreadyExists if key already re-registered. */
    create_entry_ok = false;
    atomic {
        if
        :: (dm_state[my_key] == DM_EMPTY) ->
            dm_state[my_key] = DM_MEMORY_TIER;
            dm_write_ref[my_key] = 1;  /* write ref held during promotion */
            create_entry_ok = true
        :: (dm_state[my_key] != DM_EMPTY) ->
            /* AlreadyExists — another thread won the race. */
            create_entry_ok = false
        fi
    };

    if
    :: !create_entry_ok ->
        /* Undo mt.insert to avoid dangling MT slot (P4). */
        atomic { mt_allocated[my_key] = false };
        goto done
    :: create_entry_ok -> skip
    fi;

    /* Step 3e: dm.release_write(key) — promotion complete. */
    atomic {
        assert(dm_write_ref[my_key] == 1);
        dm_write_ref[my_key] = 0
    };

    /* P1: Record successful promotion. */
    promotion_success[my_key]++;

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
            k++
        :: (k >= N_KEYS) -> break
        od
    };

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

    /* Final invariant checks. */
    k = 0;
    do
    :: (k < N_KEYS) ->
        /* P1: At most one thread succeeds in promoting each key. */
        assert(promotion_success[k] <= 1);

        /* P2: mt_allocated matches dm_state — no dangling MT slot. */
        if
        :: (dm_state[k] == DM_MEMORY_TIER) ->
            assert(mt_allocated[k] == true)
        :: (dm_state[k] != DM_MEMORY_TIER) ->
            /* P4: If not in MemoryTier, MT slot must have been freed. */
            assert(mt_allocated[k] == false ||
                   (dm_state[k] == DM_BLOCK_DEVICE && promotion_success[k] == 0))
        fi;

        /* P3: No dangling write refs after all threads complete. */
        assert(dm_write_ref[k] == 0);
        assert(dm_read_ref[k] == 0);

        k++
    :: (k >= N_KEYS) -> break
    od
}
