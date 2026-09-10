//! Global invariant **EM-KEYVEC-MEMBERSHIP** (rank 2, attachments 4) — the hard
//! key-vector modelling campaign (analogous to memory-tier's CONTAINS-REFLECTS
//! FMap gap, here over a dense `Vec<u64>` with a `FREE_KEY = u64::MAX` sentinel).
//!
//! The per-slab dense key vector (`Slab::keys`, slab.rs:12) is the SOLE
//! membership record: an extent is enumerable **iff** its slot key != FREE_KEY.
//! This module models that vector as `KeyVec { keys: Vec<u64> }` and proves the
//! reflection lemmas that discharge the whole bundle:
//!
//!  * EM-KEYVEC-MEMBERSHIP  — `member(i) <-> keys[i] != FREE_KEY` (definitional,
//!    then maintained by every mutator);
//!  * EM-PUBLISH-VISIBLE    — `set_key(i,k)`, k != FREE_KEY  => member(i), key==k;
//!  * EM-PUBLISH-FREEKEY-DISCARD / EM-REMOVE-HIDES / EM-ABORT-RELEASES —
//!    `free_key(i)`  => !member(i);
//!  * EM-RESERVE-INVISIBLE-UNTIL-PUBLISH — a freshly reserved slot keeps
//!    keys[i]==FREE_KEY  => !member(i) until publish;
//!  * frame: a mutation at slot i leaves membership of every j != i unchanged;
//!  * EM-FOREACH-COUNT / EM-GETEXTENTS-EXACT (count half) — `count_members`
//!    returns exactly the number of non-FREE_KEY slots (recursive spec).
//!
//! Source: FR-006..FR-011, US1/US5/US6. Re-authored from the inventory here.

use creusot_std::prelude::*;

pub const FREE_KEY: u64 = u64::MAX;

pub struct KeyVec {
    pub keys: Vec<u64>,
}

/// Membership predicate: slot `i` is enumerable iff its key != FREE_KEY.
#[logic(open)]
pub fn member(kv: &KeyVec, i: Int) -> bool {
    pearlite! { kv.keys@[i]@ != u64::MAX@ }
}

impl KeyVec {
    /// Mirror of the `keys: vec![FREE_KEY; num_slots]` init in `Slab::new`
    /// (slab.rs:24): every slot starts non-member (EM-RESERVE-INVISIBLE base and
    /// EM-GETEXTENTS-EMPTY-FRESH base).
    #[ensures(result.keys@.len() == n@)]
    #[ensures(forall<i: Int> 0 <= i && i < n@ ==> !member(&result, i))]
    pub fn new(n: usize) -> Self {
        KeyVec { keys: creusot_std::vec![FREE_KEY; n] }
    }

    /// Mirror of `Slab::get_key` (slab.rs:46-48).
    #[requires(idx@ < self.keys@.len())]
    #[ensures(result == self.keys@[idx@])]
    pub fn get_key(&self, idx: usize) -> u64 {
        self.keys[idx]
    }

    /// Mirror of `Slab::set_key` (slab.rs:42-44) — the publish path
    /// (region.rs:114-119). **EM-PUBLISH-VISIBLE**: with key != FREE_KEY the
    /// slot becomes a member and reads back the key; **frame**: other slots
    /// unchanged.
    #[requires(idx@ < (*self).keys@.len())]
    #[ensures((^self).keys@.len() == (*self).keys@.len())]
    #[ensures((^self).keys@[idx@] == key)]
    #[ensures(key@ != u64::MAX@ ==> member(&^self, idx@))]
    #[ensures(key@ == u64::MAX@ ==> !member(&^self, idx@))]
    #[ensures(forall<j: Int> 0 <= j && j < (*self).keys@.len() && j != idx@
        ==> (^self).keys@[j] == (*self).keys@[j])]
    pub fn set_key(&mut self, idx: usize, key: u64) {
        self.keys[idx] = key;
    }

    /// Mirror of `Slab::free_slot`'s key reset (slab.rs:39) and the
    /// remove_extent_by_offset key reset (region.rs:146). **EM-REMOVE-HIDES /
    /// EM-ABORT-RELEASES / EM-PUBLISH-FREEKEY-DISCARD**: the slot is no longer a
    /// member; **frame**: other slots unchanged.
    #[requires(idx@ < (*self).keys@.len())]
    #[ensures((^self).keys@.len() == (*self).keys@.len())]
    #[ensures(!member(&^self, idx@))]
    #[ensures(forall<j: Int> 0 <= j && j < (*self).keys@.len() && j != idx@
        ==> (^self).keys@[j] == (*self).keys@[j])]
    pub fn free_key(&mut self, idx: usize) {
        self.keys[idx] = FREE_KEY;
    }
}

/// Recursive spec: number of member (non-FREE_KEY) slots in the prefix
/// `keys[0..n]`. Underpins EM-FOREACH-COUNT / EM-GETEXTENTS-EXACT counting.
#[logic]
#[variant(n)]
#[requires(0 <= n && n <= keys.len())]
pub fn num_members(keys: Seq<u64>, n: Int) -> Int {
    pearlite! {
        if n == 0 {
            0
        } else if keys[n - 1]@ != u64::MAX@ {
            num_members(keys, n - 1) + 1
        } else {
            num_members(keys, n - 1)
        }
    }
}

/// **EM-FOREACH-COUNT** / **EM-GETEXTENTS-EXACT** (count): the number of extents
/// enumerated equals the number of member slots — the callback fires exactly
/// once per published-not-removed extent. Mirrors the enumeration loop in
/// `get_extents`/`for_each_extent` (lib.rs:640-649, 664-673), restricted to the
/// membership test `key != FREE_KEY` over one slab's key vector.
#[ensures(result@ == num_members(kv.keys@, kv.keys@.len()))]
pub fn count_members(kv: &KeyVec) -> usize {
    let mut count = 0usize;
    let n = kv.keys.len();
    let mut i = 0usize;
    #[invariant(i@ <= n@)]
    #[invariant(count@ == num_members(kv.keys@, i@))]
    #[invariant(count@ <= i@)]
    while i < n {
        if kv.keys[i] != FREE_KEY {
            count += 1;
        }
        i += 1;
    }
    count
}
