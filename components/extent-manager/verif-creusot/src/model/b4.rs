//! Batch-4 model additions (pin 2cd35bac):
//! * `ExtentManager::used_bytes` — mirror of `IExtentManager::used_bytes` (lib.rs:698-708),
//!   DEBUG-build semantics (the u64 subtraction / sum panic on overflow; excluded by `#[requires]`).
//! * `CoSys` — the checkpoint coalescing protocol (lib.rs:53-57, 729-761), one atomic step
//!   per `checkpoint_coalesce` critical section.
//! * byte-level checkpoint copy write / read (checkpoint.rs:69-97, 117-173) over the copy
//!   region's device bytes; trusted leaves `rd64`/`rd32` (+ `lemma_dec64`/`lemma_dec32`:
//!   from_le_bytes inverts to_le_bytes), and batch 2's to_le_bytes64/32, crc32_hash, as_u32.
//! * `FEm` — format() racing a checkpoint in flight (lib.rs:496-507 vs 299-307).
use crate::model::buddy::*;
use crate::model::ckpt::*;
use crate::model::component::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use crate::model::slab::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// One region's `total_usable_size() - total_free()` (lib.rs:704).
#[logic(open)]
pub fn rg_used(r: RegionState) -> Int {
    pearlite! { r.buddy.total_usable_size@ - tf(r.buddy.free_lists@, r.buddy.sector_size@, r.buddy.free_lists@.len()) }
}

/// Sum of `rg_used` over current regions `0..n` (lib.rs:700-707).
#[logic(open)]
#[variant(n)]
pub fn used_sum(arena: Seq<RegionState>, rv: Seq<usize>, n: Int) -> Int {
    pearlite! { if n <= 0 { 0 } else { used_sum(arena, rv, n - 1) + rg_used(arena[rv[n - 1]@]) } }
}

/// Every current region's term is in `0..=total_usable_size` (no underflow at lib.rs:704).
#[logic(open)]
pub fn used_terms_ok(arena: Seq<RegionState>, rv: Seq<usize>) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < rv.len() ==> bd_shape(arena[rv[i]@].buddy)
            && 0 <= rg_used(arena[rv[i]@])
            && tf(arena[rv[i]@].buddy.free_lists@, arena[rv[i]@].buddy.sector_size@, arena[rv[i]@].buddy.free_lists@.len()) <= u64::MAX@
    }
}

/// Partial sums of non-negative terms are bounded by the full sum.
#[logic]
#[variant(n - m)]
#[requires(0 <= m && m <= n && n <= rv.len())]
#[requires(used_terms_ok(arena, rv))]
#[ensures(0 <= used_sum(arena, rv, m) && used_sum(arena, rv, m) <= used_sum(arena, rv, n))]
pub fn lemma_used_mono(arena: Seq<RegionState>, rv: Seq<usize>, m: Int, n: Int) {
    if m < n {
        lemma_used_mono(arena, rv, m, n - 1)
    } else {
        lemma_used_nonneg(arena, rv, m)
    }
}

#[logic]
#[variant(n)]
#[requires(0 <= n && n <= rv.len())]
#[requires(used_terms_ok(arena, rv))]
#[ensures(0 <= used_sum(arena, rv, n))]
pub fn lemma_used_nonneg(arena: Seq<RegionState>, rv: Seq<usize>, n: Int) {
    if n > 0 {
        lemma_used_nonneg(arena, rv, n - 1)
    }
}

/// Every term equal to `c` gives `used_sum(k) == k * c`.
#[logic]
#[variant(k)]
#[requires(0 <= k && k <= rv.len())]
#[requires(forall<i: Int> 0 <= i && i < rv.len() ==> rg_used(arena[rv[i]@]) == c)]
#[ensures(used_sum(arena, rv, k) == k * c)]
pub fn lemma_used_const(arena: Seq<RegionState>, rv: Seq<usize>, c: Int, k: Int) {
    if k > 0 {
        lemma_used_const(arena, rv, c, k - 1)
    }
}

