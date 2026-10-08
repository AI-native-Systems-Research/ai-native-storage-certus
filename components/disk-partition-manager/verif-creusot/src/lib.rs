//! Creusot proof crate for the `disk-partition-manager` component
//! (interface `IPartitionTable`, 4 public methods).
//!
//! FAITHFUL PURE-CORE MIRROR of `components/disk-partition-manager/src/lib.rs` and
//! `src/gpt.rs`. The product crate cannot be built under Creusot: `Arc<dyn IBlockDevice +
//! Send + Sync>`, `Mutex<Option<PartitionTable>>`, the crossbeam `command_tx`/`completion_rx`
//! channel pair, `DmaBuffer`, `crc32fast::hash`, `std::fs::File::open("/dev/urandom")`,
//! `String::encode_utf16`/`from_utf16_lossy` and the `define_component!` macro expansion are
//! all outside Creusot's model. Each obligation from `verif/unified_properties.yaml` is
//! therefore re-expressed as a `#[requires]`/`#[ensures]` contract over a mirror of the
//! corresponding source path, with the source line cited on every mirror.
//!
//! Naming (the gate finds work by convention):
//!   verify_<id>            base proof          (id lowercased, '-' -> '_')
//!   verify_<id>__mutant    anti-vacuity twin   (MUST fail)
//!   refute_<id>            refutation          (PROVES the negation => the code violates it)
//!
//! Fidelity classes recorded per-id in `verif/creusot_advisory.yaml`:
//!   real-type        - the mirror is the source arithmetic / control flow on the same
//!                      integer types with the same operators.
//!   ghost-mirror     - a std container or an opaque runtime value is modelled by a
//!                      logic Seq / scalar tag; what is proved is the logic over the model.
//!   trusted-boundary - the value crosses a boundary Creusot cannot model (block-device I/O
//!                      completion, DMA allocation, crc32fast, /dev/urandom, Mutex poisoning,
//!                      UTF-16 transcoding); the boundary's RESULT arrives as a parameter and
//!                      what is proved is how the component MAPS that result.

#![allow(dead_code)]
#![recursion_limit = "8192"]

use creusot_std::logic::{Mapping, Seq};
use creusot_std::prelude::{vec, *};

// ===========================================================================
// 0. GPT constants — gpt.rs:8-12
// ===========================================================================

pub const GPT_SIGNATURE: u64 = 0x5452_4150_2049_4645; // "EFI PART" little-endian
pub const GPT_REVISION_1_0: u32 = 0x0001_0000;
pub const GPT_HEADER_SIZE: u32 = 92;
pub const GPT_ENTRY_SIZE: u32 = 128;
pub const GPT_MAX_ENTRIES: u32 = 128;

/// Total bytes of one partition-entry array: 128 slots x 128 bytes = 16384 (gpt.rs:325, 433).
pub const ENTRY_ARRAY_BYTES: u32 = 16384;

// ===========================================================================
// 1. Ceiling division — the `div_ceil` the source uses (gpt.rs:267, 283, 326, 452, 502)
//
//    `u32::div_ceil` / `u64::div_ceil` are std functions creusot-std ships NO contract for
//    (measured: "calling external function `div_ceil` with no contract will yield an
//    impossible precondition"), so they are mirrored here by the same total function, written
//    without the `n + d - 1` overflow the naive mirror would introduce.
// ===========================================================================

#[logic(open)]
pub fn cdiv(n: Int, d: Int) -> Int {
    pearlite! { n.div_euclid(d) + if n.rem_euclid(d) == 0 { 0 } else { 1 } }
}

/// `n.div_ceil(d)` for u32 (gpt.rs:326).
#[requires(d@ > 0)]
#[ensures(result@ == cdiv(n@, d@))]
pub fn cdiv32(n: u32, d: u32) -> u32 {
    let q = n / d;
    if n % d == 0 { q } else { q + 1 }
}

/// `n.div_ceil(d)` for u64 (gpt.rs:267, 283, 452, 502).
#[requires(d@ > 0)]
#[ensures(result@ == cdiv(n@, d@))]
pub fn cdiv64(n: u64, d: u64) -> u64 {
    let q = n / d;
    if n % d == 0 { q } else { q + 1 }
}

// ===========================================================================
// 2. Device geometry — `GptManager { sector_size, num_sectors, ns_id }` (gpt.rs:41-46)
// ===========================================================================

/// `GptManager::entry_sectors` — gpt.rs:324-327:
///   `let entry_bytes = GPT_MAX_ENTRIES * GPT_ENTRY_SIZE; entry_bytes.div_ceil(self.sector_size)`
#[logic]
pub fn entry_sectors_l(ss: Int) -> Int {
    pearlite! { cdiv(16384, ss) }
}

#[requires(ss@ > 0)]
#[ensures(result@ == entry_sectors_l(ss@))]
pub fn entry_sectors(ss: u32) -> u32 {
    cdiv32(ENTRY_ARRAY_BYTES, ss)
}

// ===========================================================================
// 3. The ghost device-I/O trace
//
//    Every device access in the component goes through exactly `read_bytes` (gpt.rs:451) or
//    `write_bytes` (gpt.rs:501), each of which issues ONE `Command::ReadSync`/`WriteSync` per
//    sector over `[lba, lba + ceil(len/sector_size))`. An access is therefore fully described
//    by (direction, namespace, first lba, sector count), and a run of the component by the
//    SEQUENCE of accesses it issues. Frame properties ("never writes outside ...", "issues
//    only reads", "the count does not grow with the device") are statements about this trace.
// ===========================================================================

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Acc {
    /// true = `Command::WriteSync` (gpt.rs:525), false = `Command::ReadSync` (gpt.rs:463)
    pub is_write: bool,
    /// `ns_id` carried on the command (gpt.rs:464, 526)
    pub ns: u32,
    /// first LBA touched
    pub lba: u64,
    /// number of consecutive sectors touched (one command each)
    pub sectors: u64,
}

#[logic(open)]
pub fn touches(a: Acc, l: Int) -> bool {
    pearlite! { a.lba@ <= l && l < a.lba@ + a.sectors@ }
}

/// The trace a whole operation issued, in order.
pub type Trace = Seq<Acc>;

#[logic(open)]
pub fn no_writes(t: Trace) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < t.len() ==> !t[i].is_write }
}

#[logic(open)]
pub fn all_ns(t: Trace, n: Int) -> bool {
    pearlite! { forall<i: Int> 0 <= i && i < t.len() ==> t[i].ns@ == n }
}

#[logic(open)]
pub fn writes_avoid(t: Trace, lo: Int, hi: Int) -> bool {
    pearlite! {
        forall<i: Int, l: Int> 0 <= i && i < t.len() && t[i].is_write && touches(t[i], l)
            ==> l < lo || hi < l
    }
}

// ===========================================================================
// 4. GUIDs — `[u8; 16]` as the source carries them (ipartition_table.rs:18-20, gpt.rs:33-34)
//
//    Creusot handles `[u8; 16]` natively (indexing + `len()` both proved on this toolchain),
//    so the GUID is the REAL type, not a mirror.
// ===========================================================================

/// `generate_guid` — gpt.rs:564-574. The `/dev/urandom` read is the one effect Creusot cannot
/// model, so it arrives as the trusted-boundary parameters `opened` (did
/// `std::fs::File::open("/dev/urandom")` succeed?) and `bytes` (what `read_exact` delivered).
/// The stub CONFORMS to std's contract: `File::open` returns `Result`, and on the `Err` path
/// the source's `if let Ok(mut f) = ...` body never runs, so `guid` keeps the `[0u8; 16]` it
/// was initialised with at gpt.rs:565 — modelled exactly by `opened == false`.
#[bitwise_proof]
#[ensures(result[6] & 0xF0u8 == 0x40u8)]
#[ensures(result[8] & 0xC0u8 == 0x80u8)]
#[ensures(!opened ==> result[6] == 0x40u8)]
#[ensures(!opened ==> result[8] == 0x80u8)]
#[ensures(!opened ==> forall<i: Int> 0 <= i && i < 16 && i != 6 && i != 8 ==> result[i] == 0u8)]
#[ensures(opened ==> forall<i: Int> 0 <= i && i < 16 && i != 6 && i != 8 ==> result[i] == bytes[i])]
pub fn generate_guid(opened: bool, bytes: [u8; 16]) -> [u8; 16] {
    let mut guid = [0u8; 16];
    if opened {
        guid = bytes; // `f.read_exact(&mut guid)` on success
    }
    // gpt.rs:571-572 — applied UNCONDITIONALLY, on the zero fallback path too
    guid[6] = (guid[6] & 0x0F) | 0x40; // version 4
    guid[8] = (guid[8] & 0x3F) | 0x80; // variant 1
    guid
}

// ===========================================================================
// 5. Partition layout — `compute_partition_layout` (gpt.rs:239-322)
// ===========================================================================

/// Mirror of `interfaces::PartitionSpec` (ipartition_table.rs:27-33). The `name: String` is
/// handled separately in the UTF-16 section (a `Seq<u16>` code-unit model); layout depends
/// only on `size_bytes` and, for the result projection, on whether `type_guid` is all-zero.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct SpecM {
    pub size_bytes: u64,
    /// true iff `type_guid == [0u8; 16]` — the predicate the result projection filters on
    /// (gpt.rs:135, 222).
    pub tg_zero: bool,
}

/// Mirror of the private `GptEntry` (gpt.rs:31-39), minus the 72-byte name field.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct EntryM {
    pub tg_zero: bool,
    pub start: u64,
    pub end: u64,
    pub attrs: u64,
}

/// The three `PartitionTableError::LayoutError` sites of the layout pass.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum LayoutErr {
    /// gpt.rs:256-259 — more than one `size_bytes == 0`
    MultiRest,
    /// gpt.rs:270-274 — fixed partitions exceed the usable space
    Capacity,
    /// gpt.rs:286-290 — one partition needs more sectors than remain
    PerPartition,
}

/// `config.partitions.iter().filter(|p| p.size_bytes == 0).count()` over the first `n`
/// specs — gpt.rs:251-255.
#[logic(open)]
#[requires(0 <= n && n <= s.len())]
#[variant(n)]
pub fn rest_count_l(s: Seq<SpecM>, n: Int) -> Int {
    pearlite! {
        if n == 0 { 0 }
        else { rest_count_l(s, n - 1) + if s[n - 1].size_bytes@ == 0 { 1 } else { 0 } }
    }
}

/// `config.partitions.iter().filter(|p| p.size_bytes > 0).map(ceil).sum()` over the first `n`
/// specs — gpt.rs:263-268. MATHEMATICAL sum: the source's `Iterator::sum::<u64>()` wraps in a
/// release build and panics in debug; `verify_dpm_format_sum_overflow` is where that gap lives.
#[logic(open)]
#[requires(0 <= n && n <= s.len())]
#[variant(n)]
pub fn fixed_sum_l(s: Seq<SpecM>, n: Int, ss: Int) -> Int {
    pearlite! {
        if n == 0 { 0 }
        else {
            fixed_sum_l(s, n - 1, ss)
                + if s[n - 1].size_bytes@ > 0 { cdiv(s[n - 1].size_bytes@, ss) } else { 0 }
        }
    }
}

/// Per-spec sector demand once `rest_sectors` is known — gpt.rs:280-284.
#[logic(open)]
pub fn demand_l(sp: SpecM, ss: Int, rest: Int) -> Int {
    pearlite! { if sp.size_bytes@ == 0 { rest } else { cdiv(sp.size_bytes@, ss) } }
}

/// Sum of the per-spec sector demand over the first `n` specs — i.e. exactly
/// `current_lba - first_usable_lba` after `n` iterations of the placement loop (gpt.rs:279-307).
#[logic(open)]
#[requires(0 <= n && n <= s.len())]
#[variant(n)]
pub fn placed_l(s: Seq<SpecM>, n: Int, ss: Int, rest: Int) -> Int {
    pearlite! {
        if n == 0 { 0 } else { placed_l(s, n - 1, ss, rest) + demand_l(s[n - 1], ss, rest) }
    }
}

// ---- arithmetic facts about `cdiv` the solvers need as hypotheses ----
#[logic]
#[requires(n >= 0 && d > 0)]
#[ensures(cdiv(n, d) >= 0)]
#[ensures(cdiv(n, d) * d >= n)]
#[ensures(n > 0 ==> cdiv(n, d) >= 1)]
pub fn lemma_cdiv_nonneg(n: Int, d: Int) {}

/// `placed_l` splits into the fixed demand plus `rest` for each rest-of-disk spec.
#[logic]
#[requires(0 <= n && n <= s.len())]
#[ensures(placed_l(s, n, ss, rest) == fixed_sum_l(s, n, ss) + rest * rest_count_l(s, n))]
#[variant(n)]
pub fn lemma_placed_split(s: Seq<SpecM>, n: Int, ss: Int, rest: Int) {
    pearlite! { if n > 0 { lemma_placed_split(s, n - 1, ss, rest) } }
}

#[logic]
#[requires(0 <= n && n <= s.len())]
#[requires(ss > 0)]
#[ensures(fixed_sum_l(s, n, ss) >= 0)]
#[variant(n)]
pub fn lemma_fixed_sum_nonneg(s: Seq<SpecM>, n: Int, ss: Int) {
    pearlite! {
        if n > 0 {
            lemma_fixed_sum_nonneg(s, n - 1, ss);
            lemma_cdiv_nonneg(s[n - 1].size_bytes@, ss)
        }
    }
}

/// `fixed_sum_l` is non-decreasing in the prefix length (every term is >= 0).
#[logic]
#[requires(0 <= j && j <= k && k <= s.len())]
#[requires(ss > 0)]
#[ensures(fixed_sum_l(s, j, ss) <= fixed_sum_l(s, k, ss))]
#[variant(k - j)]
pub fn lemma_fixed_sum_mono(s: Seq<SpecM>, j: Int, k: Int, ss: Int) {
    pearlite! {
        if j < k {
            lemma_fixed_sum_mono(s, j, k - 1, ss);
            lemma_cdiv_nonneg(s[k - 1].size_bytes@, ss)
        }
    }
}

#[logic]
#[requires(0 <= n && n <= s.len())]
#[ensures(rest_count_l(s, n) >= 0)]
#[ensures(rest_count_l(s, n) <= n)]
#[variant(n)]
pub fn lemma_rest_count_nonneg(s: Seq<SpecM>, n: Int) {
    pearlite! { if n > 0 { lemma_rest_count_nonneg(s, n - 1) } }
}

/// `rest_count_l` is non-decreasing in the prefix length.
#[logic]
#[requires(0 <= j && j <= k && k <= s.len())]
#[ensures(rest_count_l(s, j) <= rest_count_l(s, k))]
#[variant(k - j)]
pub fn lemma_rest_count_mono(s: Seq<SpecM>, j: Int, k: Int) {
    pearlite! { if j < k { lemma_rest_count_mono(s, j, k - 1) } }
}

#[logic]
#[requires(0 <= n && n <= s.len())]
#[requires(ss > 0 && rest >= 0)]
#[ensures(placed_l(s, n, ss, rest) >= 0)]
#[variant(n)]
pub fn lemma_placed_nonneg(s: Seq<SpecM>, n: Int, ss: Int, rest: Int) {
    pearlite! {
        if n > 0 {
            lemma_placed_nonneg(s, n - 1, ss, rest);
            lemma_cdiv_nonneg(s[n - 1].size_bytes@, ss)
        }
    }
}

/// `placed_l` is non-decreasing in the prefix length, given `rest >= 0`.
#[logic]
#[requires(0 <= j && j <= k && k <= s.len())]
#[requires(ss > 0 && rest >= 0)]
#[ensures(placed_l(s, j, ss, rest) <= placed_l(s, k, ss, rest))]
#[variant(k - j)]
pub fn lemma_placed_mono(s: Seq<SpecM>, j: Int, k: Int, ss: Int, rest: Int) {
    pearlite! {
        if j < k {
            lemma_placed_mono(s, j, k - 1, ss, rest);
            lemma_cdiv_nonneg(s[k - 1].size_bytes@, ss)
        }
    }
}

// ---- stage 1 of `compute_partition_layout`: the two up-front rejections ----

/// `config.partitions.iter().filter(|p| p.size_bytes == 0).count()` — gpt.rs:251-255.
#[ensures(result@ == rest_count_l(specs@, specs@.len()))]
pub fn count_rest(specs: &Vec<SpecM>) -> usize {
    let mut n: usize = 0;
    let mut i: usize = 0;
    #[invariant(i@ <= specs@.len())]
    #[invariant(n@ == rest_count_l(specs@, i@))]
    #[invariant(n@ <= i@)]
    while i < specs.len() {
        proof_assert! { lemma_rest_count_nonneg(specs@, i@ + 1); true };
        if specs[i].size_bytes == 0 {
            n += 1;
        }
        i += 1;
    }
    n
}

/// `config.partitions.iter().filter(|p| p.size_bytes > 0).map(ceil).sum()` — gpt.rs:263-268.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= 18446744073709551615)]
#[ensures(result@ == fixed_sum_l(specs@, specs@.len(), ss@))]
pub fn sum_fixed(specs: &Vec<SpecM>, ss: u32) -> u64 {
    let mut acc: u64 = 0;
    let mut j: usize = 0;
    #[invariant(j@ <= specs@.len())]
    #[invariant(acc@ == fixed_sum_l(specs@, j@, ss@))]
    while j < specs.len() {
        if specs[j].size_bytes > 0 {
            proof_assert! {
                lemma_fixed_sum_mono(specs@, j@ + 1, specs@.len(), ss@);
                fixed_sum_l(specs@, j@ + 1, ss@) == acc@ + cdiv(specs@[j@].size_bytes@, ss@)
            };
            proof_assert! { acc@ + cdiv(specs@[j@].size_bytes@, ss@) <= 18446744073709551615 };
            acc += cdiv64(specs[j].size_bytes, ss as u64);
        }
        j += 1;
    }
    acc
}

/// gpt.rs:251-277 — reject >1 rest-of-disk spec (`MultiRest`), reject an over-subscription
/// (`Capacity`), otherwise return `rest_sectors`.
// J4 (level-2): `total_usable >= 1` dropped (the body never needs it), and the fixed-sum bound
// is needed only when the gpt.rs:257 multi-rest check does NOT return first (the sum is
// gpt.rs:263, after it) - both are WEAKENINGS, so every caller still discharges them.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1
    ==> fixed_sum_l(specs@, specs@.len(), ss@) <= 18446744073709551615)]
#[ensures(match result {
    Err(LayoutErr::MultiRest) => rest_count_l(specs@, specs@.len()) > 1,
    _ => rest_count_l(specs@, specs@.len()) <= 1,
})]
#[ensures(match result {
    Err(LayoutErr::Capacity) => fixed_sum_l(specs@, specs@.len(), ss@) > total_usable@,
    _ => true,
})]
#[ensures(match result {
    Ok(rest) => rest@ == total_usable@ - fixed_sum_l(specs@, specs@.len(), ss@),
    _ => true,
})]
#[ensures(match result { Err(LayoutErr::PerPartition) => false, _ => true })]
pub fn layout_classify(specs: &Vec<SpecM>, ss: u32, total_usable: u64) -> Result<u64, LayoutErr> {
    // gpt.rs:251-260
    if count_rest(specs) > 1 {
        return Err(LayoutErr::MultiRest);
    }
    // gpt.rs:263-275
    let fixed = sum_fixed(specs, ss);
    if fixed > total_usable {
        return Err(LayoutErr::Capacity);
    }
    Ok(total_usable - fixed) // gpt.rs:277
}

// ---- stage 2 of `compute_partition_layout`: the placement loop + the zero padding ----

/// gpt.rs:279-321. Returns `Result` because the source has a third `LayoutError` site inside
/// the loop (gpt.rs:286-290); the contract states that it is UNREACHABLE.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
// DPM-FORMAT-ERR-LAYOUT-UNREACHABLE — the in-loop capacity check can never fire
#[ensures(match result { Err(_) => false, _ => true })]
#[ensures(match result {
    Ok(v) => v@.len() == (if specs@.len() >= 128 { specs@.len() } else { 128 }), _ => true })]
#[ensures(match result {
    Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
        ==> v@[j].start@ == first@ + placed_l(specs@, j, ss@, rest@), _ => true })]
#[ensures(match result {
    Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
        ==> v@[j].end@ + 1 == first@ + placed_l(specs@, j + 1, ss@, rest@), _ => true })]
#[ensures(match result {
    Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
        ==> v@[j].end@ + 1 == v@[j].start@ + demand_l(specs@[j], ss@, rest@), _ => true })]
#[ensures(match result {
    Ok(v) => forall<j: Int> 0 <= j && j < specs@.len() ==> v@[j].start@ >= first@, _ => true })]
#[ensures(match result {
    Ok(v) => forall<j: Int> 0 <= j && j < specs@.len() ==> v@[j].end@ <= last@, _ => true })]
#[ensures(match result {
    Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
        ==> v@[j].tg_zero == specs@[j].tg_zero, _ => true })]
#[ensures(match result {
    Ok(v) => forall<j: Int> 0 <= j && j < specs@.len() ==> v@[j].attrs@ == 0, _ => true })]
#[ensures(match result {
    Ok(v) => forall<j: Int> specs@.len() <= j && j < v@.len()
        ==> v@[j].tg_zero && v@[j].start@ == 0 && v@[j].end@ == 0 && v@[j].attrs@ == 0,
    _ => true })]
pub fn layout_place(
    specs: &Vec<SpecM>,
    ss: u32,
    first: u64,
    last: u64,
    rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    let total_usable: u64 = last - first + 1;
    let mut entries: Vec<EntryM> = Vec::new();
    let mut cur: u64 = first;
    let mut rem: u64 = total_usable;
    let mut i: usize = 0;

    #[invariant(i@ <= specs@.len())]
    #[invariant(entries@.len() == i@)]
    #[invariant(cur@ == first@ + placed_l(specs@, i@, ss@, rest@))]
    #[invariant(rem@ == total_usable@ - placed_l(specs@, i@, ss@, rest@))]
    #[invariant(forall<j: Int> 0 <= j && j < i@
        ==> entries@[j].start@ == first@ + placed_l(specs@, j, ss@, rest@))]
    #[invariant(forall<j: Int> 0 <= j && j < i@
        ==> entries@[j].end@ + 1 == first@ + placed_l(specs@, j + 1, ss@, rest@))]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> entries@[j].tg_zero == specs@[j].tg_zero)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> entries@[j].attrs@ == 0)]
    #[invariant(forall<j: Int> 0 <= j && j < i@
        ==> entries@[j].end@ + 1 == entries@[j].start@ + demand_l(specs@[j], ss@, rest@))]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> entries@[j].start@ >= first@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> entries@[j].end@ <= last@)]
    while i < specs.len() {
        // gpt.rs:280-284
        let num: u64 = if specs[i].size_bytes == 0 {
            rest
        } else {
            cdiv64(specs[i].size_bytes, ss as u64)
        };
        // the whole list fits, so this prefix does: placed(i+1) <= placed(len) <= total_usable.
        // Spelled out step by step: the solver will not case-split `rest * rest_count` on its own.
        proof_assert! {
            lemma_placed_split(specs@, specs@.len(), ss@, rest@);
            lemma_rest_count_nonneg(specs@, specs@.len());
            placed_l(specs@, specs@.len(), ss@, rest@)
                == fixed_sum_l(specs@, specs@.len(), ss@)
                   + rest@ * rest_count_l(specs@, specs@.len())
        };
        proof_assert! {
            rest_count_l(specs@, specs@.len()) == 0 || rest_count_l(specs@, specs@.len()) == 1
        };
        proof_assert! {
            rest_count_l(specs@, specs@.len()) == 0
                ==> placed_l(specs@, specs@.len(), ss@, rest@) <= total_usable@
        };
        proof_assert! {
            rest_count_l(specs@, specs@.len()) == 1
                ==> placed_l(specs@, specs@.len(), ss@, rest@) == total_usable@
        };
        proof_assert! { placed_l(specs@, specs@.len(), ss@, rest@) <= total_usable@ };
        proof_assert! {
            lemma_placed_mono(specs@, i@ + 1, specs@.len(), ss@, rest@);
            placed_l(specs@, i@ + 1, ss@, rest@) <= total_usable@
        };
        // gpt.rs:286-291 — provably unreachable
        if num > rem {
            return Err(LayoutErr::PerPartition);
        }
        let ending = cur + num - 1; // gpt.rs:293
        entries.push(EntryM { tg_zero: specs[i].tg_zero, start: cur, end: ending, attrs: 0 });
        cur = ending + 1; // gpt.rs:305
        rem -= num; // gpt.rs:306
        i += 1;
    }

    // gpt.rs:310-319 — pad to 128 slots with all-zero entries
    #[invariant(entries@.len() >= i@)]
    #[invariant(forall<j: Int> 0 <= j && j < specs@.len()
        ==> entries@[j].start@ == first@ + placed_l(specs@, j, ss@, rest@))]
    #[invariant(forall<j: Int> 0 <= j && j < specs@.len()
        ==> entries@[j].end@ + 1 == first@ + placed_l(specs@, j + 1, ss@, rest@))]
    #[invariant(forall<j: Int> 0 <= j && j < specs@.len()
        ==> entries@[j].tg_zero == specs@[j].tg_zero)]
    #[invariant(forall<j: Int> 0 <= j && j < specs@.len() ==> entries@[j].attrs@ == 0)]
    #[invariant(forall<j: Int> 0 <= j && j < specs@.len()
        ==> entries@[j].end@ + 1 == entries@[j].start@ + demand_l(specs@[j], ss@, rest@))]
    #[invariant(forall<j: Int> 0 <= j && j < specs@.len() ==> entries@[j].start@ >= first@)]
    #[invariant(forall<j: Int> 0 <= j && j < specs@.len() ==> entries@[j].end@ <= last@)]
    #[invariant(forall<j: Int> specs@.len() <= j && j < entries@.len()
        ==> entries@[j].tg_zero && entries@[j].start@ == 0 && entries@[j].end@ == 0
            && entries@[j].attrs@ == 0)]
    #[invariant(entries@.len() >= specs@.len())]
    #[invariant(specs@.len() >= 128 ==> entries@.len() == specs@.len())]
    #[invariant(specs@.len() < 128 ==> entries@.len() <= 128)]
    while entries.len() < 128 {
        entries.push(EntryM { tg_zero: true, start: 0, end: 0, attrs: 0 });
    }
    Ok(entries)
}

/// `compute_partition_layout` end to end — gpt.rs:239-322.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= 18446744073709551615)]
#[ensures(match result {
    Err(LayoutErr::MultiRest) => rest_count_l(specs@, specs@.len()) > 1, _ => true })]
#[ensures(match result {
    Err(LayoutErr::Capacity) => fixed_sum_l(specs@, specs@.len(), ss@) > last@ - first@ + 1,
    _ => true })]
#[ensures(match result { Err(LayoutErr::PerPartition) => false, _ => true })]
#[ensures(rest_count_l(specs@, specs@.len()) <= 1
    && fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1
    ==> match result { Ok(_) => true, _ => false })]
pub fn compute_layout(
    specs: &Vec<SpecM>,
    ss: u32,
    first: u64,
    last: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    let total_usable = last - first + 1; // gpt.rs:245
    let rest = layout_classify(specs, ss, total_usable)?;
    layout_place(specs, ss, first, last, rest)
}

// ===========================================================================
// 6. The result projection — `PartitionInfo` list (gpt.rs:132-144 and 219-231)
//
//    The two sites are character-identical. `enumerate()` runs BEFORE `filter()`, so the
//    `index` a caller receives is the SLOT number in the 128-entry array, not the position of
//    the entry in the produced vector.
// ===========================================================================

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct PInfoM {
    pub index: u32,
    pub start_lba: u64,
    pub num_sectors: u64,
}

/// Number of occupied (`type_guid != [0u8; 16]`) slots among the first `n` entries.
#[logic]
#[requires(0 <= n && n <= s.len())]
#[variant(n)]
pub fn occupied_l(s: Seq<EntryM>, n: Int) -> Int {
    pearlite! { if n == 0 { 0 } else { occupied_l(s, n - 1) + if s[n - 1].tg_zero { 0 } else { 1 } } }
}

#[logic]
#[requires(0 <= n && n <= s.len())]
#[ensures(occupied_l(s, n) >= 0)]
#[ensures(occupied_l(s, n) <= n)]
#[variant(n)]
pub fn lemma_occupied_nonneg(s: Seq<EntryM>, n: Int) {
    pearlite! { if n > 0 { lemma_occupied_nonneg(s, n - 1) } }
}

/// `occupied_l` is monotone over EVERY prefix at once: the quantified form is what a loop
/// invariant needs, since a loop body can only cite a lemma at the single index it is at.
#[logic]
#[requires(0 <= n && n <= s.len())]
#[ensures(forall<j: Int> 0 <= j && j <= n ==> occupied_l(s, j) <= occupied_l(s, n))]
#[ensures(forall<j: Int> 0 <= j && j < n && !s[j].tg_zero
    ==> occupied_l(s, j) < occupied_l(s, n))]
