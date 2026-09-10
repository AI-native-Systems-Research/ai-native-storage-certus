//! Scalar-config obligations — trivial get/set roundtrips over the `Mutex<..>`
//! fields, plus the metadata-LBA shift arithmetic.
//!
//!  * **EM-DATABASE-ROUNDTRIP** (rank 5, attachments 2; FR-036): `data_base_lba()`
//!    returns the value last set by `set_data_base_lba`, default 0; the component
//!    does no data-device I/O with it (lib.rs:721-727).
//!  * **EM-CKPTINTERVAL-SET** (FR-016/FR-027): `Some(dur)` sets the interval,
//!    `None` disables the background timer; default 30 s (lib.rs:70-73, 112, 694).
//!  * **EM-METABASE-SHIFTS-IO** (FR-036): the stored `metadata_base_lba` shifts
//!    every metadata block address by that offset — `block_lba = base + lba + i`
//!    (block_io.rs:69, 119) — and the stored value roundtrips (lib.rs:717).

use creusot_std::prelude::*;

/// Models `Mutex<u64>` for `data_base_lba` / `metadata_base_lba`: a store cell.
pub struct LbaCell {
    pub value: u64,
}

impl LbaCell {
    /// Default is 0 (lib.rs field init `data_base_lba: Mutex<u64>` -> 0).
    #[ensures(result.value@ == 0)]
    pub fn new() -> Self {
        LbaCell { value: 0 }
    }

    /// Mirror of `set_data_base_lba` / `set_metadata_base_lba` (lib.rs:717-723).
    #[ensures((^self).value == v)]
    pub fn set(&mut self, v: u64) {
        self.value = v;
    }

    /// Mirror of `data_base_lba` (lib.rs:725-727). **EM-DATABASE-ROUNDTRIP**.
    #[ensures(result == self.value)]
    pub fn get(&self) -> u64 {
        self.value
    }
}

/// **EM-DATABASE-ROUNDTRIP**: set then get returns exactly the value set.
#[ensures(result == v)]
pub fn roundtrip(mut cell: LbaCell, v: u64) -> u64 {
    cell.set(v);
    cell.get()
}

/// **EM-METABASE-SHIFTS-IO** (FR-036): the absolute metadata block address is the
/// stored base LBA plus the relative block address — mirror of `block_lba =
/// self.base_lba + lba + i` (block_io.rs:69, 119). Proves the base offset is
/// applied additively to every I/O; default base 0 => absolute == relative.
#[requires(base_lba@ + lba@ + i@ <= u64::MAX@)]
#[ensures(result@ == base_lba@ + lba@ + i@)]
#[ensures(base_lba@ == 0 ==> result@ == lba@ + i@)]
pub fn shifted_block_lba(base_lba: u64, lba: u64, i: u64) -> u64 {
    base_lba + lba + i
}

/// Models `Mutex<Option<Duration>>` for the checkpoint interval, as an
/// `Option<u64>` of nanoseconds. `None` => background timer disabled.
pub struct IntervalCell {
    pub interval: Option<u64>,
}

impl IntervalCell {
    /// Default 30 s (lib.rs:112: `set_interval(Some(30s))`).
    #[ensures(result.interval == Some(30_000_000_000u64))]
    pub fn new() -> Self {
        IntervalCell { interval: Some(30_000_000_000u64) }
    }

    /// Mirror of `set_checkpoint_interval` -> `set_interval` (lib.rs:70-73, 694):
    /// **EM-CKPTINTERVAL-SET** — `Some(dur)` enables at `dur`; `None` disables.
    #[ensures((^self).interval == interval)]
    // "disabled" is exactly the None state the background loop waits on
    // indefinitely (lib.rs:137-140).
    #[ensures(interval == None ==> (^self).interval == None)]
    pub fn set_interval(&mut self, interval: Option<u64>) {
        self.interval = interval;
    }
}