impl ExtentManager {
    /// Mirror of `IExtentManager::used_bytes` (lib.rs:698-708).
    #[requires(em_regions_wf(*self))]
    #[requires(match self.regions { Some(rv) => used_terms_ok(self.arena@, rv@) && used_sum(self.arena@, rv@, rv@.len()) <= u64::MAX@, None => true })]
    #[ensures(match self.regions { Some(rv) => result@ == used_sum(self.arena@, rv@, rv@.len()), None => result@ == 0 })]
    pub fn used_bytes(&self) -> u64 {
        match &self.regions {
            None => 0,
            Some(rv) => {
                let mut total: u64 = 0;
                let mut i: usize = 0;
                #[invariant(i@ <= rv@.len())]
                #[invariant(total@ == used_sum(self.arena@, rv@, i@))]
                while i < rv.len() {
                    proof_assert! { lemma_used_mono(self.arena@, rv@, i@ + 1, rv@.len()); used_sum(self.arena@, rv@, i@ + 1) <= u64::MAX@ };
                    let b = &self.arena[rv[i]].buddy;
                    let u = b.total_usable_size();
                    let f = b.total_free();
                    total += u - f;
                    i += 1;
                }
                total
            }
        }
    }
}

// =============================================================================
// Checkpoint coalescing protocol (lib.rs:53-57, 729-761) — EM-CKPT-INV-SINGLE-WRITER
// =============================================================================

/// `CheckpointCoalesce` (lib.rs:53-57), the state behind `checkpoint_coalesce: Mutex<..>`.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Co {
    pub completed_seq: u64,
    pub in_progress: bool,
}

/// Where one caller of `checkpoint()` is (every user thread and the background timer
/// thread, lib.rs:154, call the same function).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Th {
    /// not inside `checkpoint()`
    Idle,
    /// blocked in `checkpoint_done.wait(state)` (lib.rs:744) with its `needed` (lock released)
    Waiting(u64),
    /// between `drop(state)` (lib.rs:748) and the re-lock at lib.rs:752: running
    /// `run_checkpoint()` (lib.rs:750) — ALL checkpoint I/O (lib.rs:297-310) happens here
    Running(u64),
}

/// The coalescer plus one program counter per caller thread.
pub struct CoSys {
    pub co: Co,
    pub ts: Vec<Th>,
}

#[logic(open)]
pub fn is_run(t: Th) -> bool {
    pearlite! { match t { Th::Running(_) => true, _ => false } }
}

#[logic(open)]
pub fn th_need(t: Th) -> Int {
    pearlite! { match t { Th::Running(n) => n@, Th::Waiting(n) => n@, Th::Idle => 0 } }
}

/// A running thread implies `in_progress`.
#[logic(open)]
pub fn co_run_flag(s: CoSys) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < s.ts@.len() && is_run(s.ts@[i]) ==> s.co.in_progress }
}

/// AT MOST ONE thread is running a checkpoint.
#[logic(open)]
pub fn co_single(s: CoSys) -> bool {
    pearlite! {
        forall<i: Int, j: Int> 0 <= i && i < s.ts@.len() && 0 <= j && j < s.ts@.len()
            && is_run(s.ts@[i]) && is_run(s.ts@[j]) ==> i == j
    }
}

/// Every pending `needed` is at most `completed_seq + 2` (lib.rs:731-735), and a running
/// thread's `needed` exceeds `completed_seq` (it passed the test at lib.rs:738).
#[logic(open)]
pub fn co_bound(s: CoSys) -> bool {
    pearlite! {
        forall<i: Int> 0 <= i && i < s.ts@.len() ==> th_need(s.ts@[i]) <= s.co.completed_seq@ + 2
            && (is_run(s.ts@[i]) ==> s.co.completed_seq@ < th_need(s.ts@[i]))
    }
}

#[logic(open)]
pub fn co_inv(s: CoSys) -> bool {
    pearlite! { co_run_flag(s) && co_single(s) && co_bound(s) }
}

/// The step frame: thread count fixed, `completed_seq` monotone and growing by at most 2.
#[logic(open)]
pub fn co_step(a: CoSys, b: CoSys) -> bool {
    pearlite! {
        b.ts@.len() == a.ts@.len()
        && a.co.completed_seq@ <= b.co.completed_seq@ && b.co.completed_seq@ <= a.co.completed_seq@ + 2
    }
}

impl CoSys {
    /// One evaluation of the loop lib.rs:737-745 under the lock, for thread `t` with
    /// `needed` — on entry, or after a condvar wake-up (`notify_all` or spurious).
    #[requires(co_inv(*self) && t@ < self.ts@.len() && !is_run(self.ts@[t@]))]
    #[requires(needed@ <= self.co.completed_seq@ + 2)]
    #[ensures(co_inv(^self) && co_step(*self, ^self) && (^self).co.completed_seq == self.co.completed_seq)]
    pub fn check(&mut self, t: usize, needed: u64) {
        if self.co.completed_seq >= needed {
            self.ts[t] = Th::Idle; // return Ok(()) (lib.rs:738-740)
            return;
        }
        if !self.co.in_progress {
            // break (lib.rs:741-743); state.in_progress = true; drop(state) (lib.rs:747-748)
            self.co.in_progress = true;
            self.ts[t] = Th::Running(needed);
            return;
        }
        self.ts[t] = Th::Waiting(needed); // self.checkpoint_done.wait(state) (lib.rs:744)
    }

