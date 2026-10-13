# dispatcher: design constraints

This file gives the rules of `dispatcher` that other components depend on.
If a change breaks one of these rules, data can become incorrect without an error, vLLM can stop, or restart recovery can fail.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).
Most rules also apply to `components/dispatcher-p2p`, which has the same placement, recovery and LBA code.

Specs: `specs/001-dispatcher-cache-interface/spec.md` ("spec 001") and `specs/002-served-by-tier-attribution/spec.md` ("spec 002").

## DSP-1. Use one placement function on all paths

- **Rule:** Find the drive of a key only with `DispatcherComponent::drive_index(key, N)`. Use it on the store, write-through, read, removal, eviction and recovery paths.
- **Reason:** No component stores the drive of a key. Each path calculates it again. If two paths use different functions, one path reads or frees an extent on the wrong drive.
- **Applies to:** all dispatcher code that selects a drive.
- **Status:** Established by code. Spec 001 records an earlier defect: the SSD evictor used `key % num_drives` and freed extents on the wrong drive.
- **Evidence:** `src/lib.rs:581-590`; calls at `src/lib.rs:821`, `950`, `3129`, `3564`, `3860` and `src/background.rs:346`; spec 001 line 251.

## DSP-2. Do not change the list of data drives between restarts

- **Rule:** Keep the count and the order of `data_pci_addrs` the same between restarts, or format the drives. See DC-2 in the main document.
- **Reason:** Restart recovery puts the extents of each drive into the dispatch map, but it does not record the drive. Later reads calculate the drive again from the new list.
- **Applies to:** `initialize()`, the server configuration.
- **Status:** Established by code. No code checks it.
- **Evidence:** `src/lib.rs:1913-1931` (recovery loop); `src/lib.rs:1899-1903` (only a check for an empty list).

## DSP-3. Record the SSD location only after the data is on the SSD

- **Rule:** In the write-through job, write the data, then call `publish()`, then call `convert_to_storage`. Do not call `convert_to_storage` if the write or the publish fails.
- **Reason:** `convert_to_storage` sets `ssd_offset`, and then the dispatcher can remove the entry from DRAM. If the data is not on the SSD at that time, a later read gets incorrect data.
- **Applies to:** `process_write_job` and each new write path.
- **Status:** Established by spec (spec 001 FR-004, FR-017). **Gap:** the code ignores the result of `publish()`.
- **Evidence:** spec 001 lines 281 and 294; `src/lib.rs:1451-1464` (`let _ = write_handle.publish();`).

## DSP-4. A write job uses one read reference. Take it before you queue the job

- **Rule:** Before you queue a `WriteJob`, hold one dispatch-map read reference for the key. The job releases that reference on each exit path.
- **Reason:** `convert_to_storage` and `release_read` decrease the read count. If the job has no reference, it releases the reference of a different user, for example a connector `Pin`. An evictor can then remove the entry when a copy to the GPU still uses it.
- **Applies to:** `copy_gpu_to_memory_completed`, `schedule_write_through`, `process_write_job`.
- **Status:** Established by spec (spec 001 Assumptions, line 414). **Code and spec disagree:** `schedule_write_through` queues a job and takes no reference.
- **Evidence:** `src/lib.rs:3552` (`downgrade_reference` before the dispatcher queues the job); `src/lib.rs:1399-1464` (release on each exit); `src/lib.rs:3842-3864` (`schedule_write_through`, no reference); `components/dispatch-map/src/lib.rs:184-186`.

## DSP-5. Change the dispatch-map entry first, then free the DRAM slot

