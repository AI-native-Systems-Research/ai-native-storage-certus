# Certus design constraints

## About this document

This document gives the rules that each change to Certus must obey. It also gives the designs that people rejected, with the reason and the evidence.
It is for Certus developers and for the AI agents that change Certus, which includes the agents of the systems optimizer.

Read this document before you change data placement, the on-disk layout, restart recovery, CPU affinity or the boundary between components.
Before you change a component, also read its `DESIGN-CONSTRAINTS.md` file (see the index below).

To change a rule, open a pull request that changes this file. In the pull request, give the reason and the evidence.
Do not change the status of a rule to "Decided" without a decision from a person who owns the rule.

Each rule has these fields:

- **Rule:** the instruction.
- **Reason:** the problem that occurs if you do not obey the rule.
- **Applies to:** the code and the components that the rule controls.
- **Status:** one of these values:
  - *Decided <date>*: a person made the decision on that date.
  - *Established by code* or *Established by spec*: the code or a spec makes the rule necessary. Nobody recorded a separate decision.
  - *Proposed — needs a decision*: the evidence supports the rule, but nobody has made a decision.
- **Evidence:** file and line, commit, or the path of a session artifact. Workbench paths are relative to `/home/nara/certus/ai-native-storage-certus-workbench/`.
- **Rejected designs:** the designs that the rule does not permit, with the measured numbers.

The rules are true for branch `unstable` at commit `a052af01` (2026-10-12).

## Index of component files

| Component | File | Main topics |
|---|---|---|
| dispatcher | [components/dispatcher/DESIGN-CONSTRAINTS.md](../components/dispatcher/DESIGN-CONSTRAINTS.md) | placement, write-through order, references, eviction order, channels, shutdown |
| dispatch-map | [components/dispatch-map/DESIGN-CONSTRAINTS.md](../components/dispatch-map/DESIGN-CONSTRAINTS.md) | one global lock, references, recovery, lock order |
| memory-tier | [components/memory-tier/DESIGN-CONSTRAINTS.md](../components/memory-tier/DESIGN-CONSTRAINTS.md) | one pool, no pins, lifetime |
| eviction-policy-lru | [components/eviction-policy-lru/DESIGN-CONSTRAINTS.md](../components/eviction-policy-lru/DESIGN-CONSTRAINTS.md) | handle lifetime, no callbacks |
| extent-manager | [components/extent-manager/DESIGN-CONSTRAINTS.md](../components/extent-manager/DESIGN-CONSTRAINTS.md) | write before publish, checkpoints, format version, base LBA |
| disk-partition-manager | [components/disk-partition-manager/DESIGN-CONSTRAINTS.md](../components/disk-partition-manager/DESIGN-CONSTRAINTS.md) | partition order, format on read errors, no LBA translation |
| block-device-spdk-nvme | [components/block-device-spdk-nvme/DESIGN-CONSTRAINTS.md](../components/block-device-spdk-nvme/DESIGN-CONSTRAINTS.md) | actor thread, DMA buffer lifetime, flush, shutdown |
| spdk-env | [components/spdk-env/DESIGN-CONSTRAINTS.md](../components/spdk-env/DESIGN-CONSTRAINTS.md) | CPU affinity, `fini()`, enumeration |
| shm-queue | [lib/shm-queue/DESIGN-CONSTRAINTS.md](../lib/shm-queue/DESIGN-CONSTRAINTS.md) | store order, shared layout, channel owner |
| shmq-dispatcher | [lib/shmq-dispatcher/DESIGN-CONSTRAINTS.md](../lib/shmq-dispatcher/DESIGN-CONSTRAINTS.md) | wire protocol, served byte, one reply, reservation time-out |
| certus-shmq-connector | [certus-shmq-connector/DESIGN-CONSTRAINTS.md](../certus-shmq-connector/DESIGN-CONSTRAINTS.md) | load must not fail, store always succeeds, key fold |

## Rules

### DC-1. Store each object on one data drive