    /// `checkpoint()` called by thread `t`: lock (lib.rs:730), compute `needed`
    /// (lib.rs:731-735), first loop iteration.
    ///
    /// LEVEL-3 (phase D-B1): no headroom premise. When `completed_seq + 2` (in progress) or
    /// `completed_seq + 1` (idle) does not fit u64, lib.rs:731-735 panics while holding the
    /// lock in a debug build (the mutex is poisoned: every later `lock().unwrap()` panics too,
    /// so no checkpoint ever starts again) and wraps in a release build (`needed <=
    /// completed_seq`, so lib.rs:738-740 returns Ok(()) at once). Either way the step starts no
    /// checkpoint and changes no protocol state: modelled as a no-op for the caller (later
    /// steps of a poisoned run are over-approximated by the model's other steps, so a safety
    /// claim proved here covers them).
    #[requires(co_inv(*self) && t@ < self.ts@.len())]
    #[ensures(co_inv(^self) && co_step(*self, ^self))]
    pub fn enter(&mut self, t: usize) {
        match self.ts[t] {
            Th::Idle => {
                if self.co.in_progress {
                    if self.co.completed_seq > u64::MAX - 2 {
                        return;
                    }
                    let needed = self.co.completed_seq + 2;
                    self.check(t, needed);
                } else {
                    if self.co.completed_seq == u64::MAX {
                        return;
                    }
                    let needed = self.co.completed_seq + 1;
                    self.check(t, needed);
                }
            }
            _ => {}
        }
    }

    /// A waiting thread `t` re-acquires the lock after `wait` returns (lib.rs:744).
    #[requires(co_inv(*self) && t@ < self.ts@.len())]
    #[ensures(co_inv(^self) && co_step(*self, ^self))]
    pub fn wake(&mut self, t: usize) {
        match self.ts[t] {
            Th::Waiting(n) => self.check(t, n),
            _ => {}
        }
    }

    /// `run_checkpoint()` returned (`ok` = success) in thread `t`: re-lock (lib.rs:752),
    /// record success, clear `in_progress`, `notify_all` (lib.rs:753-758).
    #[requires(co_inv(*self) && t@ < self.ts@.len())]
    #[ensures(co_inv(^self) && co_step(*self, ^self))]
    pub fn finish(&mut self, t: usize, ok: bool) {
        match self.ts[t] {
            Th::Running(n) => {
                if ok {
                    self.co.completed_seq = n;
                }
                self.co.in_progress = false;
                self.ts[t] = Th::Idle;
            }
            _ => {}
        }
    }
}

/// One scheduler step: which thread does what next.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum CoOp {
    Enter,
    Wake,
    Finish(bool),
}

// =============================================================================
// Checkpoint copy bytes (checkpoint.rs:69-97 write, 117-173 read)
// =============================================================================

/// `u64::from_le_bytes` of 8 bytes (opaque decoding; `rd64` relates it to `le64`).
#[logic(opaque)]
pub fn dec64(b: Seq<u8>) -> u64 {
    dead
}

/// `u32::from_le_bytes` of 4 bytes.
#[logic(opaque)]
pub fn dec32(b: Seq<u8>) -> u32 {
    dead
}

/// TRUSTED std leaf: `u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap())`, named.
#[trusted]
#[requires(pos@ + 8 <= buf@.len())]
#[ensures(result == dec64(buf@.subsequence(pos@, pos@ + 8)))]
#[ensures(forall<v: u64> (forall<k: Int> 0 <= k && k < 8 ==> buf@[pos@ + k] == le64(v, k)) ==> result == v)]
pub fn rd64(buf: &Vec<u8>, pos: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&buf[pos..pos + 8]);
    u64::from_le_bytes(a)
}

/// TRUSTED std leaf: `u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap())`, named.
#[trusted]
#[requires(pos@ + 4 <= buf@.len())]
#[ensures(result == dec32(buf@.subsequence(pos@, pos@ + 4)))]
#[ensures(forall<v: u32> (forall<k: Int> 0 <= k && k < 4 ==> buf@[pos@ + k] == le32(v, k)) ==> result == v)]
pub fn rd32(buf: &Vec<u8>, pos: usize) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&buf[pos..pos + 4]);
    u32::from_le_bytes(a)
}

