# Spin/Promela Formal Verification Models

## write_before_evict.pml

Models the dispatcher's populate → write-through → eviction lifecycle to verify
that memory-tier eviction never produces dangling SSD references.

### Properties Verified

| ID | Property                                                                                                               | Type   |
| -- | ---------------------------------------------------------------------------------------------------------------------- | ------ |
| P1 | Entry reaches ON_SSD only after background writer completes SSD write                                                  | Safety |
| P2 | Entry evicted before write-through completes is removed from dispatch-map (lookup returns KeyNotFound, not stale data) | Safety |
| P3 | Dispatch-map entry cannot be removed while read references are held                                                    | Safety |

### System Abstraction

| Real component                           | Promela process                     |
| ---------------------------------------- | ----------------------------------- |
| `tokio::task::spawn_blocking` (populate) | `Client(id)`                        |
| `dispatcher-bg-writer` thread            | `BgWriter()`                        |
| `dispatcher-ssd-evictor` thread          | `SsdEvictor()`                      |
| `crossbeam_channel::unbounded()`         | `chan write_queue[QUEUE_CAP]`       |
| Memory-tier pool (DRAM)                  | `pool_used` counter + per-key state |
| Dispatch-map entry state                 | `key_state[]` enum                  |

### Running

```bash
# Install Spin ; build with makefile and copy to /usr/local/bin
git clone git@github.com:dwaddington/Spin.git

# Optionally install ispin.tcl
dnf install tcl

# Generate verifier
spin -a write_before_evict.pml

# Compile for safety verification (assertion violations + invalid end-states)
cc -O2 -DSAFETY -o pan pan.c

# Run (increase -m for deeper search)
./pan -m100000

# For deadlock/liveness checking (no -DSAFETY)
cc -O2 -o pan pan.c
./pan -a -m100000

# For LTL properties (uncomment ltl claims in the .pml first)
spin -a -f '![] (p1_safe)' write_before_evict.pml
cc -O2 -o pan pan.c
./pan -a
```

### Tuning the Model

The model uses small parameters to keep the state space tractable:

- `N_CLIENTS=2`, `N_KEYS=4`, `POOL_CAP=3`, `QUEUE_CAP=4`

This is sufficient to expose concurrency bugs because:

1. `POOL_CAP < N_KEYS` forces eviction on every populate after the pool fills.
2. Two concurrent clients + background writer create all interesting interleavings.
3. Symmetry: if the property holds for 2 clients and 4 keys, it holds for N clients
   and M keys (the protocol logic per-key is identical).

To explore larger state spaces:

```bash
# Increase parameters (expect 10-100x state space growth per increment)
spin -DPOOL_CAP=4 -DN_KEYS=6 -a write_before_evict.pml
cc -O2 -DSAFETY -DMEMLIM=8192 -o pan pan.c
./pan -m200000
```

### Correspondence to Source Code

| Model location             | Source file                               | Line range |
| -------------------------- | ----------------------------------------- | ---------- |
| `Client.populate`          | `components/dispatcher/src/lib.rs`        | 1399–1498  |
| `evict_one()` / clean path | `components/dispatcher/src/lib.rs`        | 296–348    |
| `evict_one()` / blind LRU  | `components/dispatcher/src/lib.rs`        | 336–347    |
| `BgWriter`                 | `components/dispatcher/src/background.rs` | 76–93      |
| `BgWriter.process_job`     | `components/dispatcher/src/lib.rs`        | 350–431    |
| `SsdEvictor`               | `components/dispatcher/src/background.rs` | 158–239    |
