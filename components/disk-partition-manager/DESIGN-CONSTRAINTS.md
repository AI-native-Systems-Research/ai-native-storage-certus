# disk-partition-manager: design constraints

This file gives the rules of `disk-partition-manager` that other components depend on.
If a change breaks one of these rules, data can go to the wrong partition, or a restart can format a drive that has data.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

Spec: `specs/001-gpt-partition-management/spec.md` (cited as "spec").

## DPM-1. Users find partitions by their position. Keep the order

- **Rule:** Keep the Certus partition order: index 0 metadata, index 1 extended metadata, index 2 data. Do not change the order in the `PartitionConfig` that the dispatcher gives.
- **Reason:** On load, the component does not examine the type GUIDs. The dispatcher uses `table.partitions[0]` and `table.partitions[2]` by position.
- **Applies to:** `dispatcher`, `dispatcher-p2p`, `apps/extent-benchmark`, each new user.
- **Status:** Established by code. The spec does not say that the position identifies a partition.
- **Evidence:** `src/gpt.rs:130-144` (no type GUID check); `components/dispatcher/src/lib.rs:1735-1755`, `1785-1787`; dispatcher spec 001 FR-055 (line 332).

## DPM-2. `initialize_or_format(false, ...)` can format a drive

- **Rule:** Do not use `initialize_or_format(false, ...)` as a promise to keep data. If it does not find a correct GPT, it writes a new GPT and returns `formatted = true`. The caller must then format its own metadata.
- **Reason:** A read failure of the backup GPT also gives `NoPartitionTable`. Thus a damaged primary GPT and an I/O error on the backup cause a format, not an error.
- **Applies to:** `dispatcher::create_data_drives` and each caller.
- **Status:** Established by spec (US3 scenario 2) for a blank drive. **Code and spec disagree** for read errors. Spec FR-003 limits the fallback to a corrupt primary GPT. The code also changes a backup I/O error into `NoPartitionTable`.
- **Evidence:** spec lines 55 and 67; `src/lib.rs:45-64`; `src/gpt.rs:86-96` (`.map_err(|_| ...)`); the caller logs a warning and formats, `components/dispatcher/src/lib.rs:1789-1793`, `1826`.

## DPM-3. The component does not translate or limit LBAs

- **Rule:** Each user of a partition must add the partition `start_lba` to its I/O and must stay inside `num_sectors`.
- **Reason:** The component gives only the table. It has no partition block device, no LBA offset and no bounds check.
- **Applies to:** `extent-manager` (metadata, through `set_metadata_base_lba`), `extended-metadata-store`, `dispatcher` (data).
- **Status:** Established by code. The dispatcher does not add the data partition start today; see KD-1 in the main document.
- **Evidence:** `components/interfaces/src/ipartition_table.rs:116-128`; `components/extent-manager/src/lib.rs:200-207`.

## DPM-4. The GPT has no Certus layout version

- **Rule:** Do not use the GPT to identify a Certus layout version. If you must have a version, store it in the metadata of the user component (for example the extent-manager superblock).
- **Reason:** The code writes the GPT revision but does not examine it. The code accepts a foreign GPT with correct CRCs. The dispatcher refuses a table with fewer than 3 partitions, but it accepts 3 or more partitions with incorrect sizes or types.
- **Applies to:** each change to the partition layout.
- **Status:** Established by code.
- **Evidence:** `src/gpt.rs:177` (revision written), `src/gpt.rs:130-144`; `components/dispatcher/src/lib.rs:1773-1783` (FR-055 check).
