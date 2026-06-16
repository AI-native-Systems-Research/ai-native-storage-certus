# Starvation Freedom

## Scope

Single component: `dispatcher/background` (`ParallelBackgroundWriter`)

## Description

Verifies that no client is starved indefinitely waiting to enqueue a write job
under backpressure. Models the `ParallelBackgroundWriter` with per-drive bounded
channels — multiple client threads compete to push `WriteJob` items into
drive-specific queues, and per-drive worker threads drain them. The bounded
channel capacity creates contention: when a drive's queue is full, clients
targeting that drive block until the worker processes an item.

The model establishes that under Spin's weak fairness (every continuously
enabled process eventually executes), all clients complete their enqueues and
all jobs are eventually processed — no client is indefinitely starved.

## Properties Verified

| ID | Property                                                  | Type     |
| -- | --------------------------------------------------------- | -------- |
| S1 | in_flight counter is always ≥ 0 and drains to 0 at end   | Safety   |
| S2 | Every enqueued job is processed exactly once              | Safety   |
| L1 | Every client eventually completes all enqueues            | Liveness |
| L2 | Workers make progress whenever their queue is non-empty   | Liveness |

## System Abstraction

| Real component                          | Promela process             |
| --------------------------------------- | --------------------------- |
| Client threads calling `enqueue()`      | `Client(id)` × N_CLIENTS   |
| `dispatcher-bg-writer-{n}` threads      | `Worker(drive_id)` × N_DRIVES |
| `crossbeam_channel` (bounded model)     | `chan drive_queue[N_DRIVES]` |
| `AtomicUsize` in_flight counter         | `byte in_flight[N_DRIVES]` |
| Job routing `device_index % num_drives` | `target_drive` computation |

## Assumptions / Stubs

- **IBlockDevice / IExtentManager**: Abstracted as nondeterministic processing
  delay in the worker (the write may be fast or slow, but always completes).
- **Channel type**: Real code uses `crossbeam_channel::unbounded()`, but the
  model uses bounded channels (`QUEUE_CAP=2`) to study backpressure. Unbounded
  channels trivially satisfy starvation freedom for sends; the bounded variant
  exercises the harder case.
- **Shutdown**: Orderly — shutdown flag set only after all clients finish.
  No mid-flight channel disconnection.
- **Fairness**: Spin's default weak fairness (every continuously enabled
  transition eventually fires). This matches OS thread scheduling guarantees.

## Running

```bash
# Safety verification (assertions + valid end-states)
make safety

# Liveness verification (non-progress cycles under weak fairness)
make liveness

# Manual steps
spin -a starvation_freedom.pml
cc -O2 -DSAFETY -o pan pan.c
./pan -m200000

# Liveness (non-progress cycle detection)
cc -O2 -DNP -o pan-live pan.c
./pan-live -l -m200000
```

## Tuning the Model

| Parameter       | Value | Rationale                                           |
| --------------- | ----- | --------------------------------------------------- |
| N_CLIENTS       | 3     | Odd count vs even drives → asymmetric contention    |
| N_DRIVES        | 2     | Minimum to expose routing-induced unfairness        |
| QUEUE_CAP       | 2     | Small enough to force blocking on every burst       |
| JOBS_PER_CLIENT | 2     | Each client populates twice → observes both queues  |

State space grows exponentially with these parameters. The defaults produce a
tractable model (~100K–1M states). To stress-test:

```bash
spin -DN_CLIENTS=4 -DJOBS_PER_CLIENT=3 -a starvation_freedom.pml
cc -O2 -DSAFETY -DMEMLIM=8192 -o pan pan.c
./pan -m500000
```

## Correspondence to Source Code

| Model location                 | Source file                               | Line range |
| ------------------------------ | ----------------------------------------- | ---------- |
| `Client` enqueue loop          | `components/dispatcher/src/background.rs` | 71–77      |
| `Worker` recv + process        | `components/dispatcher/src/background.rs` | 107–133    |
| `in_flight` increment/decrement| `components/dispatcher/src/background.rs` | 72, 119    |
| Drive routing logic            | `components/dispatcher/src/background.rs` | 174        |
| Shutdown drain loop            | `components/dispatcher/src/background.rs` | 121–128    |
| `init` shutdown sequence       | `components/dispatcher/src/background.rs` | 96–105     |