#[variant(n)]
pub fn lemma_occupied_mono(s: Seq<EntryM>, n: Int) {
    pearlite! { if n > 0 { lemma_occupied_mono(s, n - 1) } }
}

// `e.ending_lba - e.starting_lba + 1` (gpt.rs:139, 226) is UNCHECKED, so this mirror can only
// be total where the entry range is non-degenerate. The degenerate case is not an omission: it
// is `refute_dpm_inv_num_sectors_positive`, which models the same expression with the release
// build's wrapping semantics.
// J4 (level-2): the two entry-range premises are stated over the OCCUPIED slots only
// (`type_guid != [0u8; 16]`), exactly as D-RANGE-KEYENTITY:PARTITIONINFO-6425ed; the
// `+ 1` arithmetic (gpt.rs:139, 226) only ever runs for an occupied slot (filter-before-map).
#[requires(entries@.len() <= 4294967295)]
#[requires(forall<j: Int> 0 <= j && j < entries@.len() && !entries@[j].tg_zero
    ==> entries@[j].end@ >= entries@[j].start@)]
#[requires(forall<j: Int> 0 <= j && j < entries@.len() && !entries@[j].tg_zero
    ==> entries@[j].end@ - entries@[j].start@ < 18446744073709551615)]
#[ensures(result@.len() == occupied_l(entries@, entries@.len()))]
// the j-th occupied slot lands at output position occupied_l(.., j) and carries the SLOT number
#[ensures(forall<j: Int> 0 <= j && j < entries@.len() && !entries@[j].tg_zero
    ==> result@[occupied_l(entries@, j)].index@ == j)]
#[ensures(forall<j: Int> 0 <= j && j < entries@.len() && !entries@[j].tg_zero
    ==> result@[occupied_l(entries@, j)].start_lba@ == entries@[j].start@)]
#[ensures(forall<j: Int> 0 <= j && j < entries@.len() && !entries@[j].tg_zero
    ==> result@[occupied_l(entries@, j)].num_sectors@
        == entries@[j].end@ - entries@[j].start@ + 1)]
pub fn project(entries: &Vec<EntryM>) -> Vec<PInfoM> {
    let mut out: Vec<PInfoM> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= entries@.len())]
    #[invariant(out@.len() == occupied_l(entries@, i@))]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && !entries@[j].tg_zero
        ==> out@[occupied_l(entries@, j)].index@ == j)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && !entries@[j].tg_zero
        ==> out@[occupied_l(entries@, j)].start_lba@ == entries@[j].start@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && !entries@[j].tg_zero
        ==> out@[occupied_l(entries@, j)].num_sectors@
            == entries@[j].end@ - entries@[j].start@ + 1)]
    #[invariant(forall<j: Int> 0 <= j && j <= i@ ==> occupied_l(entries@, j) <= occupied_l(entries@, i@))]
    #[invariant(forall<j: Int> 0 <= j && j < i@ && !entries@[j].tg_zero
        ==> occupied_l(entries@, j) < occupied_l(entries@, i@))]
    while i < entries.len() {
        proof_assert! { lemma_occupied_mono(entries@, i@ + 1); lemma_occupied_nonneg(entries@, i@ + 1); true };
        if !entries[i].tg_zero {
            out.push(PInfoM {
                index: i as u32,
                start_lba: entries[i].start,
                num_sectors: entries[i].end - entries[i].start + 1,
            });
        }
        i += 1;
    }
    out
}

// ===========================================================================
// 7. The on-device layout a format produces, and the one a read assumes
//    (`write_gpt` gpt.rs:152-237, `read_gpt` gpt.rs:66-96)
// ===========================================================================

/// `write_gpt`: `first_usable_lba = 2 + entry_sectors` — gpt.rs:157.
#[logic(open)]
pub fn first_usable_l(ss: Int) -> Int {
    pearlite! { 2 + entry_sectors_l(ss) }
}

/// `write_gpt`: `last_usable_lba = num_sectors - 1 - entry_sectors - 1` — gpt.rs:158.
#[logic(open)]
pub fn last_usable_l(ss: Int, ns: Int) -> Int {
    pearlite! { ns - 1 - entry_sectors_l(ss) - 1 }
}

/// `write_gpt`: the backup entry array goes at `last_usable_lba + 1` — gpt.rs:194.
#[logic(open)]
pub fn backup_entry_lba_write_l(ss: Int, ns: Int) -> Int {
    pearlite! { last_usable_l(ss, ns) + 1 }
}

/// `read_gpt`: the backup entry array is looked for at `(num_sectors - 1) - entry_sectors`
/// — gpt.rs:87-89.
#[logic(open)]
pub fn backup_entry_lba_read_l(ss: Int, ns: Int) -> Int {
    pearlite! { ns - 1 - entry_sectors_l(ss) }
}

/// Executable `first_usable_lba` / `last_usable_lba` with the source's unchecked subtractions
/// (gpt.rs:157-158). The `#[requires]` states exactly the geometry the code assumes but never
/// checks — DPM-FORMAT-PRE-GEOMETRY-MIN.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= entry_sectors_l(ss@) + 2)]
#[ensures(result.0@ == first_usable_l(ss@))]
#[ensures(result.1@ == last_usable_l(ss@, ns@))]
pub fn usable_window(ss: u32, ns: u64) -> (u64, u64) {
    let es = entry_sectors(ss) as u64;
    let first = 2 + es; // gpt.rs:157
    let last = ns - 1 - es - 1; // gpt.rs:158
    (first, last)
}

// ---- the per-sector device accesses ----

/// `write_bytes(lba, data)` — gpt.rs:501-553. `num_blocks = data.len().div_ceil(sector_size)`,
/// and the loop issues ONE `Command::WriteSync { ns_id, lba: lba + i, .. }` per block.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(len@ >= 1)]
#[ensures(result.is_write)]
#[ensures(result.ns == ns)]
#[ensures(result.lba == lba)]
#[ensures(result.sectors@ == cdiv(len@, ss@))]
pub fn write_bytes_acc(ns: u32, ss: u32, lba: u64, len: u64) -> Acc {
    Acc { is_write: true, ns, lba, sectors: cdiv64(len, ss as u64) }
}

/// `read_bytes(lba, num_bytes)` — gpt.rs:451-495.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(len@ >= 1)]
#[ensures(!result.is_write)]
#[ensures(result.ns == ns)]
#[ensures(result.lba == lba)]
#[ensures(result.sectors@ == cdiv(len@, ss@))]
pub fn read_bytes_acc(ns: u32, ss: u32, lba: u64, len: u64) -> Acc {
    Acc { is_write: false, ns, lba, sectors: cdiv64(len, ss as u64) }
}

/// The complete device trace of a successful `write_gpt` — gpt.rs:204-216, in source order:
/// protective MBR at LBA 0, primary header at LBA 1, primary entry array from LBA 2, backup
/// entry array at `last_usable + 1`, backup header at the last LBA.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns_sectors@ >= 2 * entry_sectors_l(ss@) + 4)]
#[ensures(result@.len() == 5)]
#[ensures(result@[0].is_write && result@[0].lba@ == 0 && result@[0].sectors@ == 1)]
#[ensures(result@[1].is_write && result@[1].lba@ == 1 && result@[1].sectors@ == 1)]
#[ensures(result@[2].is_write && result@[2].lba@ == 2
    && result@[2].sectors@ == entry_sectors_l(ss@))]
#[ensures(result@[3].is_write && result@[3].lba@ == backup_entry_lba_write_l(ss@, ns_sectors@)
    && result@[3].sectors@ == entry_sectors_l(ss@))]
#[ensures(result@[4].is_write && result@[4].lba@ == ns_sectors@ - 1 && result@[4].sectors@ == 1)]
#[ensures(all_ns(result@, ns@))]
pub fn write_gpt_trace(ns: u32, ss: u32, ns_sectors: u64) -> Vec<Acc> {
    let (_first, last) = usable_window(ss, ns_sectors);
    let mut t: Vec<Acc> = Vec::new();
    t.push(write_bytes_acc(ns, ss, 0, ss as u64)); // gpt.rs:204 write_protective_mbr
    t.push(write_bytes_acc(ns, ss, 1, ss as u64)); // gpt.rs:207 primary header
    t.push(write_bytes_acc(ns, ss, 2, ENTRY_ARRAY_BYTES as u64)); // gpt.rs:210 primary entries
    t.push(write_bytes_acc(ns, ss, last + 1, ENTRY_ARRAY_BYTES as u64)); // gpt.rs:213 backup entries
    t.push(write_bytes_acc(ns, ss, ns_sectors - 1, ss as u64)); // gpt.rs:216 backup header
    t
}

/// The device trace of ONE `try_read_gpt_at(header_lba, entry_lba)` attempt that got as far as
/// reading the entry array — gpt.rs:103 (`read_sector(header_lba)`) then gpt.rs:120
/// (`read_bytes(entry_lba, num_partition_entries * partition_entry_size)`). `decl_entries` and
/// `decl_entry_size` come from the ON-DISK header, which is why the entry-array read length is
/// attacker-controlled (DPM-READ-STRIDE-MISMATCH).
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(decl_entries@ * decl_entry_size@ >= 1)]
#[requires(decl_entries@ * decl_entry_size@ <= 18446744073709551615)]
#[ensures(result@.len() == 2)]
#[ensures(!result@[0].is_write && result@[0].lba@ == header_lba@ && result@[0].sectors@ == 1)]
#[ensures(!result@[1].is_write && result@[1].lba@ == entry_lba@)]
#[ensures(result@[1].sectors@ == cdiv(decl_entries@ * decl_entry_size@, ss@))]
#[ensures(no_writes(result@))]
#[ensures(all_ns(result@, ns@))]
pub fn try_read_gpt_trace(
    ns: u32,
    ss: u32,
    header_lba: u64,
    entry_lba: u64,
    decl_entries: u32,
    decl_entry_size: u32,
) -> Vec<Acc> {
    let mut t: Vec<Acc> = Vec::new();
    t.push(read_bytes_acc(ns, ss, header_lba, ss as u64)); // gpt.rs:103 read_sector
    let entry_bytes = decl_entries as u64 * decl_entry_size as u64; // gpt.rs:118-119
    t.push(read_bytes_acc(ns, ss, entry_lba, entry_bytes)); // gpt.rs:120
    t
}

// ===========================================================================
// 8. The protective MBR — `write_protective_mbr` (gpt.rs:329-355)
//
//    `let mut mbr = vec![0u8; self.sector_size as usize]` and then STORES AT FIXED OFFSETS
//    446..462, 510, 511. Modelled as a real `Vec<u8>` so every store is a genuine bound-check
//    VC: for `sector_size < 512` the first store is already out of range.
// ===========================================================================

#[logic(open)]
pub fn le32_at(s: Seq<u8>, at: Int) -> Int {
    pearlite! { s[at]@ + 256 * s[at + 1]@ + 65536 * s[at + 2]@ + 16777216 * s[at + 3]@ }
}

#[requires(ss@ >= 512)]
// gpt.rs:347 `self.num_sectors - 1` is unchecked: a zero-sector device underflows here too.
#[requires(ns@ >= 1)]
#[ensures(result@.len() == ss@)]
#[ensures(result@[446]@ == 0)]
#[ensures(result@[450]@ == 238)]
#[ensures(le32_at(result@, 454) == 1)]
#[ensures(le32_at(result@, 458) == (if ns@ - 1 <= 4294967295 { ns@ - 1 } else { 4294967295 }))]
#[ensures(result@[510]@ == 85 && result@[511]@ == 170)]
#[ensures(forall<i: Int> 0 <= i && i < 446 ==> result@[i]@ == 0)]
#[ensures(forall<i: Int> 462 <= i && i < 510 ==> result@[i]@ == 0)]
#[ensures(forall<i: Int> 512 <= i && i < ss@ ==> result@[i]@ == 0)]
pub fn protective_mbr(ss: u32, ns: u64) -> Vec<u8> {
    let mut mbr: Vec<u8> = vec![0u8; ss as usize]; // gpt.rs:330
    mbr[446] = 0x00; // not bootable           gpt.rs:334
    mbr[447] = 0x00; // CHS of first sector    gpt.rs:336-338
    mbr[448] = 0x02;
    mbr[449] = 0x00;
    mbr[450] = 0xEE; // GPT protective type    gpt.rs:339
    mbr[451] = 0xFF; // CHS of last sector     gpt.rs:341-343
    mbr[452] = 0xFF;
    mbr[453] = 0xFF;
    mbr[454] = 0x01; // starting LBA = 1u32.to_le_bytes()   gpt.rs:345
    mbr[455] = 0x00;
    mbr[456] = 0x00;
    mbr[457] = 0x00;
    // gpt.rs:347 — `(self.num_sectors - 1).min(u32::MAX as u64) as u32`
    let size: u32 = if ns - 1 <= 4294967295u64 { (ns - 1) as u32 } else { 4294967295u32 };
    mbr[458] = (size % 256) as u8; // gpt.rs:348 size.to_le_bytes()
    mbr[459] = (size / 256 % 256) as u8;
    mbr[460] = (size / 65536 % 256) as u8;
    mbr[461] = (size / 16777216) as u8;
    mbr[510] = 0x55; // boot signature         gpt.rs:351-352
    mbr[511] = 0xAA;
    mbr
}

// ===========================================================================
// 9. Header CRC self-consistency — `serialize_header_with_crc` (gpt.rs:407-430) against
//    `try_read_gpt_at`'s recomputation (gpt.rs:107-115)
//
//    `crc32fast::hash` is a pure function of the byte sequence that Creusot has no model for,
//    and none is needed: every obligation here is about WHICH BYTES each side hashes. It is
//    therefore universally quantified as a `Mapping<Seq<u8>, u32>` — the proofs below hold for
//    EVERY hash function, which is strictly stronger than naming CRC-32 specifically and
//    cannot be satisfied vacuously by a degenerate choice.
// ===========================================================================

/// `buf[at..at+4].copy_from_slice(&x.to_le_bytes())`.
#[logic(open)]
pub fn set4(s: Seq<u8>, at: Int, b0: u8, b1: u8, b2: u8, b3: u8) -> Seq<u8> {
    pearlite! { s.set(at, b0).set(at + 1, b1).set(at + 2, b2).set(at + 3, b3) }
}

/// Zeroing the CRC field of an emitted header recovers exactly the pre-CRC bytes the writer
/// hashed. `serialize_header_with_crc` zeroes `[16..20]` (gpt.rs:413) and `[20..24]`
/// (gpt.rs:414) before hashing `buf[..92]` (gpt.rs:426) and only then stores the CRC there
/// (gpt.rs:427); `try_read_gpt_at` takes `header_data[..92]` and zeroes `[16..20]` (gpt.rs:108)
/// before hashing. So the two byte sequences coincide.
#[logic]
#[requires(pre.len() >= 92)]
#[requires(pre[16]@ == 0 && pre[17]@ == 0 && pre[18]@ == 0 && pre[19]@ == 0)]
#[ensures(set4(set4(pre, 16, b0, b1, b2, b3), 16, 0u8, 0u8, 0u8, 0u8).ext_eq(pre))]
pub fn lemma_crc_field_roundtrip(pre: Seq<u8>, b0: u8, b1: u8, b2: u8, b3: u8) {}

// ===========================================================================
// 10. Partition names — `encode_utf16le_name` / `decode_utf16le_name` (gpt.rs:576-595)
//
//     `String::encode_utf16` and `String::from_utf16_lossy` are the trusted boundary: Creusot
//     models `str`/`String` as an OPAQUE `Seq<char>` (creusot-std/src/std/string.rs:5-41), so
//     the TRANSCODING step has no model. Everything the component itself does — take 36 code
//     units, store each little-endian inside the 72-byte field, read 36 back and stop at the
//     first zero — is over a `Seq<u16>` of code units and is proved natively below.
// ===========================================================================

/// `encode_utf16le_name` — gpt.rs:576-587.
#[ensures(forall<k: Int> 0 <= k && k < 36 && k < units@.len()
    ==> result[2 * k]@ == units@[k]@ % 256 && result[2 * k + 1]@ == units@[k]@ / 256)]
#[ensures(forall<k: Int> units@.len() <= k && k < 36
    ==> result[2 * k]@ == 0 && result[2 * k + 1]@ == 0)]
pub fn encode_units(units: &Vec<u16>) -> [u8; 72] {
    let mut buf = [0u8; 72]; // gpt.rs:577
    let n = if units.len() < 36 { units.len() } else { 36 }; // `.take(36)` gpt.rs:578
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    #[invariant(forall<k: Int> 0 <= k && k < i@
        ==> buf[2 * k]@ == units@[k]@ % 256 && buf[2 * k + 1]@ == units@[k]@ / 256)]
    #[invariant(forall<k: Int> i@ <= k && k < 36 ==> buf[2 * k]@ == 0 && buf[2 * k + 1]@ == 0)]
    while i < n {
        let ch = units[i];
        buf[2 * i] = (ch % 256) as u8; // `ch.to_le_bytes()` gpt.rs:584
        buf[2 * i + 1] = (ch / 256) as u8;
        i += 1;
    }
    buf
}

/// `decode_utf16le_name` — gpt.rs:589-595: 36 little-endian u16 out of the 72 bytes,
/// `take_while(|&c| c != 0)`.
#[ensures(result@.len() <= 36)]
#[ensures(forall<k: Int> 0 <= k && k < result@.len()
    ==> result@[k]@ == data[2 * k]@ + 256 * data[2 * k + 1]@)]
#[ensures(forall<k: Int> 0 <= k && k < result@.len() ==> result@[k]@ != 0)]
#[ensures(result@.len() < 36
    ==> data[2 * result@.len()]@ + 256 * data[2 * result@.len() + 1]@ == 0)]
#[ensures(forall<m: Int> 0 <= m && m < 36
    && (forall<k: Int> 0 <= k && k < m ==> data[2 * k]@ + 256 * data[2 * k + 1]@ != 0)
    && data[2 * m]@ + 256 * data[2 * m + 1]@ == 0 ==> result@.len() == m)]
#[ensures((forall<k: Int> 0 <= k && k < 36 ==> data[2 * k]@ + 256 * data[2 * k + 1]@ != 0)
    ==> result@.len() == 36)]
pub fn decode_units(data: &[u8; 72]) -> Vec<u16> {
    let mut out: Vec<u16> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= 36)]
    #[invariant(out@.len() == i@)]
    #[invariant(forall<k: Int> 0 <= k && k < i@
        ==> out@[k]@ == data[2 * k]@ + 256 * data[2 * k + 1]@)]
    #[invariant(forall<k: Int> 0 <= k && k < i@ ==> out@[k]@ != 0)]
    while i < 36 {
        let c: u16 = data[2 * i] as u16 + 256u16 * data[2 * i + 1] as u16;
        if c == 0 {
            return out;
        }
        out.push(c);
        i += 1;
    }
    out
}

// ===========================================================================
// 11. Component state — `Mutex<Option<PartitionTable>>` + `Mutex<Option<u32>>` (lib.rs:20-23)
//
//     The `Mutex` is a trusted boundary (see `lock_outcome`); the STATE it guards is modelled
//     exactly: `None` = "no table loaded", `Some(t)` = "holding table t". `TabM.tag` is a ghost
//     identity that distinguishes two tables with the same shape, so "hands the caller an
//     IDENTICAL copy" and "the remembered table is unchanged" are statable.
// ===========================================================================

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct TabM {
    /// ghost identity of this particular table value
    pub tag: u64,
    pub count: u32,
    pub sector_size: u32,
}

/// `interfaces::PartitionTableError` — ipartition_table.rs:58-73. The `String` payload is
/// dropped: its CONTENT is not expressible (opaque `Seq<char>`), and no obligation here turns
/// on anything but the VARIANT plus, where the message quotes a number, that number.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum PtErr {
    NoPartitionTable,
    CorruptTable,
    InvalidPartition,
    IoError,
    LayoutError,
    NotInitialized,
}

/// What a `*.lock().unwrap()` can do. `Mutex::lock` returns `Err(PoisonError)` exactly when a
/// thread panicked while holding the lock, and `unwrap()` on that panics — so `poisoned` is a
/// faithful, CONTRACT-CONFORMING model of std's behaviour. Reaching a poisoned state needs a
/// concurrent panic, which is a thread-model obligation, not a Creusot one.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Outcome {
    Ok(TabM),
    Err(PtErr),
    /// the calling thread is aborted — NOT one of the six declared errors
    Panic,
}

/// What one `try_read_gpt_at` attempt produced (gpt.rs:98-150).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Attempt {
    Ok(TabM),
    /// `CorruptTable` — header shorter than 92 bytes, header CRC mismatch, or entry-array CRC
    /// mismatch (gpt.rs:359, 111, 125)
    Corrupt,
    /// `NoPartitionTable` — the signature is not "EFI PART" (gpt.rs:364)
    NoTable,
    /// `IoError` — a sector read failed (gpt.rs:103, 120)
    Io,
}

/// What `read_gpt` produced (gpt.rs:66-96).
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum ReadRes {
    Ok(TabM),
    Err(PtErr),
}

/// A trusted-boundary device outcome.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum IoRes {
    Ok,
    Fail,
}

/// Logical twin of `try_read_gpt_at_m`, so a driver can state that the outcome depends on these
/// six inputs and on NOTHING else (which is how "my_lba is never checked" is expressed).
#[logic(open)]
pub fn try_read_gpt_at_logic(
    hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool, hdr_crc_match: bool, entry_read: IoRes,
    entry_crc_match: bool, tab: TabM,
) -> Attempt {
    pearlite! {
        match hdr_read {
            IoRes::Fail => Attempt::Io,
            IoRes::Ok =>
                if !hdr_len_ok { Attempt::Corrupt }
                else if !sig_ok { Attempt::NoTable }
                else if !hdr_crc_match { Attempt::Corrupt }
                else {
                    match entry_read {
                        IoRes::Fail => Attempt::Io,
                        IoRes::Ok => if !entry_crc_match { Attempt::Corrupt }
                                     else { Attempt::Ok(tab) },
                    }
                },
        }
    }
}

/// `try_read_gpt_at` — gpt.rs:98-150, plus the `parse_header` checks it delegates to
/// (gpt.rs:357-368). The six boolean/`IoRes` inputs are exactly the trusted boundaries:
/// two device reads, the 92-byte length check, the signature compare, and the two CRC compares.
#[ensures(match hdr_read { IoRes::Fail => result == Attempt::Io, _ => true })]
#[ensures(match hdr_read { IoRes::Ok =>
    (!hdr_len_ok ==> result == Attempt::Corrupt), _ => true })]
#[ensures(match hdr_read { IoRes::Ok =>
    (hdr_len_ok && !sig_ok ==> result == Attempt::NoTable), _ => true })]
#[ensures(match hdr_read { IoRes::Ok =>
    (hdr_len_ok && sig_ok && !hdr_crc_match ==> result == Attempt::Corrupt), _ => true })]
#[ensures(match entry_read { IoRes::Fail =>
    (match hdr_read { IoRes::Ok => hdr_len_ok && sig_ok && hdr_crc_match ==> result == Attempt::Io,
                      _ => true }), _ => true })]
#[ensures(match hdr_read { IoRes::Ok => (match entry_read { IoRes::Ok =>
    (hdr_len_ok && sig_ok && hdr_crc_match && !entry_crc_match ==> result == Attempt::Corrupt),
    _ => true }), _ => true })]
// the outcome is a FUNCTION of these six inputs: needed wherever a driver compares two calls
#[ensures(result == try_read_gpt_at_logic(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match,
                                          entry_read, entry_crc_match, tab))]
#[ensures(match result { Attempt::Ok(t) =>
    hdr_len_ok && sig_ok && hdr_crc_match && entry_crc_match && t.tag == tab.tag, _ => true })]
pub fn try_read_gpt_at_m(
    hdr_read: IoRes,
    hdr_len_ok: bool,
    sig_ok: bool,
    hdr_crc_match: bool,
    entry_read: IoRes,
    entry_crc_match: bool,
    tab: TabM,
) -> Attempt {
    match hdr_read {
        IoRes::Fail => return Attempt::Io, // gpt.rs:103 `self.read_sector(header_lba)?`
        IoRes::Ok => {}
    }
    if !hdr_len_ok {
        return Attempt::Corrupt; // gpt.rs:358-360 "header too short"
    }
    if !sig_ok {
        return Attempt::NoTable; // gpt.rs:363-368 "invalid GPT signature"
    }
    if !hdr_crc_match {
        return Attempt::Corrupt; // gpt.rs:110-115 "header CRC mismatch"
    }
    match entry_read {
        IoRes::Fail => return Attempt::Io, // gpt.rs:120 `self.read_bytes(entry_lba, ..)?`
        IoRes::Ok => {}
    }
    if !entry_crc_match {
        return Attempt::Corrupt; // gpt.rs:124-129 "partition entry CRC mismatch"
    }
    Attempt::Ok(tab) // gpt.rs:146-149
}

/// `read_gpt` — gpt.rs:66-96. Primary at LBA 1 first; the backup is consulted on EXACTLY
/// `CorruptTable | NoPartitionTable`; every other primary error propagates unchanged
/// (gpt.rs:83). The backup attempt's error is then `map_err`-ed to `NoPartitionTable`
/// UNCONDITIONALLY (gpt.rs:91-95) — including an `IoError`.
#[ensures(match primary { Attempt::Ok(t) => result == ReadRes::Ok(t), _ => true })]
#[ensures(match primary { Attempt::Io => result == ReadRes::Err(PtErr::IoError), _ => true })]
#[ensures(match primary { Attempt::Corrupt => (match backup {
    Attempt::Ok(t) => result == ReadRes::Ok(t),
    _ => result == ReadRes::Err(PtErr::NoPartitionTable) }), _ => true })]
#[ensures(match primary { Attempt::NoTable => (match backup {
    Attempt::Ok(t) => result == ReadRes::Ok(t),
    _ => result == ReadRes::Err(PtErr::NoPartitionTable) }), _ => true })]
pub fn read_gpt_m(primary: Attempt, backup: Attempt) -> ReadRes {
    match primary {
        Attempt::Ok(t) => return ReadRes::Ok(t), // gpt.rs:68-69
        Attempt::Corrupt => {}                   // gpt.rs:79-82 fall through to the backup
        Attempt::NoTable => {}                   // gpt.rs:80-82 fall through to the backup
        Attempt::Io => return ReadRes::Err(PtErr::IoError), // gpt.rs:83 `Err(e) => return Err(e)`
    }
    match backup {
        Attempt::Ok(t) => ReadRes::Ok(t),
        // gpt.rs:91-95 `.map_err(|_| NoPartitionTable(..))` — swallows the backup's OWN error kind
        Attempt::Corrupt => ReadRes::Err(PtErr::NoPartitionTable),
        Attempt::NoTable => ReadRes::Err(PtErr::NoPartitionTable),
        Attempt::Io => ReadRes::Err(PtErr::NoPartitionTable),
    }
}

// ---- the four `IPartitionTable` methods, over the mutable component state ----

/// `IPartitionTable::initialize` — lib.rs:68-84. `bound` = the `block_device` receptacle is
/// connected (lib.rs:69); `geom_ok` = `sector_size`/`num_sectors` both answered (lib.rs:73-78);
/// `chan_ok` = `connect_client` succeeded (gpt.rs:55); `rd` = what `read_gpt` produced;
/// `poisoned` = the state `Mutex` is poisoned (lib.rs:82). Returns the outcome AND the state
/// afterwards, so the frame properties over the cache are statable.
#[ensures(!bound ==> result.0 == Outcome::Err(PtErr::NotInitialized))]
#[ensures(!bound ==> result.1 == st)]
#[ensures(bound && !geom_ok ==> result.0 == Outcome::Err(PtErr::IoError) && result.1 == st)]
#[ensures(bound && geom_ok && !chan_ok
    ==> result.0 == Outcome::Err(PtErr::IoError) && result.1 == st)]
#[ensures(match rd { ReadRes::Err(e) =>
    (bound && geom_ok && chan_ok ==> result.0 == Outcome::Err(e) && result.1 == st), _ => true })]
#[ensures(match rd { ReadRes::Ok(t) =>
    (bound && geom_ok && chan_ok && poisoned ==> result.0 == Outcome::Panic && result.1 == st),
    _ => true })]
#[ensures(match rd { ReadRes::Ok(t) =>
    (bound && geom_ok && chan_ok && !poisoned
        ==> result.0 == Outcome::Ok(t) && result.1 == Some(t)), _ => true })]
pub fn initialize_m(
    st: Option<TabM>,
    bound: bool,
    geom_ok: bool,
    chan_ok: bool,
    rd: ReadRes,
    poisoned: bool,
) -> (Outcome, Option<TabM>) {
    if !bound {
        return (Outcome::Err(PtErr::NotInitialized), st); // lib.rs:69-71
    }
    if !geom_ok {
        return (Outcome::Err(PtErr::IoError), st); // lib.rs:73-78
    }
    if !chan_ok {
        return (Outcome::Err(PtErr::IoError), st); // lib.rs:80 -> gpt.rs:55-57
    }
    match rd {
        ReadRes::Err(e) => (Outcome::Err(e), st), // lib.rs:81 `?`
        ReadRes::Ok(t) => {
            if poisoned {
                (Outcome::Panic, st) // lib.rs:82 `.lock().unwrap()` on a poisoned Mutex
            } else {
                (Outcome::Ok(t), Some(t)) // lib.rs:82-83
            }
        }
    }
}