/// The CRC input of checkpoint.rs:86 / :162-164: the first `total` bytes with the CRC field
/// (bytes 12..16) set to `0u32.to_le_bytes()`.
#[logic(open)]
pub fn chk(s: Seq<u8>, total: Int) -> Seq<u8> {
    pearlite! {
        s.subsequence(0, total).set(12, le32(0u32, 0)).set(13, le32(0u32, 1)).set(14, le32(0u32, 2)).set(15, le32(0u32, 3))
    }
}

/// `x` rounded up to a multiple of `d` (checkpoint.rs:91-92, 150).
#[logic(open)]
pub fn alignup(x: Int, d: Int) -> Int {
    pearlite! { (x + d - 1) / d * d }
}

/// The bytes write_checkpoint writes for (`seq`, `payload`), sector `ss` (checkpoint.rs:80-93):
/// seq (8 LE) ++ (payload.len() as u32) (4 LE) ++ CRC (4 LE) ++ payload ++ zero padding, where
/// the CRC is crc32fast of the first 16 + len bytes with a zero CRC field.
#[logic(open)]
pub fn blob_ok(b: Seq<u8>, seq: u64, l: u32, p: Seq<u8>, ss: Int) -> bool {
    pearlite! {
        b.len() == alignup(16 + p.len(), ss)
        && (forall<k: Int> 0 <= k && k < 8 ==> b[k] == le64(seq, k))
        && (forall<k: Int> 0 <= k && k < 4 ==> b[8 + k] == le32(l, k))
        && (forall<i: Int> 0 <= i && i < p.len() ==> b[16 + i] == p[i])
        && (forall<j: Int> 16 + p.len() <= j && j < b.len() ==> b[j] == 0u8)
        && (forall<k: Int> 0 <= k && k < 4 ==> b[12 + k] == le32(crc32(chk(b, 16 + p.len())), k))
    }
}

/// `l` is `n as u32` (Rust's truncating cast, slab.rs `as_u32`).
#[logic(open)]
pub fn is_trunc32(l: u32, n: Int) -> bool {
    pearlite! { l@ == n % 4294967296 }
}

/// TRUSTED std fact: `u64::from_le_bytes` inverts `u64::to_le_bytes` (the same fact batch 2's
/// get64 contract states at each call).
#[trusted]
#[logic]
#[requires(s.len() == 8 && forall<k: Int> 0 <= k && k < 8 ==> s[k] == le64(v, k))]
#[ensures(dec64(s) == v)]
pub fn lemma_dec64(s: Seq<u8>, v: u64) {}

/// TRUSTED std fact: `u32::from_le_bytes` inverts `u32::to_le_bytes`.
#[trusted]
#[logic]
#[requires(s.len() == 4 && forall<k: Int> 0 <= k && k < 4 ==> s[k] == le32(v, k))]
#[ensures(dec32(s) == v)]
pub fn lemma_dec32(s: Seq<u8>, v: u32) {}

/// `dst.extend_from_slice(src)`.
#[requires(dst@.len() + src@.len() <= usize::MAX@)]
#[ensures((^dst)@ == dst@.concat(src@))]
pub fn append(dst: &mut Vec<u8>, src: &Vec<u8>) {
    let old = snapshot! { *dst };
    let mut i: usize = 0;
    #[invariant(i@ <= src@.len())]
    #[invariant(dst@ == old@.concat(src@.subsequence(0, i@)))]
    while i < src.len() {
        dst.push(src[i]);
        i += 1;
        proof_assert! { src@.subsequence(0, i@).ext_eq(src@.subsequence(0, i@ - 1).push_back(src@[i@ - 1])) };
    }
    proof_assert! { src@.subsequence(0, src@.len()).ext_eq(src@) };
}

/// `src[a..b].to_vec()`.
#[requires(a@ <= b@ && b@ <= src@.len())]
#[ensures(result@ == src@.subsequence(a@, b@))]
pub fn slice_vec(src: &Vec<u8>, a: usize, b: usize) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::new();
    let mut i: usize = a;
    #[invariant(a@ <= i@ && i@ <= b@)]
    #[invariant(v@ == src@.subsequence(a@, i@))]
    while i < b {
        v.push(src[i]);
        i += 1;
        proof_assert! { src@.subsequence(a@, i@).ext_eq(src@.subsequence(a@, i@ - 1).push_back(src@[i@ - 1])) };
    }
    v
}

