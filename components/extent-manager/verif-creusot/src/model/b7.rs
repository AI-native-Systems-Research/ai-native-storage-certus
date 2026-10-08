//! Batch-7 model additions (pin 2cd35bac line numbers). New file; no existing contract changed.
//! * `deserialize7` — `Superblock::deserialize` (superblock.rs:99-168) with an EXACT contract: Ok
//!   iff the buffer is >= 4096 bytes, carries the magic and its CRC32 matches, and then the result
//!   is the field-by-field decoding `sb_of` (named by batch 4's opaque `dec64`/`dec32`, read by the
//!   trusted leaves `rd64`/`rd32`).
//! * `deser7` — `checkpoint::deserialize_slabs` (checkpoint.rs:183-224) with an EXACT contract:
//!   Ok iff the input is not truncated (`ck_complete`: the region count, every slab count, slab
//!   header and key vector fit — `end_regions` / `end_slabs` walk the grammar), and then the
//!   result is the positional decoding of the bytes (`parsed`). Every index is in bounds
//!   (Creusot's own slice-bounds VCs).
//! * `recover7` — `recovery::recover` (recovery.rs:11-71) composed of the superblock read result
//!   `sbr` (read_blocks(0, 4096), block_io.rs:110-155: Ok bytes or IoError), `deserialize7`, and
//!   the outcome of `read_checkpoint_region` for the active (`rd_a`, expected seq) and the
//!   inactive (`rd_i`, expected seq - 1) copy as ORACLES (Ok payload, or any error: a device read
//!   failure — batch 3 `read_ckpt`: IoError — or a header/CRC/seq check failure), then `deser7`.
//! * `ExtentManager::init_front7` (lib.rs:514-519: namespace, get_metadata_client lib.rs:189-213
//!   with the receptacle / connect_client / sector_size outcomes, recover) — takes `&self`;
//!   `initialize7` = init_front7 `?` + component.rs's `initialize` mirror (lib.rs:521-581, which
//!   has no error path).
//! * `ExtentManager::format7` — `format` (lib.rs:383-511) with the device queries, /dev/urandom,
//!   connect_client / sector_size and the superblock `write_blocks` outcome as explicit oracles;
//!   region construction as in component.rs's format_build.
use crate::model::b4::*;
use crate::model::b6::*;
use crate::model::buddy::*;
use crate::model::component::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::sbbytes::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use creusot_std::prelude::*;

// =============================================================================
// Superblock decode (superblock.rs:99-168)
// =============================================================================

/// `u64::from_le_bytes(b[p..p + 8])`.
#[logic(open)]
pub fn d64(b: Seq<u8>, p: Int) -> u64 {
    pearlite! { dec64(b.subsequence(p, p + 8)) }
}

/// `u32::from_le_bytes(b[p..p + 4])`.
#[logic(open)]
pub fn d32(b: Seq<u8>, p: Int) -> u32 {
    pearlite! { dec32(b.subsequence(p, p + 4)) }
}

/// The three checks of deserialize (superblock.rs:100-102, 108-112, 143-150) pass.
#[logic(open)]
pub fn sb_ok7(b: Seq<u8>) -> bool {
    pearlite! { b.len() >= 4096 && d64(b, 0) == SUPERBLOCK_MAGIC && d32(b, 92) == crc32(b.subsequence(0, 92)) }
}

/// The superblock deserialize builds from `b` (superblock.rs:106-141, 152-167).
#[logic(open)]
pub fn sb_of(b: Seq<u8>) -> Superblock {
    pearlite! {
        Superblock {
            magic: d64(b, 0), version: d32(b, 8), data_disk_size: d64(b, 12), sector_size: d32(b, 20),
            slab_size: d64(b, 24), max_extent_size: d32(b, 32), region_count: d32(b, 36),
            checkpoint_seq: d64(b, 40), active_copy: b[48], checkpoint_region_offset: d64(b, 56),
            checkpoint_region_size: d64(b, 64), instance_id: d64(b, 72), metadata_disk_ns_id: d32(b, 80),
            data_start_offset: d64(b, 84),
        }
    }
}