- **Rule:** To remove an entry from DRAM, call `try_evict_to_block` first. Free the memory-tier slot only if that call returns `Ok`. Do not free the slot of a pinned entry, or of an entry that is not yet on the SSD.
- **Reason:** `memory-tier` has no pins. Only the dispatch map protects a slot that a reader uses.
- **Applies to:** `evict_one_clean`, the memory-tier evictor, the SSD evictor, `clear_memory_tier`.
- **Status:** Established by spec (spec 001 FR-024, FR-049). **Gaps:** `clear_memory_tier` frees the slot also when `try_evict_to_block` fails. The SSD evictor frees the memory-tier slot before `dm.remove`. The effect of the second gap is not confirmed.
- **Evidence:** spec 001 lines 301 and 326; `src/lib.rs:1066-1071`; `src/lib.rs:3814-3820`; `src/background.rs:328-331`.

## DSP-6. Give each reader its own channel, and complete all its reads

- **Rule:** For each read from a drive, lease a channel from `ChannelPool`. Do not share one `ClientChannels` between two readers. Before you return the channel, wait until each submitted read has completed.
- **Reason:** With one shared channel, a reader can take the completions of a different reader. That caused a permanent hang.
- **Applies to:** all cold-read paths.
- **Status:** Established by spec (spec 001 clarification at line 262, FR-054). `dispatcher-p2p` still uses one cached channel for each drive.
- **Evidence:** `src/lib.rs:295-353`; `src/pipeline.rs:311-419`; spec 001 lines 262 and 331; `components/dispatcher-p2p/src/lib.rs:157`, `509`.

## DSP-7. Balance each `pin` with one `unpin`

- **Rule:** Call `unpin` one time for each successful `pin`. Do not use `pin` without an owner that calls `unpin` later.
- **Reason:** A pin is a dispatch-map read reference and has no owner. The dispatcher cannot find a pin that nobody releases, so no evictor can remove the entry again.
- **Applies to:** `pin`, `unpin` and their callers (`shmq-dispatcher`, the connector).
- **Status:** Established by spec (`IDispatcher::pin` documentation; spec 001 FR-056).
- **Evidence:** `components/interfaces/src/idispatcher.rs:748-769`; `src/pins.rs`; spec 001 line 334.

## DSP-8. A full memory tier must not cause a store error

- **Rule:** If the memory tier stays full after the backpressure time, drop the store and return `Ok`.
- **Reason:** The connector reports each store as a success, and vLLM stops if a transfer fails. A block that is not cached costs only a recompute.
- **Applies to:** `batch_populate` and the other store paths.
- **Status:** Established by spec (spec 001 FR-060).
- **Evidence:** spec 001 line 356; `src/lib.rs:3161-3175`.

## DSP-9. At shutdown, stop the writer, then checkpoint, then stop the drives

- **Rule:** In `shutdown()`, let the write-through queue drain, then call `checkpoint()` on each extent manager, then stop the block devices.
- **Reason:** Extent metadata goes to the drive only at a checkpoint (see EM-2 in `components/extent-manager/DESIGN-CONSTRAINTS.md`).
- **Applies to:** `shutdown()`.
- **Status:** Established by spec (spec 001 FR-014). A checkpoint failure is only written to the log.
- **Evidence:** spec 001 line 291; `src/lib.rs:2115-2186` (checkpoint at `src/lib.rs:2131`).

## DSP-10. A served result means that the GPU buffer has the data

- **Rule:** Report a key as served (`Ok`, a non-zero served-by byte) only after the copy to the GPU buffer has completed.
- **Reason:** The connector gives the GPU buffer to vLLM when the reply shows the key as served. If the copy failed, vLLM uses incorrect data without an error.
- **Applies to:** `batch_lookup`, served-by attribution, `shmq-dispatcher` Lookup replies.
- **Status:** Established by spec (spec 002 FR-002, FR-009, FR-020). **Code and spec disagree:** if the warm-stream synchronization fails, the code writes only a log line, and the results stay `Ok`.
- **Evidence:** spec 002 lines 430-434, 459, 478, 527; `src/lib.rs:2745-2750`.

## Known defects that affect these rules

See these defects in the main document: KD-1 (data LBA), KD-2 (checkpoint position) and KD-4 (no drive list check). Also see KD-5 (size unit on recovery), KD-7 (ignored results) and KD-8 (no restart test).
