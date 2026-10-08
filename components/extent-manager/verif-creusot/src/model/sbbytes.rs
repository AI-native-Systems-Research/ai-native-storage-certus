//! Byte-level mirror of `Superblock::serialize` / `Superblock::deserialize`
//! (components/extent-manager/src/superblock.rs:58-168).
//!
//! Trusted std leaves (their contracts are the documented std / crc32fast semantics):
//! * `u64::to_le_bytes` / `u32::to_le_bytes` — the bytes are `le64(v, k)` / `le32(v, k)`
//!   (opaque logic encodings), and `from_le_bytes` inverts them (decoding an encoding gives
//!   the encoded value back). The encodings' concrete bit layout is NOT modelled.
//! * `crc32fast::hash(&buf[..n])` — an opaque deterministic function `crc32` of the bytes.
//! The `buf[pos..pos + N].copy_from_slice(&x.to_le_bytes())` writes and the matching reads
//! are native loops over `Vec<u8>` ([`put64`], [`put32`], [`get64`], [`get32`]).
use crate::model::params::*;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// Byte `k` (0..8) of the little-endian encoding of `v`.
#[logic(opaque)]
pub fn le64(v: u64, k: Int) -> u8 {
    dead
}

/// Byte `k` (0..4) of the little-endian encoding of `v`.
#[logic(opaque)]
pub fn le32(v: u32, k: Int) -> u8 {
    dead
}

/// CRC32 of a byte sequence (`crc32fast::hash`).
#[logic(opaque)]
pub fn crc32(s: Seq<u8>) -> u32 {
    dead
}

/// TRUSTED std leaf: `v.to_le_bytes()`.
#[trusted]
#[ensures(result@.len() == 8 && forall<k: Int> 0 <= k && k < 8 ==> result@[k] == le64(v, k))]
pub fn to_le_bytes64(v: u64) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// TRUSTED std leaf: `v.to_le_bytes()`.
#[trusted]
#[ensures(result@.len() == 4 && forall<k: Int> 0 <= k && k < 4 ==> result@[k] == le32(v, k))]
pub fn to_le_bytes32(v: u32) -> Vec<u8> {
    v.to_le_bytes().to_vec()
}

/// TRUSTED std leaf: `u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap())`.
#[trusted]
#[requires(pos@ + 8 <= buf@.len())]
#[ensures(forall<v: u64> (forall<k: Int> 0 <= k && k < 8 ==> buf@[pos@ + k] == le64(v, k)) ==> result == v)]
pub fn get64(buf: &Vec<u8>, pos: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&buf[pos..pos + 8]);
    u64::from_le_bytes(a)
}

/// TRUSTED std leaf: `u32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap())`.
#[trusted]
#[requires(pos@ + 4 <= buf@.len())]
#[ensures(forall<v: u32> (forall<k: Int> 0 <= k && k < 4 ==> buf@[pos@ + k] == le32(v, k)) ==> result == v)]
pub fn get32(buf: &Vec<u8>, pos: usize) -> u32 {
    let mut a = [0u8; 4];
    a.copy_from_slice(&buf[pos..pos + 4]);
    u32::from_le_bytes(a)
}

/// TRUSTED leaf: `crc32fast::hash(&buf[..n])` (superblock.rs:93, 144).
#[trusted]
#[requires(n@ <= buf@.len())]
#[ensures(result == crc32(buf@.subsequence(0, n@)))]
pub fn crc32_hash(buf: &Vec<u8>, n: usize) -> u32 {
    0
}

/// `buf[pos..pos + 8].copy_from_slice(&v.to_le_bytes())`.
#[requires(pos@ + 8 <= buf@.len())]
#[ensures((^buf)@.len() == buf@.len())]
#[ensures(forall<k: Int> 0 <= k && k < 8 ==> (^buf)@[pos@ + k] == le64(v, k))]
#[ensures(forall<q: Int> 0 <= q && q < buf@.len() && (q < pos@ || q >= pos@ + 8) ==> (^buf)@[q] == buf@[q])]
pub fn put64(buf: &mut Vec<u8>, pos: usize, v: u64) {
    let b = to_le_bytes64(v);
    let old = snapshot! { *buf };
    let mut k: usize = 0;
    #[invariant(k@ <= 8 && buf@.len() == old@.len())]
    #[invariant(forall<j: Int> 0 <= j && j < k@ ==> buf@[pos@ + j] == le64(v, j))]
    #[invariant(forall<q: Int> 0 <= q && q < old@.len() && (q < pos@ || q >= pos@ + k@) ==> buf@[q] == old@[q])]
    while k < 8 {
        buf[pos + k] = b[k];
        k += 1;
    }
}

