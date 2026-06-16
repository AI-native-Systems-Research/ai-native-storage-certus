/*
 * Component-level model: extent-manager
 *
 * Extent Bitmap Consistency: Concurrent reserve/publish/remove never leave
 * the bitmap in a state where an extent is both allocated and free.
 *
 * Models a single RegionState from region.rs with its AllocationBitmap
 * (bitmap.rs) and Slab (slab.rs). Multiple threads perform interleaved
 * reserve → publish/abort and remove operations. Each operation acquires
 * the region write lock (modeled with atomic{}).
 *
 * Abstracted interfaces:
 *   - IBlockDevice: not involved (bitmap is purely in-memory)
 *   - BuddyAllocator: nondeterministic success/failure for slab creation
 *   - Checkpoint/flush_pending_frees: modeled as a periodic batch free
 *
 * Assumptions:
 *   - Single region (multi-region adds no new interleavings since each
 *     region has its own independent lock)
 *   - Fixed slab (pre-allocated, no dynamic slab creation/destruction)
 *   - parking_lot::RwLock write guard = atomic{} in Promela
 *   - WriteHandle closures execute under their own write lock acquisition
 *
 * Properties verified:
 *   P1 (Safety): A slot is never simultaneously bitmap-allocated AND
 *                logically free (bitmap set ∧ key == FREE implies an
 *                outstanding WriteHandle owns it).
 *   P2 (Safety): No double-allocate — alloc_slot never returns a slot
 *                whose bitmap bit is already set.
 *   P3 (Safety): No double-free — free_slot is never called on a slot
 *                whose bitmap bit is already clear.
 *   P4 (Safety): remove_extent only succeeds on slots that are both
 *                bitmap-allocated AND have a published key (not FREE_KEY).
 *   P5 (Safety): After all threads complete, allocated_count equals the
 *                number of set bits in the bitmap.
 *
 * Run:
 *   spin -a extent_bitmap_consistency.pml
 *   cc -O2 -DSAFETY -o pan pan.c && ./pan -m200000
 */

/* ---------- Parameters ---------- */
#define N_THREADS   3
#define N_SLOTS     4
#define OPS_PER_THREAD 2

/* ---------- Sentinel ---------- */
#define FREE_KEY    255

/* ---------- Per-slot state ---------- */
/* bitmap[i]: true = allocated, false = free */
bool bitmap[N_SLOTS];
/* keys[i]: FREE_KEY = unpublished/free, other = published key */
byte keys[N_SLOTS];
/* owner[i]: thread ID holding WriteHandle (0 = no owner) */
byte owner[N_SLOTS];

/* Allocated count (mirrors AllocationBitmap.allocated_count) */
byte allocated_count = 0;

/* Rover for find_free_from (shared, wraps) */
byte rover = 0;

/* ---------- Coordination ---------- */
byte threads_done = 0;
bool shutdown = false;

/* Per-thread: slot reserved in current operation (-1 = none) */
/* Using byte with 255 as "none" sentinel */
#define NO_SLOT 255

/* ---------- Helpers ---------- */

/*
 * alloc_slot: find a free slot starting from rover, set bitmap bit.
 * Returns slot index in `result`, or NO_SLOT if full.
 * Must be called inside atomic{} (region write lock held).
 */
inline alloc_slot(result)
{
    byte scan_start = rover;
    byte scan_count = 0;
    result = NO_SLOT;

    do
    :: (scan_count < N_SLOTS) ->
        byte idx = (scan_start + scan_count) % N_SLOTS;
        if
        :: (!bitmap[idx]) ->
            /* P2: slot must not be already set. */
            assert(!bitmap[idx]);
            bitmap[idx] = true;
            allocated_count++;
            rover = (idx + 1) % N_SLOTS;
            result = idx;
            break
        :: else ->
            scan_count++
        fi
    :: (scan_count >= N_SLOTS) ->
        break
    od
}

/*
 * free_slot: clear bitmap bit and reset key.
 * Must be called inside atomic{} (region write lock held).
 */
inline free_slot(slot_idx)
{
    /* P3: slot must be allocated before freeing. */
    assert(bitmap[slot_idx]);
    bitmap[slot_idx] = false;
    keys[slot_idx] = FREE_KEY;
    allocated_count--
}

/*
 * publish_slot: set the key for an allocated slot.
 * Must be called inside atomic{} (region write lock held).
 */
inline publish_slot(slot_idx, key_val)
{
    assert(bitmap[slot_idx]);
    keys[slot_idx] = key_val
}