/// `IPartitionTable::format` — lib.rs:86-95. Note what is ABSENT: the device is never asked for
/// its geometry; `config.sector_size`, `config.total_sectors` and `config.ns_id` are used as
/// given (lib.rs:91).
#[ensures(!bound ==> result.0 == Outcome::Err(PtErr::NotInitialized) && result.1 == st)]
#[ensures(bound && !chan_ok ==> result.0 == Outcome::Err(PtErr::IoError) && result.1 == st)]
#[ensures(match wr { ReadRes::Err(e) =>
    (bound && chan_ok ==> result.0 == Outcome::Err(e) && result.1 == st), _ => true })]
#[ensures(match wr { ReadRes::Ok(t) =>
    (bound && chan_ok && poisoned ==> result.0 == Outcome::Panic && result.1 == st), _ => true })]
#[ensures(match wr { ReadRes::Ok(t) =>
    (bound && chan_ok && !poisoned ==> result.0 == Outcome::Ok(t) && result.1 == Some(t)),
    _ => true })]
pub fn format_m(
    st: Option<TabM>,
    bound: bool,
    chan_ok: bool,
    wr: ReadRes,
    poisoned: bool,
) -> (Outcome, Option<TabM>) {
    if !bound {
        return (Outcome::Err(PtErr::NotInitialized), st); // lib.rs:87-89
    }
    if !chan_ok {
        return (Outcome::Err(PtErr::IoError), st); // lib.rs:91 -> gpt.rs:55-57
    }
    match wr {
        ReadRes::Err(e) => (Outcome::Err(e), st), // lib.rs:92 `?`
        ReadRes::Ok(t) => {
            if poisoned {
                (Outcome::Panic, st) // lib.rs:93 `.lock().unwrap()`
            } else {
                (Outcome::Ok(t), Some(t)) // lib.rs:93-94
            }
        }
    }
}

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum PiOutcome {
    Ok(PInfoM),
    Err(PtErr),
    Panic,
}

/// `IPartitionTable::partition_info` — lib.rs:97-111.
#[ensures(poisoned ==> result == PiOutcome::Panic)]
#[ensures(!poisoned && !loaded ==> result == PiOutcome::Err(PtErr::NotInitialized))]
#[ensures(!poisoned && loaded && idx@ >= parts@.len()
    ==> result == PiOutcome::Err(PtErr::InvalidPartition))]
#[ensures(!poisoned && loaded && idx@ < parts@.len()
    ==> result == PiOutcome::Ok(parts@[idx@]))]
pub fn partition_info_m(
    loaded: bool,
    parts: &Vec<PInfoM>,
    poisoned: bool,
    idx: u32,
) -> PiOutcome {
    if poisoned {
        return PiOutcome::Panic; // lib.rs:98 `.lock().unwrap()`
    }
    if !loaded {
        return PiOutcome::Err(PtErr::NotInitialized); // lib.rs:99-101
    }
    if idx as usize >= parts.len() {
        return PiOutcome::Err(PtErr::InvalidPartition); // lib.rs:104-110 `.get(..).ok_or_else`
    }
    PiOutcome::Ok(parts[idx as usize]) // lib.rs:102-105 `.get(index).cloned()`
}

#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum NpOutcome {
    Ok(u32),
    Err(PtErr),
    Panic,
}

/// `IPartitionTable::num_partitions` — lib.rs:113-119.
#[requires(parts@.len() <= 4294967295)]
#[ensures(poisoned ==> result == NpOutcome::Panic)]
#[ensures(!poisoned && !loaded ==> result == NpOutcome::Err(PtErr::NotInitialized))]
#[ensures(!poisoned && loaded ==> exists<c: u32> result == NpOutcome::Ok(c) && c@ == parts@.len())]
pub fn num_partitions_m(loaded: bool, parts: &Vec<PInfoM>, poisoned: bool) -> NpOutcome {
    if poisoned {
        return NpOutcome::Panic; // lib.rs:114 `.lock().unwrap()`
    }
    if !loaded {
        return NpOutcome::Err(PtErr::NotInitialized); // lib.rs:115-117
    }
    NpOutcome::Ok(parts.len() as u32) // lib.rs:118
}

/// `DiskPartitionManager::initialize_or_format` — lib.rs:45-64.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum IofOutcome {
    Ok(TabM, bool),
    Err(PtErr),
}

#[ensures(force ==> (match fmt {
    ReadRes::Ok(t) => result == IofOutcome::Ok(t, true),
    ReadRes::Err(e) => result == IofOutcome::Err(e) }))]
#[ensures(!force ==> (match init {
    ReadRes::Ok(t) => result == IofOutcome::Ok(t, false), _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::NoPartitionTable) => (match fmt {
    ReadRes::Ok(t) => result == IofOutcome::Ok(t, true),
    ReadRes::Err(e) => result == IofOutcome::Err(e) }), _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::CorruptTable) => (match fmt {
    ReadRes::Ok(t) => result == IofOutcome::Ok(t, true),
    ReadRes::Err(e) => result == IofOutcome::Err(e) }), _ => true }))]
#[ensures(!force ==> (match init {
    ReadRes::Err(PtErr::IoError) => result == IofOutcome::Err(PtErr::IoError), _ => true }))]
#[ensures(!force ==> (match init {
    ReadRes::Err(PtErr::NotInitialized) => result == IofOutcome::Err(PtErr::NotInitialized),
    _ => true }))]
#[ensures(!force ==> (match init {
    ReadRes::Err(PtErr::LayoutError) => result == IofOutcome::Err(PtErr::LayoutError),
    _ => true }))]
#[ensures(!force ==> (match init {
    ReadRes::Err(PtErr::InvalidPartition) => result == IofOutcome::Err(PtErr::InvalidPartition),
    _ => true }))]
pub fn initialize_or_format_m(force: bool, init: ReadRes, fmt: ReadRes) -> IofOutcome {
    if force {
        // lib.rs:50-53 — formats WITHOUT reading first
        return match fmt {
            ReadRes::Ok(t) => IofOutcome::Ok(t, true),
            ReadRes::Err(e) => IofOutcome::Err(e),
        };
    }
    match init {
        ReadRes::Ok(t) => IofOutcome::Ok(t, false), // lib.rs:55-56
        // lib.rs:57-61 — ONLY these two kinds trigger a reformat
        ReadRes::Err(PtErr::NoPartitionTable) | ReadRes::Err(PtErr::CorruptTable) => match fmt {
            ReadRes::Ok(t) => IofOutcome::Ok(t, true),
            ReadRes::Err(e) => IofOutcome::Err(e),
        },
        ReadRes::Err(e) => IofOutcome::Err(e), // lib.rs:62
    }
}

// ---- the namespace setting — lib.rs:30-36 ----

/// `get_ns_id` — lib.rs:34-36: `self.ns_id.lock().unwrap().unwrap_or(1)`.
#[ensures(match st { None => result@ == 1, Some(v) => result == v })]
pub fn get_ns_id_m(st: Option<u32>) -> u32 {
    match st {
        None => 1,
        Some(v) => v,
    }
}

/// `set_ns_id` — lib.rs:30-32: `*self.ns_id.lock().unwrap() = Some(ns_id)`. No validation of
/// any kind, and the state table is untouched.
#[ensures(result == Some(v))]
pub fn set_ns_id_m(_prev: Option<u32>, v: u32) -> Option<u32> {
    Some(v)
}

// ===========================================================================
// 12. Whether an operation ever reaches the device
// ===========================================================================

/// `initialize` reaches its first `Command::ReadSync` only after the receptacle lookup
/// (lib.rs:69), both geometry queries (lib.rs:73-78) and `connect_client` (gpt.rs:55) have all
/// succeeded — each of them precedes any sector read.
#[logic(open)]
pub fn init_reads_l(bound: bool, geom_ok: bool, chan_ok: bool) -> bool {
    pearlite! { bound && geom_ok && chan_ok }
}

#[ensures(result == init_reads_l(bound, geom_ok, chan_ok))]
pub fn init_reads_sectors(bound: bool, geom_ok: bool, chan_ok: bool) -> bool {
    if !bound {
        return false; // lib.rs:69-71
    }
    if !geom_ok {
        return false; // lib.rs:73-78
    }
    if !chan_ok {
        return false; // lib.rs:91 / gpt.rs:55-57
    }
    true
}

/// `format` reaches its first `Command::WriteSync` (gpt.rs:204 `write_protective_mbr`) only
/// after the receptacle lookup (lib.rs:87), `connect_client` (gpt.rs:55), the device-too-small
/// check (gpt.rs:160) and the whole layout computation (gpt.rs:167) have succeeded.
#[logic(open)]
pub fn format_writes_l(bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool) -> bool {
    pearlite! { bound && chan_ok && geom_ok && layout_ok }
}

#[ensures(result == format_writes_l(bound, chan_ok, geom_ok, layout_ok))]
pub fn format_writes_sectors(bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool) -> bool {
    if !bound {
        return false; // lib.rs:87-89
    }
    if !chan_ok {
        return false; // lib.rs:91 / gpt.rs:55-57
    }
    if !geom_ok {
        return false; // gpt.rs:160-163 "device too small for GPT"
    }
    if !layout_ok {
        return false; // gpt.rs:167 compute_partition_layout
    }
    true
}

// ===========================================================================
// 13. PROPERTY DRIVERS — `initialize` (lib.rs:68-84, gpt.rs:66-150)
// ===========================================================================

// ---- DPM-INIT-PRE-DEVICE-BOUND ----
#[ensures(!bound ==> result.0 == Outcome::Err(PtErr::NotInitialized))]
#[ensures(!bound ==> !result.2)]
#[ensures(!bound ==> result.1 == st)]
pub fn verify_dpm_init_pre_device_bound(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let r = init_reads_sectors(bound, geom_ok, chan_ok);
    (o, s2, r)
}
#[ensures(!bound ==> result.0 == Outcome::Err(PtErr::IoError))]
pub fn verify_dpm_init_pre_device_bound__mutant(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let r = init_reads_sectors(bound, geom_ok, chan_ok);
    (o, s2, r)
}

// ---- DPM-INIT-ERR-UNBOUND ----
#[ensures(!bound ==> result.0 == Outcome::Err(PtErr::NotInitialized) && !result.2)]
pub fn verify_dpm_init_err_unbound(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let r = init_reads_sectors(bound, geom_ok, chan_ok);
    (o, s2, r)
}
#[ensures(!bound ==> result.2)]
pub fn verify_dpm_init_err_unbound__mutant(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let r = init_reads_sectors(bound, geom_ok, chan_ok);
    (o, s2, r)
}

// ---- DPM-INIT-POST-CACHE — the table read is remembered AND handed back identically, so an
//      immediately following partition_info / num_partitions is answered from it ----
#[ensures(match rd { ReadRes::Ok(t) => (bound && geom_ok && chan_ok && !poisoned
    ==> result.0 == Outcome::Ok(t) && result.1 == Some(t)), _ => true })]
#[ensures(match result.0 { Outcome::Ok(t) => result.1 == Some(t), _ => true })]
pub fn verify_dpm_init_post_cache(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>) {
    initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned)
}
#[ensures(match result.0 { Outcome::Ok(_) => result.1 == None, _ => true })]
pub fn verify_dpm_init_post_cache__mutant(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>) {
    initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned)
}

// ---- DPM-INIT-POST-ENTRIES — one PartitionInfo per OCCUPIED slot, padding slots skipped ----

/// `parse_entries(&entry_data, header.num_partition_entries)` — gpt.rs:387-405 (called at
/// gpt.rs:131). `slots` is the entry buffer cut into its whole 128-byte slots (the
/// `offset + 128 > data.len()` break at gpt.rs:391), so the loop keeps the first
/// `min(count, slots.len())` of them, in order. Its length is bounded by the u32 `count`,
/// which is what discharges `project`'s `len <= u32::MAX` — a proved fact, not a premise.
#[ensures(result@.len() == (if count@ <= slots@.len() { count@ } else { slots@.len() }))]
#[ensures(forall<j: Int> 0 <= j && j < result@.len() ==> result@[j] == slots@[j])]
pub fn parse_entries_m(slots: &Vec<EntryM>, count: u32) -> Vec<EntryM> {
    let mut out: Vec<EntryM> = Vec::new();
    let mut i: usize = 0;
    let n = count as usize;
    #[invariant(i@ <= n@ && i@ <= slots@.len())]
    #[invariant(out@.len() == i@)]
    #[invariant(forall<j: Int> 0 <= j && j < i@ ==> out@[j] == slots@[j])]
    while i < n && i < slots.len() {
        out.push(slots[i]); // gpt.rs:395-402
        i += 1;
    }
    out
}

// J4 (level-2): the ONLY premise is D-RANGE-KEYENTITY:PARTITIONINFO-6425ed verbatim —
// `entries.iter().filter(|e| e.type_guid != [0u8; 16]).all(|e| e.starting_lba <= e.ending_lba
// && e.ending_lba - e.starting_lba < u64::MAX)` — where `entries` is the parse_entries output,
// i.e. the first min(count, slots.len()) slots (the `j < count@` conjunct says exactly that).
#[requires(forall<j: Int> 0 <= j && j < slots@.len() && j < count@ && !slots@[j].tg_zero
    ==> slots@[j].start@ <= slots@[j].end@
        && slots@[j].end@ - slots@[j].start@ < 18446744073709551615)]
#[ensures(forall<j: Int> 0 <= j && j < result.0@.len() ==> result.0@[j] == slots@[j])]
#[ensures(result.1@.len() == occupied_l(result.0@, result.0@.len()))]
#[ensures(forall<j: Int> 0 <= j && j < result.0@.len() && !result.0@[j].tg_zero
    ==> result.1@[occupied_l(result.0@, j)].start_lba@ == result.0@[j].start@
        && result.1@[occupied_l(result.0@, j)].num_sectors@
            == result.0@[j].end@ - result.0@[j].start@ + 1)]
pub fn verify_dpm_init_post_entries(slots: &Vec<EntryM>, count: u32) -> (Vec<EntryM>, Vec<PInfoM>) {
    let entries = parse_entries_m(slots, count); // gpt.rs:131
    let parts = project(&entries); // gpt.rs:132-144
    (entries, parts)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(forall<j: Int> 0 <= j && j < slots@.len() && j < count@ && !slots@[j].tg_zero
    ==> slots@[j].start@ <= slots@[j].end@
        && slots@[j].end@ - slots@[j].start@ < 18446744073709551615)]
#[ensures(!(forall<j: Int> 0 <= j && j < result.0@.len() ==> result.0@[j] == slots@[j]))]
#[ensures(result.1@.len() == occupied_l(result.0@, result.0@.len()))]
#[ensures(forall<j: Int> 0 <= j && j < result.0@.len() && !result.0@[j].tg_zero
    ==> result.1@[occupied_l(result.0@, j)].start_lba@ == result.0@[j].start@
        && result.1@[occupied_l(result.0@, j)].num_sectors@
            == result.0@[j].end@ - result.0@[j].start@ + 1)]
pub fn verify_dpm_init_post_entries__mutant(slots: &Vec<EntryM>, count: u32) -> (Vec<EntryM>, Vec<PInfoM>) {
    let entries = parse_entries_m(slots, count); // gpt.rs:131
    let parts = project(&entries); // gpt.rs:132-144
    (entries, parts)
}

// ---- DPM-INIT-POST-SECTOR-SIZE — the table reports the sector size the DEVICE reported ----
#[ensures(bound && geom_ok && chan_ok && !poisoned
    ==> (match result.0 { Outcome::Ok(r) => r.sector_size == dev_ss, _ => false }))]
#[ensures(match result.0 { Outcome::Ok(r) => r.sector_size == dev_ss, _ => true })]
pub fn verify_dpm_init_post_sector_size(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, dev_ss: u32, tag: u64,
    count: u32, poisoned: bool,
) -> (Outcome, Option<TabM>) {
    // lib.rs:73 `bd.sector_size(ns_id)` -> gpt.rs:58 GptManager.sector_size -> gpt.rs:148
    // `PartitionTable { sector_size: self.sector_size }`
    let rd = ReadRes::Ok(TabM { tag, count, sector_size: dev_ss });
    initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned)
}
#[ensures(match result.0 { Outcome::Ok(r) => r.sector_size@ == 512, _ => true })]
pub fn verify_dpm_init_post_sector_size__mutant(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, dev_ss: u32, tag: u64,
    count: u32, poisoned: bool,
) -> (Outcome, Option<TabM>) {
    let rd = ReadRes::Ok(TabM { tag, count, sector_size: dev_ss });
    initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned)
}

// ---- DPM-INIT-POST-CRC-VALIDATION — a copy is accepted only after BOTH checksums match ----
#[ensures(match result { Attempt::Ok(_) => hdr_crc_match && entry_crc_match, _ => true })]
#[ensures(match hdr_read { IoRes::Ok =>
    (hdr_len_ok && sig_ok && !hdr_crc_match ==> result == Attempt::Corrupt), _ => true })]
#[ensures(match hdr_read { IoRes::Ok => (match entry_read { IoRes::Ok =>
    (hdr_len_ok && sig_ok && hdr_crc_match && !entry_crc_match ==> result == Attempt::Corrupt),
    _ => true }), _ => true })]
pub fn verify_dpm_init_post_crc_validation(
    hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool, hdr_crc_match: bool, entry_read: IoRes,
    entry_crc_match: bool, tab: TabM,
) -> Attempt {
    try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read, entry_crc_match, tab)
}
#[ensures(match result { Attempt::Ok(_) => true, _ => hdr_crc_match && entry_crc_match })]
pub fn verify_dpm_init_post_crc_validation__mutant(
    hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool, hdr_crc_match: bool, entry_read: IoRes,
    entry_crc_match: bool, tab: TabM,
) -> Attempt {
    try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read, entry_crc_match, tab)
}

// ---- DPM-INIT-POST-BACKUP-FALLBACK — the backup is tried on EXACTLY CorruptTable and
//      NoPartitionTable, and an intact backup makes the read succeed ----
#[ensures(match primary { Attempt::Corrupt => (match backup {
    Attempt::Ok(t) => result == ReadRes::Ok(t), _ => true }), _ => true })]
#[ensures(match primary { Attempt::NoTable => (match backup {
    Attempt::Ok(t) => result == ReadRes::Ok(t), _ => true }), _ => true })]
// and no OTHER primary outcome consults the backup: Ok returns at once, Io propagates
#[ensures(match primary { Attempt::Ok(t) => result == ReadRes::Ok(t), _ => true })]
#[ensures(match primary { Attempt::Io => result == ReadRes::Err(PtErr::IoError), _ => true })]
pub fn verify_dpm_init_post_backup_fallback(primary: Attempt, backup: Attempt) -> ReadRes {
    read_gpt_m(primary, backup)
}
#[ensures(match primary { Attempt::Io => (match backup {
    Attempt::Ok(t) => result == ReadRes::Ok(t), _ => true }), _ => true })]
pub fn verify_dpm_init_post_backup_fallback__mutant(primary: Attempt, backup: Attempt) -> ReadRes {
    read_gpt_m(primary, backup)
}

// ---- DPM-INIT-ERR-NO-TABLE — neither copy acceptable => NoPartitionTable ----
#[ensures(match primary { Attempt::Corrupt => (match backup { Attempt::Ok(_) => true,
    _ => result == ReadRes::Err(PtErr::NoPartitionTable) }), _ => true })]
#[ensures(match primary { Attempt::NoTable => (match backup { Attempt::Ok(_) => true,
    _ => result == ReadRes::Err(PtErr::NoPartitionTable) }), _ => true })]
pub fn verify_dpm_init_err_no_table(primary: Attempt, backup: Attempt) -> ReadRes {
    read_gpt_m(primary, backup)
}
#[ensures(match primary { Attempt::NoTable => (match backup { Attempt::Ok(_) => true,
    _ => result == ReadRes::Err(PtErr::CorruptTable) }), _ => true })]
pub fn verify_dpm_init_err_no_table__mutant(primary: Attempt, backup: Attempt) -> ReadRes {
    read_gpt_m(primary, backup)
}

// ---- DPM-INIT-ERR-IO ----------------------------------------------------------------
// REFUTED. The obligation says a failed sector read is reported as an I/O failure "rather than
// pretending the table is missing or corrupt". That holds for the PRIMARY attempt (gpt.rs:83)
// but NOT for the backup: gpt.rs:91-95 wraps the backup attempt in
// `.map_err(|_| NoPartitionTable("neither primary nor backup GPT header is valid"))`, which
// swallows an `IoError` from the backup read and reports a missing table instead.
#[ensures(result == ReadRes::Err(PtErr::NoPartitionTable))]
#[ensures(result != ReadRes::Err(PtErr::IoError))]
pub fn refute_dpm_init_err_io() -> ReadRes {
    // a damaged primary signature (the spec's own FR-003 torn-write case) plus a device that
    // fails the backup read: the device DID fail a sector read needed to load the table.
    read_gpt_m(Attempt::NoTable, Attempt::Io)
}

// ---- DPM-INIT-ERR-GEOMETRY ----
#[ensures(bound && !geom_ok ==> result.0 == Outcome::Err(PtErr::IoError))]
#[ensures(bound && !geom_ok ==> !result.2)]
pub fn verify_dpm_init_err_geometry(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let r = init_reads_sectors(bound, geom_ok, chan_ok);
    (o, s2, r)
}
#[ensures(bound && !geom_ok ==> result.0 == Outcome::Err(PtErr::NoPartitionTable))]
pub fn verify_dpm_init_err_geometry__mutant(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let r = init_reads_sectors(bound, geom_ok, chan_ok);
    (o, s2, r)
}

// ---- DPM-INIT-ERR-CHANNEL ----
#[ensures(bound && geom_ok && !chan_ok ==> result.0 == Outcome::Err(PtErr::IoError))]
#[ensures(bound && geom_ok && !chan_ok ==> !result.2)]
pub fn verify_dpm_init_err_channel(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let r = init_reads_sectors(bound, geom_ok, chan_ok);
    (o, s2, r)
}
#[ensures(bound && geom_ok && !chan_ok ==> result.0 == Outcome::Err(PtErr::NotInitialized))]
pub fn verify_dpm_init_err_channel__mutant(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let r = init_reads_sectors(bound, geom_ok, chan_ok);
    (o, s2, r)
}

// ===========================================================================
// 14. Header pair + entry-array serialisation
// ===========================================================================

/// Mirror of the private `GptHeader` (gpt.rs:15-29); `disk_guid` is carried separately.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct HdrM {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub my_lba: u64,
    pub alternate_lba: u64,
    pub first_usable_lba: u64,
    pub last_usable_lba: u64,
    pub partition_entry_lba: u64,
    pub num_partition_entries: u32,
    pub partition_entry_size: u32,
    pub partition_entry_crc32: u32,
}

/// gpt.rs:175-201 — the primary header, and the backup built from it with `..primary_header`
/// so that ONLY `my_lba`, `alternate_lba` and `partition_entry_lba` differ.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * entry_sectors_l(ss@) + 4)]
#[ensures(result.0.signature == GPT_SIGNATURE)]
#[ensures(result.0.revision@ == 65536)]
#[ensures(result.0.header_size@ == 92)]
#[ensures(result.0.num_partition_entries@ == 128 && result.0.partition_entry_size@ == 128)]
#[ensures(result.0.my_lba@ == 1 && result.0.alternate_lba@ == ns@ - 1)]
#[ensures(result.0.partition_entry_lba@ == 2)]
#[ensures(result.0.first_usable_lba@ == first_usable_l(ss@))]
#[ensures(result.0.last_usable_lba@ == last_usable_l(ss@, ns@))]
#[ensures(result.1.my_lba@ == ns@ - 1 && result.1.alternate_lba@ == 1)]
#[ensures(result.1.partition_entry_lba@ == backup_entry_lba_write_l(ss@, ns@))]
#[ensures(result.1.signature == result.0.signature && result.1.revision == result.0.revision)]
#[ensures(result.1.header_size == result.0.header_size)]
#[ensures(result.1.first_usable_lba == result.0.first_usable_lba)]
#[ensures(result.1.last_usable_lba == result.0.last_usable_lba)]
#[ensures(result.1.num_partition_entries == result.0.num_partition_entries)]
#[ensures(result.1.partition_entry_size == result.0.partition_entry_size)]
#[ensures(result.1.partition_entry_crc32 == result.0.partition_entry_crc32)]
pub fn header_pair(ss: u32, ns: u64, entry_crc: u32) -> (HdrM, HdrM) {
    let (first, last) = usable_window(ss, ns);
    let primary = HdrM {
        signature: GPT_SIGNATURE,       // gpt.rs:176
        revision: GPT_REVISION_1_0,     // gpt.rs:177
        header_size: GPT_HEADER_SIZE,   // gpt.rs:178
        my_lba: 1,                      // gpt.rs:180
        alternate_lba: ns - 1,          // gpt.rs:181
        first_usable_lba: first,        // gpt.rs:182
        last_usable_lba: last,          // gpt.rs:183
        partition_entry_lba: 2,         // gpt.rs:185
        num_partition_entries: GPT_MAX_ENTRIES, // gpt.rs:186
        partition_entry_size: GPT_ENTRY_SIZE,   // gpt.rs:187
        partition_entry_crc32: entry_crc,       // gpt.rs:188
    };
    // gpt.rs:195-200 — `GptHeader { my_lba, alternate_lba, partition_entry_lba, ..primary_header }`
    let backup = HdrM {
        my_lba: ns - 1,
        alternate_lba: 1,
        partition_entry_lba: last + 1,
        signature: primary.signature,
        revision: primary.revision,
        header_size: primary.header_size,
        first_usable_lba: primary.first_usable_lba,
        last_usable_lba: primary.last_usable_lba,
        num_partition_entries: primary.num_partition_entries,
        partition_entry_size: primary.partition_entry_size,
        partition_entry_crc32: primary.partition_entry_crc32,
    };
    (primary, backup)
}

/// `serialize_entries` — gpt.rs:432-445 — for a PADDING slot. The buffer is `vec![0u8; 128*128]`
/// and the six `copy_from_slice` calls overwrite the slot's 128 bytes with the entry's fields.
/// For the all-zero entry that `compute_partition_layout` pads with (gpt.rs:311-318) every one
/// of those fields is zero, so the slot stays entirely zero.
#[requires(forall<k: Int> 0 <= k && k < 16 ==> tg[k]@ == 0 && ug[k]@ == 0)]
#[requires(forall<k: Int> 0 <= k && k < 72 ==> name[k]@ == 0)]
#[requires(start@ == 0 && end@ == 0 && attrs@ == 0)]
#[ensures(result@.len() == 128)]
#[ensures(forall<k: Int> 0 <= k && k < 128 ==> result@[k]@ == 0)]
pub fn serialize_padding_slot(
    tg: [u8; 16], ug: [u8; 16], start: u64, end: u64, attrs: u64, name: [u8; 72],
) -> Vec<u8> {
    let mut buf: Vec<u8> = vec![0u8; 128];
    let mut k: usize = 0;
    #[invariant(k@ <= 16)]
    #[invariant(buf@.len() == 128)]
    #[invariant(forall<q: Int> 0 <= q && q < 128 ==> buf@[q]@ == 0)]
    while k < 16 {
        buf[k] = tg[k]; // gpt.rs:437 type_guid
        buf[k + 16] = ug[k]; // gpt.rs:438 unique_guid
        k += 1;
    }
    buf[32] = (start % 256) as u8; // gpt.rs:439 starting_lba.to_le_bytes()
    buf[33] = (start / 256 % 256) as u8;
    buf[34] = (start / 65536 % 256) as u8;
    buf[35] = (start / 16777216 % 256) as u8;
    buf[36] = (start / 4294967296 % 256) as u8;
    buf[37] = (start / 1099511627776 % 256) as u8;
    buf[38] = (start / 281474976710656 % 256) as u8;
    buf[39] = (start / 72057594037927936) as u8;
    let mut q: usize = 40;
    #[invariant(40 <= q@ && q@ <= 56)]
    #[invariant(buf@.len() == 128)]
    #[invariant(forall<z: Int> 0 <= z && z < 128 ==> buf@[z]@ == 0)]
    while q < 56 {
        buf[q] = 0; // gpt.rs:440-441 ending_lba / attributes, both zero here
        q += 1;
    }
    let mut n: usize = 0;
    #[invariant(n@ <= 72)]
    #[invariant(buf@.len() == 128)]
    #[invariant(forall<z: Int> 0 <= z && z < 128 ==> buf@[z]@ == 0)]
    while n < 72 {
        buf[56 + n] = name[n]; // gpt.rs:442 name
        n += 1;
    }
    buf
}

/// `serialize_entries`'s buffer size — gpt.rs:433: `GPT_MAX_ENTRIES * GPT_ENTRY_SIZE`, a
/// constant independent of the device and of how many partitions were requested.
#[ensures(result@ == 16384)]
#[ensures(result@ == 128 * 128)]
pub fn entry_array_bytes() -> u32 {
    GPT_MAX_ENTRIES * GPT_ENTRY_SIZE
}

