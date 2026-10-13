# extent-manager: design constraints

This file gives the rules of `extent-manager` that other components depend on.
If a change breaks one of these rules, data can become incorrect without an error, or restart recovery can fail.
The rules for all of Certus, the status values and the change procedure are in [knowledge/design-constraints.md](../../knowledge/design-constraints.md).

Spec: `specs/001-extent-manager-v2/spec.md` (cited as "spec").

## EM-1. Write the data before you publish the extent

- **Rule:** Write the extent data to the drive before you call `publish()` on the `WriteHandle`. If the write fails, abort the handle.
- **Reason:** `publish()` makes the key visible to `for_each_extent`, and restart recovery trusts that list. `publish()` does not examine the data.
- **Applies to:** each caller of `reserve_extent` (today, the dispatcher write-through job).
- **Status:** Established by spec. The code does not enforce it; the caller must obey it.
- **Evidence:** spec lines 49-53 (write-before-commit pattern); `src/lib.rs:597-616` (publish has no data check); the caller obeys it in `components/dispatcher/src/lib.rs:1451-1464`.

## EM-2. A published extent is safe after a crash only when a checkpoint has completed

- **Rule:** Do not assume that a published or removed extent survives a crash until the next checkpoint completes.
- **Reason:** `reserve_extent`, `publish`, `abort` and `remove_extent` change only memory. Only `checkpoint()` writes the state to the drive. A crash loses the extents published after the last checkpoint. Extents removed after that checkpoint come back.
- **Applies to:** the extent-manager and each component that reports data as stored on SSD.
- **Status:** Established by code. The spec agrees (FR-016, FR-018).
- **Evidence:** `src/lib.rs:584-630`, `src/lib.rs:680-684` (no I/O); background checkpoint every 30 s, `src/lib.rs:110-163`; spec FR-016 (line 375), FR-018 (line 384).

## EM-3. Do not use a removed slot again before a checkpoint records the removal

- **Rule:** Keep a removed slot allocated until a successful checkpoint has written the removal to the drive.
- **Reason:** If a new extent uses the slot first and the system then stops, recovery restores the old key. That key then points at the data of a different key.
- **Applies to:** `remove_extent`, the checkpoint code, and the free lists.
- **Status:** Established by spec (FR-025). The code has a race that can break this rule; see "Known defects" below.
- **Evidence:** spec FR-025 (line 427); the checkpoint releases the frees that wait only after its I/O, `src/lib.rs:313-320`.

## EM-4. Increase `FORMAT_VERSION` for each change to the on-disk format

- **Rule:** When you change the superblock, the checkpoint format or the format of the slab key vectors, increase `FORMAT_VERSION`. Make `initialize()` refuse a superblock with a different version.
- **Reason:** Old metadata that the new code reads with the wrong format gives incorrect extents without an error. See DC-3 in the main document.
- **Applies to:** `src/superblock.rs`, `src/checkpoint.rs`, `src/recovery.rs`.
- **Status:** Increase of the version: established by code (`FORMAT_VERSION` = 6; the README still shows 5). Refusal on load: Proposed — needs a decision. Today `initialize()` does not compare the version.
- **Evidence:** `src/superblock.rs:5-6`, `src/superblock.rs:106-114` (the code compares only the magic); `README.md:34` (version 5); spec FR-004 (line 305: "validate its magic and CRC").

## EM-5. Extent offsets are relative to the data space. The caller adds the data base LBA

- **Rule:** Treat each extent offset as an offset in the data space of the extent manager. Before data I/O, add `data_base_lba()` to the LBA. Metadata I/O already adds `metadata_base_lba`.
- **Reason:** The extent manager does no data I/O. If the caller does not add the base, data goes to the wrong location on the drive.
- **Applies to:** each component that does data I/O at extent offsets (`dispatcher`, `dispatcher-p2p`).
- **Status:** Established by spec (FR-036). **Code and spec disagree:** no caller reads `data_base_lba()`. See defect KD-1 in the main document.
- **Evidence:** spec FR-036 (lines 462-475); `src/lib.rs:200-207` (the code adds the metadata base); `src/lib.rs:721-727` (data base is only stored).

## EM-6. Keep the checkpoint regions inside the metadata partition

- **Rule:** If metadata and data are on the same drive, set `metadata_region_size`. Use a value that keeps the two checkpoint copies inside the metadata partition.
- **Reason:** If `metadata_region_size` is 0, `format()` calculates the checkpoint copies from the size of the full namespace. The second copy then starts near the middle of the drive, where the dispatcher can also write data extents.
- **Applies to:** each caller of `format()` (today, the dispatcher and dispatcher-p2p).
- **Status:** Established by code (the calculation). The dispatcher does not obey the rule today; see defect KD-2 in the main document.
- **Evidence:** `src/lib.rs:405-431`, `src/checkpoint.rs:62-64`; `components/dispatcher/src/lib.rs:1832` (`metadata_region_size: 0`); spec FR-035 (line 309).

## Known defects that affect these rules

- **Checkpoint race (EM-2, EM-3).** `run_checkpoint` serializes each region under a read lock and then does the I/O. After the I/O, it clears `dirty` and releases the frees that wait, on all regions, without a second check. A publish or a removal that occurs during the I/O is then not on the drive, but the region shows as clean. Evidence: `src/checkpoint.rs:51-55`, `src/lib.rs:313-320`.
- **Version not checked (EM-4).** See KD-3 in the main document.
- **Size unit on recovery.** `for_each_extent` gives the size in bytes (`slab.element_size`), but `interfaces::Extent::size` says "size in blocks". See KD-5 in the main document. Evidence: `src/lib.rs:658-671`, `components/interfaces/src/iextent_manager.rs:13`.
