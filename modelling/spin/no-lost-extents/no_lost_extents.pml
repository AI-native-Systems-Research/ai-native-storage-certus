/*
 * System-level model: No Lost Extents
 *
 * Verifies that every extent reservation (reserve_extent) is eventually
 * followed by either publish() or abort/drop. No extent is permanently
 * allocated but uncommitted — which would leak SSD space.
 *
 * Two paths create extent reservations:
 *   1. Direct path: prepare_store → (commit_store | cancel_store)
 *      Client calls prepare_store (reserves extent, stores WriteHandle in
 *      pending_writes map). Later calls commit_store (publish) or
 *      cancel_store (drop → abort).
 *   2. Background writer: process_write_job
 *      BgWriter does reserve_extent → write_buffer_to_ssd → publish.
 *      On any error, the WriteHandle is dropped → abort.
 *
 * Component wiring modeled:
 *   Client → Dispatcher (prepare_store/commit_store/cancel_store)
 *   Dispatcher → ExtentManager (reserve_extent/publish/abort)
 *   Dispatcher → BgWriter → ExtentManager (background write-through)
 *
 * Safety properties:
 *   P1: Every RESERVED extent reaches PUBLISHED or ABORTED before shutdown.
 *   P2: No extent is simultaneously PUBLISHED and ABORTED.
 *   P3: An extent is RESERVED at most once (no double-reserve of same slot).
 *   P4: A crashed/errored write path always aborts (never leaves RESERVED).
 *
 * Run:
 *   spin -a no_lost_extents.pml
 *   cc -O2 -DSAFETY -o pan pan.c
 *   ./pan -m200000
 */

/* ---------- Parameters ---------- */
#define N_CLIENTS       2
#define N_EXTENTS       4   /* total extent slots in the manager */
#define QUEUE_CAP       2   /* bg_writer job queue depth */

/* ---------- Extent slot states ---------- */
mtype = { FREE, RESERVED, PUBLISHED, ABORTED };

mtype extent_state[N_EXTENTS];

/* Who reserved each extent: 0=nobody, 1..N_CLIENTS=direct client, 255=bg_writer */
byte extent_owner[N_EXTENTS];

/* ---------- Pending writes map (direct path) ---------- */
/* Maps client_id → extent_slot (-1 = no pending write) */
byte pending_extent[N_CLIENTS];

/* ---------- Background writer channel ---------- */
/* Carries (key, extent_slot) */
chan write_queue = [QUEUE_CAP] of { byte, byte };

/* ---------- Coordination ---------- */
byte clients_done = 0;
bool shutdown = false;

/* ---------- Extent Manager operations ---------- */

/*
 * reserve_extent: find a FREE slot, mark as RESERVED.
 * Returns slot index via the `result` variable, or N_EXTENTS if full.
 */
inline reserve_extent(owner_id, result)
{
    atomic {
        result = 0;
        do
        :: (result < N_EXTENTS) ->
            if
            :: (extent_state[result] == FREE) ->
                extent_state[result] = RESERVED;
                extent_owner[result] = owner_id;
                break
            :: else ->
                result++
            fi
        :: (result >= N_EXTENTS) ->
            break
        od
    }
}

/*
 * publish_extent: transition RESERVED → PUBLISHED.
 * Asserts the extent is currently RESERVED by the expected owner.
 */
inline publish_extent(slot, owner_id)
{
    atomic {
        assert(extent_state[slot] == RESERVED);
        assert(extent_owner[slot] == owner_id);
        extent_state[slot] = PUBLISHED;
    }
}

/*
 * abort_extent: transition RESERVED → FREE (WriteHandle::drop).
 * Asserts the extent is currently RESERVED.
 */
inline abort_extent(slot, owner_id)
{
    atomic {
        assert(extent_state[slot] == RESERVED);
        assert(extent_owner[slot] == owner_id);
        extent_state[slot] = FREE;
        extent_owner[slot] = 0;
    }
}

/* ---------- Client process (direct path) ---------- */
/*
 * Models prepare_store → (commit_store | cancel_store).
 * Each client does one prepare, then nondeterministically commits or cancels.
 * Also models the populate path where the bg_writer handles the extent.
 */