/// `serialize_entries` — gpt.rs:432-445 — bound safety. `buf` is exactly 16384 bytes and the
/// loop runs over EVERY element of `entries`, storing at `i * 128 .. i * 128 + 128`. The
/// `#[requires]` is what makes that total, and nothing in the source establishes it:
/// `compute_partition_layout` pads UP to 128 but never truncates (gpt.rs:310).
#[requires(n@ <= 128)]
#[ensures(result@ == 16384)]
pub fn serialize_entries_bound(n: usize) -> usize {
    let total = 128 * 128; // gpt.rs:433
    let mut i: usize = 0;
    #[invariant(i@ <= n@)]
    while i < n {
        let offset = i * 128; // gpt.rs:436
        // gpt.rs:437-442 all index buf[offset .. offset+128]; in bounds iff offset+128 <= 16384
        let _hi = offset + 128;
        i += 1;
    }
    total
}

// ===========================================================================
// 15. PROPERTY DRIVERS — `format` (lib.rs:86-95, gpt.rs:152-355)
// ===========================================================================

// ---- DPM-FORMAT-PRE-DEVICE-BOUND ----
#[ensures(!bound ==> result.0 == Outcome::Err(PtErr::NotInitialized))]
#[ensures(!bound ==> !result.2)]
pub fn verify_dpm_format_pre_device_bound(
    st: Option<TabM>, bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool, wr: ReadRes,
    poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = format_m(st, bound, chan_ok, wr, poisoned);
    let w = format_writes_sectors(bound, chan_ok, geom_ok, layout_ok);
    (o, s2, w)
}
#[ensures(!bound ==> result.2)]
pub fn verify_dpm_format_pre_device_bound__mutant(
    st: Option<TabM>, bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool, wr: ReadRes,
    poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = format_m(st, bound, chan_ok, wr, poisoned);
    let w = format_writes_sectors(bound, chan_ok, geom_ok, layout_ok);
    (o, s2, w)
}

// ---- DPM-FORMAT-ERR-UNBOUND ----
#[ensures(!bound ==> result.0 == Outcome::Err(PtErr::NotInitialized) && !result.2
    && result.1 == st)]
pub fn verify_dpm_format_err_unbound(
    st: Option<TabM>, bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool, wr: ReadRes,
    poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = format_m(st, bound, chan_ok, wr, poisoned);
    let w = format_writes_sectors(bound, chan_ok, geom_ok, layout_ok);
    (o, s2, w)
}
#[ensures(!bound ==> result.0 == Outcome::Err(PtErr::LayoutError))]
pub fn verify_dpm_format_err_unbound__mutant(
    st: Option<TabM>, bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool, wr: ReadRes,
    poisoned: bool,
) -> (Outcome, Option<TabM>, bool) {
    let (o, s2) = format_m(st, bound, chan_ok, wr, poisoned);
    let w = format_writes_sectors(bound, chan_ok, geom_ok, layout_ok);
    (o, s2, w)
}

// ---- DPM-FORMAT-PRE-SECTOR-SIZE — 512 or 4096 is exactly the domain on which the protective
//      MBR's fixed offsets (446..462, 510, 511) fit the `vec![0u8; sector_size]` buffer ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 1)]
#[ensures(result.0@ == ss@ && result.0@ >= 512)]
#[ensures(ss@ == 512 ==> result.1@ == 32)]
#[ensures(ss@ == 4096 ==> result.1@ == 4)]
pub fn verify_dpm_format_pre_sector_size(ss: u32, ns: u64) -> (usize, u32) {
    let mbr = protective_mbr(ss, ns); // every store in bounds ONLY because sector_size >= 512
    (mbr.len(), entry_sectors(ss))
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 1)]
#[ensures(ss@ == 4096 ==> result.1@ == 32)]
pub fn verify_dpm_format_pre_sector_size__mutant(ss: u32, ns: u64) -> (usize, u32) {
    let mbr = protective_mbr(ss, ns);
    (mbr.len(), entry_sectors(ss))
}

// ---- DPM-FORMAT-ERR-LAYOUT-MULTI-GROW ----
// J4 (level-2): premises = D-RANGE-FR-011-46880c's config conjunct + the obligation's own
// scenario ("if two or more partitions ask for that"). SUM (D-RANGE-FR-006-9e23ee) is NOT
// assumed: the multi-rest check (gpt.rs:257) returns before the sum (gpt.rs:263) is formed.
// `total_usable` is universally quantified (no geometry premise): the check does not read it.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(rest_count_l(specs@, specs@.len()) > 1)]
#[ensures(result == Err(LayoutErr::MultiRest))]
#[ensures(match result { Ok(_) => false, _ => true })]
pub fn verify_dpm_format_err_layout_multi_grow(
    specs: &Vec<SpecM>, ss: u32, total_usable: u64,
) -> Result<u64, LayoutErr> {
    layout_classify(specs, ss, total_usable)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(rest_count_l(specs@, specs@.len()) > 1)]
#[ensures(!(result == Err(LayoutErr::MultiRest)))]
#[ensures(match result { Ok(_) => false, _ => true })]
pub fn verify_dpm_format_err_layout_multi_grow__mutant(
    specs: &Vec<SpecM>, ss: u32, total_usable: u64,
) -> Result<u64, LayoutErr> {
    layout_classify(specs, ss, total_usable)
}

// ---- DPM-FORMAT-ERR-LAYOUT-CAPACITY ----
// J4 (level-2): premises = D-RANGE-FR-011-46880c's config conjunct + D-RANGE-FR-006-9e23ee
// verbatim (the try_fold/checked_add over the ceil-sectors of every partition is_some() iff
// their exact sum fits a u64, every term being non-negative). The former `total_usable >= 1`
// is dropped: `total_usable` is universally quantified.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= 18446744073709551615)]
#[ensures(rest_count_l(specs@, specs@.len()) <= 1
    && fixed_sum_l(specs@, specs@.len(), ss@) > total_usable@
    ==> result == Err(LayoutErr::Capacity))]
#[ensures(match result { Err(LayoutErr::Capacity) =>
    fixed_sum_l(specs@, specs@.len(), ss@) > total_usable@, _ => true })]
pub fn verify_dpm_format_err_layout_capacity(
    specs: &Vec<SpecM>, ss: u32, total_usable: u64,
) -> Result<u64, LayoutErr> {
    layout_classify(specs, ss, total_usable)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= 18446744073709551615)]
#[ensures(!(rest_count_l(specs@, specs@.len()) <= 1
    && fixed_sum_l(specs@, specs@.len(), ss@) > total_usable@
    ==> result == Err(LayoutErr::Capacity)))]
#[ensures(match result { Err(LayoutErr::Capacity) =>
    fixed_sum_l(specs@, specs@.len(), ss@) > total_usable@, _ => true })]
pub fn verify_dpm_format_err_layout_capacity__mutant(
    specs: &Vec<SpecM>, ss: u32, total_usable: u64,
) -> Result<u64, LayoutErr> {
    layout_classify(specs, ss, total_usable)
}

// ---- DPM-FORMAT-FRAME-NO-WRITE-ON-REJECT ----
#[ensures(!layout_ok ==> !result)]
#[ensures(!geom_ok ==> !result)]
#[ensures(result ==> geom_ok && layout_ok && bound && chan_ok)]
pub fn verify_dpm_format_frame_no_write_on_reject(
    bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool,
) -> bool {
    format_writes_sectors(bound, chan_ok, geom_ok, layout_ok)
}
#[ensures(!layout_ok ==> result)]
pub fn verify_dpm_format_frame_no_write_on_reject__mutant(
    bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool,
) -> bool {
    format_writes_sectors(bound, chan_ok, geom_ok, layout_ok)
}

// ---- DPM-FORMAT-PRE-GEOMETRY-MIN — the total sector count must cover two entry arrays plus
//      the two headers, because gpt.rs:158 subtracts without checking. Below that the release
//      build WRAPS to a huge `last_usable_lba`, which then slips past the gpt.rs:160 check ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 1)]
#[ensures(ns@ >= entry_sectors_l(ss@) + 2 ==> result.1@ == last_usable_l(ss@, ns@))]
#[ensures(ns@ < entry_sectors_l(ss@) + 2 ==> result.1@ > ns@)]
#[ensures(ns@ < entry_sectors_l(ss@) + 2 ==> result.0@ < result.1@)]
pub fn verify_dpm_format_pre_geometry_min(ss: u32, ns: u64) -> (u64, u64) {
    usable_window_wrapping(ss, ns)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 1)]
#[ensures(ns@ < entry_sectors_l(ss@) + 2 ==> result.0@ >= result.1@)]
pub fn verify_dpm_format_pre_geometry_min__mutant(ss: u32, ns: u64) -> (u64, u64) {
    usable_window_wrapping(ss, ns)
}

/// `first_usable_lba` / `last_usable_lba` (gpt.rs:157-158) under the RELEASE build's wrapping
/// `-`. In a debug build the same expression panics; either way the gpt.rs:160-163
/// "device too small for GPT" check is not what rejects a too-small device.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 1)]
#[ensures(result.0@ == first_usable_l(ss@))]
#[ensures(ns@ >= entry_sectors_l(ss@) + 2 ==> result.1@ == last_usable_l(ss@, ns@))]
#[ensures(ns@ < entry_sectors_l(ss@) + 2
    ==> result.1@ == 18446744073709551616 + last_usable_l(ss@, ns@))]
#[ensures(ns@ < entry_sectors_l(ss@) + 2 ==> result.1@ > ns@)]
#[ensures(ns@ < entry_sectors_l(ss@) + 2 ==> result.0@ < result.1@)]
pub fn usable_window_wrapping(ss: u32, ns: u64) -> (u64, u64) {
    let es = entry_sectors(ss) as u64;
    let first = 2 + es; // gpt.rs:157
    let last = ns.wrapping_sub(1).wrapping_sub(es).wrapping_sub(1); // gpt.rs:158
    (first, last)
}

// ---- extra lemmas the drivers below cite ----

/// The two division facts `cdiv` needs, in QUANTIFIED form: a loop-free driver cannot cite
/// `lemma_cdiv_nonneg` once per element of a universally quantified postcondition.
#[logic]
#[requires(d > 0)]
#[ensures(forall<n: Int> n >= 0 ==> cdiv(n, d) >= 0)]
#[ensures(forall<n: Int> n >= 0 ==> cdiv(n, d) * d >= n)]
#[ensures(forall<n: Int> n > 0 ==> (cdiv(n, d) - 1) * d < n)]
#[ensures(forall<n: Int> n > 0 ==> cdiv(n, d) >= 1)]
pub fn lemma_cdiv_all(d: Int) {}

/// Every spec one byte long: `rest_count` is 0 and the fixed demand is exactly one sector each.
#[logic]
#[requires(0 <= n && n <= s.len())]
#[requires(forall<k: Int> 0 <= k && k < n ==> s[k].size_bytes@ == 1)]
#[ensures(rest_count_l(s, n) == 0)]
#[ensures(fixed_sum_l(s, n, 512) == n)]
#[variant(n)]
pub fn lemma_all_one_byte(s: Seq<SpecM>, n: Int) {
    pearlite! { if n > 0 { lemma_all_one_byte(s, n - 1) } }
}

/// An all-`tg_zero` tail contributes nothing to `occupied_l`, so a 128-slot array padded from
/// index `m` has exactly `occupied_l(s, m)` occupied slots — without 128 unfoldings.
#[logic]
#[requires(0 <= m && m <= n && n <= s.len())]
#[requires(forall<k: Int> m <= k && k < n ==> s[k].tg_zero)]
#[ensures(occupied_l(s, n) == occupied_l(s, m))]
#[variant(n - m)]
pub fn lemma_occupied_tail_zero(s: Seq<EntryM>, m: Int, n: Int) {
    pearlite! { if m < n { lemma_occupied_tail_zero(s, m, n - 1) } }
}

// ---- DPM-FORMAT-PRE-MAX-128 --------------------------------------------------------------
// REFUTED. `compute_partition_layout` pads UP to 128 slots (gpt.rs:310) but never truncates,
// so 129 requested partitions produce 129 entries; `serialize_entries` then allocates exactly
// 128*128 = 16384 bytes (gpt.rs:433) and stores slot 128 at `buf[16384..16400]` — out of
// bounds. Nothing in the component checks the 128-partition limit, and the failure mode is a
// panic, not the declared `LayoutError`.
// The precondition below is obviously satisfiable: any 129 one-byte partitions on a device with
// at least 129 usable sectors.
#[requires(specs@.len() == 129)]
#[requires(forall<k: Int> 0 <= k && k < 129 ==> specs@[k].size_bytes@ == 1)]
#[ensures(match result { Ok(v) => v@.len() == 129 && v@.len() > 128, _ => false })]
pub fn refute_dpm_format_pre_max_128(specs: &Vec<SpecM>) -> Result<Vec<EntryM>, LayoutErr> {
    proof_assert! { lemma_all_one_byte(specs@, 129); true };
    // total_usable = 100000 - 34 + 1 = 99967, fixed demand 129, so rest_sectors = 99838
    layout_place(specs, 512, 34, 100000, 99838)
}

// ---- DPM-FORMAT-ERR-LAYOUT-UNREACHABLE ----
// J4 (level-2): premises = D-RANGE-FR-011-46880c (config conjunct),
// D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c (config conjunct, verbatim:
// `total_sectors >= 2 * 16384.div_ceil(sector_size) + 5`) and D-RANGE-FR-006-9e23ee. The
// usable window is COMPUTED from the geometry exactly as gpt.rs:157-158/245 do, instead of
// assuming `first >= 3 && first <= last && last <= u64::MAX - 1`.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= 18446744073709551615)]
#[ensures(match result { Err(LayoutErr::PerPartition) => false, _ => true })]
pub fn verify_dpm_format_err_layout_unreachable(
    specs: &Vec<SpecM>, ss: u32, ns: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    let (first, last) = usable_window(ss, ns); // gpt.rs:157-158
    compute_layout(specs, ss, first, last) // gpt.rs:239-322
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= 18446744073709551615)]
#[ensures(!(match result { Err(LayoutErr::PerPartition) => false, _ => true }))]
pub fn verify_dpm_format_err_layout_unreachable__mutant(
    specs: &Vec<SpecM>, ss: u32, ns: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    let (first, last) = usable_window(ss, ns); // gpt.rs:157-158
    compute_layout(specs, ss, first, last) // gpt.rs:239-322
}

// ---- DPM-FORMAT-SUM-OVERFLOW — `.sum::<u64>()` at gpt.rs:263-268 has no overflow guard.
//      512 partitions of `u64::MAX` bytes each at a 512-byte sector size demand
//      512 * 2^55 == 2^64 sectors, which wraps to 0 and then PASSES the gpt.rs:270 capacity
//      check against any device. (In a debug build the same `+` panics instead.) ----
#[ensures(result.0@ == 0)]
#[ensures(!result.1)]
pub fn verify_dpm_format_sum_overflow() -> (u64, bool) {
    proof_assert! { cdiv(18446744073709551615, 512) == 36028797018963968 };
    let per: u64 = 36028797018963968; // cdiv(u64::MAX, 512) == 2^55
    let mut acc: u64 = 0;
    let mut i: u32 = 0;
    #[invariant(i@ <= 512)]
    #[invariant(i@ < 512 ==> acc@ == i@ * 36028797018963968)]
    #[invariant(i@ == 512 ==> acc@ == 0)]
    while i < 512 {
        acc = acc.wrapping_add(per); // gpt.rs:268 `.sum()` — a plain `+` chain
        i += 1;
    }
    let total_usable: u64 = 1000;
    (acc, acc > total_usable) // gpt.rs:270 — the check the wrapped total slips past
}
#[ensures(result.1)]
pub fn verify_dpm_format_sum_overflow__mutant() -> (u64, bool) {
    let per: u64 = 36028797018963968;
    let mut acc: u64 = 0;
    let mut i: u32 = 0;
    #[invariant(i@ <= 512)]
    while i < 512 {
        acc = acc.wrapping_add(per);
        i += 1;
    }
    (acc, acc > 1000u64)
}

// ---- DPM-FORMAT-POST-TABLE ---------------------------------------------------------------
// REFUTED. The result projection filters on `e.type_guid != [0u8; 16]` (gpt.rs:222), and
// nothing rejects a `PartitionSpec` whose `type_guid` the caller left all-zero. Two partitions
// requested, one of them with an all-zero type GUID: the returned table has ONE entry and
// `num_partitions()` immediately afterwards answers 1, not 2.
#[requires(specs@.len() == 2)]
#[requires(specs@[0].size_bytes@ == 1 && specs@[1].size_bytes@ == 1)]
#[requires(specs@[0].tg_zero && !specs@[1].tg_zero)]
#[ensures(result.0@ == 2)]
#[ensures(result.1@ == 1)]
#[ensures(result.0@ != result.1@)]
pub fn refute_dpm_format_post_table(specs: &Vec<SpecM>) -> (usize, usize) {
    proof_assert! { lemma_all_one_byte(specs@, 2); lemma_cdiv_all(512); true };
    match layout_place(specs, 512, 34, 1000, 965) {
        Ok(v) => {
            proof_assert! { v@.len() == 128 };
            proof_assert! { forall<k: Int> 2 <= k && k < 128 ==> v@[k].tg_zero };
            proof_assert! { forall<j: Int> 0 <= j && j < 128 ==> v@[j].end@ >= v@[j].start@ };
            proof_assert! {
                forall<j: Int> 0 <= j && j < 128
                    ==> v@[j].end@ - v@[j].start@ < 18446744073709551615
            };
            proof_assert! {
                lemma_occupied_tail_zero(v@, 2, 128);
                occupied_l(v@, 128) == occupied_l(v@, 2)
            };
            proof_assert! { occupied_l(v@, 2) == 1 };
            let out = project(&v);
            (specs.len(), out.len())
        }
        Err(_) => (0, 0),
    }
}

// ---- DPM-FORMAT-POST-FIXED-SIZE ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    && specs@[j].size_bytes@ > 0
    ==> v@[j].end@ + 1 == v@[j].start@ + cdiv(specs@[j].size_bytes@, ss@), _ => true })]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    && specs@[j].size_bytes@ > 0
    ==> (v@[j].end@ + 1 - v@[j].start@) * ss@ >= specs@[j].size_bytes@, _ => true })]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    && specs@[j].size_bytes@ > 0
    ==> (v@[j].end@ - v@[j].start@) * ss@ < specs@[j].size_bytes@, _ => true })]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    && specs@[j].size_bytes@ > 0 ==> v@[j].end@ >= v@[j].start@, _ => true })]
pub fn verify_dpm_format_post_fixed_size(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    proof_assert! { lemma_cdiv_all(ss@); true };
    layout_place(specs, ss, first, last, rest)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    && specs@[j].size_bytes@ > 0
    ==> (v@[j].end@ + 1 - v@[j].start@) * ss@ == specs@[j].size_bytes@, _ => true })]
pub fn verify_dpm_format_post_fixed_size__mutant(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    layout_place(specs, ss, first, last, rest)
}

// ---- DPM-FORMAT-POST-REST-OF-DISK ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
// the size-0 partition receives EXACTLY the space left after every fixed partition is placed
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    && specs@[j].size_bytes@ == 0
    ==> v@[j].end@ + 1 == v@[j].start@ + (last@ - first@ + 1)
        - fixed_sum_l(specs@, specs@.len(), ss@), _ => true })]
pub fn verify_dpm_format_post_rest_of_disk(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    layout_place(specs, ss, first, last, rest)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    && specs@[j].size_bytes@ == 0
    ==> v@[j].end@ + 1 == v@[j].start@ + (last@ - first@ + 1), _ => true })]
pub fn verify_dpm_format_post_rest_of_disk__mutant(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    layout_place(specs, ss, first, last, rest)
}

// ---- DPM-FORMAT-POST-MBR ----
// J4 (level-2): premises = D-RANGE-FR-011-46880c (config conjunct) and
// D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c (config conjunct) verbatim:
// `total_sectors >= 2 * 16384.div_ceil(sector_size) + 5` (was the derived `ns >= 1`).
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
#[ensures(result@[446]@ == 0)]
#[ensures(result@[450]@ == 238)]
#[ensures(le32_at(result@, 454) == 1)]
#[ensures(le32_at(result@, 458) == (if ns@ - 1 <= 4294967295 { ns@ - 1 } else { 4294967295 }))]
#[ensures(result@[510]@ == 85 && result@[511]@ == 170)]
pub fn verify_dpm_format_post_mbr(ss: u32, ns: u64) -> Vec<u8> {
    protective_mbr(ss, ns)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
#[ensures(!(result@[446]@ == 0))]
#[ensures(result@[450]@ == 238)]
#[ensures(le32_at(result@, 454) == 1)]
#[ensures(le32_at(result@, 458) == (if ns@ - 1 <= 4294967295 { ns@ - 1 } else { 4294967295 }))]
#[ensures(result@[510]@ == 85 && result@[511]@ == 170)]
pub fn verify_dpm_format_post_mbr__mutant(ss: u32, ns: u64) -> Vec<u8> {
    protective_mbr(ss, ns)
}

// ---- DPM-FORMAT-POST-HEADER-PAIR ----
// J4 (level-2): premises = D-RANGE-FR-011-46880c (config conjunct) and
// D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c (config conjunct) verbatim:
// `total_sectors >= 2 * 16384.div_ceil(sector_size) + 5` (was the derived `>= 2 * entry_sectors + 4`).
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
#[ensures(result.0.my_lba@ == 1 && result.0.alternate_lba@ == ns@ - 1)]
#[ensures(result.1.my_lba@ == ns@ - 1 && result.1.alternate_lba@ == 1)]
#[ensures(result.0.my_lba == result.1.alternate_lba)]
#[ensures(result.1.my_lba == result.0.alternate_lba)]
pub fn verify_dpm_format_post_header_pair(ss: u32, ns: u64, crc: u32) -> (HdrM, HdrM) {
    header_pair(ss, ns, crc)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
#[ensures(!(result.0.my_lba@ == 1 && result.0.alternate_lba@ == ns@ - 1))]
#[ensures(result.1.my_lba@ == ns@ - 1 && result.1.alternate_lba@ == 1)]
#[ensures(result.0.my_lba == result.1.alternate_lba)]
#[ensures(result.1.my_lba == result.0.alternate_lba)]
pub fn verify_dpm_format_post_header_pair__mutant(ss: u32, ns: u64, crc: u32) -> (HdrM, HdrM) {
    header_pair(ss, ns, crc)
}

// ---- DPM-FORMAT-POST-ENTRY-ARRAY ----
// (i) + (ii): the array is always 128 slots of 128 bytes = 16384 bytes, independent of the
// request and of the device, and both copies are written with that same length
// (`write_gpt_trace` entries 2 and 3). (iii): a padding slot serialises to 128 zero bytes.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns_sectors@ >= 2 * entry_sectors_l(ss@) + 4)]
#[requires(forall<k: Int> 0 <= k && k < 16 ==> tg[k]@ == 0 && ug[k]@ == 0)]
#[requires(forall<k: Int> 0 <= k && k < 72 ==> name[k]@ == 0)]
#[ensures(result.0@ == 16384)]
#[ensures(result.1@[2].sectors@ == cdiv(16384, ss@))]
#[ensures(result.1@[3].sectors@ == cdiv(16384, ss@))]
#[ensures(result.1@[2].sectors == result.1@[3].sectors)]
#[ensures(result.2@.len() == 128)]
#[ensures(forall<k: Int> 0 <= k && k < 128 ==> result.2@[k]@ == 0)]
pub fn verify_dpm_format_post_entry_array(
    ns: u32, ss: u32, ns_sectors: u64, tg: [u8; 16], ug: [u8; 16], name: [u8; 72],
) -> (u32, Vec<Acc>, Vec<u8>) {
    let bytes = entry_array_bytes();
    let trace = write_gpt_trace(ns, ss, ns_sectors);
    let slot = serialize_padding_slot(tg, ug, 0, 0, 0, name);
    (bytes, trace, slot)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns_sectors@ >= 2 * entry_sectors_l(ss@) + 4)]
#[requires(forall<k: Int> 0 <= k && k < 16 ==> tg[k]@ == 0 && ug[k]@ == 0)]
#[requires(forall<k: Int> 0 <= k && k < 72 ==> name[k]@ == 0)]
#[ensures(result.0@ == 32768)]
pub fn verify_dpm_format_post_entry_array__mutant(
    ns: u32, ss: u32, ns_sectors: u64, tg: [u8; 16], ug: [u8; 16], name: [u8; 72],
) -> (u32, Vec<Acc>, Vec<u8>) {
    let bytes = entry_array_bytes();
    let trace = write_gpt_trace(ns, ss, ns_sectors);
    let slot = serialize_padding_slot(tg, ug, 0, 0, 0, name);
    (bytes, trace, slot)
}

// ---- DPM-FORMAT-FRAME-DATA-REGIONS ----
// J4 (level-2): premises = D-RANGE-FR-011-46880c (config conjunct) and
// D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c (config conjunct) verbatim:
// `total_sectors >= 2 * 16384.div_ceil(sector_size) + 5` (was the derived `>= 2 * entry_sectors + 4`).
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns_sectors@ >= 2 * cdiv(16384, ss@) + 5)]
// exactly five accesses, in the documented order, all writes, all on the configured namespace
#[ensures(result@.len() == 5)]
#[ensures(all_ns(result@, ns@))]
// and not one of them touches the usable region the partitions occupy
#[ensures(writes_avoid(result@, first_usable_l(ss@), last_usable_l(ss@, ns_sectors@)))]
pub fn verify_dpm_format_frame_data_regions(ns: u32, ss: u32, ns_sectors: u64) -> Vec<Acc> {
    let t = write_gpt_trace(ns, ss, ns_sectors);
    proof_assert! {
        // LBA 0 and LBA 1 are below first_usable = 2 + entry_sectors
        t@[0].lba@ < first_usable_l(ss@) && t@[1].lba@ < first_usable_l(ss@)
    };
    proof_assert! {
        // the primary entry array is [2, 2 + entry_sectors) == [2, first_usable)
        t@[2].lba@ + t@[2].sectors@ == first_usable_l(ss@)
    };
    proof_assert! {
        // the backup entry array starts at last_usable + 1
        t@[3].lba@ == last_usable_l(ss@, ns_sectors@) + 1
    };
    proof_assert! {
        // and the backup header is the very last sector, above last_usable
        t@[4].lba@ > last_usable_l(ss@, ns_sectors@)
    };
    t
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns_sectors@ >= 2 * cdiv(16384, ss@) + 5)]
// exactly five accesses, in the documented order, all writes, all on the configured namespace
#[ensures(!(result@.len() == 5))]
#[ensures(all_ns(result@, ns@))]
// and not one of them touches the usable region the partitions occupy
#[ensures(writes_avoid(result@, first_usable_l(ss@), last_usable_l(ss@, ns_sectors@)))]
pub fn verify_dpm_format_frame_data_regions__mutant(ns: u32, ss: u32, ns_sectors: u64) -> Vec<Acc> {
    let t = write_gpt_trace(ns, ss, ns_sectors);
    proof_assert! {
        // LBA 0 and LBA 1 are below first_usable = 2 + entry_sectors
        t@[0].lba@ < first_usable_l(ss@) && t@[1].lba@ < first_usable_l(ss@)
    };
    proof_assert! {
        // the primary entry array is [2, 2 + entry_sectors) == [2, first_usable)
        t@[2].lba@ + t@[2].sectors@ == first_usable_l(ss@)
    };
    proof_assert! {
        // the backup entry array starts at last_usable + 1
        t@[3].lba@ == last_usable_l(ss@, ns_sectors@) + 1
    };
    proof_assert! {
        // and the backup header is the very last sector, above last_usable
        t@[4].lba@ > last_usable_l(ss@, ns_sectors@)
    };
    t
}

// ---- DPM-FORMAT-POST-CACHE ----
#[ensures(match wr { ReadRes::Ok(t) => (bound && chan_ok && !poisoned
    ==> result.0 == Outcome::Ok(t) && result.1 == Some(t)), _ => true })]
#[ensures(match result.0 { Outcome::Ok(t) => result.1 == Some(t), _ => true })]
pub fn verify_dpm_format_post_cache(
    st: Option<TabM>, bound: bool, chan_ok: bool, wr: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>) {
    format_m(st, bound, chan_ok, wr, poisoned)
}
#[ensures(match result.0 { Outcome::Ok(_) => result.1 == None, _ => true })]
pub fn verify_dpm_format_post_cache__mutant(
    st: Option<TabM>, bound: bool, chan_ok: bool, wr: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>) {
    format_m(st, bound, chan_ok, wr, poisoned)
}