- **Rule:** Keep all the data of one object on one data drive. Placement can select the drive (today: a hash of the key). Do not divide one object across drives.
- **Reason:** A data drive can fail. With one object on one drive, the loss of a drive loses only the objects on that drive. The other drives continue to serve their objects.
- **Applies to:** dispatcher placement, the read and write paths, extent allocation, and each layout with more than one drive.
- **Status:** Decided 2026-10-12.
- **Evidence:** the decision came with the request for this document on 2026-10-12. No other record of it exists. Placement today: `components/dispatcher/src/lib.rs:581-590` (`drive_index` = splitmix64(key) % N).
- **Rejected designs:**
  - **RAID-0 striping of each object across all data drives.** Optimizer session `s20261010-171624` (target `evolve-coldload-throughput`) put a stripe of size `ceil(size/N)` at the same offset on each drive.
    - Striping alone: cold-load throughput 12.23 → 13.32 GB/s (+8.9%), p99 −30.5%, 4 drives, 4 MiB objects. Store throughput fell 8.8%. Evidence: `targets/evolve-coldload-throughput/experiments/s20261010-171624/session_summary.md:13`, `:145`, `:148`.
    - Measurement by hand of branch `perf/coldload-striping-fixes` (striping and the later fixes of the session), compared with `unstable` `83bcd4ff`. Evidence: `/home/nara/certus/bench-wt/verify-coldload-20261011/`, `python3 compare.py`.
    - With 4 drives and 4 MiB objects: 12.16 → 14.09 GB/s (+15.9%), p99 7014 → 4505 µs (−35.8%). With 4 drives and 1 MiB: +8.1%. With 2 drives and 4 MiB: +13.8%. With 1 drive: +0.7%.
    - Reason for the rejection: the loss of one drive loses every object.
    - Also, the design stored no stripe layout and no layout version on the drives. A restart with a different drive count or order would read incorrect data. Evidence: `session_summary.md:149` (I2-D5).
    - The code for RAID-0 striping is not in `unstable`.

### DC-2. Do not change the list of data drives between restarts without a format

- **Rule:** Keep the count and the order of `data_pci_addrs` the same between restarts. To change them, format the drives (`format_on_init = true`, `--format`).
- **Reason:** No component stores the drive of a key. Restart recovery puts the extents of all drives into the dispatch map. Each read then calculates the drive again from the key hash and the new list. With a different list, a read goes to the wrong drive at an old offset and returns incorrect data without an error.
- **Applies to:** server configuration, `dispatcher::initialize`, each change to placement.
- **Status:** Established by code. This is a current limit: no code records or examines the drive list (see KD-4).
- **Evidence:** `components/dispatcher/src/lib.rs:1913-1931` (recovery loop); `components/dispatcher/src/lib.rs:821`, `950` (read paths); `components/dispatcher/src/lib.rs:1899-1903` (only a check for an empty list).

### DC-3. Keep restart recovery correct when you change the on-disk layout

- **Rule:** A change to stored data must not make restart recovery read old data incorrectly. Stored data includes placement, extent sizes, extent records, the partition use and the metadata format. Make recovery read the old layout correctly, or make it refuse the old layout.
- **Reason:** The server recovers from the drives by default (`certus-server` without `--format`). The benchmarks always start the server with `--format`, so they cannot find a recovery that is broken.
- **Applies to:** dispatcher, dispatch-map, extent-manager, disk-partition-manager.
- **Status:** Established by spec (dispatcher spec 001 FR-025). See P-1 for the proposed method.
- **Evidence:** `components/dispatcher/specs/001-dispatcher-cache-interface/spec.md:302` (FR-025); `apps/certus-server/src/main.rs:522-526`; `--format` in the benchmark configurations `targets/evolve-coldload-throughput/configs/4d_4g.yaml:32`, `targets/evolve-coldload/configs/4d_4g.yaml:32`, `targets/evolve-latency/configs/4d_4g.yaml:30`.

### DC-4. Server threads must not keep the CPU affinity that SPDK init sets