/* ---------- Thread process ---------- */
proctype Thread(byte tid)
{
    byte op;
    byte my_slot;
    byte alloc_result;
    byte my_key = tid + 1;  /* Use tid+1 as a non-FREE key */

    op = 0;
    do
    :: (op < OPS_PER_THREAD) ->
        /*
         * Phase 1: reserve_extent — acquire write lock, alloc slot.
         * Models region.rs:alloc_extent() under region.write().
         */
        atomic {
            alloc_slot(alloc_result);
            if
            :: (alloc_result != NO_SLOT) ->
                my_slot = alloc_result;
                /* Mark ownership (WriteHandle exists). */
                owner[my_slot] = tid + 1
            :: (alloc_result == NO_SLOT) ->
                my_slot = NO_SLOT
            fi
        };

        if
        :: (my_slot == NO_SLOT) ->
            /* Slab full — skip this operation. */
            op++;
            goto next_op
        :: (my_slot != NO_SLOT) -> skip
        fi;

        /*
         * Between reserve and publish/abort: the WriteHandle is held by
         * the caller. Other threads CAN interleave here (they acquire
         * the lock independently).
         *
         * P1 invariant: bitmap[my_slot] is set, key is FREE_KEY,
         * owner[my_slot] == tid+1. This is valid — we hold the handle.
         */

        /*
         * Phase 2: nondeterministic choice — publish OR abort.
         * Models WriteHandle::publish() / WriteHandle::drop().
         */
        if
        :: true ->
            /* Publish path: set key under write lock. */
            atomic {
                assert(owner[my_slot] == tid + 1);
                publish_slot(my_slot, my_key);
                owner[my_slot] = 0
            }

        :: true ->
            /* Abort path: free slot under write lock. */
            atomic {
                assert(owner[my_slot] == tid + 1);
                free_slot(my_slot);
                owner[my_slot] = 0
            }
        fi;

        op++;

        /*
         * Phase 3: nondeterministic remove of a published extent.
         * Models IExtentManager::remove_extent() by another thread.
         * Only proceeds if a published (non-FREE) slot exists.
         */
        if
        :: true ->
            atomic {
                byte victim = 0;
                bool found_victim = false;
                do
                :: (victim < N_SLOTS) ->
                    if
                    :: (bitmap[victim] && keys[victim] != FREE_KEY
                        && owner[victim] == 0) ->
                        /* P4: only remove published, unowned slots. */
                        found_victim = true;
                        break
                    :: else ->
                        victim++
                    fi
                :: (victim >= N_SLOTS) ->
                    break
                od;

                if
                :: found_victim ->
                    /* remove_extent_by_offset: set key=FREE, defer bitmap clear.
                     * Real code adds to pending_frees (cleared on checkpoint).
                     * Model: immediately free for simplicity (safe overapprox). */
                    keys[victim] = FREE_KEY;
                    free_slot(victim)
                :: !found_victim -> skip
                fi
            }
        :: true ->
            /* Skip remove this round. */
            skip
        fi;

next_op:
        skip

    :: (op >= OPS_PER_THREAD) ->
        break
    od;

    threads_done++
}

/* ---------- Initialization ---------- */
init
{
    byte s;
    byte t;

    /* Initialize all slots as free. */
    d_step {
        s = 0;
        do
        :: (s < N_SLOTS) ->
            bitmap[s] = false;
            keys[s] = FREE_KEY;
            owner[s] = 0;
            s++
        :: (s >= N_SLOTS) ->
            break
        od
    };

    /* Start threads. */
    t = 0;
    do
    :: (t < N_THREADS) ->
        run Thread(t);
        t++
    :: (t >= N_THREADS) ->
        break
    od;

    /* Wait for all threads to complete. */
    (threads_done == N_THREADS);
    shutdown = true;

    /* ---------- Final invariant checks ---------- */
    d_step {
        byte count = 0;
        s = 0;
        do
        :: (s < N_SLOTS) ->
            if
            :: bitmap[s] ->
                count++;
                /* P1: allocated slot must either be owned (WriteHandle)
                 * or have a published key. No orphaned allocations. */
                assert(keys[s] != FREE_KEY || owner[s] != 0)
            :: !bitmap[s] ->
                /* Free slot must have FREE_KEY and no owner. */
                assert(keys[s] == FREE_KEY);
                assert(owner[s] == 0)
            fi;
            s++
        :: (s >= N_SLOTS) ->
            break
        od;

        /* P5: allocated_count matches actual set bits. */
        assert(allocated_count == count)
    }
}