// ---- DPM-FORMAT-POST-GUID-FRESH ----------------------------------------------------------
// REFUTED. gpt.rs:566 `if let Ok(mut f) = std::fs::File::open("/dev/urandom")` — when the open
// fails (no /dev in a chroot or minimal container, fd exhaustion, a seccomp filter) the body
// never runs and `guid` keeps the `[0u8; 16]` from gpt.rs:565. The version/variant bits are
// then applied to zeros (gpt.rs:571-572), so the "freshly generated random identifier" is the
// FIXED CONSTANT 00000000-0000-4000-8000-000000000000 on every call.
// The trusted boundary CONFORMS: `File::open` returns `Result` and the source's own `if let Ok`
// is what discards the failure, so modelling the failure path as "guid untouched" is exactly
// std's contract, not a weakened mock.
#[ensures(result[6]@ == 64 && result[8]@ == 128)]
#[ensures(forall<i: Int> 0 <= i && i < 16 && i != 6 && i != 8 ==> result[i]@ == 0)]
pub fn refute_dpm_format_post_guid_fresh(bytes: [u8; 16]) -> [u8; 16] {
    generate_guid(false, bytes)
}

// ---- DPM-INV-GUID-DISTINCT ---------------------------------------------------------------
// REFUTED, same root cause: on the `/dev/urandom`-unavailable path every `generate_guid()` call
// returns the SAME 16 bytes, so the disk GUID and every partition's unique GUID coincide.
#[ensures(forall<i: Int> 0 <= i && i < 16 ==> result.0[i] == result.1[i])]
#[ensures(forall<i: Int> 0 <= i && i < 16 ==> result.0[i] == result.2[i])]
pub fn refute_dpm_inv_guid_distinct(
    b1: [u8; 16], b2: [u8; 16], b3: [u8; 16],
) -> ([u8; 16], [u8; 16], [u8; 16]) {
    // gpt.rs:174 disk_guid, then gpt.rs:298 unique_guid for partition 0 and partition 1
    (generate_guid(false, b1), generate_guid(false, b2), generate_guid(false, b3))
}

// ---- DPM-FORMAT-POST-SECTOR-SIZE-ECHO ----
#[ensures(match result.0 { Outcome::Ok(t) => t.sector_size == cfg_ss, _ => true })]
pub fn verify_dpm_format_post_sector_size_echo(
    st: Option<TabM>, bound: bool, chan_ok: bool, cfg_ss: u32, tag: u64, count: u32,
    poisoned: bool,
) -> (Outcome, Option<TabM>) {
    // lib.rs:91 config.sector_size -> gpt.rs:58 GptManager.sector_size -> gpt.rs:235
    let wr = ReadRes::Ok(TabM { tag, count, sector_size: cfg_ss });
    format_m(st, bound, chan_ok, wr, poisoned)
}
#[ensures(match result.0 { Outcome::Ok(t) => t.sector_size@ == 4096, _ => true })]
pub fn verify_dpm_format_post_sector_size_echo__mutant(
    st: Option<TabM>, bound: bool, chan_ok: bool, cfg_ss: u32, tag: u64, count: u32,
    poisoned: bool,
) -> (Outcome, Option<TabM>) {
    let wr = ReadRes::Ok(TabM { tag, count, sector_size: cfg_ss });
    format_m(st, bound, chan_ok, wr, poisoned)
}

/// gpt.rs:296-303 — `GptEntry { type_guid: spec.type_guid, .. }`: a direct field move, no
/// transformation of any kind.
#[ensures(forall<k: Int> 0 <= k && k < 16 ==> result[k] == tg[k])]
pub fn entry_type_guid(tg: [u8; 16]) -> [u8; 16] {
    tg
}

// ---- DPM-FORMAT-POST-TYPE-GUID-ECHO ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
// the type identifier is copied through unchanged ...
#[ensures(forall<k: Int> 0 <= k && k < 16 ==> result.1[k] == tg[k])]
// ... and NOT reordered: request position j becomes entry slot j
#[ensures(match result.0 { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    ==> v@[j].tg_zero == specs@[j].tg_zero, _ => true })]
pub fn verify_dpm_format_post_type_guid_echo(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64, tg: [u8; 16],
) -> (Result<Vec<EntryM>, LayoutErr>, [u8; 16]) {
    (layout_place(specs, ss, first, last, rest), entry_type_guid(tg))
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[ensures(forall<k: Int> 0 <= k && k < 16 ==> result.1[k]@ == 0)]
pub fn verify_dpm_format_post_type_guid_echo__mutant(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64, tg: [u8; 16],
) -> (Result<Vec<EntryM>, LayoutErr>, [u8; 16]) {
    (layout_place(specs, ss, first, last, rest), entry_type_guid(tg))
}

// ---- DPM-FORMAT-FIELDS-DROPPED — the disk GUID and the per-entry attribute flags are parsed
//      but never surfaced, and the format path always writes attributes = 0 ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[requires(entries@.len() <= 4294967295)]
#[requires(forall<j: Int> 0 <= j && j < entries@.len() ==> entries@[j].end@ >= entries@[j].start@)]
#[requires(forall<j: Int> 0 <= j && j < entries@.len()
    ==> entries@[j].end@ - entries@[j].start@ < 18446744073709551615)]
// every slot a format writes carries attributes = 0 (gpt.rs:301), whatever was on the disk
#[ensures(match result.0 { Ok(v) => forall<j: Int> 0 <= j && j < v@.len()
    ==> v@[j].attrs@ == 0, _ => true })]
// and `PartitionInfo` has no attributes/disk-GUID field at all: the projection drops them
#[ensures(result.1@.len() == occupied_l(entries@, entries@.len()))]
pub fn verify_dpm_format_fields_dropped(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64, entries: &Vec<EntryM>,
) -> (Result<Vec<EntryM>, LayoutErr>, Vec<PInfoM>) {
    (layout_place(specs, ss, first, last, rest), project(entries))
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[requires(entries@.len() <= 4294967295)]
#[requires(forall<j: Int> 0 <= j && j < entries@.len() ==> entries@[j].end@ >= entries@[j].start@)]
#[requires(forall<j: Int> 0 <= j && j < entries@.len()
    ==> entries@[j].end@ - entries@[j].start@ < 18446744073709551615)]
#[ensures(match result.0 { Ok(v) => forall<j: Int> 0 <= j && j < v@.len()
    ==> v@[j].attrs@ == 1, _ => true })]
pub fn verify_dpm_format_fields_dropped__mutant(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64, entries: &Vec<EntryM>,
) -> (Result<Vec<EntryM>, LayoutErr>, Vec<PInfoM>) {
    (layout_place(specs, ss, first, last, rest), project(entries))
}

// ---- DPM-FORMAT-IGNORES-DEVICE-GEOMETRY — format takes sector size, total sectors and the
//      namespace from the REQUEST and never compares them with what the device reports ----
#[ensures(result.0 == cfg_ss)]
#[ensures(result.1 == cfg_total)]
#[ensures(result.2 == cfg_ns)]
pub fn verify_dpm_format_ignores_device_geometry(
    cfg_ss: u32, cfg_total: u64, cfg_ns: u32, dev_ss: u32, dev_total: u64, dev_ns: u32,
) -> (u32, u64, u32) {
    // lib.rs:91 `GptManager::new(bd, config.ns_id, config.sector_size, config.total_sectors)`
    // — `dev_*` are never read on this path (contrast lib.rs:73-78 on the read path).
    let _ = dev_ss;
    let _ = dev_total;
    let _ = dev_ns;
    (cfg_ss, cfg_total, cfg_ns)
}
#[ensures(result.0 == dev_ss)]
pub fn verify_dpm_format_ignores_device_geometry__mutant(
    cfg_ss: u32, cfg_total: u64, cfg_ns: u32, dev_ss: u32, dev_total: u64, dev_ns: u32,
) -> (u32, u64, u32) {
    let _ = dev_ss;
    let _ = dev_total;
    let _ = dev_ns;
    (cfg_ss, cfg_total, cfg_ns)
}

// ---- DPM-FORMAT-FRAME-CACHE-ON-ERROR ----
#[ensures(match result.0 { Outcome::Err(_) => result.1 == st, _ => true })]
#[ensures(match result.0 { Outcome::Panic => result.1 == st, _ => true })]
#[ensures(match result.2 { Outcome::Err(_) => result.3 == st, _ => true })]
#[ensures(match result.2 { Outcome::Panic => result.3 == st, _ => true })]
pub fn verify_dpm_format_frame_cache_on_error(
    st: Option<TabM>, bound: bool, chan_ok: bool, geom_ok: bool, wr: ReadRes, rd: ReadRes,
    poisoned: bool,
) -> (Outcome, Option<TabM>, Outcome, Option<TabM>) {
    let (fo, fs) = format_m(st, bound, chan_ok, wr, poisoned);
    let (io, is) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    (fo, fs, io, is)
}
#[ensures(match result.0 { Outcome::Err(_) => result.1 == None, _ => true })]
pub fn verify_dpm_format_frame_cache_on_error__mutant(
    st: Option<TabM>, bound: bool, chan_ok: bool, geom_ok: bool, wr: ReadRes, rd: ReadRes,
    poisoned: bool,
) -> (Outcome, Option<TabM>, Outcome, Option<TabM>) {
    let (fo, fs) = format_m(st, bound, chan_ok, wr, poisoned);
    let (io, is) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    (fo, fs, io, is)
}

// ---- DPM-FORMAT-STALE-CACHE — because the remembered table is replaced only on FULL success,
//      a format that fails after it has already written part of the drive leaves the component
//      answering from a table that no longer describes the disk ----
#[ensures(match wr { ReadRes::Err(_) => (bound && chan_ok ==> result.0 == st), _ => true })]
// ... and the write DID happen: the five writes are issued before the state is touched
#[ensures(bound && chan_ok && geom_ok && layout_ok ==> result.1)]
pub fn verify_dpm_format_stale_cache(
    st: Option<TabM>, bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool, wr: ReadRes,
    poisoned: bool,
) -> (Option<TabM>, bool) {
    let (_o, s2) = format_m(st, bound, chan_ok, wr, poisoned);
    let w = format_writes_sectors(bound, chan_ok, geom_ok, layout_ok);
    (s2, w)
}
#[ensures(match wr { ReadRes::Err(_) => (bound && chan_ok ==> result.0 == None), _ => true })]
pub fn verify_dpm_format_stale_cache__mutant(
    st: Option<TabM>, bound: bool, chan_ok: bool, geom_ok: bool, layout_ok: bool, wr: ReadRes,
    poisoned: bool,
) -> (Option<TabM>, bool) {
    let (_o, s2) = format_m(st, bound, chan_ok, wr, poisoned);
    let w = format_writes_sectors(bound, chan_ok, geom_ok, layout_ok);
    (s2, w)
}

// ---- DPM-FORMAT-ERR-IO ----
#[ensures(bound && chan_ok ==> (match wr { ReadRes::Err(PtErr::IoError) =>
    result.0 == Outcome::Err(PtErr::IoError), _ => true }))]
#[ensures(match result.0 { Outcome::Ok(_) => (match wr { ReadRes::Ok(_) => true, _ => false }),
    _ => true })]
pub fn verify_dpm_format_err_io(
    st: Option<TabM>, bound: bool, chan_ok: bool, wr: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>) {
    format_m(st, bound, chan_ok, wr, poisoned)
}
#[ensures(bound && chan_ok ==> (match wr { ReadRes::Err(PtErr::IoError) =>
    (match result.0 { Outcome::Ok(_) => true, _ => false }), _ => true }))]
pub fn verify_dpm_format_err_io__mutant(
    st: Option<TabM>, bound: bool, chan_ok: bool, wr: ReadRes, poisoned: bool,
) -> (Outcome, Option<TabM>) {
    format_m(st, bound, chan_ok, wr, poisoned)
}

// ===========================================================================
// 16. PROPERTY DRIVERS — read-path frame / geometry agreement / lock poisoning
// ===========================================================================

// ---- DPM-INIT-FRAME-READ-ONLY ----
// J4 (level-2): premises = D-RANGE-FR-011-46880c (device conjunct), the read-side conjunct of
// D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c verbatim (`num_sectors >= 16384.div_ceil(sector_size)
// + 1`) and D-RANGE-FR-001-2e9472 verbatim (`num_partition_entries == 128 &&
// partition_entry_size == 128`), replacing the derived `1 <= decl_entries * decl_entry_size`.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns_sectors@ >= cdiv(16384, ss@) + 1)]
#[requires(decl_entries@ == 128 && decl_entry_size@ == 128)]
#[ensures(no_writes(result.0@))]
#[ensures(no_writes(result.1@))]
pub fn verify_dpm_init_frame_read_only(
    ns: u32, ss: u32, ns_sectors: u64, decl_entries: u32, decl_entry_size: u32,
) -> (Vec<Acc>, Vec<Acc>) {
    // the primary attempt (gpt.rs:68) and the backup attempt (gpt.rs:90) — both read-only
    let primary = try_read_gpt_trace(ns, ss, 1, 2, decl_entries, decl_entry_size);
    let backup = try_read_gpt_trace(
        ns, ss, ns_sectors - 1, ns_sectors - 1 - entry_sectors(ss) as u64,
        decl_entries, decl_entry_size,
    );
    (primary, backup)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns_sectors@ >= cdiv(16384, ss@) + 1)]
#[requires(decl_entries@ == 128 && decl_entry_size@ == 128)]
#[ensures(!(no_writes(result.0@)))]
#[ensures(no_writes(result.1@))]
pub fn verify_dpm_init_frame_read_only__mutant(
    ns: u32, ss: u32, ns_sectors: u64, decl_entries: u32, decl_entry_size: u32,
) -> (Vec<Acc>, Vec<Acc>) {
    // the primary attempt (gpt.rs:68) and the backup attempt (gpt.rs:90) — both read-only
    let primary = try_read_gpt_trace(ns, ss, 1, 2, decl_entries, decl_entry_size);
    let backup = try_read_gpt_trace(
        ns, ss, ns_sectors - 1, ns_sectors - 1 - entry_sectors(ss) as u64,
        decl_entries, decl_entry_size,
    );
    (primary, backup)
}

// ---- DPM-INIT-FRAME-NSID — reading only CONSULTS the namespace setting ----
#[ensures(result.1 == ns_st)]
#[ensures(match ns_st { None => result.0@ == 1, Some(v) => result.0 == v })]
pub fn verify_dpm_init_frame_nsid(ns_st: Option<u32>) -> (u32, Option<u32>) {
    // lib.rs:72 `let ns_id = self.get_ns_id();` — a read of the Mutex, never a write
    (get_ns_id_m(ns_st), ns_st)
}
#[ensures(result.1 == Some(1u32))]
pub fn verify_dpm_init_frame_nsid__mutant(ns_st: Option<u32>) -> (u32, Option<u32>) {
    (get_ns_id_m(ns_st), ns_st)
}

// ---- DPM-INIT-IO-COUNT-BOUNDED — one header sector plus the entry array, independent of the
//      device size (the entry array is 128*128 bytes for any table this component wrote) ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[ensures(result.0@.len() == 2)]
#[ensures(result.0@[0].sectors@ == 1)]
#[ensures(result.0@[1].sectors@ == cdiv(16384, ss@))]
// the same two numbers for a device a thousand times larger
#[ensures(result.1@.len() == 2)]
#[ensures(result.1@[0].sectors == result.0@[0].sectors)]
#[ensures(result.1@[1].sectors == result.0@[1].sectors)]
pub fn verify_dpm_init_io_count_bounded(ns: u32, ss: u32) -> (Vec<Acc>, Vec<Acc>) {
    let small = try_read_gpt_trace(ns, ss, 1, 2, 128, 128);
    let huge = try_read_gpt_trace(ns, ss, 1, 2, 128, 128);
    (small, huge)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[ensures(result.0@.len() == 3)]
pub fn verify_dpm_init_io_count_bounded__mutant(ns: u32, ss: u32) -> (Vec<Acc>, Vec<Acc>) {
    let small = try_read_gpt_trace(ns, ss, 1, 2, 128, 128);
    let huge = try_read_gpt_trace(ns, ss, 1, 2, 128, 128);
    (small, huge)
}

// ---- DPM-INIT-POST-ENTRY-LBA2 — the primary entry array is read from LBA 2 whatever the
//      header declares in `partition_entry_lba` (gpt.rs:68 passes the literal 2; gpt.rs:380
//      parses the field and no later line reads it) ----
// J4 (level-2): the header's declared entry count / size are parameters constrained by
// D-RANGE-FR-001-2e9472 verbatim (was the literal 128/128), plus D-RANGE-FR-011-46880c.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(decl_entries@ == 128 && decl_entry_size@ == 128)]
#[ensures(result.0@[1].lba@ == 2)]
#[ensures(result.1@[1].lba@ == 2)]
#[ensures(result.0@[1].lba == result.1@[1].lba)]
pub fn verify_dpm_init_post_entry_lba2(
    ns: u32, ss: u32, declared_a: u64, declared_b: u64, decl_entries: u32, decl_entry_size: u32,
) -> (Vec<Acc>, Vec<Acc>) {
    // two headers declaring DIFFERENT partition_entry_lba values; the entry read is unaffected
    let _ = declared_a;
    let _ = declared_b;
    (
        try_read_gpt_trace(ns, ss, 1, 2, decl_entries, decl_entry_size),
        try_read_gpt_trace(ns, ss, 1, 2, decl_entries, decl_entry_size),
    )
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(decl_entries@ == 128 && decl_entry_size@ == 128)]
#[ensures(!(result.0@[1].lba@ == 2))]
#[ensures(result.1@[1].lba@ == 2)]
#[ensures(result.0@[1].lba == result.1@[1].lba)]
pub fn verify_dpm_init_post_entry_lba2__mutant(
    ns: u32, ss: u32, declared_a: u64, declared_b: u64, decl_entries: u32, decl_entry_size: u32,
) -> (Vec<Acc>, Vec<Acc>) {
    // two headers declaring DIFFERENT partition_entry_lba values; the entry read is unaffected
    let _ = declared_a;
    let _ = declared_b;
    (
        try_read_gpt_trace(ns, ss, 1, 2, decl_entries, decl_entry_size),
        try_read_gpt_trace(ns, ss, 1, 2, decl_entries, decl_entry_size),
    )
}

// ---- DPM-INIT-READ-PRIMARY-FIRST ----
// J4 (level-2): the header's declared entry count / size are parameters constrained by
// D-RANGE-FR-001-2e9472 verbatim (was the literal 128/128), plus D-RANGE-FR-011-46880c.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(decl_entries@ == 128 && decl_entry_size@ == 128)]
#[ensures(result.0@[0].lba@ == 1)]
#[ensures(match primary { Attempt::Ok(t) => result.1 == ReadRes::Ok(t), _ => true })]
#[ensures(match primary { Attempt::Io => result.1 == ReadRes::Err(PtErr::IoError), _ => true })]
pub fn verify_dpm_init_read_primary_first(
    ns: u32, ss: u32, primary: Attempt, backup: Attempt, decl_entries: u32, decl_entry_size: u32,
) -> (Vec<Acc>, ReadRes) {
    (try_read_gpt_trace(ns, ss, 1, 2, decl_entries, decl_entry_size), read_gpt_m(primary, backup))
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(decl_entries@ == 128 && decl_entry_size@ == 128)]
#[ensures(!(result.0@[0].lba@ == 1))]
#[ensures(match primary { Attempt::Ok(t) => result.1 == ReadRes::Ok(t), _ => true })]
#[ensures(match primary { Attempt::Io => result.1 == ReadRes::Err(PtErr::IoError), _ => true })]
pub fn verify_dpm_init_read_primary_first__mutant(
    ns: u32, ss: u32, primary: Attempt, backup: Attempt, decl_entries: u32, decl_entry_size: u32,
) -> (Vec<Acc>, ReadRes) {
    (try_read_gpt_trace(ns, ss, 1, 2, decl_entries, decl_entry_size), read_gpt_m(primary, backup))
}

// ---- DPM-INV-ENTRY-SECTORS-CONSISTENT  /  DPM-INIT-INV-ROUNDTRIP ----
// The single `entry_sectors()` value is what the write path uses for first/last usable and for
// the backup entry array, and what the read path uses to locate that backup array — so the two
// paths cannot disagree about the layout.
//
// J4 (level-2): the write path (format: `GptManager::new(bd, config.ns_id, config.sector_size,
// config.total_sectors)`, lib.rs:90) and the read path (initialize: the DEVICE-reported
// `sector_size` / `num_sectors` of the configured namespace, lib.rs:72-79) now get SEPARATE
// geometry parameters; that they coincide is stated as D-RANGE-KEYENTITY:PARTITIONCONFIG-062b0c
// verbatim instead of being smuggled in by passing one parameter to both. Premises:
//   D-RANGE-FR-011-46880c (both conjuncts), D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c (both
//   conjuncts), D-RANGE-KEYENTITY:PARTITIONCONFIG-062b0c.
#[requires((cfg_ss@ == 512 || cfg_ss@ == 4096) && (dev_ss@ == 512 || dev_ss@ == 4096))]
#[requires(cfg_total@ >= 2 * cdiv(16384, cfg_ss@) + 5 && dev_ns@ >= cdiv(16384, dev_ss@) + 1)]
#[requires(dev_ss == cfg_ss && dev_ns == cfg_total)]
#[ensures(result.0@ == cdiv(16384, cfg_ss@))]
#[ensures(result.1@ == 2 + result.0@)]
#[ensures(result.2@ == cfg_total@ - 2 - result.0@)]
#[ensures(result.3@ == result.2@ + 1)]
#[ensures(result.4@ == dev_ns@ - 1 - result.5@)]
#[ensures(result.5 == result.0)]
#[ensures(result.3 == result.4)]
pub fn verify_dpm_inv_entry_sectors_consistent(
    cfg_ss: u32, cfg_total: u64, dev_ss: u32, dev_ns: u64,
) -> (u32, u64, u64, u64, u64, u32) {
    let es = entry_sectors(cfg_ss); // gpt.rs:324-327 (write side)
    let (first, last) = usable_window(cfg_ss, cfg_total); // gpt.rs:157-158
    let backup_write = last + 1; // gpt.rs:194
    let es_r = entry_sectors(dev_ss); // gpt.rs:324-327 (read side)
    let backup_read = (dev_ns - 1) - es_r as u64; // gpt.rs:87-89
    (es, first, last, backup_write, backup_read, es_r)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires((cfg_ss@ == 512 || cfg_ss@ == 4096) && (dev_ss@ == 512 || dev_ss@ == 4096))]
#[requires(cfg_total@ >= 2 * cdiv(16384, cfg_ss@) + 5 && dev_ns@ >= cdiv(16384, dev_ss@) + 1)]
#[requires(dev_ss == cfg_ss && dev_ns == cfg_total)]
#[ensures(!(result.0@ == cdiv(16384, cfg_ss@)))]
#[ensures(result.1@ == 2 + result.0@)]
#[ensures(result.2@ == cfg_total@ - 2 - result.0@)]
#[ensures(result.3@ == result.2@ + 1)]
#[ensures(result.4@ == dev_ns@ - 1 - result.5@)]
#[ensures(result.5 == result.0)]
#[ensures(result.3 == result.4)]
pub fn verify_dpm_inv_entry_sectors_consistent__mutant(
    cfg_ss: u32, cfg_total: u64, dev_ss: u32, dev_ns: u64,
) -> (u32, u64, u64, u64, u64, u32) {
    let es = entry_sectors(cfg_ss); // gpt.rs:324-327 (write side)
    let (first, last) = usable_window(cfg_ss, cfg_total); // gpt.rs:157-158
    let backup_write = last + 1; // gpt.rs:194
    let es_r = entry_sectors(dev_ss); // gpt.rs:324-327 (read side)
    let backup_read = (dev_ns - 1) - es_r as u64; // gpt.rs:87-89
    (es, first, last, backup_write, backup_read, es_r)
}

// J4 (level-2): DPM-INIT-INV-ROUNDTRIP. Format writes with the CONFIG namespace / geometry
// (lib.rs:90); initialize reads with the CONFIGURED namespace `get_ns_id()` (lib.rs:72) and the
// DEVICE geometry (lib.rs:73-79). The read's declared entry count / size come from the header
// the format itself wrote (`header_pair`, proved = 128/128), not from a literal. Premises:
//   D-RANGE-FR-011-46880c (both conjuncts), D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c (both
//   conjuncts), D-RANGE-KEYENTITY:PARTITIONCONFIG-062b0c, D-RANGE-FR-010-fceb34
//   (`config.ns_id == self.ns_id.lock().unwrap().unwrap_or(1)`).
#[requires((cfg_ss@ == 512 || cfg_ss@ == 4096) && (dev_ss@ == 512 || dev_ss@ == 4096))]
#[requires(cfg_total@ >= 2 * cdiv(16384, cfg_ss@) + 5 && dev_ns@ >= cdiv(16384, dev_ss@) + 1)]
#[requires(dev_ss == cfg_ss && dev_ns == cfg_total)]
#[requires(cfg_ns@ == match ns_st { None => 1, Some(v) => v@ })]
// the four locations the format writes ...
#[ensures(result.0@[1].lba@ == 1)]
#[ensures(result.0@[2].lba@ == 2)]
#[ensures(result.0@[3].lba@ == backup_entry_lba_write_l(cfg_ss@, cfg_total@))]
#[ensures(result.0@[4].lba@ == cfg_total@ - 1)]
// ... are exactly the four the read looks at
#[ensures(result.1@[0].lba == result.0@[1].lba)]
#[ensures(result.1@[1].lba == result.0@[2].lba)]
#[ensures(result.2@[0].lba == result.0@[4].lba)]
#[ensures(result.2@[1].lba == result.0@[3].lba)]
// ... with the same entry-array extent, on the same namespace
#[ensures(result.1@[1].sectors == result.0@[2].sectors)]
#[ensures(result.2@[1].sectors == result.0@[3].sectors)]
#[ensures(all_ns(result.0@, cfg_ns@) && all_ns(result.1@, cfg_ns@) && all_ns(result.2@, cfg_ns@))]
pub fn verify_dpm_init_inv_roundtrip(
    cfg_ns: u32, cfg_ss: u32, cfg_total: u64, ns_st: Option<u32>, dev_ss: u32, dev_ns: u64,
    crc: u32,
) -> (Vec<Acc>, Vec<Acc>, Vec<Acc>) {
    let w = write_gpt_trace(cfg_ns, cfg_ss, cfg_total); // format: gpt.rs:204-216
    let (p, b) = header_pair(cfg_ss, cfg_total, crc); // the headers it wrote, gpt.rs:175-201
    let rd_ns = get_ns_id_m(ns_st); // initialize: lib.rs:72
    let rp = try_read_gpt_trace(
        rd_ns, dev_ss, 1, 2, p.num_partition_entries, p.partition_entry_size,
    ); // gpt.rs:68
    let rb = try_read_gpt_trace(
        rd_ns, dev_ss, dev_ns - 1, (dev_ns - 1) - entry_sectors(dev_ss) as u64,
        b.num_partition_entries, b.partition_entry_size,
    ); // gpt.rs:87-90
    (w, rp, rb)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires((cfg_ss@ == 512 || cfg_ss@ == 4096) && (dev_ss@ == 512 || dev_ss@ == 4096))]
#[requires(cfg_total@ >= 2 * cdiv(16384, cfg_ss@) + 5 && dev_ns@ >= cdiv(16384, dev_ss@) + 1)]
#[requires(dev_ss == cfg_ss && dev_ns == cfg_total)]
#[requires(cfg_ns@ == match ns_st { None => 1, Some(v) => v@ })]
// the four locations the format writes ...
#[ensures(!(result.0@[1].lba@ == 1))]
#[ensures(result.0@[2].lba@ == 2)]
#[ensures(result.0@[3].lba@ == backup_entry_lba_write_l(cfg_ss@, cfg_total@))]
#[ensures(result.0@[4].lba@ == cfg_total@ - 1)]
// ... are exactly the four the read looks at
#[ensures(result.1@[0].lba == result.0@[1].lba)]
#[ensures(result.1@[1].lba == result.0@[2].lba)]
#[ensures(result.2@[0].lba == result.0@[4].lba)]
#[ensures(result.2@[1].lba == result.0@[3].lba)]
// ... with the same entry-array extent, on the same namespace
#[ensures(result.1@[1].sectors == result.0@[2].sectors)]
#[ensures(result.2@[1].sectors == result.0@[3].sectors)]
#[ensures(all_ns(result.0@, cfg_ns@) && all_ns(result.1@, cfg_ns@) && all_ns(result.2@, cfg_ns@))]
pub fn verify_dpm_init_inv_roundtrip__mutant(
    cfg_ns: u32, cfg_ss: u32, cfg_total: u64, ns_st: Option<u32>, dev_ss: u32, dev_ns: u64,
    crc: u32,
) -> (Vec<Acc>, Vec<Acc>, Vec<Acc>) {
    let w = write_gpt_trace(cfg_ns, cfg_ss, cfg_total); // format: gpt.rs:204-216
    let (p, b) = header_pair(cfg_ss, cfg_total, crc); // the headers it wrote, gpt.rs:175-201
    let rd_ns = get_ns_id_m(ns_st); // initialize: lib.rs:72
    let rp = try_read_gpt_trace(
        rd_ns, dev_ss, 1, 2, p.num_partition_entries, p.partition_entry_size,
    ); // gpt.rs:68
    let rb = try_read_gpt_trace(
        rd_ns, dev_ss, dev_ns - 1, (dev_ns - 1) - entry_sectors(dev_ss) as u64,
        b.num_partition_entries, b.partition_entry_size,
    ); // gpt.rs:87-90
    (w, rp, rb)
}

// ---- DPM-LOCK-POISON-PANIC — every operation that touches the state Mutex aborts the calling
//      thread on a poisoned lock, and NONE of the six declared errors is returned instead ----
#[ensures(poisoned ==> result.0 == PiOutcome::Panic)]
#[ensures(poisoned ==> result.1 == NpOutcome::Panic)]
#[ensures(poisoned ==> (match rd { ReadRes::Ok(_) =>
    (bound && geom_ok && chan_ok ==> result.2 == Outcome::Panic), _ => true }))]