/// Mirror of `Superblock::deserialize` (superblock.rs:99-168), exact.
#[ensures(match result {
    Ok(sb) => sb_ok7(buf@) && sb == sb_of(buf@),
    Err(e) => !sb_ok7(buf@) && e == EmError::CorruptMetadata,
})]
#[ensures(forall<sb: Superblock> sb_layout(buf@, sb) && sb.magic == SUPERBLOCK_MAGIC ==> result == Ok(sb))]
pub fn deserialize7(buf: &Vec<u8>) -> Result<Superblock, EmError> {
    if buf.len() < SUPERBLOCK_SIZE {
        return Err(EmError::CorruptMetadata);
    }
    let magic = rd64(buf, 0);
    if magic != SUPERBLOCK_MAGIC {
        return Err(EmError::CorruptMetadata);
    }
    let version = rd32(buf, 8);
    let data_disk_size = rd64(buf, 12);
    let sector_size = rd32(buf, 20);
    let slab_size = rd64(buf, 24);
    let max_extent_size = rd32(buf, 32);
    let region_count = rd32(buf, 36);
    let checkpoint_seq = rd64(buf, 40);
    let active_copy = buf[48];
    // 7 bytes reserved (superblock.rs:130-131)
    let checkpoint_region_offset = rd64(buf, 56);
    let checkpoint_region_size = rd64(buf, 64);
    let instance_id = rd64(buf, 72);
    let metadata_disk_ns_id = rd32(buf, 80);
    let data_start_offset = rd64(buf, 84);
    let stored_crc = rd32(buf, 92);
    let computed_crc = crc32_hash(buf, 92);
    if stored_crc != computed_crc {
        return Err(EmError::CorruptMetadata);
    }
    Ok(Superblock {
        magic,
        version,
        data_disk_size,
        sector_size,
        slab_size,
        max_extent_size,
        region_count,
        checkpoint_seq,
        active_copy,
        checkpoint_region_offset,
        checkpoint_region_size,
        instance_id,
        metadata_disk_ns_id,
        data_start_offset,
    })
}

// =============================================================================
// Checkpoint payload decode (checkpoint.rs:183-224)
// =============================================================================

/// Where `k` slabs starting at `pos` end, or None if one of them is truncated
/// (checkpoint.rs:206-208 header, :218-220 key vector).
#[logic(open)]
#[variant(k)]
pub fn end_slabs(d: Seq<u8>, pos: Int, k: Int) -> Option<Int> {
    pearlite! {
        if k <= 0 { Some(pos) }
        else if pos + 24 > d.len() { None }
        else if pos + 24 + 8 * d32(d, pos + 20)@ > d.len() { None }
        else { end_slabs(d, pos + 24 + 8 * d32(d, pos + 20)@, k - 1) }
    }
}

/// Where `n` regions starting at `pos` end, or None (checkpoint.rs:198-200 slab count).
#[logic(open)]
#[variant(n)]
pub fn end_regions(d: Seq<u8>, pos: Int, n: Int) -> Option<Int> {
    pearlite! {
        if n <= 0 { Some(pos) }
        else if pos + 4 > d.len() { None }
        else {
            match end_slabs(d, pos + 4, d32(d, pos)@) {
                None => None,
                Some(p) => end_regions(d, p, n - 1),
            }
        }
    }
}

/// The payload is NOT truncated: the region count (checkpoint.rs:186-188) and everything it
/// announces are present.
#[logic(open)]
pub fn ck_complete(d: Seq<u8>) -> bool {
    pearlite! { d.len() >= 4 && end_regions(d, 4, d32(d, 0)@) != None }
}

/// Encoded length of the first `k` slab records of `ss`.
#[logic(open)]
#[variant(k)]
pub fn sl_len(ss: Seq<SlabDescriptor>, k: Int) -> Int {
    pearlite! { if k <= 0 { 0 } else { sl_len(ss, k - 1) + 24 + 8 * ss[k - 1].keys@.len() } }
}

/// Encoded length of the first `r` region records of `v`.
#[logic(open)]
#[variant(r)]
pub fn rg_len(v: Seq<Vec<SlabDescriptor>>, r: Int) -> Int {
    pearlite! { if r <= 0 { 0 } else { rg_len(v, r - 1) + 4 + sl_len(v[r - 1]@, v[r - 1]@.len()) } }
}

/// Slab record at byte `q` decodes to `s` (checkpoint.rs:209-229).
#[logic(open)]
pub fn slab_at(d: Seq<u8>, q: Int, s: SlabDescriptor) -> bool {
    pearlite! {
        q + 24 <= d.len()
        && s.start_offset == d64(d, q) && s.slab_size == d64(d, q + 8) && s.element_size == d32(d, q + 16)
        && s.keys@.len() == d32(d, q + 20)@ && q + 24 + 8 * s.keys@.len() <= d.len()
        && forall<t: Int> 0 <= t && t < s.keys@.len() ==> s.keys@[t] == d64(d, q + 24 + 8 * t)
    }
}

/// Region record at byte `p` decodes to slabs `ss` (checkpoint.rs:197-240).
#[logic(open)]
pub fn region_at(d: Seq<u8>, p: Int, ss: Seq<SlabDescriptor>) -> bool {
    pearlite! {
        p + 4 <= d.len() && d32(d, p)@ == ss.len()
        && forall<s: Int> 0 <= s && s < ss.len() ==> slab_at(d, p + 4 + sl_len(ss, s), ss[s])
    }
}

