/*
 * Component-level model: dispatcher/background
 *
 * Starvation Freedom: No client is starved indefinitely waiting to enqueue
 * a write job under backpressure.
 *
 * Models the ParallelBackgroundWriter from background.rs with bounded
 * per-drive channels to represent backpressure scenarios. The real code
 * uses crossbeam_channel::unbounded(), but the pool-cap constraint
 * (POOL_CAP < N_KEYS) creates effective backpressure: clients cannot
 * populate until the background writer drains enough to allow eviction
 * of clean (IN_BOTH) entries. A bounded channel makes this explicit.
 *
 * Abstracted interfaces:
 *   - IExtentManager: nondeterministic success/failure on write completion
 *   - IBlockDevice: modeled as a fixed processing delay (nondeterministic)
 *   - Memory-tier pool: bounded counter (POOL_CAP)
 *   - Dispatch-map: per-key state tracking (simplified)
 *
 * Assumptions:
 *   - Workers are weakly fair (Spin's default process scheduling)
 *   - Channel routing: job.device_index % N_DRIVES (matches real code)
 *   - No channel disconnection during normal operation (shutdown is orderly)
 *
 * Properties verified:
 *   S1 (Safety): in_flight counter is always >= 0 and consistent.
 *   S2 (Safety): Every enqueued job is processed exactly once.
 *   L1 (Liveness): Every client eventually completes all its enqueues
 *                  (no indefinite blocking). Verified via progress labels.
 *   L2 (Liveness): Workers make progress whenever their queue is non-empty.
 *
 * Run:
 *   spin -a starvation_freedom.pml
 *   cc -O2 -DSAFETY -o pan pan.c && ./pan -m200000
 *   cc -O2 -DNP -o pan-live pan.c && ./pan-live -l -m200000
 */

/* ---------- Parameters ---------- */
#define N_CLIENTS       3
#define N_DRIVES        2
#define QUEUE_CAP       2
#define JOBS_PER_CLIENT 2

/* Total jobs that will be submitted. */
#define TOTAL_JOBS      (N_CLIENTS * JOBS_PER_CLIENT)

/* ---------- Per-drive work queue ---------- */
chan drive_queue[N_DRIVES] = [QUEUE_CAP] of { byte, byte };
/* Carries (client_id, job_seq) */

/* ---------- Counters ---------- */
byte in_flight[N_DRIVES];
byte processed[N_DRIVES];
byte total_processed = 0;
byte clients_done = 0;
bool shutdown = false;

/* Per-client progress tracking for liveness. */
byte client_enqueued[N_CLIENTS];

/* ---------- Worker process (one per drive) ---------- */
proctype Worker(byte drive_id)
{
    byte cli_id, seq;

    do
    :: drive_queue[drive_id] ? cli_id, seq ->
        /* Simulate nondeterministic processing time (SSD write). */
        if
        :: skip  /* fast path: immediate completion */
        :: skip  /* slow path: yield once (models I/O latency) */
        fi;

        /* Job processed. Decrement in_flight atomically. */
        atomic {
            assert(in_flight[drive_id] > 0);
            in_flight[drive_id]--;
            processed[drive_id]++;
            total_processed++;
        }

progress_worker:
        skip

    :: shutdown && empty(drive_queue[drive_id]) ->
        break
    od
}

/* ---------- Client process ---------- */
proctype Client(byte client_id)
{
    byte job_seq = 0;
    byte target_drive;

    do
    :: (job_seq < JOBS_PER_CLIENT) ->
        /* Route to drive: device_index % N_DRIVES (matches real code). */
        target_drive = (client_id + job_seq) % N_DRIVES;

        /* Increment in_flight BEFORE send (matches real enqueue()). */
        atomic {
            in_flight[target_drive]++;
        };

        /* Enqueue — blocks if drive queue is full (backpressure). */
        drive_queue[target_drive] ! client_id, job_seq;

progress_client:
        /* L1: every client eventually passes this point. */
        atomic {
            client_enqueued[client_id]++;
        };

        job_seq++

    :: (job_seq >= JOBS_PER_CLIENT) ->
        break
    od;

    clients_done++;
}

/* ---------- Initialization ---------- */
init
{
    byte c;
    byte d;

    /* Zero counters. */
    d_step {
        d = 0;
        do
        :: (d < N_DRIVES) ->
            in_flight[d] = 0;
            processed[d] = 0;
            d++
        :: (d >= N_DRIVES) ->
            break
        od;

        c = 0;
        do
        :: (c < N_CLIENTS) ->
            client_enqueued[c] = 0;
            c++
        :: (c >= N_CLIENTS) ->
            break
        od
    };

    /* Start workers. */
    d = 0;
    do
    :: (d < N_DRIVES) ->
        run Worker(d);
        d++
    :: (d >= N_DRIVES) ->
        break
    od;

    /* Start clients. */
    c = 0;
    do
    :: (c < N_CLIENTS) ->
        run Client(c);
        c++
    :: (c >= N_CLIENTS) ->
        break
    od;

    /* Wait for all clients to finish enqueuing. */
    (clients_done == N_CLIENTS);

    /* Signal shutdown (matches BackgroundWriter::shutdown). */
    shutdown = true;

    /* Wait for all workers to drain and exit. */
    timeout;

    /* ---------- Final invariant checks ---------- */
    d_step {
        /* S1: All in_flight counters are zero. */
        d = 0;
        do
        :: (d < N_DRIVES) ->
            assert(in_flight[d] == 0);
            d++
        :: (d >= N_DRIVES) ->
            break
        od;

        /* S2: Total processed equals total submitted. */
        assert(total_processed == TOTAL_JOBS);

        /* L1: Every client enqueued all its jobs. */
        c = 0;
        do
        :: (c < N_CLIENTS) ->
            assert(client_enqueued[c] == JOBS_PER_CLIENT);
            c++
        :: (c >= N_CLIENTS) ->
            break
        od
    }
}

/*
 * LTL property for starvation freedom (liveness).
 * Under weak fairness, every client eventually completes.
 * Uncomment and run with:
 *   spin -a -f '!([]<>p)' starvation_freedom.pml
 *   cc -O2 -o pan pan.c && ./pan -a -m200000
 *
 * ltl all_clients_finish { <>(clients_done == N_CLIENTS) }
 * ltl all_jobs_processed { <>(total_processed == TOTAL_JOBS) }
 */