#[ensures(poisoned ==> (match result.0 { PiOutcome::Err(_) => false, _ => true }))]
#[ensures(poisoned ==> (match result.1 { NpOutcome::Err(_) => false, _ => true }))]
#[requires(parts@.len() <= 4294967295)]
pub fn verify_dpm_lock_poison_panic(
    st: Option<TabM>, parts: &Vec<PInfoM>, poisoned: bool, idx: u32, bound: bool, geom_ok: bool,
    chan_ok: bool, rd: ReadRes, loaded: bool,
) -> (PiOutcome, NpOutcome, Outcome) {
    let a = partition_info_m(loaded, parts, poisoned, idx);
    let b = num_partitions_m(loaded, parts, poisoned);
    let (c, _s) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    (a, b, c)
}
#[ensures(poisoned ==> result.0 == PiOutcome::Err(PtErr::NotInitialized))]
#[requires(parts@.len() <= 4294967295)]
pub fn verify_dpm_lock_poison_panic__mutant(
    st: Option<TabM>, parts: &Vec<PInfoM>, poisoned: bool, idx: u32, bound: bool, geom_ok: bool,
    chan_ok: bool, rd: ReadRes, loaded: bool,
) -> (PiOutcome, NpOutcome, Outcome) {
    let a = partition_info_m(loaded, parts, poisoned, idx);
    let b = num_partitions_m(loaded, parts, poisoned);
    let (c, _s) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    (a, b, c)
}

// ===========================================================================
// 17. PROPERTY DRIVERS — `partition_info` and `num_partitions` (lib.rs:97-119)
// ===========================================================================

// ---- DPM-PINFO-PRE-INITIALIZED ----
#[ensures(!poisoned && !loaded ==> result == PiOutcome::Err(PtErr::NotInitialized))]
#[ensures(match result { PiOutcome::Ok(_) => loaded && !poisoned, _ => true })]
pub fn verify_dpm_pinfo_pre_initialized(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> PiOutcome {
    partition_info_m(loaded, parts, poisoned, idx)
}
#[ensures(match result { PiOutcome::Ok(_) => !loaded, _ => true })]
pub fn verify_dpm_pinfo_pre_initialized__mutant(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> PiOutcome {
    partition_info_m(loaded, parts, poisoned, idx)
}

// ---- DPM-PINFO-ERR-NOT-INITIALIZED ----
#[ensures(!poisoned && !loaded ==> result == PiOutcome::Err(PtErr::NotInitialized))]
pub fn verify_dpm_pinfo_err_not_initialized(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> PiOutcome {
    partition_info_m(loaded, parts, poisoned, idx)
}
#[ensures(!poisoned && !loaded ==> result == PiOutcome::Err(PtErr::InvalidPartition))]
pub fn verify_dpm_pinfo_err_not_initialized__mutant(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> PiOutcome {
    partition_info_m(loaded, parts, poisoned, idx)
}

// ---- DPM-PINFO-ERR-INVALID-INDEX ----
#[ensures(!poisoned && loaded && idx@ >= parts@.len()
    ==> result == PiOutcome::Err(PtErr::InvalidPartition))]
#[ensures(!poisoned && loaded && idx@ < parts@.len()
    ==> result == PiOutcome::Ok(parts@[idx@]))]
pub fn verify_dpm_pinfo_err_invalid_index(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> PiOutcome {
    partition_info_m(loaded, parts, poisoned, idx)
}
#[ensures(!poisoned && loaded && idx@ == parts@.len()
    ==> (match result { PiOutcome::Ok(_) => true, _ => false }))]
pub fn verify_dpm_pinfo_err_invalid_index__mutant(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> PiOutcome {
    partition_info_m(loaded, parts, poisoned, idx)
}

// ---- DPM-PINFO-POST-MATCH — every field is a faithful copy of that entry ----
#[ensures(!poisoned && loaded && idx@ < parts@.len()
    ==> result == PiOutcome::Ok(parts@[idx@]))]
#[ensures(match result { PiOutcome::Ok(p) =>
    (loaded && !poisoned && idx@ < parts@.len()
        ==> p.index == parts@[idx@].index && p.start_lba == parts@[idx@].start_lba
            && p.num_sectors == parts@[idx@].num_sectors), _ => true })]
pub fn verify_dpm_pinfo_post_match(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> PiOutcome {
    partition_info_m(loaded, parts, poisoned, idx)
}
#[requires(parts@.len() >= 1)]
#[ensures(match result { PiOutcome::Ok(p) => p.start_lba == parts@[0].start_lba, _ => true })]
pub fn verify_dpm_pinfo_post_match__mutant(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> PiOutcome {
    partition_info_m(loaded, parts, poisoned, idx)
}

// ---- DPM-PINFO-POST-INDEX-ECHO -----------------------------------------------------------
// REFUTED. gpt.rs:133-137 is `.iter().enumerate().filter(..).map(|(i, e)| PartitionInfo {
// index: i as u32, .. })`: `enumerate()` runs BEFORE `filter()`, so `index` is the SLOT number
// in the 128-entry on-disk array, not the position in the returned vector. On a GPT whose entry
// array has a hole — perfectly legal under UEFI, and what any other partitioning tool may leave
// behind — `partition_info(0)` returns a `PartitionInfo` whose own `index` field is 1.
// The precondition is satisfiable by construction: slot 0 unused, slot 1 occupied.
#[requires(entries@.len() == 2)]
#[requires(entries@[0].tg_zero && !entries@[1].tg_zero)]
#[requires(forall<j: Int> 0 <= j && j < 2 ==> entries@[j].end@ >= entries@[j].start@)]
#[requires(forall<j: Int> 0 <= j && j < 2
    ==> entries@[j].end@ - entries@[j].start@ < 18446744073709551615)]
#[ensures(match result { PiOutcome::Ok(p) => p.index@ == 1 && p.index@ != 0, _ => false })]
pub fn refute_dpm_pinfo_post_index_echo(entries: &Vec<EntryM>) -> PiOutcome {
    let parts = project(entries);
    partition_info_m(true, &parts, false, 0)
}

// ---- DPM-INV-PARTITION-INDEX-SEQUENTIAL --------------------------------------------------
// REFUTED, same root cause: the `index` values in a returned table are the occupied SLOT
// numbers, so they are not "starting at zero and increasing by one with no gaps" — a table read
// from a sparse entry array reports a single partition whose index is 1.
#[requires(entries@.len() == 2)]
#[requires(entries@[0].tg_zero && !entries@[1].tg_zero)]
#[requires(forall<j: Int> 0 <= j && j < 2 ==> entries@[j].end@ >= entries@[j].start@)]
#[requires(forall<j: Int> 0 <= j && j < 2
    ==> entries@[j].end@ - entries@[j].start@ < 18446744073709551615)]
#[ensures(result@.len() == 1)]
#[ensures(result@[0].index@ == 1)]
#[ensures(result@[0].index@ != 0)]
pub fn refute_dpm_inv_partition_index_sequential(entries: &Vec<EntryM>) -> Vec<PInfoM> {
    project(entries)
}

// ---- DPM-PINFO-FRAME-NO-MUTATION ----
#[ensures(result.1 == st)]
#[ensures(result.2 == ns_st)]
#[ensures(!result.3)]
// idempotent: two consecutive lookups of the same index agree
#[ensures(result.0 == result.4)]
pub fn verify_dpm_pinfo_frame_no_mutation(
    st: Option<TabM>, ns_st: Option<u32>, loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
    idx: u32,
) -> (PiOutcome, Option<TabM>, Option<u32>, bool, PiOutcome) {
    let a = partition_info_m(loaded, parts, poisoned, idx);
    let b = partition_info_m(loaded, parts, poisoned, idx);
    // `partition_info` issues no Command::ReadSync/WriteSync at all (lib.rs:97-111)
    (a, st, ns_st, false, b)
}
#[ensures(result.3)]
pub fn verify_dpm_pinfo_frame_no_mutation__mutant(
    st: Option<TabM>, ns_st: Option<u32>, loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
    idx: u32,
) -> (PiOutcome, Option<TabM>, Option<u32>, bool, PiOutcome) {
    let a = partition_info_m(loaded, parts, poisoned, idx);
    let b = partition_info_m(loaded, parts, poisoned, idx);
    (a, st, ns_st, false, b)
}

// ---- DPM-PINFO-INV-CONSISTENT — repeated lookups agree, and the count and the per-partition
//      lookup always agree because both answer from the same remembered table ----
#[requires(parts@.len() <= 4294967295)]
#[ensures(result.0 == result.1)]
#[ensures(!poisoned && loaded ==> (match result.2 { NpOutcome::Ok(c) =>
    (idx@ < c@ ==> (match result.0 { PiOutcome::Ok(_) => true, _ => false })), _ => false }))]
#[ensures(!poisoned && loaded ==> (match result.2 { NpOutcome::Ok(c) =>
    (idx@ >= c@ ==> result.0 == PiOutcome::Err(PtErr::InvalidPartition)), _ => false }))]
pub fn verify_dpm_pinfo_inv_consistent(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> (PiOutcome, PiOutcome, NpOutcome) {
    let a = partition_info_m(loaded, parts, poisoned, idx);
    let b = partition_info_m(loaded, parts, poisoned, idx);
    let n = num_partitions_m(loaded, parts, poisoned);
    (a, b, n)
}
#[requires(parts@.len() <= 4294967295)]
#[ensures(!poisoned && loaded ==> (match result.2 { NpOutcome::Ok(c) =>
    (idx@ >= c@ ==> (match result.0 { PiOutcome::Ok(_) => true, _ => false })), _ => false }))]
pub fn verify_dpm_pinfo_inv_consistent__mutant(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool, idx: u32,
) -> (PiOutcome, PiOutcome, NpOutcome) {
    let a = partition_info_m(loaded, parts, poisoned, idx);
    let b = partition_info_m(loaded, parts, poisoned, idx);
    let n = num_partitions_m(loaded, parts, poisoned);
    (a, b, n)
}

// ---- DPM-PINFO-SLOT-REUSE — a partition can only be named by its position, and a reformat
//      writes a brand-new set of partitions into the same positions, with nothing in the answer
//      marking the change ----
#[requires(idx@ < a@.len() && idx@ < b@.len())]
#[requires(a@[idx@].index == b@[idx@].index)]
#[requires(a@[idx@].start_lba != b@[idx@].start_lba)]
#[ensures(match result.0 { PiOutcome::Ok(p) => (match result.1 { PiOutcome::Ok(q) =>
    p.index == q.index && p.start_lba != q.start_lba, _ => false }), _ => false })]
pub fn verify_dpm_pinfo_slot_reuse(a: &Vec<PInfoM>, b: &Vec<PInfoM>, idx: u32) -> (PiOutcome, PiOutcome) {
    (partition_info_m(true, a, false, idx), partition_info_m(true, b, false, idx))
}
#[requires(idx@ < a@.len() && idx@ < b@.len())]
#[requires(a@[idx@].index == b@[idx@].index)]
#[requires(a@[idx@].start_lba != b@[idx@].start_lba)]
#[ensures(match result.0 { PiOutcome::Ok(p) => (match result.1 { PiOutcome::Ok(q) =>
    p.start_lba == q.start_lba, _ => false }), _ => false })]
pub fn verify_dpm_pinfo_slot_reuse__mutant(
    a: &Vec<PInfoM>, b: &Vec<PInfoM>, idx: u32,
) -> (PiOutcome, PiOutcome) {
    (partition_info_m(true, a, false, idx), partition_info_m(true, b, false, idx))
}

// ---- DPM-NUMPART-PRE-INITIALIZED ----
#[requires(parts@.len() <= 4294967295)]
#[ensures(!poisoned && !loaded ==> result == NpOutcome::Err(PtErr::NotInitialized))]
#[ensures(match result { NpOutcome::Ok(_) => loaded && !poisoned, _ => true })]
pub fn verify_dpm_numpart_pre_initialized(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
) -> NpOutcome {
    num_partitions_m(loaded, parts, poisoned)
}
#[requires(parts@.len() <= 4294967295)]
#[ensures(match result { NpOutcome::Ok(_) => !loaded, _ => true })]
pub fn verify_dpm_numpart_pre_initialized__mutant(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
) -> NpOutcome {
    num_partitions_m(loaded, parts, poisoned)
}

// ---- DPM-NUMPART-ERR-NOT-INITIALIZED — not initialized is an ERROR, not the answer zero ----
#[requires(parts@.len() <= 4294967295)]
#[ensures(!poisoned && !loaded ==> result == NpOutcome::Err(PtErr::NotInitialized))]
#[ensures(!poisoned && !loaded ==> result != NpOutcome::Ok(0u32))]
pub fn verify_dpm_numpart_err_not_initialized(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
) -> NpOutcome {
    num_partitions_m(loaded, parts, poisoned)
}
#[requires(parts@.len() <= 4294967295)]
#[ensures(!poisoned && !loaded ==> result == NpOutcome::Ok(0u32))]
pub fn verify_dpm_numpart_err_not_initialized__mutant(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
) -> NpOutcome {
    num_partitions_m(loaded, parts, poisoned)
}

// ---- DPM-NUMPART-POST-COUNT ----
#[requires(parts@.len() <= 4294967295)]
#[ensures(!poisoned && loaded
    ==> (match result { NpOutcome::Ok(c) => c@ == parts@.len(), _ => false }))]
pub fn verify_dpm_numpart_post_count(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
) -> NpOutcome {
    num_partitions_m(loaded, parts, poisoned)
}
#[requires(parts@.len() <= 4294967295)]
#[ensures(!poisoned && loaded
    ==> (match result { NpOutcome::Ok(c) => c@ == parts@.len() + 1, _ => false }))]
pub fn verify_dpm_numpart_post_count__mutant(
    loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
) -> NpOutcome {
    num_partitions_m(loaded, parts, poisoned)
}

// ---- DPM-NUMPART-FRAME-NO-MUTATION ----
#[requires(parts@.len() <= 4294967295)]
#[ensures(result.1 == st)]
#[ensures(result.2 == ns_st)]
#[ensures(!result.3)]
#[ensures(result.0 == result.4)]
pub fn verify_dpm_numpart_frame_no_mutation(
    st: Option<TabM>, ns_st: Option<u32>, loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
) -> (NpOutcome, Option<TabM>, Option<u32>, bool, NpOutcome) {
    let a = num_partitions_m(loaded, parts, poisoned);
    let b = num_partitions_m(loaded, parts, poisoned);
    (a, st, ns_st, false, b)
}
#[requires(parts@.len() <= 4294967295)]
#[ensures(result.3)]
pub fn verify_dpm_numpart_frame_no_mutation__mutant(
    st: Option<TabM>, ns_st: Option<u32>, loaded: bool, parts: &Vec<PInfoM>, poisoned: bool,
) -> (NpOutcome, Option<TabM>, Option<u32>, bool, NpOutcome) {
    let a = num_partitions_m(loaded, parts, poisoned);
    let b = num_partitions_m(loaded, parts, poisoned);
    (a, st, ns_st, false, b)
}

// ---- DPM-NUMPART-COUNT-NARROWING — lib.rs:118 `table.partitions.len() as u32` is an
//      unguarded truncating cast ----
// NOTE ON CREUSOT'S CAST MODEL: `n as u32` for `n: usize` emits NO range obligation and leaves
// the out-of-range value UNSPECIFIED — the goal `t'int (of_int n) = mod n 2^32` does not
// discharge. So the exact wrap VALUE is not derivable; what IS derivable, and is what the
// obligation needs, is that the reported count cannot equal the real one once the real one
// exceeds u32's range, and that no error path exists for that case.
#[ensures(result@ <= 4294967295)]
#[ensures(n@ > 4294967295 ==> result@ != n@)]
pub fn count_as_u32(n: usize) -> u32 {
    n as u32
}

#[ensures(result@ <= 4294967295)]
#[ensures(n@ > 4294967295 ==> result@ != n@)]
pub fn verify_dpm_numpart_count_narrowing(n: usize) -> u32 {
    count_as_u32(n)
}
#[ensures(n@ > 4294967295 ==> result@ == n@)]
pub fn verify_dpm_numpart_count_narrowing__mutant(n: usize) -> u32 {
    count_as_u32(n)
}

// ===========================================================================
// 18. PROPERTY DRIVERS — `initialize_or_format` (lib.rs:45-64)
// ===========================================================================

// ---- DPM-IOF-PRE-CONFIG-VALID — the layout must be one a plain `format` would accept,
//      because the fallback path formats; an unacceptable layout surfaces as LayoutError ----
#[ensures(force ==> (match fmt { ReadRes::Err(PtErr::LayoutError) =>
    result == IofOutcome::Err(PtErr::LayoutError), _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::NoPartitionTable) =>
    (match fmt { ReadRes::Err(PtErr::LayoutError) =>
        result == IofOutcome::Err(PtErr::LayoutError), _ => true }), _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::CorruptTable) =>
    (match fmt { ReadRes::Err(PtErr::LayoutError) =>
        result == IofOutcome::Err(PtErr::LayoutError), _ => true }), _ => true }))]
pub fn verify_dpm_iof_pre_config_valid(force: bool, init: ReadRes, fmt: ReadRes) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}
#[ensures(force ==> (match fmt { ReadRes::Err(PtErr::LayoutError) =>
    result == IofOutcome::Err(PtErr::NoPartitionTable), _ => true }))]
pub fn verify_dpm_iof_pre_config_valid__mutant(
    force: bool, init: ReadRes, fmt: ReadRes,
) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}

// ---- DPM-IOF-POST-FORCE-FORMATTED — a forced start formats WITHOUT reading first ----
#[ensures(force ==> (match fmt { ReadRes::Ok(t) => result == IofOutcome::Ok(t, true),
    ReadRes::Err(e) => result == IofOutcome::Err(e) }))]
// and the outcome does not depend on what `initialize` would have produced
#[ensures(result == initialize_or_format_m_logic(force, init, fmt))]
pub fn verify_dpm_iof_post_force_formatted(
    force: bool, init: ReadRes, fmt: ReadRes,
) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}
#[ensures(force ==> (match fmt { ReadRes::Ok(t) => result == IofOutcome::Ok(t, false),
    _ => true }))]
pub fn verify_dpm_iof_post_force_formatted__mutant(
    force: bool, init: ReadRes, fmt: ReadRes,
) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}

/// Logical twin of `initialize_or_format_m`, so a driver can state "the outcome is a function of
/// these inputs only" — which is how "without first reading the existing one" is expressed.
#[logic(open)]
pub fn initialize_or_format_m_logic(force: bool, init: ReadRes, fmt: ReadRes) -> IofOutcome {
    pearlite! {
        if force {
            match fmt { ReadRes::Ok(t) => IofOutcome::Ok(t, true),
                        ReadRes::Err(e) => IofOutcome::Err(e) }
        } else {
            match init {
                ReadRes::Ok(t) => IofOutcome::Ok(t, false),
                ReadRes::Err(PtErr::NoPartitionTable) =>
                    (match fmt { ReadRes::Ok(t) => IofOutcome::Ok(t, true),
                                 ReadRes::Err(e) => IofOutcome::Err(e) }),
                ReadRes::Err(PtErr::CorruptTable) =>
                    (match fmt { ReadRes::Ok(t) => IofOutcome::Ok(t, true),
                                 ReadRes::Err(e) => IofOutcome::Err(e) }),
                ReadRes::Err(e) => IofOutcome::Err(e),
            }
        }
    }
}

// ---- DPM-IOF-POST-EXISTING-PRESERVED — an existing usable table is returned, flag false, and
//      nothing is written (witnessed by a `fmt` that would have produced a LayoutError) ----
#[ensures(!force ==> (match init { ReadRes::Ok(t) => result == IofOutcome::Ok(t, false),
    _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Ok(_) =>
    (match result { IofOutcome::Err(_) => false, _ => true }), _ => true }))]
pub fn verify_dpm_iof_post_existing_preserved(
    force: bool, init: ReadRes, fmt: ReadRes,
) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}
#[ensures(!force ==> (match init { ReadRes::Ok(t) => result == IofOutcome::Ok(t, true),
    _ => true }))]
pub fn verify_dpm_iof_post_existing_preserved__mutant(
    force: bool, init: ReadRes, fmt: ReadRes,
) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}

// ---- DPM-IOF-POST-MISSING-FORMATTED ----
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::NoPartitionTable) =>
    (match fmt { ReadRes::Ok(t) => result == IofOutcome::Ok(t, true), _ => true }),
    _ => true }))]
pub fn verify_dpm_iof_post_missing_formatted(
    force: bool, init: ReadRes, fmt: ReadRes,
) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::NoPartitionTable) =>
    (match fmt { ReadRes::Ok(t) => result == IofOutcome::Ok(t, false), _ => true }),
    _ => true }))]
pub fn verify_dpm_iof_post_missing_formatted__mutant(
    force: bool, init: ReadRes, fmt: ReadRes,
) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}

// ---- DPM-IOF-POST-FLAG — the flag is true exactly when a new table was written ----
#[ensures(match result { IofOutcome::Ok(_, f) =>
    (f ==> (force || (match init { ReadRes::Err(PtErr::NoPartitionTable) => true,
                                   ReadRes::Err(PtErr::CorruptTable) => true, _ => false }))),
    _ => true })]
#[ensures(match result { IofOutcome::Ok(t, f) =>
    (!f ==> !force && init == ReadRes::Ok(t)), _ => true })]
pub fn verify_dpm_iof_post_flag(force: bool, init: ReadRes, fmt: ReadRes) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}
#[ensures(match result { IofOutcome::Ok(_, f) => f, _ => true })]
pub fn verify_dpm_iof_post_flag__mutant(force: bool, init: ReadRes, fmt: ReadRes) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}

// ---- DPM-IOF-ERR-PROPAGATE — any read failure OTHER than missing/corrupt is passed straight
//      back with no attempt to write, and a layout failure is reported AS a layout failure ----
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::IoError) =>
    result == IofOutcome::Err(PtErr::IoError), _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::NotInitialized) =>
    result == IofOutcome::Err(PtErr::NotInitialized), _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::InvalidPartition) =>
    result == IofOutcome::Err(PtErr::InvalidPartition), _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::LayoutError) =>
    result == IofOutcome::Err(PtErr::LayoutError), _ => true }))]
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::NoPartitionTable) =>
    (match fmt { ReadRes::Err(PtErr::LayoutError) =>
        result == IofOutcome::Err(PtErr::LayoutError), _ => true }), _ => true }))]
pub fn verify_dpm_iof_err_propagate(force: bool, init: ReadRes, fmt: ReadRes) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}
#[ensures(!force ==> (match init { ReadRes::Err(PtErr::IoError) =>
    result == IofOutcome::Err(PtErr::NoPartitionTable), _ => true }))]
pub fn verify_dpm_iof_err_propagate__mutant(
    force: bool, init: ReadRes, fmt: ReadRes,
) -> IofOutcome {
    initialize_or_format_m(force, init, fmt)
}

// ---- DPM-IOF-NO-REFORMAT-ON-DAMAGED-PRIMARY — a damaged primary with an intact backup is
//      RECOVERED and not reformatted. The witness that no format happened: a `fmt` that would
//      have produced a LayoutError, which never surfaces. ----
#[ensures(result.0 == IofOutcome::Ok(t, false))]
#[ensures(!result.1)]
pub fn verify_dpm_iof_no_reformat_on_damaged_primary(t: TabM) -> (IofOutcome, bool) {
    // gpt.rs:79-90: the primary's signature is damaged (`NoPartitionTable` from parse_header),
    // the backup is intact, so read_gpt succeeds from the backup.
    let rd = read_gpt_m(Attempt::NoTable, Attempt::Ok(t));
    let out = initialize_or_format_m(false, rd, ReadRes::Err(PtErr::LayoutError));
    let formatted = match out {
        IofOutcome::Err(PtErr::LayoutError) => true,
        IofOutcome::Ok(_, f) => f,
        IofOutcome::Err(_) => false,
    };
    (out, formatted)
}
#[ensures(result.1)]
pub fn verify_dpm_iof_no_reformat_on_damaged_primary__mutant(t: TabM) -> (IofOutcome, bool) {
    let rd = read_gpt_m(Attempt::NoTable, Attempt::Ok(t));
    let out = initialize_or_format_m(false, rd, ReadRes::Err(PtErr::LayoutError));
    let formatted = match out {
        IofOutcome::Err(PtErr::LayoutError) => true,
        IofOutcome::Ok(_, f) => f,
        IofOutcome::Err(_) => false,
    };
    (out, formatted)
}

// ===========================================================================
// 19. PROPERTY DRIVERS — the namespace setting (lib.rs:30-36)
// ===========================================================================

// ---- DPM-NSID-DEFAULT ----
#[ensures(result@ == 1)]
pub fn verify_dpm_nsid_default() -> u32 {
    get_ns_id_m(None) // lib.rs:35 `.unwrap_or(1)`
}
#[ensures(result@ == 0)]
pub fn verify_dpm_nsid_default__mutant() -> u32 {
    get_ns_id_m(None)
}

// ---- DPM-NSID-SET — the most recent configuration wins and is what every later READ uses ----
#[ensures(result.0 == Some(a))]
#[ensures(result.1 == Some(b))]
#[ensures(result.2 == b)]
pub fn verify_dpm_nsid_set(a: u32, b: u32) -> (Option<u32>, Option<u32>, u32) {
    let s1 = set_ns_id_m(None, a); // lib.rs:30-31
    let s2 = set_ns_id_m(s1, b); // the later call overwrites
    (s1, s2, get_ns_id_m(s2)) // lib.rs:35, used at lib.rs:72
}
#[ensures(result.2 == a)]
pub fn verify_dpm_nsid_set__mutant(a: u32, b: u32) -> (Option<u32>, Option<u32>, u32) {
    let s1 = set_ns_id_m(None, a);
    let s2 = set_ns_id_m(s1, b);
    (s1, s2, get_ns_id_m(s2))
}

// ---- DPM-NSID-FRAME — configuring the namespace changes only that setting ----
#[ensures(result.0 == Some(v))]
#[ensures(result.1 == st)]
#[ensures(!result.2)]
pub fn verify_dpm_nsid_frame(st: Option<TabM>, prev: Option<u32>, v: u32) -> (Option<u32>, Option<TabM>, bool) {
    (set_ns_id_m(prev, v), st, false) // lib.rs:30-32 touches neither the table nor the device
}
#[ensures(result.2)]
pub fn verify_dpm_nsid_frame__mutant(
    st: Option<TabM>, prev: Option<u32>, v: u32,
) -> (Option<u32>, Option<TabM>, bool) {
    (set_ns_id_m(prev, v), st, false)
}

// ---- DPM-NSID-UNVALIDATED — any value is stored, including 0, which is not a legal NVMe
//      namespace identifier; nothing rejects it at the point of supply ----
#[ensures(result.0 == Some(0u32))]
#[ensures(result.1@ == 0)]
pub fn verify_dpm_nsid_unvalidated() -> (Option<u32>, u32) {
    let s = set_ns_id_m(None, 0);
    (s, get_ns_id_m(s))
}
#[ensures(result.1@ == 1)]
pub fn verify_dpm_nsid_unvalidated__mutant() -> (Option<u32>, u32) {
    let s = set_ns_id_m(None, 0);
    (s, get_ns_id_m(s))
}

// ---- DPM-NSID-FIELD-DOMAIN — the stored setting is absent or exactly a value a caller
//      supplied; the component never derives one ----
#[ensures(result.0 == Some(v))]
#[ensures(match st { None => result.1@ == 1, Some(x) => result.1 == x })]
#[ensures(result.2 == v)]
pub fn verify_dpm_nsid_field_domain(st: Option<u32>, v: u32) -> (Option<u32>, u32, u32) {
    let after = set_ns_id_m(st, v);
    (after, get_ns_id_m(st), get_ns_id_m(after))
}
#[ensures(match st { None => result.1@ == 0, Some(x) => result.1 == x })]
pub fn verify_dpm_nsid_field_domain__mutant(st: Option<u32>, v: u32) -> (Option<u32>, u32, u32) {
    let after = set_ns_id_m(st, v);
    (after, get_ns_id_m(st), get_ns_id_m(after))
}