/// LEVEL-3 (phase D-A): every descriptor deserialize_slabs decodes has at most u32::MAX keys
/// (its slot count is a u32 field, checkpoint.rs:215-224) — the decoder fact `t_dec_keys`.
#[logic]
#[requires(parsed(d, v))]
#[ensures(forall<r: Int, q: Int> 0 <= r && r < v.len() && 0 <= q && q < v[r]@.len() ==> t_dec_keys(v[r]@[q]))]
pub fn lemma_parsed_keys_u32(d: Seq<u8>, v: Seq<Vec<SlabDescriptor>>) {}

/// `v` is the decoding of `d`.
#[logic(open)]
pub fn parsed(d: Seq<u8>, v: Seq<Vec<SlabDescriptor>>) -> bool {
    pearlite! {
        d.len() >= 4 && d32(d, 0)@ == v.len()
        && forall<r: Int> 0 <= r && r < v.len() ==> region_at(d, 4 + rg_len(v, r), v[r]@)
    }
}

#[logic]
#[requires(0 <= s && s <= a.len())]
#[variant(s)]
#[ensures(sl_len(a.push_back(x), s) == sl_len(a, s))]
pub fn lemma_sl_push(a: Seq<SlabDescriptor>, x: SlabDescriptor, s: Int) {
    pearlite! { if s > 0 { lemma_sl_push(a, x, s - 1) } }
}

#[logic]
#[requires(0 <= r && r <= a.len())]
#[variant(r)]
#[ensures(rg_len(a.push_back(x), r) == rg_len(a, r))]
pub fn lemma_rg_push(a: Seq<Vec<SlabDescriptor>>, x: Vec<SlabDescriptor>, r: Int) {
    pearlite! { if r > 0 { lemma_rg_push(a, x, r - 1) } }
}

/// `for _ in 0..num_slots { keys.push(u64::from_le_bytes(..)); pos += 8 }` (checkpoint.rs:221-226).
#[requires(pos@ + 8 * n@ <= data@.len())]
#[ensures(result@.len() == n@)]
#[ensures(forall<u: Int> 0 <= u && u < n@ ==> result@[u] == d64(data@, pos@ + 8 * u))]
pub fn read_keys7(data: &Vec<u8>, pos: usize, n: usize) -> Vec<u64> {
    let mut keys: Vec<u64> = Vec::new();
    let mut p = pos;
    let mut t: usize = 0;
    #[invariant(t@ <= n@ && keys@.len() == t@ && p@ == pos@ + 8 * t@)]
    #[invariant(forall<u: Int> 0 <= u && u < t@ ==> keys@[u] == d64(data@, pos@ + 8 * u))]
    while t < n {
        let k = rd64(data, p);
        p += 8;
        keys.push(k);
        t += 1;
    }
    keys
}

/// The slabs of one region (checkpoint.rs:198-238): the slab count at `p`, then the records.
/// Returns the slabs and the position after them.
#[requires(p@ + 4 <= data@.len() && data@.len() <= 9223372036854775807)]
#[ensures(match result {
    Ok((ss, q)) => end_slabs(data@, p@ + 4, d32(data@, p@)@) == Some(q@) && q@ <= data@.len()
        && region_at(data@, p@, ss@) && q@ == p@ + 4 + sl_len(ss@, ss@.len()),
    Err(e) => end_slabs(data@, p@ + 4, d32(data@, p@)@) == None && e == EmError::CorruptMetadata,
})]
pub fn region7(data: &Vec<u8>, p: usize) -> Result<(Vec<SlabDescriptor>, usize), EmError> {
    let num_slabs = rd32(data, p) as usize;
    let mut pos = p + 4;
    let mut slabs: Vec<SlabDescriptor> = Vec::new();
    let mut j: usize = 0;
    #[invariant(j@ <= num_slabs@ && slabs@.len() == j@)]
    #[invariant(pos@ == p@ + 4 + sl_len(slabs@, j@) && pos@ <= data@.len())]
    #[invariant(forall<s: Int> 0 <= s && s < j@ ==> slab_at(data@, p@ + 4 + sl_len(slabs@, s), slabs@[s]))]
    #[invariant(end_slabs(data@, p@ + 4, num_slabs@) == end_slabs(data@, pos@, num_slabs@ - j@))]
    while j < num_slabs {
        if pos + 24 > data.len() {
            return Err(EmError::CorruptMetadata);
        }
        let start_offset = rd64(data, pos);
        let slab_size = rd64(data, pos + 8);
        let element_size = rd32(data, pos + 16);
        let num_slots = rd32(data, pos + 20) as usize;
        let q = snapshot! { pos };
        pos += 24;
        let keys_bytes = num_slots * 8;
        if pos + keys_bytes > data.len() {
            return Err(EmError::CorruptMetadata);
        }
        let keys = read_keys7(data, pos, num_slots);
        pos += keys_bytes;
        let old = snapshot! { slabs@ };
        let sd = SlabDescriptor { start_offset, slab_size, element_size, keys };
        let sdg = snapshot! { sd };
        proof_assert! { slab_at(data@, q@, *sdg) };
        slabs.push(sd);
        proof_assert! { slabs@ == old.push_back(*sdg) };
        proof_assert! { forall<s: Int> 0 <= s && s <= j@ ==> { lemma_sl_push(*old, *sdg, s); sl_len(slabs@, s) == sl_len(*old, s) } };
        proof_assert! { sl_len(slabs@, j@ + 1) == sl_len(*old, j@) + 24 + 8 * num_slots@ };
        j += 1;
    }
    Ok((slabs, pos))
}

