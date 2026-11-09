/*
 * Certus Dispatcher: Write-Before-Evict Safety Model (v3)
 *
 * Verifies that the memory-tier eviction logic never produces a dangling
 * BlockDevice reference (pointing to SSD data that was never written).
 *
 * Key modeling decisions matching the real code (lib.rs:296-348):
 *   - mt.evict_lru() frees the pool slot UNCONDITIONALLY (pool_used--)
 *   - THEN tries dm.convert_memory_tier_to_block (may fail if no ssd_offset)
 *   - If convert fails, does dm.remove (may fail if read_refs > 0)
 *
 * The core invariant is that has_ssd_offset is ONLY set by the background
 * writer at the moment it completes the SSD write. Therefore:
 *   convert_memory_tier_to_block succeeds ⟹ has_ssd_offset ⟹ data is on SSD.
 *
 * Safety properties verified:
 *   P1: has_ssd_offset is only set simultaneously with write completion.
 *       (Structural — enforced by BgWriter being the only setter.)
 *   P2: ON_SSD state requires has_ssd_offset (no dangling references).
 *   P3: LOST/EVICTED_PENDING → !in_dispatch_map OR read_refs will drain.
 *   P4: dm.remove never succeeds while read_refs > 0.
 *
 * Parameters tuned for full coverage:
 *   POOL_CAP=2, QUEUE_CAP=1 → writer backpressure, frequent blind-LRU.
 */

/* ---------- Parameters ---------- */
#define N_CLIENTS   2
#define N_KEYS      4
#define POOL_CAP    2
#define QUEUE_CAP   1

/* ---------- Per-key state ---------- */
mtype = { EMPTY, IN_DRAM, IN_DRAM_QUEUED, IN_BOTH, ON_SSD, LOST, EVICTED_PENDING };

mtype key_state[N_KEYS];
byte read_refs[N_KEYS];
byte pool_used = 0;
bool in_dispatch_map[N_KEYS];
bool has_ssd_offset[N_KEYS];
bool in_pool[N_KEYS];

/*
 * Generation counter per key. Incremented on each populate.
 * Prevents stale writer completions from setting has_ssd_offset
 * on a re-populated key (models the real code's mt.peek→None check).
 */
byte key_gen[N_KEYS];
/* The generation the writer holds for the current job. */
byte writer_job_gen;

/* ---------- Write job channel ---------- */
/* Carries (key_index, generation) */
chan write_queue = [QUEUE_CAP] of { byte, byte };

/* ---------- Coordination ---------- */
byte clients_done = 0;
bool shutdown = false;