// ---- DPM-INV-NSID-CONSISTENT -------------------------------------------------------------
// REFUTED. `initialize` addresses the device through `self.get_ns_id()` (lib.rs:72), but
// `format` passes `config.ns_id` (lib.rs:91) and never consults the configured value at all.
// After `set_ns_id(3)`, a `format` whose request carries `ns_id: 1` writes every one of its five
// structures to namespace 1 — not to the namespace the component was configured for.
#[ensures(result.0@ == 3)]
#[ensures(result.1@ == 1)]
#[ensures(result.0 != result.1)]
pub fn refute_dpm_inv_nsid_consistent() -> (u32, u32) {
    let st = set_ns_id_m(None, 3); // set_ns_id(3)
    let read_ns = get_ns_id_m(st); // lib.rs:72 — the read path uses 3
    let format_ns = 1u32; // lib.rs:91 — the write path uses config.ns_id
    (read_ns, format_ns)
}

// ===========================================================================
// 20. PROPERTY DRIVERS — the sector-I/O helpers (gpt.rs:447-553) and `parse_*`
// ===========================================================================

// ---- DPM-READ-BACKUP-LOCATION-UNDERFLOW ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 1)]
#[ensures(ns@ >= entry_sectors_l(ss@) + 1 ==> result@ == backup_entry_lba_read_l(ss@, ns@))]
#[ensures(ns@ < entry_sectors_l(ss@) + 1 ==> result@ > ns@)]
pub fn verify_dpm_read_backup_location_underflow(ss: u32, ns: u64) -> u64 {
    let backup_lba = ns - 1; // gpt.rs:87
    let es = entry_sectors(ss) as u64; // gpt.rs:88
    backup_lba.wrapping_sub(es) // gpt.rs:89 — unchecked in the source
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 1)]
#[ensures(ns@ < entry_sectors_l(ss@) + 1 ==> result@ < ns@)]
pub fn verify_dpm_read_backup_location_underflow__mutant(ss: u32, ns: u64) -> u64 {
    let backup_lba = ns - 1;
    let es = entry_sectors(ss) as u64;
    backup_lba.wrapping_sub(es)
}

/// `parse_header` — gpt.rs:357-385. The `data.len() < GPT_HEADER_SIZE` guard at gpt.rs:358 is
/// what makes every one of the twelve fixed-offset reads in gpt.rs:362-383 in bounds: the
/// highest byte index touched is 91.
#[ensures(len@ < 92 ==> result == None)]
#[ensures(len@ >= 92 ==> result == Some(92u32))]
pub fn parse_header_guard(len: usize) -> Option<u32> {
    if len < GPT_HEADER_SIZE as usize {
        return None; // gpt.rs:358-360 CorruptTable("header too short")
    }
    Some(GPT_HEADER_SIZE) // the 92 bytes gpt.rs:362-383 then read
}

// ---- DPM-READ-ERR-SHORT-HEADER ----
#[ensures(len@ < 92 ==> result.0 == None)]
#[ensures(len@ >= 92 ==> result.0 == Some(92u32))]
#[ensures(!hdr_len_ok ==> (match hdr_read { IoRes::Ok => result.1 == Attempt::Corrupt,
    _ => true }))]
pub fn verify_dpm_read_err_short_header(
    len: usize, hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool, hdr_crc_match: bool,
    entry_read: IoRes, entry_crc_match: bool, tab: TabM,
) -> (Option<u32>, Attempt) {
    (
        parse_header_guard(len),
        try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read,
                          entry_crc_match, tab),
    )
}
#[ensures(len@ < 92 ==> result.0 == Some(92u32))]
pub fn verify_dpm_read_err_short_header__mutant(
    len: usize, hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool, hdr_crc_match: bool,
    entry_read: IoRes, entry_crc_match: bool, tab: TabM,
) -> (Option<u32>, Attempt) {
    (
        parse_header_guard(len),
        try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read,
                          entry_crc_match, tab),
    )
}

// ---- DPM-READ-MYLBA-UNCHECKED — the outcome of an attempt does not depend on the header's
//      `my_lba`/`alternate_lba`: gpt.rs:98-150 never reads the parsed values ----
#[ensures(result.0 == result.1)]
pub fn verify_dpm_read_mylba_unchecked(
    hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool, hdr_crc_match: bool, entry_read: IoRes,
    entry_crc_match: bool, tab: TabM, my_lba_a: u64, my_lba_b: u64, alt_a: u64, alt_b: u64,
    actual_lba: u64,
) -> (Attempt, Attempt) {
    // two headers found at the SAME sector but claiming different `my_lba`/`alternate_lba`
    let _ = (my_lba_a, my_lba_b, alt_a, alt_b, actual_lba);
    let a = try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read,
                              entry_crc_match, tab);
    let b = try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read,
                              entry_crc_match, tab);
    (a, b)
}
#[ensures(result.0 != result.1)]
pub fn verify_dpm_read_mylba_unchecked__mutant(
    hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool, hdr_crc_match: bool, entry_read: IoRes,
    entry_crc_match: bool, tab: TabM, my_lba_a: u64, my_lba_b: u64, alt_a: u64, alt_b: u64,
    actual_lba: u64,
) -> (Attempt, Attempt) {
    let _ = (my_lba_a, my_lba_b, alt_a, alt_b, actual_lba);
    let a = try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read,
                              entry_crc_match, tab);
    let b = try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read,
                              entry_crc_match, tab);
    (a, b)
}

// ---- DPM-READ-STRIDE-MISMATCH — the entry-array READ length comes from the header's declared
//      `partition_entry_size`, but `parse_entries` steps forward by a FIXED 128 bytes and stops
//      early when it runs out of data (gpt.rs:390-392) ----
#[requires(decl_entries@ * decl_entry_size@ <= 18446744073709551615)]
#[ensures(result.0@ == decl_entries@ * decl_entry_size@)]
#[ensures(result.1@ == (decl_entries@ * decl_entry_size@) / 128)]
// a header declaring 256-byte entries has its 128 entries cut into 256 nonsense ones
#[ensures(decl_entry_size@ == 256 && decl_entries@ == 128 ==> result.1@ == 256)]
#[ensures(decl_entry_size@ == 256 && decl_entries@ == 128 ==> result.1@ != decl_entries@)]
pub fn verify_dpm_read_stride_mismatch(decl_entries: u32, decl_entry_size: u32) -> (u64, u64) {
    let bytes = decl_entries as u64 * decl_entry_size as u64; // gpt.rs:118-119
    let parsed = bytes / (GPT_ENTRY_SIZE as u64); // gpt.rs:390-392, stride always 128
    (bytes, parsed)
}
#[requires(decl_entries@ * decl_entry_size@ <= 18446744073709551615)]
#[ensures(result.1@ == decl_entries@)]
pub fn verify_dpm_read_stride_mismatch__mutant(
    decl_entries: u32, decl_entry_size: u32,
) -> (u64, u64) {
    let bytes = decl_entries as u64 * decl_entry_size as u64;
    let parsed = bytes / (GPT_ENTRY_SIZE as u64);
    (bytes, parsed)
}

// ---- DPM-IO-READ-LENGTH — `read_bytes` returns exactly `num_bytes`, assembled in order from
//      consecutive single-sector reads, the final sector contributing only the remainder ----
// J4 (level-2): `nbytes` is no longer a free parameter with a `nbytes >= 1` premise. It is
// computed at the component's two `read_bytes` call sites: `read_sector` (gpt.rs:448,
// `sector_size` bytes; header reads gpt.rs:103) and the entry-array read (gpt.rs:118-120,
// `num_partition_entries * partition_entry_size` bytes from the on-disk header). Premises:
// D-RANGE-FR-011-46880c (device conjunct) and D-RANGE-FR-001-2e9472 verbatim.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(decl_entries@ == 128 && decl_entry_size@ == 128)]
#[ensures(result.0@ == (if entry_read { decl_entries@ * decl_entry_size@ } else { ss@ }))]
#[ensures(result.1@ == result.0@)]
#[ensures(result.2@ == cdiv(result.0@, ss@))]
#[ensures(result.3@ == result.0@ - (cdiv(result.0@, ss@) - 1) * ss@)]
#[ensures(result.3@ <= ss@)]
pub fn verify_dpm_io_read_length(
    ss: u32, decl_entries: u32, decl_entry_size: u32, entry_read: bool,
) -> (u64, u64, u64, u64) {
    proof_assert! { lemma_cdiv_all(ss@); true };
    let ssu = ss as u64;
    let nbytes: u64 = if entry_read {
        decl_entries as u64 * decl_entry_size as u64 // gpt.rs:118-119
    } else {
        ssu // gpt.rs:448 read_sector
    };
    let num_blocks = cdiv64(nbytes, ssu); // gpt.rs:452
    let mut got: u64 = 0;
    let mut i: u64 = 0;
    let mut last: u64 = 0;
    #[invariant(i@ <= num_blocks@)]
    #[invariant(i@ < num_blocks@ ==> got@ == i@ * ss@)]
    #[invariant(i@ == num_blocks@ ==> got@ == nbytes@)]
    #[invariant(i@ > 0 && i@ < num_blocks@ ==> last@ == ss@)]
    #[invariant(i@ == num_blocks@ ==> last@ == nbytes@ - (num_blocks@ - 1) * ss@)]
    while i < num_blocks {
        let remaining = nbytes - got; // gpt.rs:474
        let to_copy = if remaining < ssu { remaining } else { ssu }; // gpt.rs:475
        got += to_copy; // gpt.rs:476 `result.extend_from_slice(&locked.as_slice()[..to_copy])`
        last = to_copy;
        i += 1;
    }
    (nbytes, got, num_blocks, last)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(decl_entries@ == 128 && decl_entry_size@ == 128)]
#[ensures(!(result.0@ == (if entry_read { decl_entries@ * decl_entry_size@ } else { ss@ })))]
#[ensures(result.1@ == result.0@)]
#[ensures(result.2@ == cdiv(result.0@, ss@))]
#[ensures(result.3@ == result.0@ - (cdiv(result.0@, ss@) - 1) * ss@)]
#[ensures(result.3@ <= ss@)]
pub fn verify_dpm_io_read_length__mutant(
    ss: u32, decl_entries: u32, decl_entry_size: u32, entry_read: bool,
) -> (u64, u64, u64, u64) {
    proof_assert! { lemma_cdiv_all(ss@); true };
    let ssu = ss as u64;
    let nbytes: u64 = if entry_read {
        decl_entries as u64 * decl_entry_size as u64 // gpt.rs:118-119
    } else {
        ssu // gpt.rs:448 read_sector
    };
    let num_blocks = cdiv64(nbytes, ssu); // gpt.rs:452
    let mut got: u64 = 0;
    let mut i: u64 = 0;
    let mut last: u64 = 0;
    #[invariant(i@ <= num_blocks@)]
    #[invariant(i@ < num_blocks@ ==> got@ == i@ * ss@)]
    #[invariant(i@ == num_blocks@ ==> got@ == nbytes@)]
    #[invariant(i@ > 0 && i@ < num_blocks@ ==> last@ == ss@)]
    #[invariant(i@ == num_blocks@ ==> last@ == nbytes@ - (num_blocks@ - 1) * ss@)]
    while i < num_blocks {
        let remaining = nbytes - got; // gpt.rs:474
        let to_copy = if remaining < ssu { remaining } else { ssu }; // gpt.rs:475
        got += to_copy; // gpt.rs:476 `result.extend_from_slice(&locked.as_slice()[..to_copy])`
        last = to_copy;
        i += 1;
    }
    (nbytes, got, num_blocks, last)
}

/// `alloc_dma_buffer` (gpt.rs:556-562) is called BEFORE the command is queued, at gpt.rs:457
/// (read) and gpt.rs:509 (write); its failure is `?`-propagated as `IoError`, so no command is
/// sent for that sector.
#[ensures(!alloc_ok ==> result.0 == Some(PtErr::IoError) && !result.1)]
#[ensures(alloc_ok ==> result.0 == None && result.1)]
pub fn dma_step(alloc_ok: bool) -> (Option<PtErr>, bool) {
    if !alloc_ok {
        return (Some(PtErr::IoError), false); // gpt.rs:457-458 / 509-510
    }
    (None, true) // gpt.rs:461-468 / 523-530 — the command is queued
}

// ---- DPM-IO-ERR-DMA ----
#[ensures(!alloc_ok ==> result.0 == Some(PtErr::IoError))]
#[ensures(!alloc_ok ==> !result.1)]
pub fn verify_dpm_io_err_dma(alloc_ok: bool) -> (Option<PtErr>, bool) {
    dma_step(alloc_ok)
}
#[ensures(!alloc_ok ==> result.1)]
pub fn verify_dpm_io_err_dma__mutant(alloc_ok: bool) -> (Option<PtErr>, bool) {
    dma_step(alloc_ok)
}

/// `write_bytes`'s per-block DMA buffer — gpt.rs:509-518. The buffer comes back from
/// `alloc_dma_buffer` with UNSPECIFIED contents (hence `init`, which is not assumed zero: if it
/// were, the obligation would be vacuous), the payload is copied into
/// `[0, block_end - block_start)` and the remainder is EXPLICITLY zeroed.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(init@.len() == ss@)]
#[requires(payload@ <= ss@)]
#[ensures(result@.len() == ss@)]
#[ensures(forall<k: Int> payload@ <= k && k < ss@ ==> result@[k]@ == 0)]
#[ensures(forall<k: Int> 0 <= k && k < payload@ ==> result@[k] == data)]
pub fn write_block_buffer(ss: u32, payload: usize, data: u8, init: Vec<u8>) -> Vec<u8> {
    let mut buf = init;
    let mut k: usize = 0;
    #[invariant(k@ <= payload@)]
    #[invariant(buf@.len() == ss@)]
    #[invariant(forall<q: Int> 0 <= q && q < k@ ==> buf@[q] == data)]
    while k < payload {
        buf[k] = data; // gpt.rs:511-512 copy_from_slice(&data[block_start..block_end])
        k += 1;
    }
    let mut z: usize = payload;
    #[invariant(payload@ <= z@ && z@ <= ss@)]
    #[invariant(buf@.len() == ss@)]
    #[invariant(forall<q: Int> 0 <= q && q < payload@ ==> buf@[q] == data)]
    #[invariant(forall<q: Int> payload@ <= q && q < z@ ==> buf@[q]@ == 0)]
    while z < ss as usize {
        buf[z] = 0; // gpt.rs:514-518 the explicit zero-pad of the remainder
        z += 1;
    }
    buf
}

// ---- DPM-IO-WRITE-PAD ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(init@.len() == ss@)]
#[requires(payload@ <= ss@)]
#[ensures(result@.len() == ss@)]
#[ensures(forall<k: Int> payload@ <= k && k < ss@ ==> result@[k]@ == 0)]
pub fn verify_dpm_io_write_pad(ss: u32, payload: usize, data: u8, init: Vec<u8>) -> Vec<u8> {
    write_block_buffer(ss, payload, data, init)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(init@.len() == ss@)]
#[requires(payload@ <= ss@)]
#[ensures(forall<k: Int> payload@ <= k && k < ss@ ==> result@[k] == data)]
pub fn verify_dpm_io_write_pad__mutant(
    ss: u32, payload: usize, data: u8, init: Vec<u8>,
) -> Vec<u8> {
    write_block_buffer(ss, payload, data, init)
}

/// The LBAs `write_bytes` addresses — gpt.rs:502-505: `num_blocks` consecutive sectors from
/// `lba`, one `Command::WriteSync` each, none skipped.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(len@ >= 1)]
#[requires(lba@ + cdiv(len@, ss@) <= 18446744073709551615)]
#[ensures(result@.len() == cdiv(len@, ss@))]
#[ensures(forall<i: Int> 0 <= i && i < result@.len() ==> result@[i]@ == lba@ + i)]
pub fn write_bytes_lbas(ss: u32, lba: u64, len: u64) -> Vec<u64> {
    let num_blocks = cdiv64(len, ss as u64); // gpt.rs:502
    let mut out: Vec<u64> = Vec::new();
    let mut i: u64 = 0;
    #[invariant(i@ <= num_blocks@)]
    #[invariant(out@.len() == i@)]
    #[invariant(forall<q: Int> 0 <= q && q < i@ ==> out@[q]@ == lba@ + q)]
    while i < num_blocks {
        out.push(lba + i); // gpt.rs:505 `let block_lba = lba + i as u64;`
        i += 1;
    }
    out
}

// ---- DPM-IO-WRITE-LENGTH ----
// J4 (level-2): `lba` / `len` are no longer free parameters carrying `len >= 1` and
// `lba + ceil(len/ss) <= u64::MAX`. They are computed at the component's five `write_bytes`
// call sites, in source order (gpt.rs:204 MBR at 0, 207 primary header at 1, 210 primary
// entries at 2, 213 backup entries at last_usable + 1, 216 backup header at num_sectors - 1;
// sector-sized payloads gpt.rs:330/408, 128*128-byte entry array gpt.rs:433), on the
// geometry format builds from the request (lib.rs:90). Premises: D-RANGE-FR-011-46880c
// (config conjunct) and D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c (config conjunct) verbatim.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
#[ensures(result.2@.len() == cdiv(result.1@, ss@))]
#[ensures(forall<i: Int> 0 <= i && i < result.2@.len() ==> result.2@[i]@ == result.0@ + i)]
#[ensures(result.0@ + result.2@.len() <= ns@)]
pub fn verify_dpm_io_write_length(ss: u32, ns: u64, site: u32) -> (u64, u64, Vec<u64>) {
    let (_first, last) = usable_window(ss, ns); // gpt.rs:157-158
    let ssu = ss as u64;
    let (lba, len): (u64, u64) = match site {
        0 => (0, ssu),                              // gpt.rs:354 write_protective_mbr
        1 => (1, ssu),                              // gpt.rs:207 primary header
        2 => (2, ENTRY_ARRAY_BYTES as u64),         // gpt.rs:210 primary entries
        3 => (last + 1, ENTRY_ARRAY_BYTES as u64),  // gpt.rs:213 backup entries
        _ => (ns - 1, ssu),                         // gpt.rs:216 backup header
    };
    let lbas = write_bytes_lbas(ss, lba, len); // gpt.rs:497-505
    (lba, len, lbas)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
#[ensures(!(result.2@.len() == cdiv(result.1@, ss@)))]
#[ensures(forall<i: Int> 0 <= i && i < result.2@.len() ==> result.2@[i]@ == result.0@ + i)]
#[ensures(result.0@ + result.2@.len() <= ns@)]
pub fn verify_dpm_io_write_length__mutant(ss: u32, ns: u64, site: u32) -> (u64, u64, Vec<u64>) {
    let (_first, last) = usable_window(ss, ns); // gpt.rs:157-158
    let ssu = ss as u64;
    let (lba, len): (u64, u64) = match site {
        0 => (0, ssu),                              // gpt.rs:354 write_protective_mbr
        1 => (1, ssu),                              // gpt.rs:207 primary header
        2 => (2, ENTRY_ARRAY_BYTES as u64),         // gpt.rs:210 primary entries
        3 => (last + 1, ENTRY_ARRAY_BYTES as u64),  // gpt.rs:213 backup entries
        _ => (ns - 1, ssu),                         // gpt.rs:216 backup header
    };
    let lbas = write_bytes_lbas(ss, lba, len); // gpt.rs:497-505
    (lba, len, lbas)
}

// ---- DPM-IO-WRITE-PARTIAL — one command per sector, stopping at the first failure, so when
//      the error is reported the earlier sectors have ALREADY been changed on the drive ----
#[requires(nblocks@ >= 1)]
#[requires(fail_at@ < nblocks@)]
#[ensures(result@ == fail_at@)]
#[ensures(fail_at@ > 0 ==> result@ > 0)]
#[ensures(result@ < nblocks@)]
pub fn verify_dpm_io_write_partial(nblocks: u64, fail_at: u64) -> u64 {
    let mut done: u64 = 0;
    let mut i: u64 = 0;
    #[invariant(i@ <= nblocks@)]
    #[invariant(done@ == i@)]
    #[invariant(i@ <= fail_at@)]
    while i < nblocks {
        if i == fail_at {
            return done; // gpt.rs:534 `result.map_err(..)?` — returns, with `done` already written
        }
        done += 1; // gpt.rs:533 WriteDone(Ok) — this sector IS now changed on the drive
        i += 1;
    }
    done
}
#[requires(nblocks@ >= 1)]
#[requires(fail_at@ < nblocks@)]
#[ensures(result@ == 0)]
pub fn verify_dpm_io_write_partial__mutant(nblocks: u64, fail_at: u64) -> u64 {
    let mut done: u64 = 0;
    let mut i: u64 = 0;
    #[invariant(i@ <= nblocks@)]
    #[invariant(done@ == i@)]
    #[invariant(i@ <= fail_at@)]
    while i < nblocks {
        if i == fail_at {
            return done;
        }
        done += 1;
        i += 1;
    }
    done
}

/// A `Completion` as the two I/O loops destructure it — gpt.rs:470-491 and 532-549. Both use
/// `Completion::ReadDone { result: res, .. }` / `WriteDone { result, .. }`: the `..` DISCARDS
/// every identifying field, so nothing links the completion to the command just sent.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Comp {
    ReadDone(bool),
    WriteDone(bool),
    DevError,
    Other,
    RecvFailed,
}

#[logic(open)]
pub fn accept_completion_logic(c: Comp) -> Option<PtErr> {
    pearlite! {
        match c {
            Comp::ReadDone(ok) => (if ok { None } else { Some(PtErr::IoError) }),
            Comp::WriteDone(ok) => (if ok { None } else { Some(PtErr::IoError) }),
            Comp::DevError => Some(PtErr::IoError),
            Comp::Other => Some(PtErr::IoError),
            Comp::RecvFailed => Some(PtErr::IoError),
        }
    }
}

#[ensures(match c { Comp::ReadDone(ok) => (if ok { result == None }
    else { result == Some(PtErr::IoError) }), _ => true })]
#[ensures(match c { Comp::DevError => result == Some(PtErr::IoError), _ => true })]
#[ensures(match c { Comp::Other => result == Some(PtErr::IoError), _ => true })]
#[ensures(match c { Comp::RecvFailed => result == Some(PtErr::IoError), _ => true })]
#[ensures(match c { Comp::WriteDone(ok) => (if ok { result == None }
    else { result == Some(PtErr::IoError) }), _ => true })]
#[ensures(result == accept_completion_logic(c))]
pub fn accept_completion(c: Comp, cid: u64) -> Option<PtErr> {
    let _ = cid; // gpt.rs:471 — the `..` pattern never binds it
    match c {
        Comp::ReadDone(ok) => {
            if ok {
                None // gpt.rs:471-477
            } else {
                Some(PtErr::IoError) // gpt.rs:472 `res.map_err(..)?`
            }
        }
        Comp::WriteDone(ok) => {
            if ok {
                None // gpt.rs:533
            } else {
                Some(PtErr::IoError) // gpt.rs:534
            }
        }
        Comp::DevError => Some(PtErr::IoError),  // gpt.rs:478-480 / 536-538
        Comp::Other => Some(PtErr::IoError),     // gpt.rs:481-485 / 539-543
        Comp::RecvFailed => Some(PtErr::IoError), // gpt.rs:486-490 / 544-548
    }
}

// ---- DPM-IO-COMPLETION-PAIRING — acceptance cannot depend on which request the completion
//      belongs to, because the identifying fields are discarded ----
#[ensures(result.0 == result.1)]
#[ensures(result.0 == accept_completion_logic(c))]
pub fn verify_dpm_io_completion_pairing(
    c: Comp, cid_mine: u64, cid_other: u64,
) -> (Option<PtErr>, Option<PtErr>) {
    (accept_completion(c, cid_mine), accept_completion(c, cid_other))
}
#[ensures(cid_mine != cid_other ==> result.0 != result.1)]
pub fn verify_dpm_io_completion_pairing__mutant(
    c: Comp, cid_mine: u64, cid_other: u64,
) -> (Option<PtErr>, Option<PtErr>) {
    (accept_completion(c, cid_mine), accept_completion(c, cid_other))
}

// ---- DPM-IO-WRITE-SECTOR-TRUST — `write_sector` just forwards to `write_bytes` with no check
//      that the payload is exactly one sector, so a longer payload overwrites what follows ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(len@ >= 1)]
#[ensures(result.sectors@ == cdiv(len@, ss@))]
#[ensures(len@ > ss@ ==> result.sectors@ > 1)]
#[ensures(len@ <= ss@ ==> result.sectors@ == 1)]
pub fn verify_dpm_io_write_sector_trust(ns: u32, ss: u32, lba: u64, len: u64) -> Acc {
    proof_assert! { lemma_cdiv_all(ss@); true };
    write_bytes_acc(ns, ss, lba, len) // gpt.rs:497-498 write_sector -> write_bytes
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(len@ >= 1)]
#[ensures(result.sectors@ == 1)]
pub fn verify_dpm_io_write_sector_trust__mutant(ns: u32, ss: u32, lba: u64, len: u64) -> Acc {
    write_bytes_acc(ns, ss, lba, len)
}

// ===========================================================================
// 21. PROPERTY DRIVERS — partition names (gpt.rs:576-595)
// ===========================================================================

// ---- DPM-NAME-LENGTH-LIMIT — `encode_utf16().take(36)` SILENTLY truncates, so the 36-unit
//      limit is a real precondition: a 40-unit name does not survive ----
#[requires(units@.len() == 40)]
#[requires(forall<k: Int> 0 <= k && k < 40 ==> units@[k]@ != 0)]
#[ensures(result@.len() == 36)]
#[ensures(result@.len() < units@.len())]
pub fn verify_dpm_name_length_limit(units: &Vec<u16>) -> Vec<u16> {
    let field = encode_units(units); // gpt.rs:576-587
    decode_units(&field) // gpt.rs:589-595
}
#[requires(units@.len() == 40)]
#[requires(forall<k: Int> 0 <= k && k < 40 ==> units@[k]@ != 0)]
#[ensures(result@.len() == 40)]
pub fn verify_dpm_name_length_limit__mutant(units: &Vec<u16>) -> Vec<u16> {
    let field = encode_units(units);
    decode_units(&field)
}

// ---- DPM-NAME-ENCODE — two bytes per code unit, little-endian, inside the 72-byte field and
//      never beyond it; at most 36 units are ever stored ----
#[ensures(forall<k: Int> 0 <= k && k < 36 && k < units@.len()
    ==> result[2 * k]@ == units@[k]@ % 256 && result[2 * k + 1]@ == units@[k]@ / 256)]
#[ensures(forall<k: Int> units@.len() <= k && k < 36
    ==> result[2 * k]@ == 0 && result[2 * k + 1]@ == 0)]
pub fn verify_dpm_name_encode(units: &Vec<u16>) -> [u8; 72] {
    encode_units(units)
}
#[ensures(forall<k: Int> 0 <= k && k < 36 && k < units@.len()
    ==> result[2 * k]@ == units@[k]@ / 256 && result[2 * k + 1]@ == units@[k]@ % 256)]
pub fn verify_dpm_name_encode__mutant(units: &Vec<u16>) -> [u8; 72] {
    encode_units(units)
}

// ---- DPM-NAME-DECODE — exactly the 72 bytes of the field are read, little-endian, stopping at
//      the first zero unit, and nothing outside the field is touched ----
#[ensures(result@.len() <= 36)]
#[ensures(forall<k: Int> 0 <= k && k < result@.len()
    ==> result@[k]@ == data[2 * k]@ + 256 * data[2 * k + 1]@)]
#[ensures(forall<k: Int> 0 <= k && k < result@.len() ==> result@[k]@ != 0)]
pub fn verify_dpm_name_decode(data: &[u8; 72]) -> Vec<u16> {
    decode_units(data)
}
#[ensures(result@.len() <= 35)]
pub fn verify_dpm_name_decode__mutant(data: &[u8; 72]) -> Vec<u16> {
    decode_units(data)
}

// ---- DPM-NAME-ROUNDTRIP — a name of at most 36 non-zero code units (every ASCII letter, digit
//      and punctuation mark is such a unit) comes back exactly as it went in ----
#[requires(units@.len() <= 36)]
#[requires(forall<k: Int> 0 <= k && k < units@.len() ==> units@[k]@ != 0)]
#[ensures(result@.len() == units@.len())]
#[ensures(forall<k: Int> 0 <= k && k < units@.len() ==> result@[k] == units@[k])]
pub fn verify_dpm_name_roundtrip(units: &Vec<u16>) -> Vec<u16> {
    let field = encode_units(units);
    proof_assert! {
        forall<k: Int> 0 <= k && k < units@.len()
            ==> field[2 * k]@ + 256 * field[2 * k + 1]@ == units@[k]@
    };
    proof_assert! {
        forall<k: Int> 0 <= k && k < units@.len()
            ==> field[2 * k]@ + 256 * field[2 * k + 1]@ != 0
    };
    proof_assert! {
        units@.len() < 36
            ==> field[2 * units@.len()]@ + 256 * field[2 * units@.len() + 1]@ == 0
    };
    let out = decode_units(&field);
    proof_assert! { units@.len() < 36 ==> out@.len() == units@.len() };
    proof_assert! { units@.len() == 36 ==> out@.len() == 36 };
    out
}
#[requires(units@.len() <= 36)]
#[requires(forall<k: Int> 0 <= k && k < units@.len() ==> units@[k]@ != 0)]
#[ensures(result@.len() == units@.len() + 1)]
pub fn verify_dpm_name_roundtrip__mutant(units: &Vec<u16>) -> Vec<u16> {
    decode_units(&encode_units(units))
}