/// Mirror of `checkpoint::deserialize_slabs` (checkpoint.rs:183-241), exact.
/// (`Vec::with_capacity` is `Vec::new`: allocation failure is not modelled.)
#[requires(data@.len() <= 9223372036854775807)]
#[ensures(match result {
    Ok(v) => ck_complete(data@) && parsed(data@, v@),
    Err(e) => !ck_complete(data@) && e == EmError::CorruptMetadata,
})]
pub fn deser7(data: &Vec<u8>) -> Result<Vec<Vec<SlabDescriptor>>, EmError> {
    if data.len() < 4 {
        return Err(EmError::CorruptMetadata);
    }
    let region_count = rd32(data, 0) as usize;
    let mut pos: usize = 4;
    let mut result: Vec<Vec<SlabDescriptor>> = Vec::new();
    let mut i: usize = 0;
    #[invariant(i@ <= region_count@ && result@.len() == i@)]
    #[invariant(pos@ == 4 + rg_len(result@, i@) && pos@ <= data@.len())]
    #[invariant(forall<r: Int> 0 <= r && r < i@ ==> region_at(data@, 4 + rg_len(result@, r), result@[r]@))]
    #[invariant(end_regions(data@, 4, region_count@) == end_regions(data@, pos@, region_count@ - i@))]
    while i < region_count {
        if pos + 4 > data.len() {
            return Err(EmError::CorruptMetadata);
        }
        let (slabs, np) = match region7(data, pos) {
            Ok(x) => x,
            Err(e) => return Err(e),
        };
        let old = snapshot! { result@ };
        let sg = snapshot! { slabs };
        result.push(slabs);
        proof_assert! { result@ == old.push_back(*sg) };
        proof_assert! { forall<r: Int> 0 <= r && r <= i@ ==> { lemma_rg_push(*old, *sg, r); rg_len(result@, r) == rg_len(*old, r) } };
        proof_assert! { rg_len(result@, i@ + 1) == rg_len(*old, i@) + 4 + sl_len(sg@, sg@.len()) };
        pos = np;
        i += 1;
    }
    Ok(result)
}

// =============================================================================
// recover (recovery.rs:11-71) and initialize (lib.rs:514-582)
// =============================================================================

/// A checkpoint-copy read that does not yield a decodable state: the read failed (device error
/// or header/CRC/seq check) or the payload is truncated.
#[logic(open)]
pub fn bad7(rd: Result<Vec<u8>, EmError>) -> bool {
    pearlite! { match rd { Err(_) => true, Ok(d) => !ck_complete(d@) } }
}

/// The copy read yielded a complete payload decoding to `pr`.
#[logic(open)]
pub fn good7(rd: Result<Vec<u8>, EmError>, pr: Seq<Vec<SlabDescriptor>>) -> bool {
    pearlite! { match rd { Err(_) => false, Ok(d) => ck_complete(d@) && parsed(d@, pr) } }
}

/// Every pair `recover` can return on these inputs (recovery.rs:15-66).
#[logic(open)]
pub fn front_ok7(sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>,
                 sb: Superblock, pr: Seq<Vec<SlabDescriptor>>) -> bool {
    pearlite! {
        match sbr {
            Err(_) => false,
            Ok(b) => sb_ok7(b@) && sb == sb_of(b@)
                && (if sb.checkpoint_seq@ == 0 {
                        pr.len() == sb.region_count@ && forall<i: Int> 0 <= i && i < pr.len() ==> pr[i]@.len() == 0
                    } else {
                        match rd_a {
                            Ok(_) => good7(rd_a, pr),
                            Err(_) => sb.checkpoint_seq@ > 1 && good7(rd_i, pr),
                        }
                    }),
        }
    }
}