/// `blob[pos..pos + 4].copy_from_slice(&v.to_le_bytes())`.
#[requires(pos@ + 4 <= buf@.len())]
#[ensures((^buf)@.len() == buf@.len())]
#[ensures(forall<k: Int> 0 <= k && k < 4 ==> (^buf)@[pos@ + k] == le32(v, k))]
#[ensures(forall<q: Int> 0 <= q && q < buf@.len() && (q < pos@ || q >= pos@ + 4) ==> (^buf)@[q] == buf@[q])]
pub fn put32b(buf: &mut Vec<u8>, pos: usize, v: u32) {
    let b = to_le_bytes32(v);
    let old = snapshot! { *buf };
    let mut k: usize = 0;
    #[invariant(k@ <= 4 && buf@.len() == old@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < k@ ==> buf@[pos@ + j] == le32(v, j))]
    #[invariant(forall<q: Int> 0 <= q && q < old@.len() && (q < pos@ || q >= pos@ + k@) ==> buf@[q] == old@[q])]
    while k < 4 {
        buf[pos + k] = b[k];
        k += 1;
    }
}

/// Mirror of write_checkpoint's blob construction (checkpoint.rs:69-93) for sector `ss`
/// (any `ss > 0`, dfd6b0; level 3 widened from `ss >= 16`).
#[requires(ss@ > 0)]
#[requires(16 + payload@.len() + ss@ <= usize::MAX@)]
#[ensures(forall<l: u32> is_trunc32(l, payload@.len()) ==> blob_ok(result@, seq, l, payload@, ss@))]
pub fn build_blob(seq: u64, payload: &Vec<u8>, ss: usize) -> Vec<u8> {
    let mut blob: Vec<u8> = Vec::new();
    append(&mut blob, &to_le_bytes64(seq)); // 8 bytes
    let plen = as_u32(payload.len() as u64);
    proof_assert! { is_trunc32(plen, payload@.len()) };
    proof_assert! { forall<l: u32> is_trunc32(l, payload@.len()) ==> l == plen };
    append(&mut blob, &to_le_bytes32(plen)); // 4 bytes
    append(&mut blob, &to_le_bytes32(0u32)); // 4 bytes CRC placeholder
    append(&mut blob, payload);
    let b0 = snapshot! { blob@ };
    proof_assert! { b0.len() == 16 + payload@.len() };
    proof_assert! { forall<k: Int> 0 <= k && k < 4 ==> b0[12 + k] == le32(0u32, k) };
    proof_assert! { chk(*b0, 16 + payload@.len()).ext_eq(*b0) };
    let n = blob.len();
    let crc = crc32_hash(&blob, n);
    proof_assert! { b0.subsequence(0, n@).ext_eq(*b0) };
    proof_assert! { crc == crc32(*b0) };
    put32b(&mut blob, 12, crc);
    let b1 = snapshot! { blob@ };
    proof_assert! { chk(*b1, 16 + payload@.len()).ext_eq(*b0) };
    // pad to the sector boundary (checkpoint.rs:91-93)
    let aligned = (n + ss - 1) / ss * ss;
    proof_assert! { lemma_divmod(n@ + ss@ - 1, ss@); aligned@ >= n@ && aligned@ <= n@ + ss@ };
    let mut i: usize = n;
    #[invariant(n@ <= i@ && i@ <= aligned@ && blob@.len() == i@)]
    #[invariant(forall<j: Int> 0 <= j && j < n@ ==> blob@[j] == b1[j])]
    #[invariant(forall<j: Int> n@ <= j && j < i@ ==> blob@[j] == 0u8)]
    while i < aligned {
        blob.push(0u8);
        i += 1;
    }
    proof_assert! { chk(blob@, 16 + payload@.len()).ext_eq(chk(*b1, 16 + payload@.len())) };
    blob
}

/// The metadata device bytes of one checkpoint copy region, from its first LBA on, as the
/// device holds them (block_io.rs write_blocks / read_blocks per the IBlockDevice contract:
/// a completed write is read back unchanged; reads succeed).
#[ensures((^area)@.len() == area@.len())]
#[ensures(forall<i: Int> 0 <= i && i < blob@.len() ==> (^area)@[i] == blob@[i])]
#[ensures(forall<i: Int> blob@.len() <= i && i < area@.len() ==> (^area)@[i] == area@[i])]
#[requires(blob@.len() <= area@.len())]
pub fn dev_write_area(area: &mut Vec<u8>, blob: &Vec<u8>) {
    let old = snapshot! { *area };
    let mut i: usize = 0;
    #[invariant(i@ <= blob@.len() && area@.len() == old@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> area@[j] == blob@[j])]
    #[invariant(forall<j: Int> i@ <= j && j < old@.len() ==> area@[j] == old@[j])]
    while i < blob.len() {
        area[i] = blob[i];
        i += 1;
    }
}

