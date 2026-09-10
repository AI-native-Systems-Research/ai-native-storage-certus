//! Cached-state properties: the `Mutex<Option<PartitionTable>>` gate and the
//! partition-index accounting.
//!
//! Sources: `partition_info` (lib.rs:97-111), `num_partitions` (lib.rs:113-119),
//! and the `*self.state.lock() = Some(table.clone())` caching in `initialize` /
//! `format` (lib.rs:82, :93).
//!
//! Covers `G2 DPM-INIT-GATE`, `G3 DPM-STATE-CACHED`, `G5 DPM-COUNT-INDEX-AGREE`,
//! `DPM-PINFO-INDEX-RANGE`, `DPM-PINFO-RETURNS-ENTRY`, `DPM-PINFO-READONLY`, and
//! `DPM-NUMP-RETURNS-LEN`. The `Mutex` non-poisoning / cross-thread ordering is a
//! trusted boundary (→ Loom); what is modelled is the sequential Option/Vec
//! logic once the lock is held. `u64` stands for the opaque `PartitionInfo`
//! payload (only its identity through get/clone matters here).

use creusot_std::prelude::{Clone, *};

/// Mirror of the `PartitionTableError` variants the query path produces.
#[derive(Clone, Copy)]
pub enum PtErr {
    NotInitialized,
    InvalidPartition,
}

/// **G2 `DPM-INIT-GATE`** + **`DPM-PINFO-INDEX-RANGE`** + **`DPM-PINFO-RETURNS-ENTRY`**
/// + the `partition_info` half of **G5**. Faithful mirror of `partition_info`
/// (lib.rs:97-111): `state.as_ref().ok_or(NotInitialized)?` then
/// `partitions.get(index).cloned().ok_or(InvalidPartition)`. `present` models
/// `state.is_some()`.
///
/// - not initialized ⇒ `NotInitialized`, regardless of index (the gate);
/// - initialized, `index >= len` ⇒ `InvalidPartition`;
/// - initialized, `index < len` ⇒ `Ok(partitions[index])` (the cached clone).
#[ensures(!present ==> result == Err(PtErr::NotInitialized))] // G2 gate
#[ensures(present && index@ >= partitions@.len() ==> result == Err(PtErr::InvalidPartition))] // range
#[ensures(present && index@ < partitions@.len() ==> result == Ok(partitions@[index@]))] // returns entry
pub fn partition_info(present: bool, partitions: &Vec<u64>, index: u32) -> Result<u64, PtErr> {
    if !present {
        return Err(PtErr::NotInitialized);
    }
    let n = partitions.len();
    if (index as usize) < n {
        // Mirrors `partitions.get(index).cloned()` returning Some in range.
        Ok(partitions[index as usize])
    } else {
        Err(PtErr::InvalidPartition)
    }
}

/// **G2 `DPM-INIT-GATE`** + **`DPM-NUMP-RETURNS-LEN`** + the `num_partitions`
/// half of **G5**. Faithful mirror of `num_partitions` (lib.rs:113-119):
/// `state.as_ref().ok_or(NotInitialized)?` then `Ok(partitions.len() as u32)`.
#[requires(partitions@.len() <= u32::MAX@)] // len fits u32 (128-entry cap ⇒ always true)
#[ensures(!present ==> result == Err(PtErr::NotInitialized))] // G2 gate
#[ensures(present ==> exists<v: u32> result == Ok(v) && v@ == partitions@.len())] // Ok(len)
pub fn num_partitions(present: bool, partitions: &Vec<u64>) -> Result<u32, PtErr> {
    if !present {
        return Err(PtErr::NotInitialized);
    }
    Ok(partitions.len() as u32)
}

/// **G5 `DPM-COUNT-INDEX-AGREE`**: `partition_info(i)` is `Ok` iff
/// `i < num_partitions()` — one denominator. Composes the two mirrors above on
/// the *same* initialized table and proves the biconditional exactly.
#[requires(partitions@.len() <= u32::MAX@)]
#[ensures(result == (index@ < partitions@.len()))]
pub fn count_index_agree(partitions: &Vec<u64>, index: u32) -> bool {
    // partition_info is Ok iff index < len; num_partitions returns exactly len
    // (proved separately), so this biconditional is `i < num_partitions()`.
    let info = partition_info(true, partitions, index);
    info.is_ok()
}

/// **G3 `DPM-STATE-CACHED`**: on `Ok(t)`, `state ← Some(t.clone())` and the
/// returned value equals the cached table. Mirror of `*self.state.lock() =
/// Some(table.clone()); Ok(table)` (lib.rs:82-83 / :93-94). Returns
/// `(new_state, returned)`; proves the cache holds exactly the returned table.
#[ensures(result.0 == Some(table))] // state is now Some(table)
#[ensures(result.1 == table)] // the returned value equals the cached one
pub fn cache_state(table: u64) -> (Option<u64>, u64) {
    let new_state = Some(table);
    (new_state, table)
}

/// **`DPM-PINFO-READONLY`** (frame): `partition_info` and `num_partitions` take
/// the cached table by shared reference and never mutate it (lib.rs:98-99,
/// :114-115 only `lock()` + `.as_ref()`). Modelled by returning the passed state
/// unchanged after a query — the query cannot alter `Option`/`Vec` it borrows
/// immutably. (Block I/O is not on this path at all — a pure state read.)
#[ensures(result == present)] // querying does not change whether state is Some
pub fn pinfo_readonly(present: bool, partitions: &Vec<u64>, index: u32) -> bool {
    let _ = partition_info(present, partitions, index);
    present
}