/// What recover needs of the inputs to avoid overflow (recovery.rs:23-26: `1 - active_copy` on
/// u8 and the copy offsets): only for a decodable superblock with a checkpoint; and the payload
/// length bound every Rust slice satisfies.
#[logic(open)]
pub fn rec_pre7(sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> bool {
    pearlite! {
        (match sbr {
            Ok(b) => sb_ok7(b@) && sb_of(b@).checkpoint_seq@ > 0 ==>
                a_0de740(sb_of(b@).active_copy@)
                && a_a23fc2(sb_of(b@).checkpoint_region_offset@, sb_of(b@).checkpoint_region_size@),
            Err(e) => e == EmError::IoError,
        })
        && (match rd_a { Ok(d) => t_slice_len(d@.len()), Err(_) => true })
        && (match rd_i { Ok(d) => t_slice_len(d@.len()), Err(_) => true })
    }
}

/// Error view of a result (None = Ok).
#[logic(open)]
pub fn eo<T>(r: Result<T, EmError>) -> Option<EmError> {
    pearlite! { match r { Ok(_) => None, Err(e) => Some(e) } }
}

/// The superblock read failed: its error is returned (recovery.rs:15 `?`).
#[logic(open)]
pub fn rp_sbio(sbr: Result<Vec<u8>, EmError>, o: Option<EmError>) -> bool {
    pearlite! { match sbr { Err(e) => o == Some(e), Ok(_) => true } }
}

/// The superblock bytes fail a check: CorruptMetadata (recovery.rs:16 `?`).
#[logic(open)]
pub fn rp_sbbad(sbr: Result<Vec<u8>, EmError>, o: Option<EmError>) -> bool {
    pearlite! { match sbr { Ok(b) => !sb_ok7(b@) ==> o == Some(EmError::CorruptMetadata), Err(_) => true } }
}

/// Neither the active copy nor (when one exists) the previous one yields a state:
/// CorruptMetadata (recovery.rs:29-70).
#[logic(open)]
pub fn rp_both(sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>, o: Option<EmError>) -> bool {
    pearlite! {
        match sbr {
            Ok(b) => sb_ok7(b@) && sb_of(b@).checkpoint_seq@ > 0 && bad7(rd_a) && (sb_of(b@).checkpoint_seq@ == 1 || bad7(rd_i))
                ==> o == Some(EmError::CorruptMetadata),
            Err(_) => true,
        }
    }
}

/// The active copy passed its checks but its payload is truncated: CorruptMetadata
/// (recovery.rs:36 `?`, no fallback).
#[logic(open)]
pub fn rp_mal(sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, o: Option<EmError>) -> bool {
    pearlite! {
        match (sbr, rd_a) {
            (Ok(b), Ok(d)) => sb_ok7(b@) && sb_of(b@).checkpoint_seq@ > 0 && !ck_complete(d@) ==> o == Some(EmError::CorruptMetadata),
            _ => true,
        }
    }
}

/// A decodable superblock with no checkpoint yet, or a copy that yields a state: Ok.
#[logic(open)]
pub fn rp_okc(sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>, o: Option<EmError>) -> bool {
    pearlite! {
        match sbr {
            Ok(b) => sb_ok7(b@) && (sb_of(b@).checkpoint_seq@ == 0
                || (match rd_a { Ok(d) => ck_complete(d@), Err(_) => false })
                || (match rd_a { Ok(_) => false, Err(_) => sb_of(b@).checkpoint_seq@ > 1 && !bad7(rd_i) }))
                ==> o == None,
            Err(_) => true,
        }
    }
}

/// Mirror of `recovery::recover` (recovery.rs:11-71). `sbr` = `read_blocks(0, SUPERBLOCK_SIZE)`;
/// `rd_a` / `rd_i` = `read_checkpoint_region` of the active copy (expected `checkpoint_seq`) and of
/// the inactive copy (expected `checkpoint_seq - 1`); the log calls are not modelled.
#[requires(rec_pre7(sbr, rd_a, rd_i))]
#[ensures(match result { Ok((sb, pr)) => front_ok7(sbr, rd_a, rd_i, sb, pr@), Err(_) => true })]
#[ensures(rp_sbio(sbr, eo(result)))]
#[ensures(rp_sbbad(sbr, eo(result)))]
#[ensures(rp_both(sbr, rd_a, rd_i, eo(result)))]
#[ensures(rp_mal(sbr, rd_a, eo(result)))]
#[ensures(rp_okc(sbr, rd_a, rd_i, eo(result)))]
pub fn recover7(sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>)
    -> Result<(Superblock, Vec<Vec<SlabDescriptor>>), EmError> {
    let sb_data = match sbr {
        Ok(b) => b,
        Err(e) => return Err(e),
    };
    let sb = match deserialize7(&sb_data) {
        Ok(s) => s,
        Err(e) => return Err(e),
    };
    if sb.checkpoint_seq == 0 {
        let mut empty: Vec<Vec<SlabDescriptor>> = Vec::new();
        let mut i: usize = 0;
        #[invariant(i@ <= sb.region_count@ && empty@.len() == i@)]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==> empty@[j]@.len() == 0)]
        while i < sb.region_count as usize {
            empty.push(Vec::new());
            i += 1;
        }
        return Ok((sb, empty));
    }
    let _active_offset = sb.checkpoint_region_offset + sb.active_copy as u64 * sb.checkpoint_region_size;
    let _inactive_offset = sb.checkpoint_region_offset + (1 - sb.active_copy) as u64 * sb.checkpoint_region_size;
    match rd_a {
        Ok(data) => {
            let regions = deser7(&data)?;
            return Ok((sb, regions));
        }
        Err(_) => {}
    }
    let prev_seq = if sb.checkpoint_seq >= 1 { sb.checkpoint_seq - 1 } else { 0 }; // saturating_sub
    if prev_seq > 0 {
        match rd_i {
            Ok(data) => {
                let regions = deser7(&data)?;
                return Ok((sb, regions));
            }
            Err(_) => {}
        }
    }
    Err(EmError::CorruptMetadata)
}

/// What `initialize`'s rebuild (component.rs `initialize`) relies on, for every pair recover can
/// return on these inputs.
#[logic(open)]
pub fn init_pre7(sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>, rd_i: Result<Vec<u8>, EmError>) -> bool {
    pearlite! {
        rec_pre7(sbr, rd_a, rd_i)
        && forall<sb: Superblock, pr: Seq<Vec<SlabDescriptor>>> front_ok7(sbr, rd_a, rd_i, sb, pr) ==> sb_sane(sb) && recovered_ok(sb, pr)
    }
}

impl ExtentManager {
    /// lib.rs:514-519: the namespace (lib.rs:517), `get_metadata_client(ns_id)?` (lib.rs:189-213:
    /// the receptacle -> NotInitialized; `connect_client` (`conn`) and `sector_size(ns)` (`sq`)
    /// failures -> nvme_to_em = IoError), then `recovery::recover(..)?`. Takes `&self`.
    #[requires(self.dev.connected && conn && sq ==> rec_pre7(sbr, rd_a, rd_i))]
    #[ensures(!self.dev.connected ==> result == Err(EmError::NotInitialized))]
    #[ensures(self.dev.connected && (!conn || !sq) ==> result == Err(EmError::IoError))]
    #[ensures(self.dev.connected && conn && sq ==> rp_sbio(sbr, eo(result)) && rp_sbbad(sbr, eo(result))
        && rp_both(sbr, rd_a, rd_i, eo(result)) && rp_mal(sbr, rd_a, eo(result)) && rp_okc(sbr, rd_a, rd_i, eo(result)))]
    #[ensures(match result { Ok((sb, pr)) => front_ok7(sbr, rd_a, rd_i, sb, pr@), Err(_) => true })]
    pub fn init_front7(&self, conn: bool, sq: bool, sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>,
                       rd_i: Result<Vec<u8>, EmError>) -> Result<(Superblock, Vec<Vec<SlabDescriptor>>), EmError> {
        let _ns_id = match self.metadata_ns_id { Some(n) => n, None => 1u32 };
        // get_metadata_client (lib.rs:189-213)
        if !self.dev.connected {
            return Err(EmError::NotInitialized);
        }
        if !conn {
            return Err(EmError::IoError);
        }
        if !sq {
            return Err(EmError::IoError);
        }
        recover7(sbr, rd_a, rd_i)
    }

    /// Mirror of `IExtentManager::initialize` (lib.rs:514-582): `init_front7` (lib.rs:514-519, no
    /// state change) then component.rs's `initialize` mirror of lib.rs:521-581 (no error path;
    /// publishes regions and shared state at lib.rs:576-577).
    #[requires(em_regions_wf(*self))]
    #[requires(self.dev.connected && conn && sq ==> init_pre7(sbr, rd_a, rd_i))]
    #[ensures(match result { Err(_) => ^self == *self, Ok(()) => true })]
    #[ensures(!self.dev.connected ==> result == Err(EmError::NotInitialized))]
    #[ensures(self.dev.connected && (!conn || !sq) ==> result == Err(EmError::IoError))]
    #[ensures(self.dev.connected && conn && sq ==> rp_sbio(sbr, eo(result)) && rp_sbbad(sbr, eo(result))
        && rp_both(sbr, rd_a, rd_i, eo(result)) && rp_mal(sbr, rd_a, eo(result)) && rp_okc(sbr, rd_a, rd_i, eo(result)))]
    #[ensures(result == Ok(()) ==> match ((^self).regions, (^self).shared, sbr) {
        (Some(rv), Some(sh), Ok(b)) => sh.superblock == sb_of(b@) && rv@.len() == sb_of(b@).region_count@,
        _ => false,
    })]
    #[ensures(result == Ok(()) ==> em_regions_wf(^self) && em_regions_ok(^self))]
    #[ensures(result == Ok(()) ==> exists<sb: Superblock, pr: Seq<Vec<SlabDescriptor>>> front_ok7(sbr, rd_a, rd_i, sb, pr)
        && match ((^self).regions, (^self).shared) {
            (Some(rv), Some(sh)) => sh.superblock == sb
                && forall<i: Int> 0 <= i && i < rv@.len() ==> init_rg_ok((^self).arena@[rv@[i]@], dsel(pr, i)),
            _ => false,
        })]
    pub fn initialize7(&mut self, conn: bool, sq: bool, sbr: Result<Vec<u8>, EmError>, rd_a: Result<Vec<u8>, EmError>,
                       rd_i: Result<Vec<u8>, EmError>) -> Result<(), EmError> {
        let (sb, pr) = match self.init_front7(conn, sq, sbr, rd_a, rd_i) {
            Ok(x) => x,
            Err(e) => return Err(e),
        };
        self.initialize(sb, &pr);
        Ok(())
    }
}