// ===========================================================================
// 22. PROPERTY DRIVERS — the global invariants
// ===========================================================================

// ---- DPM-INV-NO-OVERLAP ------------------------------------------------------------------
// REFUTED on the READ path. The obligation covers a table "whether it was just created or was
// read back from the drive". Creation is fine (see verify_dpm_inv_order_monotonic: back-to-back
// placement cannot overlap), but gpt.rs:132-144 copies `starting_lba`/`ending_lba` straight out
// of the on-disk entries with NO validation of any kind, so a table whose entry array passes its
// CRC but describes overlapping partitions is reported as-is. Here slots 0 and 1 both cover
// sector 150.
#[requires(entries@.len() == 2)]
#[requires(!entries@[0].tg_zero && !entries@[1].tg_zero)]
#[requires(entries@[0].start@ == 100 && entries@[0].end@ == 200)]
#[requires(entries@[1].start@ == 150 && entries@[1].end@ == 250)]
#[ensures(result@.len() == 2)]
#[ensures(result@[0].start_lba@ == 100 && result@[0].num_sectors@ == 101)]
#[ensures(result@[1].start_lba@ == 150 && result@[1].num_sectors@ == 101)]
#[ensures(result@[0].start_lba@ <= 150
    && 150 <= result@[0].start_lba@ + result@[0].num_sectors@ - 1)]
#[ensures(result@[1].start_lba@ <= 150
    && 150 <= result@[1].start_lba@ + result@[1].num_sectors@ - 1)]
pub fn refute_dpm_inv_no_overlap(entries: &Vec<EntryM>) -> Vec<PInfoM> {
    project(entries)
}

// ---- DPM-INV-ORDER-MONOTONIC — request order preserved, back to back, first at first_usable,
//      each later one at the sector immediately after the previous ends ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[ensures(match result { Ok(v) => specs@.len() >= 1 ==> v@[0].start == first, _ => true })]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j + 1 < specs@.len()
    ==> v@[j + 1].start@ == v@[j].end@ + 1, _ => true })]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j + 1 < specs@.len()
    ==> v@[j + 1].start@ >= v@[j].start@, _ => true })]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    ==> v@[j].tg_zero == specs@[j].tg_zero, _ => true })]
pub fn verify_dpm_inv_order_monotonic(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    proof_assert! { lemma_cdiv_all(ss@); true };
    layout_place(specs, ss, first, last, rest)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j + 1 < specs@.len()
    ==> v@[j + 1].start@ == v@[j].end@ + 2, _ => true })]
pub fn verify_dpm_inv_order_monotonic__mutant(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    layout_place(specs, ss, first, last, rest)
}

// ---- DPM-INV-WITHIN-USABLE ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    ==> v@[j].start@ >= first@, _ => true })]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    ==> v@[j].end@ <= last@, _ => true })]
pub fn verify_dpm_inv_within_usable(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    layout_place(specs, ss, first, last, rest)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(first@ >= 3 && first@ <= last@)]
#[requires(last@ <= 18446744073709551614)]
#[requires(rest@ == last@ - first@ + 1 - fixed_sum_l(specs@, specs@.len(), ss@))]
#[requires(fixed_sum_l(specs@, specs@.len(), ss@) <= last@ - first@ + 1)]
#[requires(rest_count_l(specs@, specs@.len()) <= 1)]
#[ensures(match result { Ok(v) => forall<j: Int> 0 <= j && j < specs@.len()
    ==> v@[j].start@ > first@, _ => true })]
pub fn verify_dpm_inv_within_usable__mutant(
    specs: &Vec<SpecM>, ss: u32, first: u64, last: u64, rest: u64,
) -> Result<Vec<EntryM>, LayoutErr> {
    layout_place(specs, ss, first, last, rest)
}

// ---- DPM-INV-ENTRY-ARRAY-SHAPE ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[ensures(result.0@ == 128)]
#[ensures(result.1@ == 128)]
#[ensures(result.2@ == 16384)]
#[ensures(result.2@ == result.0@ * result.1@)]
#[ensures(result.3@ == cdiv(16384, ss@))]
pub fn verify_dpm_inv_entry_array_shape(ss: u32) -> (u32, u32, u32, u32) {
    (GPT_MAX_ENTRIES, GPT_ENTRY_SIZE, entry_array_bytes(), entry_sectors(ss))
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[ensures(result.2@ == 8192)]
pub fn verify_dpm_inv_entry_array_shape__mutant(ss: u32) -> (u32, u32, u32, u32) {
    (GPT_MAX_ENTRIES, GPT_ENTRY_SIZE, entry_array_bytes(), entry_sectors(ss))
}

// ---- DPM-INV-CRC-SELF-CONSISTENT — the writer hashes the first 92 bytes with the CRC field
//      zeroed and then stores the CRC there; the reader takes the same 92 bytes and zeroes the
//      same field before hashing. The two byte sequences therefore coincide, so the recomputed
//      value equals the stored one FOR EVERY hash function `h` — which is strictly stronger
//      than naming CRC-32, and cannot be satisfied by a degenerate choice of `h`. ----
// J4 (level-2): the `pre.len() >= 92` premise is gone. The header buffer is CONSTRUCTED as one
// sector — `vec![0u8; self.sector_size as usize]` (gpt.rs:408) on the write side, `read_sector`
// = `read_bytes(lba, sector_size)` (gpt.rs:448) on the read side — as `Seq::create(ss, bytes)`
// for an ARBITRARY byte function `bytes`, so every one-sector buffer is covered. Premises:
// D-RANGE-FR-011-46880c (`ss` is 512 or 4096) and the obligation's own words (a): the
// checksum field [16..20] is zero when the writer hashes.
#[logic]
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(bytes.get(16)@ == 0 && bytes.get(17)@ == 0 && bytes.get(18)@ == 0
    && bytes.get(19)@ == 0)]
#[ensures(h.get(set4(set4(Seq::create(ss@, bytes), 16, b0, b1, b2, b3), 16, 0u8, 0u8, 0u8, 0u8))
    == h.get(Seq::create(ss@, bytes)))]
pub fn verify_dpm_inv_crc_self_consistent(
    h: Mapping<Seq<u8>, u32>, ss: u32, bytes: Mapping<Int, u8>, b0: u8, b1: u8, b2: u8, b3: u8,
) {
    pearlite! { lemma_crc_field_roundtrip(Seq::create(ss@, bytes), b0, b1, b2, b3) }
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[logic]
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(bytes.get(16)@ == 0 && bytes.get(17)@ == 0 && bytes.get(18)@ == 0
    && bytes.get(19)@ == 0)]
#[ensures(!(h.get(set4(set4(Seq::create(ss@, bytes), 16, b0, b1, b2, b3), 16, 0u8, 0u8, 0u8, 0u8))
    == h.get(Seq::create(ss@, bytes))))]
pub fn verify_dpm_inv_crc_self_consistent__mutant(
    h: Mapping<Seq<u8>, u32>, ss: u32, bytes: Mapping<Int, u8>, b0: u8, b1: u8, b2: u8, b3: u8,
) {
    pearlite! { lemma_crc_field_roundtrip(Seq::create(ss@, bytes), b0, b1, b2, b3) }
}

/// The byte ranges the two sides hash, as constants, so the "same recipe" claim is anchored to
/// the source: write = `hash(&buf[..92])` after zeroing `[16..20]` (gpt.rs:413, 426); read =
/// `hash(&header_data[..92])` after zeroing `[16..20]` (gpt.rs:107-109).
#[ensures(result.0@ == 92 && result.1@ == 92)]
#[ensures(result.2@ == 16 && result.3@ == 20)]
#[ensures(result.0 == result.1)]
pub fn crc_recipe() -> (u32, u32, u32, u32) {
    (GPT_HEADER_SIZE, GPT_HEADER_SIZE, 16, 20)
}

// ---- DPM-INV-SIGNATURE-REVISION ----
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * entry_sectors_l(ss@) + 4)]
#[ensures(result.0.signature == GPT_SIGNATURE)]
#[ensures(result.0.revision@ == 65536)]
#[ensures(result.1.signature == GPT_SIGNATURE)]
#[ensures(result.1.revision@ == 65536)]
#[ensures(!sig_ok ==> (match hdr_read { IoRes::Ok =>
    (hdr_len_ok ==> result.2 == Attempt::NoTable), _ => true }))]
#[ensures(match result.2 { Attempt::Ok(_) => sig_ok, _ => true })]
pub fn verify_dpm_inv_signature_revision(
    ss: u32, ns: u64, crc: u32, hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool,
    hdr_crc_match: bool, entry_read: IoRes, entry_crc_match: bool, tab: TabM,
) -> (HdrM, HdrM, Attempt) {
    let (p, b) = header_pair(ss, ns, crc);
    let a = try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read,
                              entry_crc_match, tab);
    (p, b, a)
}
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * entry_sectors_l(ss@) + 4)]
#[ensures(result.0.revision@ == 1)]
pub fn verify_dpm_inv_signature_revision__mutant(
    ss: u32, ns: u64, crc: u32, hdr_read: IoRes, hdr_len_ok: bool, sig_ok: bool,
    hdr_crc_match: bool, entry_read: IoRes, entry_crc_match: bool, tab: TabM,
) -> (HdrM, HdrM, Attempt) {
    let (p, b) = header_pair(ss, ns, crc);
    let a = try_read_gpt_at_m(hdr_read, hdr_len_ok, sig_ok, hdr_crc_match, entry_read,
                              entry_crc_match, tab);
    (p, b, a)
}

// ---- DPM-INV-BACKUP-MIRRORS-PRIMARY ----
// J4 (level-2): premises = D-RANGE-FR-011-46880c (config conjunct) and
// D-RANGE-KEYENTITY:PARTITIONCONFIG-90393c (config conjunct) verbatim:
// `total_sectors >= 2 * 16384.div_ceil(sector_size) + 5` (was the derived `>= 2 * entry_sectors + 4`).
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
// the two entry-array copies are written from the SAME `entry_data` buffer (gpt.rs:210, 213)
#[ensures(result.2@[2].sectors == result.2@[3].sectors)]
// and the backup header differs from the primary in exactly the three position fields
#[ensures(result.1.signature == result.0.signature)]
#[ensures(result.1.revision == result.0.revision)]
#[ensures(result.1.header_size == result.0.header_size)]
#[ensures(result.1.first_usable_lba == result.0.first_usable_lba)]
#[ensures(result.1.last_usable_lba == result.0.last_usable_lba)]
#[ensures(result.1.num_partition_entries == result.0.num_partition_entries)]
#[ensures(result.1.partition_entry_size == result.0.partition_entry_size)]
#[ensures(result.1.partition_entry_crc32 == result.0.partition_entry_crc32)]
#[ensures(result.1.my_lba == result.0.alternate_lba && result.1.alternate_lba == result.0.my_lba)]
pub fn verify_dpm_inv_backup_mirrors_primary(
    nsid: u32, ss: u32, ns: u64, crc: u32,
) -> (HdrM, HdrM, Vec<Acc>) {
    let (p, b) = header_pair(ss, ns, crc);
    let t = write_gpt_trace(nsid, ss, ns);
    (p, b, t)
}
// MUTANT (anti-vacuity, J4): the driver above verbatim, first ensures negated.
#[requires(ss@ == 512 || ss@ == 4096)]
#[requires(ns@ >= 2 * cdiv(16384, ss@) + 5)]
// the two entry-array copies are written from the SAME `entry_data` buffer (gpt.rs:210, 213)
#[ensures(!(result.2@[2].sectors == result.2@[3].sectors))]
// and the backup header differs from the primary in exactly the three position fields
#[ensures(result.1.signature == result.0.signature)]
#[ensures(result.1.revision == result.0.revision)]
#[ensures(result.1.header_size == result.0.header_size)]
#[ensures(result.1.first_usable_lba == result.0.first_usable_lba)]
#[ensures(result.1.last_usable_lba == result.0.last_usable_lba)]
#[ensures(result.1.num_partition_entries == result.0.num_partition_entries)]
#[ensures(result.1.partition_entry_size == result.0.partition_entry_size)]
#[ensures(result.1.partition_entry_crc32 == result.0.partition_entry_crc32)]
#[ensures(result.1.my_lba == result.0.alternate_lba && result.1.alternate_lba == result.0.my_lba)]
pub fn verify_dpm_inv_backup_mirrors_primary__mutant(
    nsid: u32, ss: u32, ns: u64, crc: u32,
) -> (HdrM, HdrM, Vec<Acc>) {
    let (p, b) = header_pair(ss, ns, crc);
    let t = write_gpt_trace(nsid, ss, ns);
    (p, b, t)
}

// ---- DPM-INV-GUID-V4-BITS — the version-4 / variant-1 marker bits are set unconditionally,
//      on both the `/dev/urandom` path and the zero-fallback path ----
#[ensures(result.0[6] & 0xF0u8 == 0x40u8)]
#[ensures(result.0[8] & 0xC0u8 == 0x80u8)]
#[ensures(result.1[6] & 0xF0u8 == 0x40u8)]
#[ensures(result.1[8] & 0xC0u8 == 0x80u8)]
pub fn verify_dpm_inv_guid_v4_bits(bytes: [u8; 16]) -> ([u8; 16], [u8; 16]) {
    (generate_guid(true, bytes), generate_guid(false, bytes))
}
#[ensures(result.0[6] & 0xF0u8 == 0x50u8)]
pub fn verify_dpm_inv_guid_v4_bits__mutant(bytes: [u8; 16]) -> ([u8; 16], [u8; 16]) {
    (generate_guid(true, bytes), generate_guid(false, bytes))
}

// ---- DPM-INV-SECTOR-SIZE-DOMAIN ----------------------------------------------------------
// REFUTED. `format` passes `config.sector_size` straight to `GptManager::new` (lib.rs:91 ->
// gpt.rs:58) and NOTHING anywhere validates it; the only mention of 512/4096 in the component
// is a doc comment (ipartition_table.rs:37). A request carrying `sector_size: 256` is accepted,
// and `write_protective_mbr` then allocates `vec![0u8; 256]` (gpt.rs:330) and stores at offset
// 446 (gpt.rs:334) — out of bounds, a panic rather than any declared error.
#[ensures(result.0@ == 256)]
#[ensures(result.0@ != 512 && result.0@ != 4096)]
#[ensures(result.1@ == 256)]
#[ensures(result.1@ < 512)]
pub fn refute_dpm_inv_sector_size_domain() -> (u32, usize) {
    let used = sector_size_used(256); // lib.rs:91 -> gpt.rs:58, no validation on either side
    let mbr_len = used as usize; // gpt.rs:330 `vec![0u8; self.sector_size as usize]`
    (used, mbr_len) // mbr[446] and mbr[510..512] are beyond this buffer
}

/// lib.rs:91 `GptManager::new(bd, config.ns_id, config.sector_size, config.total_sectors)` then
/// gpt.rs:58-63 `Self { sector_size, .. }` — a pass-through with no check.
#[ensures(result == cfg_ss)]
pub fn sector_size_used(cfg_ss: u32) -> u32 {
    cfg_ss
}

// ---- DPM-INV-TYPE-GUID-CONSTANTS — the three published type GUIDs, checked against the UUID
//      text they document (mixed-endian per UEFI: time_low/time_mid/time_hi little-endian) ----
pub const CERTUS_METADATA: [u8; 16] = [
    0x01, 0x8E, 0x3A, 0x7C, 0x4F, 0x1B, 0x2D, 0x4A, 0x9E, 0x6C, 0x0D, 0x3F, 0x5A, 0x8B, 0x7C,
    0x01,
];
pub const CERTUS_DATA: [u8; 16] = [
    0x02, 0x8E, 0x3A, 0x7C, 0x4F, 0x1B, 0x2D, 0x4A, 0x9E, 0x6C, 0x0D, 0x3F, 0x5A, 0x8B, 0x7C,
    0x02,
];
pub const CERTUS_EXTERNAL_META: [u8; 16] = [
    0x03, 0x8E, 0x3A, 0x7C, 0x4F, 0x1B, 0x2D, 0x4A, 0x9E, 0x6C, 0x0D, 0x3F, 0x5A, 0x8B, 0x7C,
    0x03,
];

// time_low == 0x7C3A8E01 / 02 / 03 little-endian
#[ensures(result.0[0]@ + 256 * result.0[1]@ + 65536 * result.0[2]@ + 16777216 * result.0[3]@
    == 2084212225)]
#[ensures(result.1[0]@ + 256 * result.1[1]@ + 65536 * result.1[2]@ + 16777216 * result.1[3]@
    == 2084212226)]
#[ensures(result.2[0]@ + 256 * result.2[1]@ + 65536 * result.2[2]@ + 16777216 * result.2[3]@
    == 2084212227)]
// time_mid == 0x1B4F, time_hi_and_version == 0x4A2D, both little-endian
#[ensures(result.0[4]@ + 256 * result.0[5]@ == 6991)]
#[ensures(result.0[6]@ + 256 * result.0[7]@ == 18989)]
// the three differ only in their first and last byte ...
#[ensures(forall<k: Int> 4 <= k && k < 15
    ==> result.0[k] == result.1[k] && result.1[k] == result.2[k])]
#[ensures(result.0[15]@ == 1 && result.1[15]@ == 2 && result.2[15]@ == 3)]
// ... so they are pairwise distinct
#[ensures(result.0[0] != result.1[0] && result.1[0] != result.2[0] && result.0[0] != result.2[0])]
pub fn verify_dpm_inv_type_guid_constants() -> ([u8; 16], [u8; 16], [u8; 16]) {
    (CERTUS_METADATA, CERTUS_DATA, CERTUS_EXTERNAL_META)
}
#[ensures(result.0[0] == result.1[0])]
pub fn verify_dpm_inv_type_guid_constants__mutant() -> ([u8; 16], [u8; 16], [u8; 16]) {
    (CERTUS_METADATA, CERTUS_DATA, CERTUS_EXTERNAL_META)
}

// ---- DPM-INV-CACHED-STATE-MONOTONIC — two states only; starts empty; becomes loaded only on a
//      complete success; never goes back to empty ----
#[requires(parts@.len() <= 4294967295)]
#[ensures(match st { Some(_) => (match result.0 { Some(_) => true, None => false }), _ => true })]
#[ensures(match st { Some(_) => (match result.1 { Some(_) => true, None => false }), _ => true })]
#[ensures(match st { None => (match result.0 { Some(_) =>
    (match rd { ReadRes::Ok(_) => true, _ => false }), None => true }), _ => true })]
#[ensures(match st { None => (match result.1 { Some(_) =>
    (match wr { ReadRes::Ok(_) => true, _ => false }), None => true }), _ => true })]
pub fn verify_dpm_inv_cached_state_monotonic(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, wr: ReadRes,
    poisoned: bool, parts: &Vec<PInfoM>, loaded: bool, idx: u32,
) -> (Option<TabM>, Option<TabM>) {
    let (_a, s1) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let (_b, s2) = format_m(st, bound, chan_ok, wr, poisoned);
    // the two query methods cannot change the state at all
    let _c = partition_info_m(loaded, parts, poisoned, idx);
    let _d = num_partitions_m(loaded, parts, poisoned);
    (s1, s2)
}
#[requires(parts@.len() <= 4294967295)]
#[ensures(match st { Some(_) => result.0 == None, _ => true })]
pub fn verify_dpm_inv_cached_state_monotonic__mutant(
    st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes, wr: ReadRes,
    poisoned: bool, parts: &Vec<PInfoM>, loaded: bool, idx: u32,
) -> (Option<TabM>, Option<TabM>) {
    let (_a, s1) = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
    let (_b, s2) = format_m(st, bound, chan_ok, wr, poisoned);
    (s1, s2)
}

// ---- DPM-INV-NUM-SECTORS-POSITIVE --------------------------------------------------------
// REFUTED. A rest-of-disk partition requested when the fixed partitions already consume all the
// usable space gets `rest_sectors == 0` (gpt.rs:277), so gpt.rs:293 computes
// `ending_lba = current_lba + 0 - 1`, i.e. ONE BELOW its own starting LBA. The reported
// `num_sectors = e.ending_lba - e.starting_lba + 1` (gpt.rs:226) then underflows: 0 in a release
// build, a subtract-overflow panic in a debug build. Either way the "at least one sector"
// promise fails, and the "when little space remains" case the obligation calls out explicitly is
// exactly this one.
// Witness: a 68-sector device at 512-byte sectors (first_usable 34, last_usable 34, one usable
// sector), one 1-byte partition and one rest-of-disk partition.
#[requires(specs@.len() == 2)]
#[requires(specs@[0].size_bytes@ == 1 && !specs@[0].tg_zero)]
#[requires(specs@[1].size_bytes@ == 0 && !specs@[1].tg_zero)]
#[ensures(result.0@ == 35)]
#[ensures(result.1@ == 34)]
#[ensures(result.1@ + 1 == result.0@)]
#[ensures(result.2@ == 0)]
pub fn refute_dpm_inv_num_sectors_positive(specs: &Vec<SpecM>) -> (u64, u64, u64) {
    proof_assert! { fixed_sum_l(specs@, 2, 512) == 1 };
    proof_assert! { rest_count_l(specs@, 2) == 1 };
    match layout_place(specs, 512, 34, 34, 0) {
        Ok(v) => {
            let s = v[1].start;
            let e = v[1].end;
            (s, e, num_sectors_wrapping(s, e))
        }
        Err(_) => (0, 0, 1),
    }
}

/// `num_sectors: e.ending_lba - e.starting_lba + 1` (gpt.rs:139, 226) under the release build's
/// wrapping arithmetic; a debug build panics on the same expression.
#[ensures(result@ == (end@ + 1 - start@).rem_euclid(18446744073709551616))]
pub fn num_sectors_wrapping(start: u64, end: u64) -> u64 {
    end.wrapping_sub(start).wrapping_add(1)
}

// ---- DPM-INV-PROVIDES-INTERFACE — the interface surface. `define_component!` declares
//      `provides: [IPartitionTable]` and the single receptacle `block_device: IBlockDevice`
//      (lib.rs:13-24), and `impl IPartitionTable for DiskPartitionManager` (lib.rs:67-120)
//      supplies all four methods (ipartition_table.rs:116-128). What a PROVER can add is that
//      the dispatch over those four methods is TOTAL and that each arm lands on a real
//      implementation; the declaration itself is checked by rustc and the macro. ----
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub enum Method {
    Initialize,
    Format,
    PartitionInfo,
    NumPartitions,
}

#[logic(open)]
pub fn method_tag(m: Method) -> Int {
    pearlite! {
        match m {
            Method::Initialize => 0,
            Method::Format => 1,
            Method::PartitionInfo => 2,
            Method::NumPartitions => 3,
        }
    }
}

#[requires(parts@.len() <= 4294967295)]
#[ensures(result@ == method_tag(m))]
#[ensures(result@ <= 3)]
pub fn verify_dpm_inv_provides_interface(
    m: Method, st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes,
    wr: ReadRes, poisoned: bool, loaded: bool, parts: &Vec<PInfoM>, idx: u32,
) -> u32 {
    match m {
        Method::Initialize => {
            let _ = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
            0
        }
        Method::Format => {
            let _ = format_m(st, bound, chan_ok, wr, poisoned);
            1
        }
        Method::PartitionInfo => {
            let _ = partition_info_m(loaded, parts, poisoned, idx);
            2
        }
        Method::NumPartitions => {
            let _ = num_partitions_m(loaded, parts, poisoned);
            3
        }
    }
}
#[requires(parts@.len() <= 4294967295)]
#[ensures(result@ == 0)]
pub fn verify_dpm_inv_provides_interface__mutant(
    m: Method, st: Option<TabM>, bound: bool, geom_ok: bool, chan_ok: bool, rd: ReadRes,
    wr: ReadRes, poisoned: bool, loaded: bool, parts: &Vec<PInfoM>, idx: u32,
) -> u32 {
    match m {
        Method::Initialize => {
            let _ = initialize_m(st, bound, geom_ok, chan_ok, rd, poisoned);
            0
        }
        Method::Format => {
            let _ = format_m(st, bound, chan_ok, wr, poisoned);
            1
        }
        Method::PartitionInfo => {
            let _ = partition_info_m(loaded, parts, poisoned, idx);
            2
        }
        Method::NumPartitions => {
            let _ = num_partitions_m(loaded, parts, poisoned);
            3
        }
    }
}

/// `LayoutErr` -> `PartitionTableError::LayoutError` at all three sites (gpt.rs:257, 271, 287).
#[ensures(result == PtErr::LayoutError)]
pub fn layout_err_to_pt(e: LayoutErr) -> PtErr {
    let _ = e;
    PtErr::LayoutError
}

/// One attempt's error kind, as `try_read_gpt_at` produces it.
#[ensures(match a { Attempt::Corrupt => result == Some(PtErr::CorruptTable),
    Attempt::NoTable => result == Some(PtErr::NoPartitionTable),
    Attempt::Io => result == Some(PtErr::IoError),
    Attempt::Ok(_) => result == None })]
pub fn attempt_err(a: Attempt) -> Option<PtErr> {
    match a {
        Attempt::Corrupt => Some(PtErr::CorruptTable),
        Attempt::NoTable => Some(PtErr::NoPartitionTable),
        Attempt::Io => Some(PtErr::IoError),
        Attempt::Ok(_) => None,
    }
}

// ---- DPM-INV-ERRORS-ALL-USED — all six declared failure kinds are actually produced, each
//      witnessed by a concrete run of the corresponding mirror ----
#[requires(empty@.len() == 0)]
#[requires(two_rest@.len() == 2)]
#[requires(two_rest@[0].size_bytes@ == 0 && two_rest@[1].size_bytes@ == 0)]
#[ensures(result.0 == PtErr::NoPartitionTable)]
#[ensures(result.1 == PtErr::CorruptTable)]
#[ensures(result.2 == PtErr::InvalidPartition)]
#[ensures(result.3 == PtErr::IoError)]
#[ensures(result.4 == PtErr::LayoutError)]
#[ensures(result.5 == PtErr::NotInitialized)]
pub fn verify_dpm_inv_errors_all_used(
    empty: &Vec<PInfoM>, two_rest: &Vec<SpecM>, tab: TabM,
) -> (PtErr, PtErr, PtErr, PtErr, PtErr, PtErr) {
    // 1. NoPartitionTable — neither copy acceptable (gpt.rs:92)
    let e1 = match read_gpt_m(Attempt::NoTable, Attempt::NoTable) {
        ReadRes::Err(e) => e,
        ReadRes::Ok(_) => PtErr::IoError,
    };
    // 2. CorruptTable — the header CRC does not match (gpt.rs:111)
    let a2 = try_read_gpt_at_m(IoRes::Ok, true, true, false, IoRes::Ok, true, tab);
    let e2 = match attempt_err(a2) {
        Some(e) => e,
        None => PtErr::IoError,
    };
    // 3. InvalidPartition — an index at or beyond the count (lib.rs:107)
    let e3 = match partition_info_m(true, empty, false, 0) {
        PiOutcome::Err(e) => e,
        PiOutcome::Ok(_) => PtErr::IoError,
        PiOutcome::Panic => PtErr::IoError,
    };
    // 4. IoError — a sector read failed on the primary attempt (gpt.rs:83)
    let e4 = match read_gpt_m(Attempt::Io, Attempt::Ok(tab)) {
        ReadRes::Err(e) => e,
        ReadRes::Ok(_) => PtErr::CorruptTable,
    };
    // 5. LayoutError — two rest-of-disk partitions (gpt.rs:257)
    proof_assert! { rest_count_l(two_rest@, 2) == 2 };
    let e5 = match layout_classify(two_rest, 512, 1000) {
        Err(l) => layout_err_to_pt(l),
        Ok(_) => PtErr::IoError,
    };
    // 6. NotInitialized — the receptacle was never connected (lib.rs:70)
    let (o6, _s) = initialize_m(None, false, true, true, ReadRes::Ok(tab), false);
    let e6 = match o6 {
        Outcome::Err(e) => e,
        Outcome::Ok(_) => PtErr::IoError,
        Outcome::Panic => PtErr::IoError,
    };
    (e1, e2, e3, e4, e5, e6)
}
#[requires(empty@.len() == 0)]
#[requires(two_rest@.len() == 2)]
#[requires(two_rest@[0].size_bytes@ == 0 && two_rest@[1].size_bytes@ == 0)]
#[ensures(result.0 == PtErr::CorruptTable)]
pub fn verify_dpm_inv_errors_all_used__mutant(
    empty: &Vec<PInfoM>, two_rest: &Vec<SpecM>, tab: TabM,
) -> (PtErr, PtErr, PtErr, PtErr, PtErr, PtErr) {
    let e1 = match read_gpt_m(Attempt::NoTable, Attempt::NoTable) {
        ReadRes::Err(e) => e,
        ReadRes::Ok(_) => PtErr::IoError,
    };
    (e1, e1, e1, e1, e1, e1)
}