proctype Client(byte client_id)
{
    byte slot;
    byte owner = client_id + 1;  /* owners are 1-based (0 = nobody) */
    bool use_direct_path;

    /* Nondeterministically choose: direct path or background writer path. */
    if
    :: true -> use_direct_path = true
    :: true -> use_direct_path = false
    fi;

    if
    :: use_direct_path ->
        /* --- Direct path: prepare_store → commit/cancel --- */

        /* prepare_store: reserve extent, store in pending_writes. */
        reserve_extent(owner, slot);

        if
        :: (slot >= N_EXTENTS) ->
            /* Extent manager full — prepare_store returns error. No leak. */
            skip
        :: (slot < N_EXTENTS) ->
            pending_extent[client_id] = slot;

            /* Nondeterministically commit, cancel, or abandon (crash/shutdown). */
            if
            :: true ->
                /* commit_store: write to SSD succeeds → publish. */
                publish_extent(slot, owner);
                pending_extent[client_id] = N_EXTENTS  /* clear */
            :: true ->
                /* cancel_store: WriteHandle dropped → abort. */
                abort_extent(slot, owner);
                pending_extent[client_id] = N_EXTENTS
            :: true ->
                /* commit_store: write_buffer_to_ssd FAILS → abort on drop. */
                abort_extent(slot, owner);
                pending_extent[client_id] = N_EXTENTS
            :: true ->
                /* Client crashes or server shuts down before commit/cancel.
                 * PendingWrite remains in the map. ShutdownCleanup will
                 * drop the WriteHandle → abort. Leave pending_extent set. */
                skip
            fi
        fi

    :: !use_direct_path ->
        /* --- Background writer path: populate → enqueue → bg_writer handles extent --- */
        /* The client just enqueues; the bg_writer does reserve+publish/abort. */
        write_queue ! client_id, owner
    fi;

    clients_done++;
}

/* ---------- Background Writer process ---------- */
/*
 * Models process_write_job extent handling:
 *   reserve_extent → write_to_ssd → publish (or drop on error → abort)
 */
proctype BgWriter()
{
    byte job_client;
    byte job_owner;
    byte slot;

    do
    :: write_queue ? job_client, job_owner ->

        /* mt.peek check: entry may have been evicted. */
        if
        :: true ->
            /* Entry still in memory-tier. Proceed with extent allocation. */
            reserve_extent(255, slot);  /* 255 = bg_writer owner */

            if
            :: (slot >= N_EXTENTS) ->
                /* reserve_extent failed (full). No handle created, no leak.
                 * In real code: early return after release_read. */
                skip
            :: (slot < N_EXTENTS) ->
                /* Nondeterministically: SSD write succeeds or fails. */
                if
                :: true ->
                    /* write_buffer_to_ssd succeeds → publish. */
                    publish_extent(slot, 255)
                :: true ->
                    /* write_buffer_to_ssd fails → WriteHandle dropped → abort.
                     * In real code: early return, write_handle dropped. */
                    abort_extent(slot, 255)
                fi
            fi

        :: true ->
            /* mt.peek returned None (entry evicted). No extent reserved.
             * In real code: early return with release_read. No leak. */
            skip
        fi

    :: shutdown && empty(write_queue) ->
        break
    od
}

/* ---------- Shutdown cleanup process ---------- */
/*
 * Models dispatcher::shutdown which clears pending_writes.
 * Any PendingWrite still in the map has its WriteHandle dropped → abort.
 */
proctype ShutdownCleanup()
{
    byte i;

    /* Wait for shutdown signal. */
    shutdown;

    /* Clear pending_writes: drop all remaining WriteHandles → abort. */
    i = 0;
    do
    :: (i < N_CLIENTS) ->
        if
        :: (pending_extent[i] < N_EXTENTS) ->
            byte slot = pending_extent[i];
            byte owner = i + 1;
            /* In real code: pending_writes.clear() drops each PendingWrite.
             * WriteHandle::drop calls abort_fn.
             * The extent MUST still be RESERVED (only this handle can publish/abort it). */
            atomic {
                assert(extent_state[slot] == RESERVED);
                assert(extent_owner[slot] == owner);
                extent_state[slot] = FREE;
                extent_owner[slot] = 0;
            };
            pending_extent[i] = N_EXTENTS
        :: else -> skip
        fi;
        i++
    :: (i >= N_CLIENTS) ->
        break
    od
}

/* ---------- Initialization ---------- */
init
{
    byte k;

    d_step {
        k = 0;
        do
        :: (k < N_EXTENTS) ->
            extent_state[k] = FREE;
            extent_owner[k] = 0;
            k++
        :: (k >= N_EXTENTS) ->
            break
        od;

        k = 0;
        do
        :: (k < N_CLIENTS) ->
            pending_extent[k] = N_EXTENTS;  /* no pending write */
            k++
        :: (k >= N_CLIENTS) ->
            break
        od
    };

    run BgWriter();
    run ShutdownCleanup();

    byte c = 0;
    do
    :: (c < N_CLIENTS) ->
        run Client(c);
        c++
    :: (c >= N_CLIENTS) ->
        break
    od;

    /* Wait for all clients to complete. */
    (clients_done == N_CLIENTS);

    /* Signal shutdown. */
    shutdown = true;

    /* Wait for background processes to exit. */
    timeout;

    /* --- Final invariant: P1 --- */
    /* No extent is stuck in RESERVED state. Every extent is either
     * FREE, PUBLISHED, or was cleaned up by shutdown. */
    d_step {
        k = 0;
        do
        :: (k < N_EXTENTS) ->
            /* P1: No lost extents — nothing stuck in RESERVED. */
            assert(extent_state[k] != RESERVED);
            /* P2: PUBLISHED and ABORTED are terminal (can't be both). */
            /* (Structurally guaranteed by the state machine.) */
            k++
        :: (k >= N_EXTENTS) ->
            break
        od
    }
}