/// `buf[pos..pos + 4].copy_from_slice(&v.to_le_bytes())`.
#[requires(pos@ + 4 <= buf@.len())]
#[ensures((^buf)@.len() == buf@.len())]
#[ensures(forall<k: Int> 0 <= k && k < 4 ==> (^buf)@[pos@ + k] == le32(v, k))]
#[ensures(forall<q: Int> 0 <= q && q < buf@.len() && (q < pos@ || q >= pos@ + 4) ==> (^buf)@[q] == buf@[q])]
pub fn put32(buf: &mut Vec<u8>, pos: usize, v: u32) {
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

/// The 8 bytes at `pos` encode `v`.
#[logic(open)]
pub fn has64(s: Seq<u8>, pos: Int, v: u64) -> bool {
    pearlite! { forall<k: Int> 0 <= k && k < 8 ==> s[pos + k] == le64(v, k) }
}

/// The 4 bytes at `pos` encode `v`.
#[logic(open)]
pub fn has32(s: Seq<u8>, pos: Int, v: u32) -> bool {
    pearlite! { forall<k: Int> 0 <= k && k < 4 ==> s[pos + k] == le32(v, k) }
}

/// `s` is the superblock layout of `sb` (superblock.rs:58-96): every field at its offset,
/// reserved bytes 49..56 and padding 96..4096 zero, CRC32 of bytes 0..92 at byte 92.
#[logic(open)]
pub fn sb_layout(s: Seq<u8>, sb: Superblock) -> bool {
    pearlite! {
        s.len() == 4096
        && has64(s, 0, sb.magic) && has32(s, 8, sb.version)
        && has64(s, 12, sb.data_disk_size) && has32(s, 20, sb.sector_size)
        && has64(s, 24, sb.slab_size) && has32(s, 32, sb.max_extent_size)
        && has32(s, 36, sb.region_count) && has64(s, 40, sb.checkpoint_seq)
        && s[48] == sb.active_copy
        && (forall<q: Int> 49 <= q && q < 56 ==> s[q] == 0u8)
        && has64(s, 56, sb.checkpoint_region_offset) && has64(s, 64, sb.checkpoint_region_size)
        && has64(s, 72, sb.instance_id) && has32(s, 80, sb.metadata_disk_ns_id)
        && has64(s, 84, sb.data_start_offset)
        && has32(s, 92, crc32(s.subsequence(0, 92)))
        && (forall<q: Int> 96 <= q && q < 4096 ==> s[q] == 0u8)
    }
}

impl Superblock {
    /// Mirror of `Superblock::serialize` (superblock.rs:58-97).
    #[ensures(sb_layout(result@, *self))]
    pub fn serialize(&self) -> Vec<u8> {
        let mut buf: Vec<u8> = creusot_std::vec![0u8; 4096];
        put64(&mut buf, 0, self.magic);
        put32(&mut buf, 8, self.version);
        put64(&mut buf, 12, self.data_disk_size);
        put32(&mut buf, 20, self.sector_size);
        put64(&mut buf, 24, self.slab_size);
        put32(&mut buf, 32, self.max_extent_size);
        put32(&mut buf, 36, self.region_count);
        put64(&mut buf, 40, self.checkpoint_seq);
        buf[48] = self.active_copy;
        // 7 bytes reserved (superblock.rs:80-81)
        put64(&mut buf, 56, self.checkpoint_region_offset);
        put64(&mut buf, 64, self.checkpoint_region_size);
        put64(&mut buf, 72, self.instance_id);
        put32(&mut buf, 80, self.metadata_disk_ns_id);
        put64(&mut buf, 84, self.data_start_offset);
        let pre = snapshot! { buf@ };
        let crc = crc32_hash(&buf, 92);
        put32(&mut buf, 92, crc);
        proof_assert! { forall<q: Int> 0 <= q && q < 92 ==> buf@[q] == pre[q] };
        proof_assert! { buf@.subsequence(0, 92).ext_eq(pre.subsequence(0, 92)) };
        buf
    }

    /// Mirror of `Superblock::deserialize` (superblock.rs:99-168).
    #[ensures(match result {
        Ok(sb) => sb.magic == SUPERBLOCK_MAGIC,
        Err(e) => e == EmError::CorruptMetadata,
    })]
    #[ensures(forall<sb: Superblock> sb_layout(buf@, sb) && sb.magic == SUPERBLOCK_MAGIC ==> result == Ok(sb))]
    pub fn deserialize(buf: &Vec<u8>) -> Result<Superblock, EmError> {
        if buf.len() < SUPERBLOCK_SIZE {
            return Err(EmError::CorruptMetadata);
        }
        let magic = get64(buf, 0);
        if magic != SUPERBLOCK_MAGIC {
            return Err(EmError::CorruptMetadata);
        }
        let version = get32(buf, 8);
        let data_disk_size = get64(buf, 12);
        let sector_size = get32(buf, 20);
        let slab_size = get64(buf, 24);
        let max_extent_size = get32(buf, 32);
        let region_count = get32(buf, 36);
        let checkpoint_seq = get64(buf, 40);
        let active_copy = buf[48];
        // 7 bytes reserved (superblock.rs:130-131)
        let checkpoint_region_offset = get64(buf, 56);
        let checkpoint_region_size = get64(buf, 64);
        let instance_id = get64(buf, 72);
        let metadata_disk_ns_id = get32(buf, 80);
        let data_start_offset = get64(buf, 84);
        let stored_crc = get32(buf, 92);
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
}

/// What every superblock this component builds satisfies: written by Superblock::new
/// (magic, version 6, active copy 0, superblock.rs:27-56) and changed afterwards only by
/// write_checkpoint's `active_copy = 1 - active_copy` / `checkpoint_seq = seq + 1`
/// (checkpoint.rs:105-112).
#[logic(open)]
pub fn sb_wf(sb: Superblock) -> bool {
    pearlite! { sb.magic == SUPERBLOCK_MAGIC && sb.version == FORMAT_VERSION && sb.active_copy@ <= 1 }
}