/// `x <= r` and `r` a multiple of `s` give `alignup(x, s) <= r` (LEVEL-3 phase D-A: the
/// padded copy write / read stays inside a copy region whose size is a multiple of the
/// metadata sector, D-RANGE-FMT-METADATA-DEVICE-LAYOUT-dfd6b0).
#[logic]
#[requires(s > 0 && 0 <= x && x <= r && r % s == 0)]
#[ensures(alignup(x, s) <= r)]
pub fn lemma_alignup_le(x: Int, s: Int, r: Int) {
    pearlite! {
        lemma_divmod(x + s - 1, s);
        lemma_divmod(r, s);
        proof_assert! { (x + s - 1) / s * s <= x + s - 1 };
        proof_assert! { r == s * (r / s) };
        if (x + s - 1) / s > r / s {
            lemma_mul_le(r / s + 1, (x + s - 1) / s, s);
            proof_assert! { (r / s + 1) * s == r + s }
        }
    }
}

/// Phases 2-3 of write_checkpoint (checkpoint.rs:69-97) for (`new_seq`, `payload`) into the
/// copy region `area` of `region_size` bytes; the device write succeeds (any `ss > 0`).
/// LEVEL-3 (phase D-A): the copy region lies on the device (`region_size <= area.len()`,
/// D-RANGE-FR-036-1d780b) and is a whole number of metadata sectors (`region_size % ss == 0`,
/// D-RANGE-FMT-METADATA-DEVICE-LAYOUT-dfd6b0) — replaces the undeclared `region_size + ss <= area.len()`.
#[requires(ss@ > 0 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(16 + payload@.len() + ss@ <= usize::MAX@)]
#[ensures(match result {
    Ok(()) => 16 + payload@.len() <= region_size@
        && forall<l: u32> is_trunc32(l, payload@.len()) ==>
            blob_ok((^area)@.subsequence(0, alignup(16 + payload@.len(), ss@)), new_seq, l, payload@, ss@),
    Err(e) => e == EmError::CorruptMetadata && ^area == *area,
})]
#[ensures((^area)@.len() == area@.len())]
#[ensures(16 + payload@.len() <= region_size@ ==> result == Ok(()))]
pub fn write_ckpt_bytes(area: &mut Vec<u8>, ss: usize, region_size: u64, new_seq: u64, payload: &Vec<u8>) -> Result<(), EmError> {
    let total_needed = 16 + payload.len();
    if total_needed as u64 > region_size {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:69-75
    }
    let blob = build_blob(new_seq, payload, ss);
    let l = as_u32(payload.len() as u64);
    proof_assert! { is_trunc32(l, payload@.len()) };
    proof_assert! { blob_ok(blob@, new_seq, l, payload@, ss@) && blob@.len() == alignup(16 + payload@.len(), ss@) };
    proof_assert! { lemma_divmod(16 + payload@.len() + ss@ - 1, ss@); blob@.len() <= 16 + payload@.len() + ss@ - 1 };
    proof_assert! { lemma_alignup_le(16 + payload@.len(), ss@, region_size@); blob@.len() <= region_size@ };
    proof_assert! { blob@.len() <= area@.len() };
    dev_write_area(area, &blob);
    proof_assert! { area@.subsequence(0, blob@.len()).ext_eq(blob@) };
    proof_assert! { forall<m: u32> is_trunc32(m, payload@.len()) ==> m == l };
    Ok(())
}