// =============================================================================
// format (lib.rs:383-511) with every external outcome as an oracle
// =============================================================================

/// What `format` leaves unchanged when it fails: every in-memory field of the component
/// (regions, shared state, namespace, base LBAs) and every region object that existed (arena
/// prefix — handles reserved before the call keep acting on the same objects). New arena entries
/// are the unreachable `Arc`s format built before failing; the metadata device is not in-memory
/// state (a failed superblock write may have reached it).
#[logic(open)]
pub fn fmt_frame7(a: ExtentManager, b: ExtentManager) -> bool {
    pearlite! {
        b.regions == a.regions && b.shared == a.shared && b.metadata_ns_id == a.metadata_ns_id
        && b.metadata_base_lba == a.metadata_base_lba && b.data_base_lba == a.data_base_lba
        && arena_prefix(a.arena@, b.arena@, a.arena@.len())
    }
}

/// Every check of lib.rs:384-452 passes and an instance id is available (lib.rs:471-481): only
/// the client (lib.rs:495) and the superblock write (lib.rs:498) remain.
#[logic(open)]
pub fn fmt_reach7(em: ExtentManager, p: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>) -> bool {
    pearlite! {
        fr002(p) && em.dev.connected
        && match (qn, qs) {
            (Some(n), Some(s)) =>
                cksize_l(n@ * s@, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@) > 0
                && p.data_disk_size@ > ds_l(n@ * s@, p.metadata_region_size@, p.metadata_alignment@, p.sector_size@),
            _ => false,
        }
        && (p.instance_id != None || rnd != None)
    }
}