/* ---------- Eviction logic ---------- */
inline evict_one()
{
    byte victim;
    bool found_clean = false;
    bool did_evict = false;

    /*
     * Clean eviction: atomic scan-and-act (real code holds MT mutex).
     * Look for IN_BOTH entry with ssd_offset and no refs.
     */
    atomic {
        victim = 0;
        do
        :: (victim < N_KEYS) ->
            if
            :: (key_state[victim] == IN_BOTH && has_ssd_offset[victim]
                && read_refs[victim] == 0 && in_pool[victim]) ->
                found_clean = true;
                break
            :: else ->
                victim++
            fi
        :: (victim >= N_KEYS) ->
            break
        od;

        if
        :: found_clean ->
            /* P2: transitioning to ON_SSD requires has_ssd_offset. */
            assert(has_ssd_offset[victim]);
            key_state[victim] = ON_SSD;
            in_pool[victim] = false;
            pool_used--;
            did_evict = true
        :: !found_clean -> skip
        fi
    };

    if
    :: !did_evict ->
        /*
         * Blind LRU: mt.evict_lru() pops oldest entry unconditionally.
         * Two-phase: free pool slot, then handle DM.
         */
        atomic {
            victim = 0;
            do
            :: (victim < N_KEYS) ->
                if
                :: (in_pool[victim] &&
                    (key_state[victim] == IN_DRAM_QUEUED ||
                     key_state[victim] == IN_BOTH ||
                     key_state[victim] == IN_DRAM)) ->
                    break
                :: else ->
                    victim++
                fi
            :: (victim >= N_KEYS) ->
                break
            od;

            /* When pool_used >= POOL_CAP, at least one in-pool entry must exist. */
            assert(victim < N_KEYS && in_pool[victim]);
            /* Phase A: mt.evict_lru() — free pool slot. */
            in_pool[victim] = false;
            pool_used--;
            did_evict = true
        };

        /* Phase B: DM transition (non-atomic with Phase A). */
        if
        :: did_evict ->
            if
            :: (has_ssd_offset[victim]) ->
                /* convert_memory_tier_to_block succeeds. */
                /* P2: has_ssd_offset guarantees data was written. */
                atomic {
                    key_state[victim] = ON_SSD
                }
            :: (!has_ssd_offset[victim]) ->
                /* convert fails. Try dm.remove. */
                atomic {
                    if
                    :: (read_refs[victim] == 0) ->
                        /* P4: only remove when refs == 0. */
                        key_state[victim] = LOST;
                        in_dispatch_map[victim] = false
                    :: (read_refs[victim] > 0) ->
                        /* dm.remove fails. Entry is pool-evicted but DM persists.
                         * Writer still holds ref; it will release, then cleanup. */
                        key_state[victim] = EVICTED_PENDING
                    fi
                }
            fi
        :: !did_evict -> skip
        fi
    :: did_evict -> skip
    fi
}

/* ---------- Client process ---------- */
proctype Client(byte client_id)
{
    byte my_key;
    byte base = client_id * (N_KEYS / N_CLIENTS);
    byte count = N_KEYS / N_CLIENTS;
    byte i = 0;
    bool inserted;

    do
    :: (i < count) ->
        my_key = base + i;
        i++;

        inserted = false;
        do
        :: !inserted ->
            atomic {
                if
                :: (pool_used < POOL_CAP &&
                    (key_state[my_key] == EMPTY || key_state[my_key] == LOST)) ->
                    pool_used++;
                    key_state[my_key] = IN_DRAM;
                    in_dispatch_map[my_key] = true;
                    in_pool[my_key] = true;
                    has_ssd_offset[my_key] = false;
                    key_gen[my_key]++;
                    inserted = true
                :: else ->
                    skip
                fi
            };
            if
            :: !inserted -> evict_one()
            :: inserted -> skip
            fi
        :: inserted -> break
        od;

        /* dm.downgrade_reference: write→read ref for bg_writer. */
        atomic {
            read_refs[my_key]++;
            key_state[my_key] = IN_DRAM_QUEUED;
        };

        /* bg_writer.enqueue — blocks if queue full (backpressure). */
        write_queue ! my_key, key_gen[my_key];

    :: (i >= count) ->
        break
    od;

    clients_done++;
}