/// Mirror of `read_checkpoint_region` (checkpoint.rs:117-173) over the copy region `area`
/// (reads succeed and return the stored bytes; `ss` = metadata sector size). Its EXACT
/// accept condition: header seq == expected, 16 + header length <= region size, and the
/// stored CRC field equals crc32 of the first 16 + length bytes with a zeroed CRC field;
/// otherwise CorruptMetadata (never another error). Level 3 (phase F): any sector size
/// `ss > 0` (dfd6b0); a sector shorter than the 16-byte header is CorruptMetadata
/// (checkpoint.rs:128-130), so the exact accept condition is stated for `ss >= 16`.
/// LEVEL-3 (phase D-A): as write_ckpt_bytes (1d780b, dfd6b0), plus the header read's one
/// sector on the device (`ss <= area.len()`, implied when the copy region is non-empty).
#[requires(ss@ > 0 && region_size@ <= area@.len() && region_size@ % ss@ == 0 && area@.len() + ss@ <= usize::MAX@)]
#[requires(region_size@ > 0 || ss@ <= area@.len())]
#[ensures(ss@ < 16 ==> result == Err(EmError::CorruptMetadata))]
#[ensures(match result {
    Ok(v) => ss@ >= 16 && rd_accepts(area@, region_size@, expected)
        && v@ == area@.subsequence(16, 16 + dec32(area@.subsequence(8, 12))@),
    Err(e) => e == EmError::CorruptMetadata && (ss@ >= 16 ==> !rd_accepts(area@, region_size@, expected)),
})]
pub fn read_ckpt_bytes(area: &Vec<u8>, ss: usize, region_size: u64, expected: u64) -> Result<Vec<u8>, EmError> {
    proof_assert! { region_size@ > 0 ==> { lemma_divmod(region_size@, ss@); lemma_mul_le(1, region_size@ / ss@, ss@); region_size@ >= ss@ } };
    // Phase 1: one sector (checkpoint.rs:127)
    let header = slice_vec(area, 0, ss);
    if header.len() < 16 {
        return Err(EmError::CorruptMetadata);
    }
    let seq = rd64(&header, 0);
    let payload_len = rd32(&header, 8) as usize;
    let stored_crc = rd32(&header, 12);
    proof_assert! { header@.subsequence(0, 8).ext_eq(area@.subsequence(0, 8)) };
    proof_assert! { header@.subsequence(8, 12).ext_eq(area@.subsequence(8, 12)) };
    proof_assert! { header@.subsequence(12, 16).ext_eq(area@.subsequence(12, 16)) };
    if seq != expected {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:136-140
    }
    let total = 16 + payload_len;
    if total as u64 > region_size {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:142-147
    }
    // Phase 2 (checkpoint.rs:150-156)
    let aligned_len = (total + ss - 1) / ss * ss;
    proof_assert! { lemma_divmod(total@ + ss@ - 1, ss@); aligned_len@ >= total@ && aligned_len@ <= total@ + ss@ };
    proof_assert! { lemma_alignup_le(total@, ss@, region_size@); aligned_len@ <= region_size@ };
    let raw = if aligned_len <= ss { header } else { slice_vec(area, 0, aligned_len) };
    proof_assert! { raw@.len() >= total@ };
    proof_assert! { forall<j: Int> 0 <= j && j < total@ ==> raw@[j] == area@[j] };
    if raw.len() < total {
        return Err(EmError::CorruptMetadata); // checkpoint.rs:158-160
    }
    // CRC (checkpoint.rs:162-170)
    let mut check_buf = slice_vec(&raw, 0, total);
    put32b(&mut check_buf, 12, 0u32);
    let computed = crc32_hash(&check_buf, total);
    proof_assert! { check_buf@.subsequence(0, total@).ext_eq(chk(area@, total@)) };
    if stored_crc != computed {
        return Err(EmError::CorruptMetadata);
    }
    let out = slice_vec(&raw, 16, total);
    proof_assert! { out@.ext_eq(area@.subsequence(16, total@)) };
    Ok(out)
}

/// read_checkpoint_region's accept condition over the region bytes `a`.
#[logic(open)]
pub fn rd_accepts(a: Seq<u8>, region_size: Int, expected: u64) -> bool {
    pearlite! {
        dec64(a.subsequence(0, 8)) == expected
        && 16 + dec32(a.subsequence(8, 12))@ <= region_size
        && dec32(a.subsequence(12, 16)) == crc32(chk(a, 16 + dec32(a.subsequence(8, 12))@))
    }
}

// =============================================================================
// format() racing a checkpoint in flight (lib.rs:496-507 vs lib.rs:299-307)
// =============================================================================

/// Decoded metadata device for the race: the superblock sector and the two copies.
pub struct FDev {
    pub sb: Option<Superblock>,
    pub copy0: Option<CkCopy>,
    pub copy1: Option<CkCopy>,
}

/// What the race touches: the current regions and the in-memory superblock
/// (`shared.superblock`; `shared.checkpoint_seq` mirrors its checkpoint_seq, EM-INV-SEQ-AGREE).
pub struct FEm {
    pub regions: Vec<RegionState>,
    pub sb: Superblock,
    pub dev: FDev,
}