impl ExtentManager {
    /// Mirror of `IExtentManager::format` (lib.rs:383-511). Oracles: `qn` / `qs` the device's
    /// answers to num_sectors / sector_size (lib.rs:410-415, None = the call failed), `rnd` the 8
    /// bytes of /dev/urandom (None = open/read failed), `conn` / `sq` connect_client and
    /// sector_size(ns) inside get_metadata_client (lib.rs:495 -> 189-213), `wr` the outcome of
    /// `write_blocks(0, &sb.serialize())` (lib.rs:498; IBlockDevice + block_io.rs: Ok or IoError).
    /// The arithmetic headroom is required only once the FR-002 checks have passed.
    /// LEVEL-3 (phase D-B2): the parameter ranges are [`fmt_sane`] (e0fe79, 3b58ea — both list `format`),
    /// not `sane_params`: no 7cb7c3 (format accepts slab_size 0, lib.rs:383-512).
    #[requires(fr002(params) ==> a_2ee144(params.metadata_alignment@) && fmt_sane(params))]
    #[requires(fr002(params) ==> match (qn, qs) { (Some(n), Some(s)) => a_67ea4f(n@, s@), _ => true })]
    #[requires(wr == Ok(()) || wr == Err(EmError::IoError))]
    #[ensures(!fr002(params) ==> result == Err(EmError::CorruptMetadata) && ^self == *self)]
    #[ensures(match result { Err(_) => fmt_frame7(*self, ^self), Ok(()) => true })]
    #[ensures(fmt_reach7(*self, params, qn, qs, rnd) && !(conn && sq) ==> result == Err(EmError::IoError))]
    #[ensures(fmt_reach7(*self, params, qn, qs, rnd) && conn && sq ==> result == wr)]
    #[ensures(result == Ok(()) ==> match ((^self).regions, (^self).shared) {
        (Some(rv), Some(sh)) => rv@.len() == params.region_count@
            && sh.superblock.checkpoint_region_offset@ == ckoff_l(params.metadata_alignment@)
            && (^self).dev.sb == Some(sh.superblock),
        _ => false,
    })]
    pub fn format7(&mut self, params: FormatParams, qn: Option<u64>, qs: Option<u32>, rnd: Option<u64>,
                   conn: bool, sq: bool, wr: Result<(), EmError>) -> Result<(), EmError> {
        if params.sector_size == 0 {
            return Err(EmError::CorruptMetadata);
        }
        if params.slab_size % params.sector_size as u64 != 0 {
            return Err(EmError::CorruptMetadata);
        }
        if params.max_extent_size as u64 > params.slab_size {
            return Err(EmError::CorruptMetadata);
        }
        if params.region_count == 0 || !params.region_count.is_power_of_two() {
            return Err(EmError::CorruptMetadata);
        }
        proof_assert! { fr002(params) };
        proof_assert! { lemma_fmt_hd(params); params.slab_size@ + params.sector_size@ <= u64::MAX@
            && 2 * params.data_disk_size@ + params.sector_size@ <= u64::MAX@ };
        let data_disk_size = params.data_disk_size;
        // lib.rs:406-409
        if !self.dev.connected {
            return Err(EmError::NotInitialized);
        }
        // lib.rs:410-415
        let n = match qn {
            Some(n) => n,
            None => return Err(EmError::IoError),
        };
        let s = match qs {
            Some(s) => s,
            None => return Err(EmError::IoError),
        };
        let metadata_disk_size = n * s as u64;
        // lib.rs:418-438
        let alignment = params.metadata_alignment;
        let sb_size = SUPERBLOCK_SIZE as u64;
        let checkpoint_region_offset = if alignment == 0 { sb_size } else { (sb_size + alignment - 1) / alignment * alignment /* lib.rs:423, same evaluation order (debug overflow semantics; 2ee144) */ };
        let effective_metadata_size = if params.metadata_region_size > 0 {
            if metadata_disk_size <= params.metadata_region_size { metadata_disk_size } else { params.metadata_region_size }
        } else {
            metadata_disk_size
        };
        let remaining = if effective_metadata_size >= checkpoint_region_offset {
            effective_metadata_size - checkpoint_region_offset
        } else {
            0
        }; // saturating_sub (lib.rs:430)
        let sector_size_u64 = params.sector_size as u64;
        let checkpoint_region_size = (remaining / 2) / sector_size_u64 * sector_size_u64;
        proof_assert! { lemma_divmod(remaining@ / 2, sector_size_u64@); checkpoint_region_size@ <= remaining@ / 2 };
        proof_assert! { checkpoint_region_offset@ == ckoff_l(params.metadata_alignment@) };
        proof_assert! { effective_metadata_size@ == eff_l(metadata_disk_size@, params.metadata_region_size@) };
        proof_assert! { checkpoint_region_size@ == cksize_l(metadata_disk_size@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) };
        proof_assert! { checkpoint_region_size@ > 0 ==> checkpoint_region_offset@ + 2 * checkpoint_region_size@ <= effective_metadata_size@ };
        if checkpoint_region_size == 0 {
            return Err(EmError::CorruptMetadata);
        }
        // lib.rs:442-452
        let data_start_offset = if params.metadata_region_size > 0 { checkpoint_region_offset + 2 * checkpoint_region_size } else { 0 };
        proof_assert! { data_start_offset@ == ds_l(metadata_disk_size@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) };
        proof_assert! { qn == Some(n) && qs == Some(s) && metadata_disk_size@ == n@ * s@ };
        proof_assert! { fmt_reach7(*self, params, qn, qs, rnd) ==> data_disk_size@ > data_start_offset@ };
        let usable_data_size = if data_disk_size >= data_start_offset { data_disk_size - data_start_offset } else { 0 };
        if usable_data_size == 0 {
            proof_assert! { !fmt_reach7(*self, params, qn, qs, rnd) };
            return Err(EmError::CorruptMetadata);
        }
        // lib.rs:454-468: regions (built locally; published only at lib.rs:506)
        let region_count = params.region_count as usize;
        let a0 = snapshot! { self.arena };
        let mut region_vec: Vec<usize> = Vec::new();
        let mut i: usize = 0;
        #[invariant(i@ <= region_count@ && region_vec@.len() == i@)]
        #[invariant(self.arena@.len() == a0@.len() + i@)]
        #[invariant(arena_prefix(a0@, self.arena@, a0@.len()))]
        while i < region_count {
            let (base, size) = region_extent(data_start_offset, usable_data_size, region_count, i);
            let buddy = BuddyAllocator::new(base, size, params.sector_size);
            let region = RegionState::new(buddy, params);
            let ar0 = snapshot! { self.arena };
            self.arena.push(region);
            proof_assert! { forall<j: Int> 0 <= j && j < ar0@.len() ==> self.arena@[j] == ar0@[j] };
            region_vec.push(self.arena.len() - 1);
            i += 1;
        }
        // lib.rs:471-481
        let instance_id = match params.instance_id {
            Some(id) => id,
            None => match rnd {
                Some(v) => v,
                None => return Err(EmError::IoError),
            },
        };
        let sb = Superblock::new(
            data_disk_size,
            params.sector_size,
            params.slab_size,
            params.max_extent_size,
            params.region_count,
            checkpoint_region_offset,
            checkpoint_region_size,
            instance_id,
            params.metadata_disk_ns_id,
            data_start_offset,
        );
        // lib.rs:495: get_metadata_client (the receptacle is bound: lib.rs:406-409 passed)
        if !conn {
            return Err(EmError::IoError);
        }
        if !sq {
            return Err(EmError::IoError);
        }
        // lib.rs:496-498
        let _sb_data = sb.serialize();
        match wr {
            Ok(()) => {}
            Err(e) => return Err(e),
        }
        self.dev.sb = Some(sb);
        // lib.rs:500-507
        let shared = SharedState { format_params: params, checkpoint_seq: 0, superblock: sb };
        self.publish_regions(region_vec);
        self.publish_shared(shared);
        Ok(())
    }
}