/* ---------- Background Writer process ---------- */
proctype BgWriter()
{
    byte job_key;
    byte job_gen;

    do
    :: write_queue ? job_key, job_gen ->

        /*
         * mt.peek check: does the entry still belong to this generation?
         * If the key was evicted and re-populated, key_gen won't match
         * and mt.peek would return a different pointer (or None if evicted).
         */
        if
        :: (key_state[job_key] == IN_DRAM_QUEUED && in_pool[job_key]
            && key_gen[job_key] == job_gen) ->
            /* Normal path: entry still in memory-tier, same generation. */

            /* SSD write completes. Atomically set offset. */
            atomic {
                has_ssd_offset[job_key] = true;
                key_state[job_key] = IN_BOTH;
            };

            /* Release read reference. */
            atomic {
                assert(read_refs[job_key] > 0);
                read_refs[job_key]--;
            }

        :: (key_state[job_key] == IN_DRAM_QUEUED && !in_pool[job_key]
            && key_gen[job_key] == job_gen) ->
            /* Pool was freed (blind LRU) but DM entry may persist.
             * mt.peek returns None → release ref, abort write. */
            atomic {
                assert(read_refs[job_key] > 0);
                read_refs[job_key]--;
            }

        :: (key_state[job_key] == EVICTED_PENDING
            && key_gen[job_key] == job_gen) ->
            /* Entry was pool-evicted while we held the ref.
             * Release ref so cleanup can proceed. */
            atomic {
                assert(read_refs[job_key] > 0);
                read_refs[job_key]--;
            }

        :: (key_state[job_key] == LOST && key_gen[job_key] == job_gen) ->
            /* Entry fully removed before we ran. Release ref. */
            atomic {
                assert(read_refs[job_key] > 0);
                read_refs[job_key]--;
            }

        :: (key_gen[job_key] != job_gen) ->
            /* Stale job from a previous generation. Release ref. */
            atomic {
                assert(read_refs[job_key] > 0);
                read_refs[job_key]--;
            }

        :: else ->
            /* Any other state (IN_BOTH, ON_SSD, EMPTY, IN_DRAM). */
            atomic {
                assert(read_refs[job_key] > 0);
                read_refs[job_key]--;
            }
        fi

    :: shutdown && empty(write_queue) ->
        break
    od
}

/* ---------- Background SSD Evictor process ---------- */
proctype SsdEvictor()
{
    byte victim;
    bool did_work;

    do
    :: !shutdown ->
        did_work = false;

        /* Free SSD space: remove ON_SSD entries. */
        atomic {
            victim = 0;
            do
            :: (victim < N_KEYS) ->
                if
                :: (key_state[victim] == ON_SSD && read_refs[victim] == 0) ->
                    break
                :: else ->
                    victim++
                fi
            :: (victim >= N_KEYS) ->
                break
            od;

            if
            :: (victim < N_KEYS && key_state[victim] == ON_SSD) ->
                /* P2 check: ON_SSD must have had data written. */
                assert(has_ssd_offset[victim]);
                key_state[victim] = EMPTY;
                in_dispatch_map[victim] = false;
                has_ssd_offset[victim] = false;
                did_work = true
            :: else -> skip
            fi
        };

        /* Cleanup EVICTED_PENDING entries whose refs have drained. */
        if
        :: !did_work ->
            atomic {
                victim = 0;
                do
                :: (victim < N_KEYS) ->
                    if
                    :: (key_state[victim] == EVICTED_PENDING
                        && read_refs[victim] == 0) ->
                        break
                    :: else ->
                        victim++
                    fi
                :: (victim >= N_KEYS) ->
                    break
                od;

                if
                :: (victim < N_KEYS && key_state[victim] == EVICTED_PENDING
                    && read_refs[victim] == 0) ->
                    key_state[victim] = LOST;
                    in_dispatch_map[victim] = false;
                    did_work = true
                :: else -> skip
                fi
            }
        :: else -> skip
        fi;

        /* Yield if nothing to do. */
        if
        :: !did_work -> (shutdown || pool_used > 0)
        :: else -> skip
        fi

    :: shutdown ->
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
        :: (k < N_KEYS) ->
            key_state[k] = EMPTY;
            read_refs[k] = 0;
            in_dispatch_map[k] = false;
            has_ssd_offset[k] = false;
            in_pool[k] = false;
            key_gen[k] = 0;
            k++
        :: (k >= N_KEYS) ->
            break
        od
    };

    run BgWriter();
    run SsdEvictor();

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

    /* Final invariant checks. */
    d_step {
        k = 0;
        do
        :: (k < N_KEYS) ->
            /* P2: ON_SSD → has_ssd_offset */
            assert(key_state[k] != ON_SSD || has_ssd_offset[k]);
            /* P3: LOST → !in_dispatch_map */
            assert(key_state[k] != LOST || !in_dispatch_map[k]);
            /* P4: !in_dispatch_map → read_refs == 0 */
            assert(in_dispatch_map[k] || read_refs[k] == 0);
            k++
        :: (k >= N_KEYS) ->
            break
        od
    }
}