impl FEm {
    /// write_checkpoint (checkpoint.rs:35-115) with every device write succeeding: runs
    /// ENTIRELY under `regions.read()` (checkpoint.rs:40 — format's `regions.write()` at
    /// lib.rs:506 waits for it) — serialise every region, write copy `1 - active_copy` with
    /// `checkpoint_seq + 1`, then advance the in-memory superblock.
    #[requires(a_0de740(self.sb.active_copy@) && a_9c0757(self.sb.checkpoint_seq@))]
    #[ensures((^self).regions == self.regions && (^self).dev.sb == self.dev.sb)]
    #[ensures((^self).sb.checkpoint_seq@ == self.sb.checkpoint_seq@ + 1 && (^self).sb.active_copy@ == 1 - self.sb.active_copy@)]
    #[ensures(sb_frame(self.sb, (^self).sb))]
    #[ensures(match fcopy((^self).dev, (^self).sb.active_copy) { Some(c) => c.seq == (^self).sb.checkpoint_seq && *c.img == self.regions@, None => false })]
    #[ensures(fcopy((^self).dev, self.sb.active_copy) == fcopy(self.dev, self.sb.active_copy))]
    pub fn write_checkpoint(&mut self) {
        let img = snapshot! { self.regions@ };
        let inactive: u8 = if self.sb.active_copy == 0 { 1 } else { 0 };
        let new_seq = self.sb.checkpoint_seq + 1;
        if inactive == 0 {
            self.dev.copy0 = Some(CkCopy { seq: new_seq, img });
        } else {
            self.dev.copy1 = Some(CkCopy { seq: new_seq, img });
        }
        self.sb.active_copy = inactive;
        self.sb.checkpoint_seq = new_seq;
    }

    /// run_checkpoint's superblock write (lib.rs:302-307): the CURRENT in-memory superblock.
    #[ensures((^self).dev.sb == Some(self.sb))]
    #[ensures((^self).regions == self.regions && (^self).sb == self.sb)]
    #[ensures((^self).dev.copy0 == self.dev.copy0 && (^self).dev.copy1 == self.dev.copy1)]
    pub fn write_superblock(&mut self) {
        self.dev.sb = Some(self.sb);
    }

    /// format's superblock write (lib.rs:496-498): taken under NO lock that run_checkpoint
    /// also takes (format holds neither `checkpoint_coalesce` nor `regions` here).
    #[ensures((^self).dev.sb == Some(sb))]
    #[ensures((^self).regions == self.regions && (^self).sb == self.sb)]
    #[ensures((^self).dev.copy0 == self.dev.copy0 && (^self).dev.copy1 == self.dev.copy1)]
    pub fn format_write_sb(&mut self, sb: Superblock) {
        self.dev.sb = Some(sb);
    }

    /// format's publish (lib.rs:506-507) and `Ok(())` (lib.rs:511).
    #[ensures((^self).regions == rv && (^self).sb == sb && (^self).dev.sb == self.dev.sb)]
    #[ensures((^self).dev.copy0 == self.dev.copy0 && (^self).dev.copy1 == self.dev.copy1)]
    #[ensures(result == Ok(()))]
    pub fn format_publish(&mut self, rv: Vec<RegionState>, sb: Superblock) -> Result<(), EmError> {
        self.regions = rv;
        self.sb = sb;
        Ok(())
    }
}

#[logic(open)]
pub fn fcopy(d: FDev, c: u8) -> Option<CkCopy> {
    pearlite! { if c@ == 0 { d.copy0 } else { d.copy1 } }
}

/// `recovery::recover` (recovery.rs:11-71) over the decoded device: the superblock and the
/// image a fresh component's initialize() rebuilds from (no superblock: IoError/Corrupt).
#[ensures(match d.sb {
    Some(sb) => sb.checkpoint_seq@ > 0 && sb.active_copy@ <= 1 ==> match fcopy(*d, sb.active_copy) {
        Some(c) => c.seq == sb.checkpoint_seq ==> result == Ok((sb, c.img)),
        None => true,
    },
    None => result == Err(EmError::CorruptMetadata),
})]
pub fn recover_f(d: &FDev) -> Result<(Superblock, Snapshot<Seq<RegionState>>), EmError> {
    let sb = match d.sb {
        Some(sb) => sb,
        None => return Err(EmError::CorruptMetadata),
    };
    if sb.checkpoint_seq == 0 {
        return Ok((sb, snapshot! { Seq::empty() }));
    }
    let (act, inact) = if sb.active_copy == 0 { (&d.copy0, &d.copy1) } else { (&d.copy1, &d.copy0) };
    match act {
        Some(c) => {
            if c.seq == sb.checkpoint_seq {
                return Ok((sb, snapshot! { *c.img }));
            }
        }
        None => {}
    }
    let prev_seq = sb.checkpoint_seq - 1;
    if prev_seq > 0 {
        match inact {
            Some(c) => {
                if c.seq == prev_seq {
                    return Ok((sb, snapshot! { *c.img }));
                }
            }
            None => {}
        }
    }
    Err(EmError::CorruptMetadata)
}