- **Rule:** After `spdk_env_init` (DPDK EAL init), set the CPU affinity of the thread back to its value before the init. Start server threads only after that.
- **Reason:** DPDK EAL init pins the thread that calls it to its main lcore (CPU 0 with the default core mask). Each thread that the server starts later gets the same single CPU. Then the shmq poller, the shmq workers, the write-through threads and the evictors all run on CPU 0.
- **Applies to:** `components/spdk-env`, each application that starts SPDK or DPDK.
- **Status:** Established by code (commit `d102a00b`, Certus PR #507) and by spec (spdk-env spec 002 FR-022).
- **Evidence:** `components/spdk-env/src/env.rs:91-102`; `components/spdk-env/specs/002-spdk-env-vfio-init/spec.md:163`; mechanism and log line: `targets/evolve-latency/experiments/s20261003-092109/session_summary.md:183-185`, `discoveries/index.md:108`.
- **Rejected designs:**
  - **Keep the EAL affinity on the main thread.** On the connector benchmark (`bench_connector_lifecycle.py`, TP=1, 4 drives, 4 MiB blocks), the restore of the affinity gave these changes. Touch+lookup p99: 72,858.5 → 6,275.35 µs (−91.4%). Steps/s: 4.50 → 10.25 (+127.8%). Store: 7.13 → 12.23 GB/s (+71.5%). Evidence: `s20261003-092109/session_summary.md:180`, `:194-207`.
  - The request for this document gives an end-to-end inference speedup of approximately 30% for the fix. This document did not find a stored measurement of that number.

### DC-5. Components use other components only through interfaces and receptacles

- **Rule:** Use a different component only through the traits in `components/interfaces` and through receptacles. Do not use the structs or the inherent methods of a different component.
- **Reason:** This keeps the components independent, and it keeps the context of each change small. A direct use of a struct hides a dependency that the bindings do not show.
- **Applies to:** all components.
- **Status:** Established by the project rules (`CLAUDE.md`, "Component Model").
- **Evidence:** `CLAUDE.md` (Component Model). Known exception: the dispatcher makes a `DiskPartitionManager` and calls its inherent method `initialize_or_format`, which is not on `IPartitionTable` (`components/dispatcher/src/lib.rs:1721`, `1758`; `components/interfaces/src/ipartition_table.rs:115-128`).

### DC-6. A failed load stops vLLM

- **Rule:** On the load path, do not add a failure for a key that the connector reported as present and pinned. Treat each new failure on this path as a vLLM crash, not as a cache miss.
- **Reason:** The connector `_do_load` returns `False` if one key is not served. vLLM asserts that each transfer succeeds, so the engine stops. Find a miss earlier, at `lookup` (Check), where vLLM can recompute the block.
- **Applies to:** connector load path, `shmq-dispatcher` Pin and Lookup, dispatcher `batch_lookup`, eviction, promotion.
- **Status:** Established by code.
- **Evidence:** `certus-shmq-connector/certus_shmq_connector/handler.py:325-347` (comment at line 333); the store path never reports a failure, `handler.py:299-307`. Component rules: CONN-1, CONN-3, CONN-4, DSP-5, DSP-7.

## Proposed rules — need a decision

### P-1. Record a layout version, refuse a mismatch, and test write → restart → read

- **Rule:** For each change to the on-disk layout, increase a layout version in stored metadata. Make recovery refuse data with a different version. The operator must then format the drives. Add a test that writes data, restarts without a format and reads the data again.
- **Reason:** DC-3 must have a method that finds a different layout. Today, the extent-manager superblock has a `version` field, but recovery does not examine it (KD-3). No test does a restart without a format (KD-8).
- **Applies to:** dispatcher, extent-manager, dispatch-map.
- **Status:** Proposed — needs a decision. The optimizer target `evolve-coldload-throughput` gives this rule to its agents.
- **Evidence:** `targets/evolve-coldload-throughput/target.yaml:3` ("Persisted-layout rule"); `components/extent-manager/src/superblock.rs:5-6`, `:106-114`.

### P-2. Examine tuned performance constants on 1, 2 and 4 drives and on 1 MiB and 4 MiB objects

- **Rule:** If you tune a performance constant on one setup, measure it on other setups too. Use 1, 2 and 4 drives, and 1 MiB and 4 MiB objects. Make the constant depend on the drive count where necessary.
- **Reason:** A constant that helps one setup can make a different setup slower.
- **Applies to:** queue depths, read caps, batch sizes, time-outs in the dispatcher and in `shmq-dispatcher`.
- **Status:** Proposed — needs a decision. The optimizer target uses these setups as guards.
- **Evidence:**
  - The optimizer tuned a fixed cap of 24 reads in flight for each drive on 4 drives. It lost 14% of cold-load throughput on 1 drive and all of the gain on 2 drives (`targets/evolve-coldload-throughput/target.yaml:3`).
  - Measurement by hand of the optimizer best `ba462acb` on 1 drive, 4 MiB: 6.31 → 5.43 GB/s (−13.9%), p99 +18.9% (`verify-coldload-20261011`, `python3 compare.py`).
  - On 4 drives, 1 MiB, the same build had scheduler-step drain p99 +83.6%.

### P-3. Limit each busy-spin thread and give it a switch to stop the spin

- **Rule:** Give each thread that spins on a CPU a time limit or a count limit. Give it a switch (an environment variable or an argument) that stops the spin. Make sure that the switch also works when the server starts through `sudo`.
- **Reason:** A thread that spins uses one CPU all the time, also when there is no work. The cost is small on one test machine, but it is not visible in the benchmark result.
- **Applies to:** `shmq-dispatcher` poller, NVMe actor threads, new spin loops.
- **Status:** Proposed — needs a decision. Today, the `shmq-dispatcher` poller spins until shutdown and has no switch.
- **Evidence:**
  - `lib/shmq-dispatcher/src/serve.rs:326-340` (poller loop).
  - The optimizer "hot worker" (commit `84589187`, switch `CERTUS_SHMQ_HOT_SPIN_US`, default 300 µs) gave cold load 14.24 → 15.26 GB/s (+7.2%) for +0.13 CPU cores (+2.6%). Evidence: `/home/nara/certus/bench-wt/verify-hotworker-20261012/run.log:4-14`, `runs/*.json`.
  - The evaluator does not pass its switch through `sudo`, so the switch was not tested (`targets/evolve-coldload-throughput/experiments/s20261011-200406/session_summary.md:110`, `:219`).

### P-4. Do not make a change depend on the request pattern of the benchmark

- **Rule:** Do not tune a change to the batch sizes, the request order or the operations that only the benchmark sends. Do not change the request pattern of the benchmark.
- **Reason:** vLLM sends requests in a different pattern. A change that depends on the benchmark pattern can do nothing in production, or it can make production slower.
- **Applies to:** all performance changes.
- **Status:** Proposed — needs a decision. The optimizer target gives this instruction to its agents.
- **Evidence:**
  - `targets/evolve-coldload-throughput/target.yaml:3` ("do not change the benchmark's request pattern").
  - An optimizer agent set a threshold between the batch sizes of the benchmark (`targets/evolve-coldload/experiments/s20261009-211818/iterations/iter-003/proposal-001/hypothesis.md:26-28`).
  - One proposal pinned threads when `FLUSH_TO_SSD` arrived, but only the benchmark sends `FLUSH_TO_SSD` (`targets/evolve-coldload-throughput/experiments/s20261011-200406/session_summary.md:518`).

## Rejected designs (index)

| Design | Rule | Main reason |
|---|---|---|
| RAID-0 striping of each object across drives | DC-1 | The loss of one drive loses all objects. No stored layout. |
| Keep the DPDK EAL affinity on server threads | DC-4 | All threads ran on CPU 0. |
| Shards in `dispatch-map` and `memory-tier` | DMAP-1, MT-1 | +2.2% (noise). Eviction from the wrong shard crashed vLLM. |
| One shared channel for each drive for all readers | DSP-6 | Readers took the completions of other readers. Permanent hang. |
| `Pin(promote=True)` before Lookup | CONN-3 | Two promotions of one key make the load fail. |

## Known defects that affect these rules

These are defects in `unstable`, not rules. Each defect comes from a read of the code. No test on hardware has shown one.

- **KD-1. Data I/O does not add the start LBA of the data partition.** The dispatcher calls `set_data_base_lba(partitions[2].start_lba)`, but no code calls `data_base_lba()`. Data LBA = extent offset / block size, and extent offsets start at 0 (`metadata_region_size: 0`). Thus data I/O starts at LBA 0 of the namespace, where the GPT and the metadata partition are. Evidence: `components/dispatcher/src/lib.rs:1787`, `824`, `953`, `1448`, `1832`; `components/extent-manager/src/lib.rs:442-446`, `721-727`. `dispatcher-p2p` has the same code (`components/dispatcher-p2p/src/lib.rs:506`, `809`, `2788`).
- **KD-2. The checkpoint copies are not inside the metadata partition.** With `metadata_region_size: 0`, `format()` calculates the size of each checkpoint copy from the full namespace. The second copy then starts near the middle of the drive, in the range that data extents use. Evidence: `components/extent-manager/src/lib.rs:405-431`; `components/extent-manager/src/checkpoint.rs:62-64`; `components/dispatcher/src/lib.rs:1832`.
- **KD-3. Recovery does not examine the format version.** `Superblock::deserialize` compares only the magic and the CRC. The code writes `FORMAT_VERSION` (6) but does not compare it. Evidence: `components/extent-manager/src/superblock.rs:5-6`, `106-114`.
- **KD-4. Nothing records or examines the data drive list.** See DC-2. The superblock has no drive index and no drive count. Evidence: `components/extent-manager/src/superblock.rs:9-24`.
- **KD-5. Recovery gives the extent size in bytes where the dispatch map expects 4 KiB blocks.** `recover_extent(key, offset, size_blocks)` gets `extent.size`, which is in bytes. Later, `entry_size` multiplies by 4096 in `u32`. For a 4 MiB object the result overflows. In a release build, the result is 0. Evidence: `components/dispatcher/src/lib.rs:1922`; `components/extent-manager/src/lib.rs:658-671`; `components/dispatch-map/src/lib.rs:369`, `406`, `583`; dispatch-map spec FR-024.
- **KD-6. A checkpoint can lose a change that occurs during its I/O.** After the I/O, the checkpoint clears the `dirty` flags on all regions. It also releases the frees that wait, without a second check. Evidence: `components/extent-manager/src/lib.rs:313-320`; `components/extent-manager/src/checkpoint.rs:51-55`.
- **KD-7. The dispatcher ignores errors on the persistence path.** It ignores the result of `publish()` before `convert_to_storage`, and the result of each `recover_extent`. Evidence: `components/dispatcher/src/lib.rs:1463-1464`, `1922`.
- **KD-8. No test does write → restart without a format → read.** The dispatcher hardware tests use a mock dispatch map whose `recover_extent` does nothing. These tests also set `metadata_pci_addr`, which `DispatcherConfig` does not have now. Evidence: `components/dispatcher/tests/integration.rs:409-416`, `464`; `components/interfaces/src/idispatcher.rs:26-100`.

## Open questions

- **Q-1. Must the SSD tier keep its data through a restart?** One possible answer: the SSD tier keeps data (FR-025 recovery, the `certus-server` default). The other: the SSD tier is a cache, and the server formats it at each start. The benchmarks always format, and KD-1 to KD-8 make recovery unsafe today. Until a person decides, DC-3 applies.
- **Q-2. Which setups must Certus support?** P-2 must have a list of drive counts and object sizes that each change must keep. The optimizer target uses 1, 2 and 4 drives and 1 MiB and 4 MiB objects. Nobody has decided on this list for the project.
