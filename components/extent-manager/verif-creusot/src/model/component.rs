//! Mirror of the `ExtentManager` component state and its `IExtentManager` methods
//! (`components/extent-manager/src/lib.rs:53-762`, `checkpoint.rs:35-115`).
//!
//! Locking model. Each `RwLock`/`Mutex` critical section of the source is one
//! atomic step here. `format`/`initialize` publish `regions` and `shared` under two
//! DIFFERENT locks (lib.rs:506-507, 576-577); the mirror keeps that split as two
//! functions (`publish_regions`, `publish_shared`) so an interleaving between them
//! can be expressed. Background checkpoint thread, condvar coalescing (lib.rs:729-761)
//! and logging are not modelled (no property of batch 1 depends on them).
//!
//! `Arc<RwLock<RegionState>>` aliasing is modelled with an ARENA: every
//! `RegionState` ever created lives in `arena` (append-only); `regions` holds arena
//! indices. A `WriteHandle`'s publish/abort closures capture their `Arc` — here the
//! handle's `region` index — so a handle reserved before a `format()` still acts on
//! the OLD region object, exactly like the source.
//!
//! Metadata device (`IBlockDevice`, per its declared contract): a ghost store of the
//! DECODED contents of the superblock sector and the two checkpoint copies, written
//! by contract-carrying `#[trusted]` leaves that may fail (`IoError`). Byte layout,
//! CRC and the client channel protocol (block_io.rs) are behind those leaves.
use crate::model::bitmap::*;
use crate::model::buddy::*;
use crate::model::params::*;
use crate::model::region::*;
use crate::model::slab::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use crate::model::l2::p2;
use creusot_std::prelude::*;

/// Decoded metadata-device contents (ghost sector store).
pub struct MetaDevice {
    /// `metadata_device` receptacle bound (lib.rs:406-409, 186-189).
    pub connected: bool,
    pub sector_size: u32,
    pub num_sectors: u64,
    /// Superblock sector (LBA 0) as last successfully written.
    pub sb: Option<Superblock>,
    /// Checkpoint copy 0/1 as `(seq, region count)` last successfully written.
    pub ckpt0: Option<(u64, usize)>,
    pub ckpt1: Option<(u64, usize)>,
}

/// TRUSTED device leaf: `metadata_client.write_blocks(0, &sb.serialize())`
/// (lib.rs:496-498, 302-307). Per the `IBlockDevice` contract it either succeeds
/// (the sector now decodes to `sb`) or reports an I/O error.
#[trusted]
#[requires(dev.connected)]
#[ensures(match result {
    Ok(()) => (^dev).sb == Some(sb) && (^dev).ckpt0 == dev.ckpt0 && (^dev).ckpt1 == dev.ckpt1,
    Err(e) => e == EmError::IoError,
})]
#[ensures((^dev).connected == dev.connected && (^dev).sector_size == dev.sector_size && (^dev).num_sectors == dev.num_sectors)]
pub fn dev_write_superblock(dev: &mut MetaDevice, sb: Superblock) -> Result<(), EmError> {
    dev.sb = Some(sb);
    Ok(())
}

/// TRUSTED device leaf: serialize all regions, size-check and write the inactive
/// checkpoint copy (checkpoint.rs:47-97). May fail (`CorruptMetadata` when the payload
/// exceeds the region, checkpoint.rs:69-75; `IoError` from the device).
#[trusted]
#[requires(dev.connected)]
#[ensures(match result {
    Ok(()) => (^dev).sb == dev.sb
        && (if copy@ == 0 { (^dev).ckpt0 == Some((seq, nregions)) && (^dev).ckpt1 == dev.ckpt1 }
            else { (^dev).ckpt1 == Some((seq, nregions)) && (^dev).ckpt0 == dev.ckpt0 }),
    Err(e) => (e == EmError::IoError || e == EmError::CorruptMetadata) && (^dev).sb == dev.sb,
})]
#[ensures((^dev).connected == dev.connected && (^dev).sector_size == dev.sector_size && (^dev).num_sectors == dev.num_sectors)]
pub fn dev_write_checkpoint(dev: &mut MetaDevice, copy: u8, seq: u64, nregions: usize) -> Result<(), EmError> {
    if copy == 0 { dev.ckpt0 = Some((seq, nregions)) } else { dev.ckpt1 = Some((seq, nregions)) }
    Ok(())
}

/// TRUSTED OS leaf: 8 bytes from `/dev/urandom` (lib.rs:474-479); may fail.
#[trusted]
#[ensures(match result { Ok(_) => true, Err(e) => e == EmError::IoError })]
pub fn urandom_u64() -> Result<u64, EmError> {
    Ok(0)
}

/// `WriteHandle` (interfaces/src/iextent_manager.rs:95-147) with its two closures
/// (lib.rs:597-621) reduced to the captured values they act on.
#[derive(::core::clone::Clone, ::core::marker::Copy)]
pub struct Handle {
    pub region: usize,
    pub key: u64,
    pub offset: u64,
    pub size: u32,
    pub slab_start: u64,
    pub slot_idx: usize,
}

pub struct ExtentManager {
    pub arena: Vec<RegionState>,
    pub regions: Option<Vec<usize>>,
    pub shared: Option<SharedState>,
    pub metadata_ns_id: Option<u32>,
    pub metadata_base_lba: u64,
    pub data_base_lba: u64,
    pub dev: MetaDevice,
}

// ------------------------------------------------------------------ logic view

/// Every current region index points into the arena.
#[logic(open)]
pub fn em_regions_wf(em: ExtentManager) -> bool {
    pearlite! {
        match em.regions {
            None => true,
            Some(rv) => rv@.len() > 0 && (forall<i: Int> 0 <= i && i < rv@.len() ==> rv@[i]@ < em.arena@.len())
                && (forall<i: Int, j: Int> 0 <= i && i < j && j < rv@.len() ==> rv@[i] != rv@[j]),
        }
    }
}

/// Every current region satisfies the region invariant.
#[logic(open)]
pub fn em_regions_ok(em: ExtentManager) -> bool {
    pearlite! {
        match em.regions {
            None => true,
            Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> rg_inv(em.arena@[rv@[i]@]),
        }
    }
}

/// LEVEL-3 (phase D-B2): the part of the region invariant the listing calls read — every slab of a
/// region is well-formed (`slab_inv`: slot geometry, one key per slot). Implied by `rg_inv`; holds
/// for every region `format` builds whatever its parameters (no slabs).
#[logic(open)]
pub fn rg_list_ok(r: RegionState) -> bool {
    pearlite! { forall<k: Int> r.slabs@.contains(k) ==> slab_inv(r.slabs@.lookup(k)) }
}

/// [`rg_list_ok`] of every current region.
#[logic(open)]
pub fn em_list_ok(em: ExtentManager) -> bool {
    pearlite! {
        match em.regions {
            None => true,
            Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> rg_list_ok(em.arena@[rv@[i]@]),
        }
    }
}

/// `rg_inv` implies [`rg_list_ok`].
#[logic]
#[requires(rg_inv(r))]
#[ensures(rg_list_ok(r))]
pub fn lemma_rg_list(r: RegionState) {
    pearlite! { proof_assert! { forall<k: Int> r.slabs@.contains(k) ==> slab_ok(r, r.slabs@.lookup(k)) } }
}

/// `em_regions_ok` implies [`em_list_ok`].
#[logic]
#[requires(em_regions_ok(em))]
#[ensures(em_list_ok(em))]
pub fn lemma_em_list(em: ExtentManager) {
    pearlite! {
        match em.regions {
            None => (),
            Some(rv) => proof_assert! { forall<i: Int> 0 <= i && i < rv@.len() ==> {
                lemma_rg_list(em.arena@[rv@[i]@]); rg_list_ok(em.arena@[rv@[i]@]) } },
        }
    }
}

/// EM-INV-INITIALIZED-TOGETHER (sequential state half).
#[logic(open)]
pub fn em_together(em: ExtentManager) -> bool {
    pearlite! { (em.regions == None) == (em.shared == None) }
}

/// Sum of `total_usable_size` over current regions `0..n` (capacity_bytes, lib.rs:710-715).
#[logic(open)]
#[variant(n)]
pub fn cap_sum(arena: Seq<RegionState>, rv: Seq<usize>, n: Int) -> Int {
    pearlite! { if n <= 0 { 0 } else { cap_sum(arena, rv, n - 1) + arena[rv[n - 1]@].buddy.total_usable_size@ } }
}

/// Two arenas agree on the regions' capacities.
#[logic]
#[variant(n)]
#[requires(0 <= n && n <= rv.len())]
#[requires(forall<i: Int> 0 <= i && i < rv.len() ==> a[rv[i]@].buddy.total_usable_size == b[rv[i]@].buddy.total_usable_size)]
#[ensures(cap_sum(a, rv, n) == cap_sum(b, rv, n))]
pub fn lemma_cap_frame(a: Seq<RegionState>, b: Seq<RegionState>, rv: Seq<usize>, n: Int) {
    if n > 0 {
        lemma_cap_frame(a, b, rv, n - 1)
    }
}

/// `x & m <= m` — `key as usize & (regions.len() - 1)` (lib.rs:219).
#[bitwise_proof]
#[ensures(result <= m)]
#[ensures(result == x & m)]
pub fn and_mask_bv(x: usize, m: usize) -> usize {
    x & m
}

#[ensures(result@ <= m@)]
#[ensures(result == x & m)]
pub fn and_mask(x: usize, m: usize) -> usize {
    and_mask_bv(x, m)
}

/// Region `i` of `n` over `[data_start, data_start + usable)`: (base, size) (lib.rs:459-464).
#[requires(n@ > 0 && i@ < n@ && data_start@ + usable@ <= u64::MAX@)]
#[ensures(result.0@ == data_start@ + i@ * (usable@ / n@))]
#[ensures(result.1@ == if i@ < n@ - 1 { usable@ / n@ } else { usable@ - (n@ - 1) * (usable@ / n@) })]
#[ensures(data_start@ <= result.0@ && result.0@ + result.1@ <= data_start@ + usable@)]
pub fn region_extent(data_start: u64, usable: u64, n: usize, i: usize) -> (u64, u64) {
    let region_bytes = usable / n as u64;
    proof_assert! { lemma_divmod(usable@, n@); usable@ == n@ * (usable@ / n@) + usable@ % n@ };
    proof_assert! { lemma_mul_le(i@, n@ - 1, usable@ / n@); i@ * (usable@ / n@) <= (n@ - 1) * (usable@ / n@) };
    proof_assert! { lemma_mul_le(n@ - 1, n@, usable@ / n@); (n@ - 1) * (usable@ / n@) <= n@ * (usable@ / n@) };
    proof_assert! { lemma_distrib(i@, 1, usable@ / n@); (i@ + 1) * (usable@ / n@) == i@ * (usable@ / n@) + usable@ / n@ };
    proof_assert! { i@ < n@ - 1 ==> { lemma_mul_le(i@ + 1, n@, usable@ / n@); (i@ + 1) * (usable@ / n@) <= n@ * (usable@ / n@) } };
    let base = data_start + i as u64 * region_bytes;
    let size = if i < n - 1 { region_bytes } else { usable - (n as u64 - 1) * region_bytes };
    (base, size)
}

impl ExtentManager {
    /// Mirror of `IExtentManager::get_instance_id` (lib.rs:686-692).
    #[ensures(match self.shared {
        Some(sh) => result == Ok(sh.superblock.instance_id),
        None => result == Err(EmError::NotInitialized),
    })]
    pub fn get_instance_id(&self) -> Result<u64, EmError> {
        match &self.shared {
            Some(s) => Ok(s.superblock.instance_id),
            None => Err(EmError::NotInitialized),
        }
    }

    /// Mirror of `IExtentManager::set_data_base_lba` (lib.rs:721-723).
    #[ensures((^self).data_base_lba == base_lba)]
    #[ensures((^self).arena == self.arena && (^self).regions == self.regions && (^self).shared == self.shared)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).metadata_ns_id == self.metadata_ns_id)]
    #[ensures((^self).dev == self.dev)]
    pub fn set_data_base_lba(&mut self, base_lba: u64) {
        self.data_base_lba = base_lba;
    }

    /// Mirror of `IExtentManager::data_base_lba` (lib.rs:725-727).
    #[ensures(result == self.data_base_lba)]
    pub fn data_base_lba(&self) -> u64 {
        self.data_base_lba
    }

    /// Mirror of `IExtentManager::set_metadata_base_lba` (lib.rs:717-719).
    #[ensures((^self).metadata_base_lba == base_lba)]
    #[ensures((^self).arena == self.arena && (^self).regions == self.regions && (^self).shared == self.shared)]
    #[ensures((^self).data_base_lba == self.data_base_lba && (^self).dev == self.dev)]
    pub fn set_metadata_base_lba(&mut self, base_lba: u64) {
        self.metadata_base_lba = base_lba;
    }

    /// Mirror of `IExtentManager::capacity_bytes` (lib.rs:710-715).
    #[requires(em_regions_wf(*self))]
    #[requires(match self.regions { Some(rv) => cap_sum(self.arena@, rv@, rv@.len()) <= u64::MAX@, None => true })]
    #[ensures(match self.regions { Some(rv) => result@ == cap_sum(self.arena@, rv@, rv@.len()), None => result@ == 0 })]
    pub fn capacity_bytes(&self) -> u64 {
        match &self.regions {
            None => 0,
            Some(rv) => {
                let mut total: u64 = 0;
                let mut i: usize = 0;
                #[invariant(i@ <= rv@.len())]
                #[invariant(total@ == cap_sum(self.arena@, rv@, i@))]
                while i < rv.len() {
                    proof_assert! { lemma_cap_mono(self.arena@, rv@, i@ + 1, rv@.len()); cap_sum(self.arena@, rv@, i@ + 1) <= u64::MAX@ };
                    total += self.arena[rv[i]].buddy.total_usable_size();
                    i += 1;
                }
                total
            }
        }
    }

    /// Mirror of `region_for_key` (lib.rs:211-221).
    #[requires(em_regions_wf(*self))]
    #[ensures(match self.regions {
        None => result == Err(EmError::NotInitialized),
        Some(rv) => exists<i: Int> 0 <= i && i < rv@.len() && result == Ok(rv@[i]),
    })]
    pub fn region_for_key(&self, key: u64) -> Result<usize, EmError> {
        match &self.regions {
            None => Err(EmError::NotInitialized),
            Some(rv) => {
                let idx = and_mask(key as usize, rv.len() - 1);
                Ok(rv[idx])
            }
        }
    }

    /// Mirror of `IExtentManager::reserve_extent` (lib.rs:584-630).
    #[requires(em_regions_wf(*self) && em_regions_ok(*self))]
    #[requires(size@ > 0)]
    #[requires(rsv_cur_ok(*self, size@))]
    #[ensures(em_regions_wf(^self) && em_regions_ok(^self))]
    #[ensures((^self).regions == self.regions && (^self).shared == self.shared && (^self).dev == self.dev)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures((^self).arena@.len() == self.arena@.len())]
    #[ensures(forall<i: Int> 0 <= i && i < self.arena@.len() ==> rg_frame(self.arena@[i], (^self).arena@[i]))]
    #[ensures(match result { Ok(h) => (exists<i: Int> 0 <= i && match self.regions { Some(rv) => i < rv@.len() && h.region == rv@[i], None => false })
                && forall<i: Int> 0 <= i && i < self.arena@.len() && i != h.region@ ==> (^self).arena@[i] == self.arena@[i],
                Err(_) => true })]
    #[ensures(match result { Ok(h) => h.key == key && h.region@ < (^self).arena@.len()
                && rsv_slot((^self).arena@[h.region@], h),
                Err(_) => true })]
    #[ensures(forall<i: Int> 0 <= i && i < self.arena@.len() ==> no_new_keys(self.arena@[i], (^self).arena@[i]))]
    pub fn reserve_extent(&mut self, key: u64, size: u32) -> Result<Handle, EmError> {
        let region = self.region_for_key(key)?;
        proof_assert! { forall<i: Int> 0 <= i && i < self.arena@.len() ==> no_new_keys(self.arena@[i], self.arena@[i]) };
        let (slab_start, slot_idx, offset) = self.arena[region].alloc_extent(size)?;
        let bs = self.arena[region].format_params.sector_size;
        proof_assert! { a_3783f8(size@, bs@) };
        // lib.rs:591 `(size + bs - 1) / bs * bs`, same evaluation order (3783f8: size + bs <= u32::MAX).
        let aligned_size = (size + bs - 1) / bs * bs;
        Ok(Handle { region, key, offset, size: aligned_size, slab_start, slot_idx })
    }

    /// `WriteHandle::publish` → the publish closure (lib.rs:597-616).
    #[requires(h.region@ < self.arena@.len() && rg_inv(self.arena@[h.region@]))]
    #[requires(h.key == FREE_KEY ==> free_ok(self.arena@[h.region@], h.slab_start, h.slot_idx))]
    #[requires(match self.arena@[h.region@].slabs@.get(h.slab_start@) { Some(sl) => h.slot_idx@ < sl.keys@.len(), None => true })]
    #[ensures((^self).arena@.len() == self.arena@.len() && rg_inv((^self).arena@[h.region@]))]
    #[ensures(rg_frame(self.arena@[h.region@], (^self).arena@[h.region@]))]
    #[ensures(forall<i: Int> 0 <= i && i < self.arena@.len() && i != h.region@ ==> (^self).arena@[i] == self.arena@[i])]
    #[ensures((^self).regions == self.regions && (^self).shared == self.shared && (^self).dev == self.dev)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures(pub_post(*self, ^self, h))]
    #[ensures(result == Ok((h.key, h.offset, h.size)))]
    pub fn publish(&mut self, h: Handle) -> Result<(u64, u64, u32), EmError> {
        let r = &mut self.arena[h.region];
        if h.key == FREE_KEY {
            // Special sentinel: silently discard — free the slot and return Ok.
            r.free_slot(h.slab_start, h.slot_idx);
            return Ok((h.key, h.offset, h.size));
        }
        r.publish_slot(h.slab_start, h.slot_idx, h.key);
        Ok((h.key, h.offset, h.size))
    }

    /// `WriteHandle::abort` / `Drop` → the abort closure (lib.rs:618-621).
    #[requires(h.region@ < self.arena@.len() && rg_inv(self.arena@[h.region@]))]
    #[requires(free_ok(self.arena@[h.region@], h.slab_start, h.slot_idx))]
    #[ensures((^self).arena@.len() == self.arena@.len() && rg_inv((^self).arena@[h.region@]))]
    #[ensures(rg_frame(self.arena@[h.region@], (^self).arena@[h.region@]))]
    #[ensures(forall<i: Int> 0 <= i && i < self.arena@.len() && i != h.region@ ==> (^self).arena@[i] == self.arena@[i])]
    #[ensures((^self).regions == self.regions && (^self).shared == self.shared && (^self).dev == self.dev)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures(forall<sl: Slab> self.arena@[h.region@].slabs@.get(h.slab_start@) == Some(sl) ==>
        free_case(self.arena@[h.region@], (^self).arena@[h.region@], h.slab_start, h.slot_idx, sl))]
    pub fn abort(&mut self, h: Handle) {
        self.arena[h.region].free_slot(h.slab_start, h.slot_idx);
    }
}

/// The publish closure's effect on a live slot (lib.rs:610 → region.rs:113-118): a non-FREE
/// key is written into the handle's slot; bitmap and geometry are untouched.
#[logic(open)]
pub fn pub_post(a: ExtentManager, b: ExtentManager, h: Handle) -> bool {
    pearlite! {
        h.key != FREE_KEY ==> forall<sl: Slab> a.arena@[h.region@].slabs@.get(h.slab_start@) == Some(sl) ==>
            b.arena@[h.region@].slabs@.contains(h.slab_start@)
            && b.arena@[h.region@].slabs@.lookup(h.slab_start@).keys@ == sl.keys@.set(h.slot_idx@, h.key)
            && b.arena@[h.region@].slabs@.lookup(h.slab_start@).bitmap == sl.bitmap
            && b.arena@[h.region@].slabs@.lookup(h.slab_start@).element_size == sl.element_size
            && b.arena@[h.region@].slabs@.lookup(h.slab_start@).start_offset == sl.start_offset
            && b.arena@[h.region@].slabs@.lookup(h.slab_start@).slab_size == sl.slab_size
    }
}

/// The slot a handle names exists, is allocated, and is where the handle's offset points.
#[logic(open)]
pub fn rsv_slot(r: RegionState, h: Handle) -> bool {
    pearlite! {
        r.slabs@.contains(h.slab_start@)
        && h.slot_idx@ < r.slabs@.lookup(h.slab_start@).bitmap.num_slots@
        && slot_bit(r.slabs@.lookup(h.slab_start@).bitmap, h.slot_idx@)
        && h.offset@ == slot_off(r.slabs@.lookup(h.slab_start@), h.slot_idx@)
    }
}

/// Geometry of region `i` of `n` over `usable` bytes from `ds` (lib.rs:540-545).
#[logic(open)]
pub fn rbase(ds: Int, usable: Int, n: Int, i: Int) -> Int {
    pearlite! { ds + i * (usable / n) }
}

#[logic(open)]
pub fn rsize(usable: Int, n: Int, i: Int) -> Int {
    pearlite! { if i < n - 1 { usable / n } else { usable - (n - 1) * (usable / n) } }
}

/// What `initialize` relies on in the recovered superblock (all true of one written by a
/// successful `format` within `sane_params`; NOT re-validated on recovery, lib.rs:533-536 —
/// see EM-INIT-ERR-SUPERBLOCK-GEOMETRY).
#[logic(open)]
pub fn sb_sane(sb: Superblock) -> bool {
    pearlite! {
        a_91a1f6(sb) && a_7cb7c3(sb.slab_size@, sb.sector_size@) && a_e0fe79(sb.slab_size@, sb.sector_size@)
        && a_3b58ea(sb.data_disk_size@, sb.sector_size@)
        && a_a23fc2(sb.checkpoint_region_offset@, sb.checkpoint_region_size@)
    }
}

/// DERIVED from [`sb_sane`] by [`lemma_sb_hd`]: the superblock headroom initialize uses.
#[logic(open)]
pub fn sb_hd(sb: Superblock) -> bool {
    pearlite! {
        sb.sector_size@ > 0 && sb.slab_size@ > 0 && sb.slab_size@ % sb.sector_size@ == 0
        && sb.slab_size@ + sb.sector_size@ <= u64::MAX@
        && sb.region_count@ > 0
        && sb.data_start_offset@ <= sb.data_disk_size@
        && 2 * sb.data_disk_size@ + sb.sector_size@ <= u64::MAX@
        && sb.checkpoint_region_offset@ + 2 * sb.checkpoint_region_size@ <= u64::MAX@
    }
}

#[logic]
#[requires(sb_sane(sb))]
#[ensures(sb_hd(sb))]
pub fn lemma_sb_hd(sb: Superblock) {
    pearlite! {
        lemma_hd_params(sb.slab_size@, sb.sector_size@, sb.data_disk_size@);
        lemma_p2_pos(sb.region_count@)
    }
}

/// `desc_ok` stated over the superblock's geometry.
#[logic(open)]
pub fn desc_ok_sb(d: SlabDescriptor, base: Int, size: Int, sb: Superblock) -> bool {
    pearlite! {
        a_980f61_d(d, base, size) && a_3b612e_d(d, base, size, sb.slab_size@)
        && u_desc_slots_u32(d) && t_dec_keys(d) /* LEVEL-3 D-A: u_desc_slots_u32 is implied by the 1st and 4th conjuncts (lemma_desc_slots, lemma_desc_ok_sb_intro); kept as a solver hint (initialize does not re-prove without it), so the premise is equivalent to 980f61 + 3b612e + the decoder fact t_dec_keys */
    }
}

/// The span form of [`desc_ok_sb`] (DERIVED by [`lemma_desc_ok_sb`] under the superblock's
/// 91a1f6 + 7cb7c3).
#[logic(open)]
pub fn desc_geo_sb(d: SlabDescriptor, base: Int, size: Int, sb: Superblock) -> bool {
    pearlite! {
        d.slab_size == sb.slab_size
        && d.element_size@ > 0
        && d.keys@.len() == slots_of(d.slab_size@, d.element_size@)
        && d.start_offset@ >= base
        && (d.start_offset@ - base) % span(sb.sector_size@, ord(sb.slab_size@ / sb.sector_size@)) == 0
        && d.start_offset@ - base + span(sb.sector_size@, ord(sb.slab_size@ / sb.sector_size@)) <= size
    }
}

/// LEVEL-3 (phase D-A): `desc_ok_sb` is EQUIVALENT to its declared + decoder part
/// (980f61 per descriptor, 3b612e, t_dec_keys): the slot-count conjunct follows.
#[logic]
#[requires(a_980f61_d(d, base, size) && a_3b612e_d(d, base, size, sb.slab_size@) && t_dec_keys(d))]
#[ensures(desc_ok_sb(d, base, size, sb))]
pub fn lemma_desc_ok_sb_intro(d: SlabDescriptor, base: Int, size: Int, sb: Superblock) {
    pearlite! { lemma_desc_slots(d, base, size) }
}

#[logic]
#[requires(sb_sane(sb) && desc_ok_sb(d, base, size, sb))]
#[ensures(desc_geo_sb(d, base, size, sb))]
pub fn lemma_desc_ok_sb(d: SlabDescriptor, base: Int, size: Int, sb: Superblock) {
    pearlite! {
        lemma_slab_span(sb.slab_size@, sb.sector_size@);
        lemma_desc_slots(d, base, size);
        proof_assert! { d.slab_size@ / d.element_size@ >= 0 && d.slab_size@ / d.element_size@ < 4294967296 };
        proof_assert! { slots_of(d.slab_size@, d.element_size@) == d.slab_size@ / d.element_size@ }
    }
}

/// What initialize's rebuild guarantees of region `r` built from descriptors `ds`.
#[logic(open)]
pub fn init_rg_ok(r: RegionState, ds: Seq<SlabDescriptor>) -> bool {
    pearlite! { nonfull_listed(r) && (starts_distinct(ds) ==> rebuilt_all(r, ds)) }
}

/// Everything initialize's loop maintains about one rebuilt region.
#[logic(open)]
pub fn init_reg(r: RegionState, lo: Int, hi: Int, ds: Seq<SlabDescriptor>, b: Int, sz: Int, fp: FormatParams) -> bool {
    pearlite! { rg_inv(r) && rg_within(r, lo, hi) && init_rg_ok(r, ds) && init_geo(r, b, sz) && r.format_params == fp }
}

/// Region `r` sits at base `b` with `sz` usable bytes (batch 4: initialize's region geometry).
#[logic(open)]
pub fn init_geo(r: RegionState, b: Int, sz: Int) -> bool {
    pearlite! { r.buddy.base_offset@ == b && r.buddy.total_usable_size@ == sz }
}

/// The descriptors initialize hands region `i` (lib.rs:546-550): `per_region[i]`, or none.
#[logic(open)]
pub fn dsel(pr: Seq<Vec<SlabDescriptor>>, i: Int) -> Seq<SlabDescriptor> {
    pearlite! { if i < pr.len() { pr[i]@ } else { Seq::empty() } }
}

/// Every recovered descriptor of every region fits that region's geometry.
#[logic(open)]
pub fn recovered_ok(sb: Superblock, per_region: Seq<Vec<SlabDescriptor>>) -> bool {
    pearlite! {
        forall<i: Int, p: Int> 0 <= i && i < sb.region_count@ && i < per_region.len()
            && 0 <= p && p < per_region[i]@.len() ==>
            desc_ok_sb(per_region[i]@[p],
                rbase(sb.data_start_offset@, sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i),
                rsize(sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i), sb)
    }
}

/// D-RANGE-FR-005-3783f8 for every CURRENT region (the one reserve_extent picks is one of them).
#[logic(open)]
pub fn rsv_cur_ok(em: ExtentManager, sz: Int) -> bool {
    pearlite! {
        match em.regions {
            Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> a_3783f8(sz, em.arena@[rv@[i]@].format_params.sector_size@),
            None => true,
        }
    }
}

/// The parameter ranges format's region construction relies on (phase F, level 3): exactly
/// D-RANGE-FR-002-7cb7c3 (slab/sector power of two => slab > 0), -e0fe79 (=> slab + ss <=
/// u64::MAX) and -3b58ea (2*data_disk_size + ss <= u64::MAX: buddy.rs:58, :98 do not overflow).
#[logic(open)]
pub fn sane_params(p: FormatParams) -> bool {
    pearlite! {
        a_7cb7c3(p.slab_size@, p.sector_size@) && a_e0fe79(p.slab_size@, p.sector_size@)
        && a_3b58ea(p.data_disk_size@, p.sector_size@)
    }
}

/// LEVEL-3 (phase D-B2): the parameter ranges `format` itself needs — D-RANGE-FMT-CHECKPOINT-PAYLOAD-e0fe79
/// (=> slab + ss <= u64::MAX) and D-RANGE-FR-002-3b58ea (2*data_disk_size + ss <= u64::MAX), both listing
/// `format`. Unlike [`sane_params`] it does NOT include D-RANGE-FR-002-7cb7c3 (whose methods omit `format`):
/// the real format accepts slab_size 0 (lib.rs:383-512 has no such check), and so does this mirror; the
/// region invariant of the built regions is then ensured only for slab_size > 0.
#[logic(open)]
pub fn fmt_sane(p: FormatParams) -> bool {
    pearlite! { a_e0fe79(p.slab_size@, p.sector_size@) && a_3b58ea(p.data_disk_size@, p.sector_size@) }
}

/// DERIVED from [`fmt_sane`]: `ss > 0`, `slab + ss <= MAX`, `2*dds + ss <= MAX`.
#[logic]
#[requires(fmt_sane(p))]
#[ensures(p.sector_size@ > 0 && p.slab_size@ + p.sector_size@ <= u64::MAX@ && 2 * p.data_disk_size@ + p.sector_size@ <= u64::MAX@)]
pub fn lemma_fmt_hd(p: FormatParams) {
    pearlite! { lemma_e0fe79_hd(p.slab_size@, p.sector_size@) }
}

/// [`sane_params`] is [`fmt_sane`] plus 7cb7c3, which gives slab_size > 0.
#[logic]
#[requires(sane_params(p))]
#[ensures(fmt_sane(p) && p.slab_size@ > 0)]
pub fn lemma_sane_fmt(p: FormatParams) {
    pearlite! { lemma_7cb7c3_pos(p.slab_size@, p.sector_size@) }
}

/// DERIVED from [`sane_params`] (lemma_hd_params): `slab > 0`, `slab + ss <= MAX`, `2*dds + ss <= MAX`.
#[logic]
#[requires(sane_params(p))]
#[ensures(hd_params(p.slab_size@, p.sector_size@, p.data_disk_size@))]
pub fn lemma_sane_hd(p: FormatParams) {
    pearlite! { lemma_hd_params(p.slab_size@, p.sector_size@, p.data_disk_size@) }
}

/// EM-INV-SEQ-AGREE: the recorded checkpoint sequence equals the in-memory superblock's.
#[logic(open)]
pub fn seq_agree(em: ExtentManager) -> bool {
    pearlite! { match em.shared { Some(s) => s.checkpoint_seq == s.superblock.checkpoint_seq, None => true } }
}

/// `b` is superblock `a` with at most `checkpoint_seq` / `active_copy` changed
/// (checkpoint.rs:106-112 is the only in-memory superblock update).
#[logic(open)]
pub fn sb_frame(a: Superblock, b: Superblock) -> bool {
    pearlite! {
        b.magic == a.magic && b.version == a.version && b.data_disk_size == a.data_disk_size
        && b.sector_size == a.sector_size && b.slab_size == a.slab_size && b.max_extent_size == a.max_extent_size
        && b.region_count == a.region_count && b.checkpoint_region_offset == a.checkpoint_region_offset
        && b.checkpoint_region_size == a.checkpoint_region_size && b.instance_id == a.instance_id
        && b.metadata_disk_ns_id == a.metadata_disk_ns_id && b.data_start_offset == a.data_start_offset
    }
}

/// Every current region lies inside `[data_start_offset, data_disk_size]` of the
/// published superblock and uses the published parameters.
#[logic(open)]
pub fn em_layout(em: ExtentManager) -> bool {
    pearlite! {
        match (em.regions, em.shared) {
            (Some(rv), Some(sh)) => forall<i: Int> 0 <= i && i < rv@.len() ==>
                rg_within(em.arena@[rv@[i]@], sh.superblock.data_start_offset@, sh.superblock.data_disk_size@),
            _ => true,
        }
    }
}

/// Region `r`'s byte range lies inside `[lo, hi]`.
#[logic(open)]
pub fn rg_within(r: RegionState, lo: Int, hi: Int) -> bool {
    pearlite! { lo <= r.buddy.base_offset@ && r.buddy.base_offset@ + r.buddy.total_usable_size@ <= hi }
}

/// `1u32.is_power_of_two()` — the bit fact `1 & (1 - 1) == 0`.
#[bitwise_proof]
#[ensures((1u32 & (1u32 - 1u32)) == 0u32)]
pub fn fact_pow2_one() {}

/// A concrete class of inputs on which every `format` check passes (lib.rs:384-452):
/// valid FR-002 parameters, one region, data on its own device (metadata_region_size 0),
/// a connected metadata device with room for the superblock and two checkpoint copies,
/// and an explicit instance id. On these inputs the only failure left is the device's
/// own I/O error on the superblock write (lib.rs:498).
#[logic(open)]
pub fn fmt_ok_inputs(em: ExtentManager, p: FormatParams) -> bool {
    pearlite! { fmt_ok_core(em, p) && p.region_count@ == 1 }
}

/// `fmt_ok_inputs` without the region count (batch 3): every check of lib.rs:384-452 except
/// the power-of-two region count passes.
#[logic(open)]
pub fn fmt_ok_core(em: ExtentManager, p: FormatParams) -> bool {
    pearlite! {
        p.sector_size@ > 0 && p.slab_size@ % p.sector_size@ == 0 && p.max_extent_size@ <= p.slab_size@
        && p.metadata_region_size@ == 0 && p.metadata_alignment@ == 0
        && p.data_disk_size@ > 0 && p.instance_id != None
        && em.dev.connected && em.dev.num_sectors@ * em.dev.sector_size@ >= 4096 + 2 * p.sector_size@
    }
}

/// The same input class with EIGHT regions (batch 3; 8 = 2^3 passes lib.rs:397) and enough
/// data bytes for every region to be non-empty.
#[logic(open)]
pub fn fmt_ok8(em: ExtentManager, p: FormatParams) -> bool {
    pearlite! { fmt_ok_core(em, p) && p.region_count@ == 8 }
}

/// `8u32.is_power_of_two()` — the bit fact `8 & (8 - 1) == 0`.
#[bitwise_proof]
#[ensures((8u32 & (8u32 - 1u32)) == 0u32)]
pub fn fact_pow2_eight() {}

/// `align_up(4096, metadata_alignment)` (lib.rs:420-424).
#[logic(open)]
pub fn ckoff_l(a: Int) -> Int {
    pearlite! { if a == 0 { 4096 } else { (4096 + a - 1) / a * a } }
}

/// The metadata bytes the checkpoint layout may use (lib.rs:425-429): `md` capped to a
/// non-zero `metadata_region_size`.
#[logic(open)]
pub fn eff_l(md: Int, mrs: Int) -> Int {
    pearlite! { if mrs > 0 { if md <= mrs { md } else { mrs } } else { md } }
}

/// `checkpoint_region_size` (lib.rs:430-432): half of what is left after copy 0's offset,
/// rounded down to a multiple of the DATA sector size `params.sector_size`.
#[logic(open)]
pub fn cksize_l(md: Int, mrs: Int, a: Int, ss: Int) -> Int {
    pearlite! {
        if eff_l(md, mrs) >= ckoff_l(a) { ((eff_l(md, mrs) - ckoff_l(a)) / 2) / ss * ss } else { 0 }
    }
}

/// `data_start_offset` (lib.rs:442-446).
#[logic(open)]
pub fn ds_l(md: Int, mrs: Int, a: Int, ss: Int) -> Int {
    pearlite! { if mrs > 0 { ckoff_l(a) + 2 * cksize_l(md, mrs, a, ss) } else { 0 } }
}

/// A region exactly as `format` builds it (lib.rs:465-466): a fresh buddy allocator over
/// (`base`, `size`) with the format parameters, no slabs, empty size classes, clean.
#[logic(open)]
pub fn fresh_rg(r: RegionState, base: Int, size: Int, p: FormatParams) -> bool {
    pearlite! {
        r.format_params == p && r.buddy.base_offset@ == base && r.buddy.total_usable_size@ == size
        && r.buddy.sector_size == p.sector_size
        && r.slabs@ == FMap::empty() && (forall<e: Int> sc_get(r.size_classes, e) == Seq::empty())
        && !r.dirty && r.pending_frees@.len() == 0
        && (forall<m: Int> 0 <= m && m.pow2() == size / p.sector_size@ ==>
            r.buddy.max_order@ == m && r.buddy.free_lists@[m]@ == Seq::singleton(0u64)
            && forall<o: Int> 0 <= o && o < m ==> r.buddy.free_lists@[o]@.len() == 0)
        && tf(r.buddy.free_lists@, p.sector_size@, r.buddy.free_lists@.len()) == (size / p.sector_size@) * p.sector_size@
    }
}

/// Arena prefix `0..n` unchanged.
#[logic(open)]
pub fn arena_prefix(a: Seq<RegionState>, b: Seq<RegionState>, n: Int) -> bool {
    pearlite! { n <= a.len() && n <= b.len() && forall<i: Int> 0 <= i && i < n ==> a[i] == b[i] }
}

/// Partial capacity sums are bounded by the full sum.
#[logic]
#[variant(n - m)]
#[requires(0 <= m && m <= n && n <= rv.len())]
#[ensures(cap_sum(arena, rv, m) <= cap_sum(arena, rv, n))]
pub fn lemma_cap_mono(arena: Seq<RegionState>, rv: Seq<usize>, m: Int, n: Int) {
    if m < n {
        lemma_cap_mono(arena, rv, m, n - 1)
    }
}

impl ExtentManager {
    /// lib.rs:383-504 — everything `format` does BEFORE publishing (validation, layout,
    /// region construction, superblock write). Returns the new region indices and the
    /// new `SharedState`; the caller publishes them (lib.rs:506-507).
    #[requires(em_regions_wf(*self))]
    #[requires(self.dev.connected ==> a_67ea4f(self.dev.num_sectors@, self.dev.sector_size@))]
    #[requires(a_2ee144(params.metadata_alignment@))]
    #[requires(fmt_sane(params))]
    #[ensures(sane_params(params) ==> params.slab_size@ > 0)]
    #[ensures(arena_prefix(self.arena@, (^self).arena@, self.arena@.len()))]
    #[ensures((^self).regions == self.regions && (^self).shared == self.shared)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures(em_regions_wf(^self))]
    #[ensures(match result {
        Ok((rv, sh)) => rv@.len() == params.region_count@ && rv@.len() > 0
            && (forall<i: Int> 0 <= i && i < rv@.len() ==> rv@[i]@ == self.arena@.len() + i && rv@[i]@ < (^self).arena@.len())
            && (params.slab_size@ > 0 ==> forall<i: Int> 0 <= i && i < rv@.len() ==> rg_inv((^self).arena@[rv@[i]@]))
            && (forall<i: Int> 0 <= i && i < rv@.len() ==> (^self).arena@[rv@[i]@].slabs@ == FMap::empty())
            && sh.superblock.data_disk_size == params.data_disk_size
            && (match params.instance_id { Some(id) => sh.superblock.instance_id == id, None => true })
            && (^self).dev.sb == Some(sh.superblock)
            && (forall<i: Int> 0 <= i && i < rv@.len() ==>
                rg_within((^self).arena@[rv@[i]@], sh.superblock.data_start_offset@, sh.superblock.data_disk_size@)),
        Err(_) => true,
    })]
    #[ensures(match result {
        Ok((rv, sh)) => sh.format_params == params
            && params.sector_size@ > 0 && params.slab_size@ % params.sector_size@ == 0
            && params.max_extent_size@ <= params.slab_size@ && params.region_count@ > 0 && p2(params.region_count@)
            && sh.checkpoint_seq@ == 0 && sh.superblock.checkpoint_seq@ == 0 && sh.superblock.active_copy@ == 0
            && sh.superblock.sector_size == params.sector_size && sh.superblock.slab_size == params.slab_size
            && sh.superblock.region_count == params.region_count && sh.superblock.max_extent_size == params.max_extent_size
            && sh.superblock.metadata_disk_ns_id == params.metadata_disk_ns_id
            && sh.superblock.checkpoint_region_offset@ == ckoff_l(params.metadata_alignment@)
            && sh.superblock.checkpoint_region_size@ == cksize_l(self.dev.num_sectors@ * self.dev.sector_size@,
                    params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
            && sh.superblock.checkpoint_region_size@ > 0
            && sh.superblock.data_start_offset@ == ds_l(self.dev.num_sectors@ * self.dev.sector_size@,
                    params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
            && sh.superblock.data_start_offset@ < sh.superblock.data_disk_size@
            && sh.superblock.checkpoint_region_offset@ + 2 * sh.superblock.checkpoint_region_size@ <= u64::MAX@
            && (forall<i: Int> 0 <= i && i < rv@.len() ==>
                fresh_rg((^self).arena@[rv@[i]@],
                    rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i),
                    rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i), params)),
        Err(_) => true,
    })]
    #[ensures(fmt_ok_inputs(*self, params) ==> match result { Ok(_) => true, Err(e) => e == EmError::IoError })]
    #[ensures(fmt_ok8(*self, params) ==> match result { Ok(_) => true, Err(e) => e == EmError::IoError })]
    pub fn format_build(&mut self, params: FormatParams) -> Result<(Vec<usize>, SharedState), EmError> {
        proof_assert! { lemma_fmt_hd(params); params.sector_size@ > 0 && params.slab_size@ + params.sector_size@ <= u64::MAX@
            && 2 * params.data_disk_size@ + params.sector_size@ <= u64::MAX@ };
        proof_assert! { sane_params(params) ==> { lemma_sane_fmt(params); params.slab_size@ > 0 } };
        if params.sector_size == 0 {
            return Err(EmError::CorruptMetadata);
        }
        if params.slab_size % params.sector_size as u64 != 0 {
            return Err(EmError::CorruptMetadata);
        }
        if params.max_extent_size as u64 > params.slab_size {
            return Err(EmError::CorruptMetadata);
        }
        fact_pow2_one();
        fact_pow2_eight();
        proof_assert! { fmt_ok_inputs(*self, params) ==> params.region_count == 1u32 };
        proof_assert! { fmt_ok8(*self, params) ==> params.region_count == 8u32 };
        if params.region_count == 0 || !params.region_count.is_power_of_two() {
            return Err(EmError::CorruptMetadata);
        }
        crate::props::batch7::pow2_bits7(params.region_count); // PROOF-ONLY: lib.rs:397's bit test => p2
        proof_assert! { p2(params.region_count@) };

        let data_disk_size = params.data_disk_size;

        // Query metadata device size (lib.rs:406-415)
        if !self.dev.connected {
            return Err(EmError::NotInitialized);
        }
        let metadata_disk_size = self.dev.num_sectors * self.dev.sector_size as u64;

        // Compute checkpoint region layout on metadata device (lib.rs:418-438)
        let alignment = params.metadata_alignment;
        let sb_size = SUPERBLOCK_SIZE as u64;
        let checkpoint_region_offset = if alignment == 0 {
            sb_size
        } else {
            (sb_size + alignment - 1) / alignment * alignment /* lib.rs:423, same evaluation order (debug overflow semantics; 2ee144) */
        };
        let effective_metadata_size = if params.metadata_region_size > 0 {
            if metadata_disk_size <= params.metadata_region_size { metadata_disk_size } else { params.metadata_region_size }
        } else {
            metadata_disk_size
        };
        let remaining = if effective_metadata_size >= checkpoint_region_offset {
            effective_metadata_size - checkpoint_region_offset
        } else {
            0
        }; // `saturating_sub` (lib.rs:430)
        let sector_size_u64 = params.sector_size as u64;
        let checkpoint_region_size = (remaining / 2) / sector_size_u64 * sector_size_u64;
        proof_assert! { lemma_divmod(remaining@ / 2, sector_size_u64@); checkpoint_region_size@ <= remaining@ / 2 };
        proof_assert! { checkpoint_region_offset@ == ckoff_l(params.metadata_alignment@) };
        proof_assert! { effective_metadata_size@ == eff_l(metadata_disk_size@, params.metadata_region_size@) };
        proof_assert! { checkpoint_region_size@ == cksize_l(metadata_disk_size@, params.metadata_region_size@, params.metadata_alignment@, params.sector_size@) };
        proof_assert! { checkpoint_region_size@ > 0 ==> checkpoint_region_offset@ + 2 * checkpoint_region_size@ <= effective_metadata_size@ };
        proof_assert! { fmt_ok_core(*self, params) ==> remaining@ / 2 >= sector_size_u64@ };
        proof_assert! { fmt_ok_core(*self, params) ==> {
            lemma_div_le(1, sector_size_u64@, remaining@ / 2);
            (remaining@ / 2) / sector_size_u64@ >= 1 } };
        proof_assert! { fmt_ok_core(*self, params) ==> {
            lemma_mul_le(1, (remaining@ / 2) / sector_size_u64@, sector_size_u64@);
            1 * sector_size_u64@ <= ((remaining@ / 2) / sector_size_u64@) * sector_size_u64@ } };
        proof_assert! { fmt_ok_core(*self, params) ==> checkpoint_region_size@ >= sector_size_u64@ };

        if checkpoint_region_size == 0 {
            return Err(EmError::CorruptMetadata);
        }

        let data_start_offset = if params.metadata_region_size > 0 {
            checkpoint_region_offset + 2 * checkpoint_region_size
        } else {
            0
        };
        let usable_data_size = if data_disk_size >= data_start_offset { data_disk_size - data_start_offset } else { 0 };
        if usable_data_size == 0 {
            return Err(EmError::CorruptMetadata);
        }

        let region_count = params.region_count as usize;
        let n0 = snapshot! { self.arena@.len() };
        let a0 = snapshot! { self.arena };
        let mut region_vec: Vec<usize> = Vec::new();
        let mut i: usize = 0;
        #[invariant(i@ <= region_count@ && region_vec@.len() == i@)]
        #[invariant(self.arena@.len() == *n0 + i@)]
        #[invariant(arena_prefix(a0@, self.arena@, *n0))]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==> region_vec@[j]@ == *n0 + j)]
        #[invariant(params.slab_size@ > 0 ==> forall<j: Int> 0 <= j && j < i@ ==> rg_inv(self.arena@[*n0 + j]))]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==> self.arena@[*n0 + j].slabs@ == FMap::empty())]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==> rg_within(self.arena@[*n0 + j], data_start_offset@, data_start_offset@ + usable_data_size@))]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==> fresh_rg(self.arena@[*n0 + j],
            rbase(data_start_offset@, usable_data_size@, region_count@, j), rsize(usable_data_size@, region_count@, j), params))]
        while i < region_count {
            let (base, size) = region_extent(data_start_offset, usable_data_size, region_count, i);
            let buddy = BuddyAllocator::new(base, size, params.sector_size);
            proof_assert! { data_start_offset@ <= buddy.base_offset@ && buddy.base_offset@ + buddy.total_usable_size@ <= data_start_offset@ + usable_data_size@ };
            proof_assert! { lemma_span_pos(params.sector_size@, buddy.max_order@); buddy.max_order@ == 0 ==> span(params.sector_size@, 0) == params.sector_size@ };
            let region = RegionState::new(buddy, params);
            proof_assert! { region.slabs@ == FMap::empty() };
            proof_assert! { rg_buddy(region) && rg_slabs(region) };
            proof_assert! { region.format_params.sector_size == region.buddy.sector_size && region.format_params.sector_size@ > 0
                && region.format_params.slab_size@ % region.format_params.sector_size@ == 0
                && region.format_params.slab_size@ + region.format_params.sector_size@ <= u64::MAX@ };
            proof_assert! { params.slab_size@ > 0 ==> rg_params(region) };
            proof_assert! { params.slab_size@ > 0 ==> rg_inv(region) };
            proof_assert! { rg_within(region, data_start_offset@, data_start_offset@ + usable_data_size@) };
            let ar0 = snapshot! { self.arena };
            let rg = snapshot! { region };
            self.arena.push(region);
            proof_assert! { self.arena@ == ar0@.push_back(*rg) };
            proof_assert! { self.arena@[*n0 + i@] == *rg };
            proof_assert! { forall<j: Int> 0 <= j && j < *n0 + i@ ==> self.arena@[j] == ar0@[j] };
            proof_assert! { forall<j: Int> 0 <= j && j < i@ ==> self.arena@[*n0 + j] == ar0@[*n0 + j] };
            proof_assert! { params.slab_size@ > 0 ==> rg_inv(*rg) };
            proof_assert! { params.slab_size@ > 0 ==> rg_inv(self.arena@[*n0 + i@]) };
            proof_assert! { params.slab_size@ > 0 ==> forall<j: Int> 0 <= j && j < i@ ==> rg_inv(self.arena@[*n0 + j]) };
            proof_assert! { params.slab_size@ > 0 ==> forall<j: Int> 0 <= j && j < i@ + 1 ==> rg_inv(self.arena@[*n0 + j]) };
            proof_assert! { rg.slabs@ == FMap::empty() };
            proof_assert! { forall<j: Int> 0 <= j && j < i@ ==> self.arena@[*n0 + j].slabs@ == FMap::empty() };
            proof_assert! { forall<j: Int> 0 <= j && j < i@ + 1 ==> self.arena@[*n0 + j].slabs@ == FMap::empty() };
            proof_assert! { *rg == self.arena@[*n0 + i@] && rg_within(*rg, data_start_offset@, data_start_offset@ + usable_data_size@) };
            proof_assert! { rg_within(self.arena@[*n0 + i@], data_start_offset@, data_start_offset@ + usable_data_size@) };
            proof_assert! { forall<j: Int> 0 <= j && j < i@ + 1 ==> rg_within(self.arena@[*n0 + j], data_start_offset@, data_start_offset@ + usable_data_size@) };
            proof_assert! { fresh_rg(*rg, base@, size@, params) };
            proof_assert! { forall<j: Int> 0 <= j && j < i@ + 1 ==> fresh_rg(self.arena@[*n0 + j],
                rbase(data_start_offset@, usable_data_size@, region_count@, j), rsize(usable_data_size@, region_count@, j), params) };
            region_vec.push(self.arena.len() - 1);
            i += 1;
        }

        proof_assert! { i@ == region_count@ };
        proof_assert! { region_vec@.len() == region_count@ };
        proof_assert! { forall<j: Int> 0 <= j && j < region_vec@.len() ==> region_vec@[j]@ == *n0 + j };
        proof_assert! { data_start_offset@ + usable_data_size@ == data_disk_size@ };
        proof_assert! { forall<j: Int> 0 <= j && j < region_vec@.len() ==> rg_within(self.arena@[*n0 + j], data_start_offset@, data_disk_size@) };
        let arena_done = snapshot! { self.arena };
        // Write superblock to metadata device (lib.rs:471-498)
        let instance_id = match params.instance_id {
            Some(id) => id,
            None => urandom_u64()?,
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
        dev_write_superblock(&mut self.dev, sb)?;

        let shared = SharedState { format_params: params, checkpoint_seq: 0, superblock: sb };
        proof_assert! { sb.data_start_offset == data_start_offset && sb.data_disk_size == data_disk_size };
        proof_assert! { data_start_offset@ + usable_data_size@ == data_disk_size@ };
        proof_assert! { self.arena == *arena_done };
        proof_assert! { usable_data_size@ == params.data_disk_size@ - sb.data_start_offset@ };
        proof_assert! { forall<j: Int> 0 <= j && j < region_vec@.len() ==> fresh_rg(self.arena@[region_vec@[j]@],
            rbase(sb.data_start_offset@, params.data_disk_size@ - sb.data_start_offset@, params.region_count@, j),
            rsize(params.data_disk_size@ - sb.data_start_offset@, params.region_count@, j), params) };
        Ok((region_vec, shared))
    }

    /// `*self.regions.write() = Some(region_vec)` (lib.rs:506, 576).
    #[ensures((^self).regions == Some(rv))]
    #[ensures((^self).arena == self.arena && (^self).shared == self.shared && (^self).dev == self.dev)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    pub fn publish_regions(&mut self, rv: Vec<usize>) {
        self.regions = Some(rv);
    }

    /// `*self.shared.lock().unwrap() = Some(shared)` (lib.rs:507, 577).
    #[ensures((^self).shared == Some(sh))]
    #[ensures((^self).arena == self.arena && (^self).regions == self.regions && (^self).dev == self.dev)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    pub fn publish_shared(&mut self, sh: SharedState) {
        self.shared = Some(sh);
    }

    /// Mirror of `IExtentManager::format` (lib.rs:383-512), single-threaded.
    #[requires(em_regions_wf(*self))]
    #[requires(self.dev.connected ==> a_67ea4f(self.dev.num_sectors@, self.dev.sector_size@))]
    #[requires(a_2ee144(params.metadata_alignment@))]
    #[requires(fmt_sane(params))]
    #[ensures(sane_params(params) ==> params.slab_size@ > 0)]
    #[ensures(arena_prefix(self.arena@, (^self).arena@, self.arena@.len()))]
    #[ensures(em_regions_wf(^self))]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures(match result {
        Ok(()) => match ((^self).regions, (^self).shared) {
            (Some(rv), Some(sh)) =>
                (forall<i: Int> 0 <= i && i < rv@.len() ==> self.arena@.len() <= rv@[i]@)
                && (params.slab_size@ > 0 ==> em_regions_ok(^self))
                && em_list_ok(^self)
                && em_layout(^self)
                && (^self).dev.sb == Some(sh.superblock)
                && (match params.instance_id { Some(id) => sh.superblock.instance_id == id, None => true }),
            _ => false,
        },
        Err(_) => (^self).regions == self.regions && (^self).shared == self.shared,
    })]
    #[ensures(match result {
        Ok(()) => match ((^self).regions, (^self).shared) {
            (Some(rv), Some(sh)) => rv@.len() == params.region_count@
                && sh.format_params == params && seq_agree(^self)
                && params.sector_size@ > 0 && params.slab_size@ % params.sector_size@ == 0
                && params.max_extent_size@ <= params.slab_size@ && params.region_count@ > 0 && p2(params.region_count@)
                && sh.superblock.checkpoint_seq@ == 0 && sh.superblock.active_copy@ == 0
                && sh.superblock.data_disk_size == params.data_disk_size
                && sh.superblock.sector_size == params.sector_size && sh.superblock.slab_size == params.slab_size
                && sh.superblock.region_count == params.region_count && sh.superblock.max_extent_size == params.max_extent_size
                && sh.superblock.metadata_disk_ns_id == params.metadata_disk_ns_id
                && sh.superblock.checkpoint_region_offset@ == ckoff_l(params.metadata_alignment@)
                && sh.superblock.checkpoint_region_size@ == cksize_l(self.dev.num_sectors@ * self.dev.sector_size@,
                        params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
                && sh.superblock.checkpoint_region_size@ > 0
                && sh.superblock.data_start_offset@ == ds_l(self.dev.num_sectors@ * self.dev.sector_size@,
                        params.metadata_region_size@, params.metadata_alignment@, params.sector_size@)
                && sh.superblock.data_start_offset@ < sh.superblock.data_disk_size@
                && sh.superblock.checkpoint_region_offset@ + 2 * sh.superblock.checkpoint_region_size@ <= u64::MAX@
                && (forall<i: Int> 0 <= i && i < rv@.len() ==>
                    fresh_rg((^self).arena@[rv@[i]@],
                        rbase(sh.superblock.data_start_offset@, params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i),
                        rsize(params.data_disk_size@ - sh.superblock.data_start_offset@, params.region_count@, i), params)),
            _ => false,
        },
        Err(_) => true,
    })]
    #[ensures(fmt_ok_inputs(*self, params) || fmt_ok8(*self, params) ==> match result { Ok(_) => true, Err(e) => e == EmError::IoError })]
    pub fn format(&mut self, params: FormatParams) -> Result<(), EmError> {
        let (region_vec, shared) = self.format_build(params)?;
        self.publish_regions(region_vec);
        self.publish_shared(shared);
        proof_assert! { em_list_ok(*self) };
        Ok(())
    }

    /// Mirror of `remove_extent` (lib.rs:680-684) with `region_for_offset`
    /// (lib.rs:223-255) inlined.
    #[requires(em_regions_wf(*self) && em_regions_ok(*self))]
    #[ensures(em_regions_wf(^self) && em_regions_ok(^self))]
    #[ensures((^self).regions == self.regions && (^self).shared == self.shared && (^self).dev == self.dev)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures((^self).arena@.len() == self.arena@.len())]
    #[ensures(forall<i: Int> 0 <= i && i < self.arena@.len() ==> rg_frame(self.arena@[i], (^self).arena@[i]))]
    #[ensures(forall<i: Int> 0 <= i && i < self.arena@.len() ==> (^self).arena@[i].buddy == self.arena@[i].buddy)]
    #[ensures(match (self.regions, self.shared) {
        (Some(rv), Some(sh)) => sh.format_params.data_disk_size@ >= sh.superblock.data_start_offset@
            && (sh.format_params.data_disk_size@ - sh.superblock.data_start_offset@) / rv@.len() > 0
            && (if offset@ >= sh.superblock.data_start_offset@ { offset@ - sh.superblock.data_start_offset@ } else { 0 })
                / ((sh.format_params.data_disk_size@ - sh.superblock.data_start_offset@) / rv@.len()) >= rv@.len()
            ==> result == Err(EmError::OffsetNotFound(offset)) && ^self == *self,
        _ => true,
    })]
    #[ensures(match result {
        Ok(()) => exists<x: Int> 0 <= x && x < self.arena@.len() && rg_inv(self.arena@[x]) && rm_step(self.arena@[x], (^self).arena@[x], offset)
            && forall<i: Int> 0 <= i && i < self.arena@.len() && i != x ==> (^self).arena@[i] == self.arena@[i],
        Err(_) => (^self).arena@ == self.arena@,
    })]
    pub fn remove_extent(&mut self, offset: u64) -> Result<(), EmError> {
        let rv = match &self.regions {
            Some(rv) => rv,
            None => return Err(EmError::NotInitialized),
        };
        let (data_disk_size, data_start_offset) = match &self.shared {
            Some(s) => (s.format_params.data_disk_size, s.superblock.data_start_offset),
            None => return Err(EmError::NotInitialized),
        };
        if data_disk_size < data_start_offset {
            return Err(EmError::NotInitialized); // `data_disk_size - data_start_offset` underflows (lib.rs:243): excluded
        }
        let usable = data_disk_size - data_start_offset;
        let region_bytes = usable / rv.len() as u64;
        if region_bytes == 0 {
            return Err(EmError::NotInitialized);
        }
        let relative_offset = if offset >= data_start_offset { offset - data_start_offset } else { 0 };
        let idx = (relative_offset / region_bytes) as usize;
        if idx >= rv.len() {
            return Err(EmError::OffsetNotFound(offset));
        }
        let r = rv[idx];
        let pre = snapshot! { *self };
        let res = self.arena[r].remove_extent_by_offset(offset);
        proof_assert! { match res { Err(_) => self.arena@.ext_eq(pre.arena@), Ok(()) => rm_step(pre.arena@[r@], self.arena@[r@], offset) } };
        res
    }

    /// Mirror of `run_checkpoint` (lib.rs:275-370) and `write_checkpoint`
    /// (checkpoint.rs:35-115), single-threaded; the coalescing wrapper `checkpoint()`
    /// (lib.rs:729-761) only serialises calls to it.
    #[requires(em_regions_wf(*self) && em_regions_ok(*self))]
    #[requires(match self.regions { Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> pending_ok(self.arena@[rv@[i]@]), None => true })]
    #[requires(em_together(*self))]
    #[ensures(em_regions_wf(^self) && em_regions_ok(^self))]
    #[ensures((^self).regions == self.regions)]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures((^self).arena@.len() == self.arena@.len())]
    #[ensures(forall<i: Int> 0 <= i && i < self.arena@.len() ==> rg_frame(self.arena@[i], (^self).arena@[i]))]
    #[ensures(match (self.shared, (^self).shared) {
        (Some(a), Some(b)) => b.superblock.instance_id == a.superblock.instance_id
            && b.superblock.data_start_offset == a.superblock.data_start_offset
            && b.superblock.data_disk_size == a.superblock.data_disk_size
            && b.format_params == a.format_params,
        (None, None) => true,
        _ => false,
    })]
    #[ensures(seq_agree(*self) ==> seq_agree(^self))]
    #[ensures(match (self.shared, (^self).shared) { (Some(a), Some(b)) => sb_frame(a.superblock, b.superblock), _ => true })]
    #[ensures(match result {
        Ok(()) => (^self).dev.sb == self.dev.sb || match (^self).shared { Some(s) => (^self).dev.sb == Some(s.superblock), None => false },
        Err(_) => true,
    })]
    #[ensures((^self).dev.connected == self.dev.connected)]
    pub fn checkpoint(&mut self) -> Result<(), EmError> {
        let n = match &self.regions {
            Some(rv) => rv.len(),
            None => return Err(EmError::NotInitialized),
        };
        // any_dirty (lib.rs:276-286)
        let mut any_dirty = false;
        let mut i: usize = 0;
        while i < n {
            match &self.regions {
                Some(rv) => {
                    if self.arena[rv[i]].dirty {
                        any_dirty = true;
                    }
                }
                None => {}
            }
            i += 1;
        }
        if !any_dirty {
            return Ok(());
        }
        if !self.dev.connected {
            return Err(EmError::NotInitialized); // get_metadata_client (lib.rs:297)
        }

        // write_checkpoint Phase 2-4 (checkpoint.rs:58-112)
        let (inactive_copy, new_seq) = match &self.shared {
            Some(s) => {
                let inactive = if s.superblock.active_copy == 0 { 1u8 } else { 0u8 }; // `1 - active_copy` (checkpoint.rs:61)
                if s.checkpoint_seq == u64::MAX {
                    return Err(EmError::CorruptMetadata); // `checkpoint_seq + 1` overflows (checkpoint.rs:65): excluded
                }
                (inactive, s.checkpoint_seq + 1)
            }
            None => return Err(EmError::NotInitialized), // `.unwrap()` (checkpoint.rs:60)
        };
        dev_write_checkpoint(&mut self.dev, inactive_copy, new_seq, n)?;
        match &mut self.shared {
            Some(s) => {
                s.superblock.active_copy = inactive_copy;
                s.superblock.checkpoint_seq = new_seq;
                s.checkpoint_seq = new_seq;
            }
            None => {}
        }

        // Write updated superblock (lib.rs:302-307)
        let sb = match &self.shared {
            Some(s) => s.superblock,
            None => return Err(EmError::NotInitialized),
        };
        dev_write_superblock(&mut self.dev, sb)?;

        // Clear dirty flags and release deferred frees (lib.rs:312-321)
        let old = snapshot! { *self };
        let mut i: usize = 0;
        #[invariant(i@ <= n@)]
        #[invariant(self.regions == old.regions && self.shared == old.shared)]
        #[invariant(self.metadata_base_lba == old.metadata_base_lba && self.data_base_lba == old.data_base_lba)]
        #[invariant(self.arena@.len() == old.arena@.len() && em_regions_wf(*self) && em_regions_ok(*self))]
        #[invariant(forall<j: Int> 0 <= j && j < self.arena@.len() ==> rg_frame(old.arena@[j], self.arena@[j]))]
        #[invariant(match self.regions { Some(rv) => rv@.len() == n@
            && (forall<j: Int> i@ <= j && j < rv@.len() ==> self.arena@[rv@[j]@] == old.arena@[rv@[j]@]), None => false })]
        #[invariant(forall<j: Int> 0 <= j && j < self.arena@.len() && (match self.regions { Some(rv) => forall<k: Int> 0 <= k && k < rv@.len() ==> rv@[k]@ != j, None => true }) ==> self.arena@[j] == old.arena@[j])]
        while i < n {
            let r = match &self.regions {
                Some(rv) => rv[i],
                None => return Ok(()),
            };
            proof_assert! { pending_ok(self.arena@[r@]) };
            let before = snapshot! { self.arena };
            self.arena[r].dirty = false;
            self.arena[r].flush_pending_frees();
            proof_assert! { forall<j: Int> 0 <= j && j < self.arena@.len() && j != r@ ==> self.arena@[j] == before@[j] };
            i += 1;
        }
        Ok(())
    }
}

impl ExtentManager {
    /// Mirror of `IExtentManager::initialize` (lib.rs:514-582) after recovery: `sb` and
    /// `per_region` are what `recovery::recover` (lib.rs:519) returned. Everything before
    /// that point (`get_metadata_client`, `recover`) either fails with `?` before any state
    /// is touched, or yields these values; the checkpoint decode itself is owned by the
    /// checkpoint batch and is NOT modelled here.
    #[requires(em_regions_wf(*self))]
    #[requires(sb_sane(sb))]
    #[requires(recovered_ok(sb, per_region@))]
    #[ensures(arena_prefix(self.arena@, (^self).arena@, self.arena@.len()))]
    #[ensures(em_regions_wf(^self))]
    #[ensures((^self).metadata_base_lba == self.metadata_base_lba && (^self).data_base_lba == self.data_base_lba)]
    #[ensures((^self).dev == self.dev)]
    #[ensures(match ((^self).regions, (^self).shared) {
        (Some(rv), Some(sh)) =>
            rv@.len() == sb.region_count@
            && (forall<i: Int> 0 <= i && i < rv@.len() ==> self.arena@.len() <= rv@[i]@)
            && em_regions_ok(^self)
            && em_layout(^self)
            && sh.superblock == sb
            && sh.checkpoint_seq == sb.checkpoint_seq
            && sh.format_params.metadata_disk_ns_id == sb.metadata_disk_ns_id
            && (forall<i: Int> 0 <= i && i < rv@.len() ==> init_rg_ok((^self).arena@[rv@[i]@], dsel(per_region@, i)))
            && (forall<i: Int> 0 <= i && i < rv@.len() ==> (^self).arena@[rv@[i]@].format_params == sh.format_params)
            && sh.format_params.data_disk_size == sb.data_disk_size && sh.format_params.slab_size == sb.slab_size
            && sh.format_params.sector_size == sb.sector_size,
        _ => false,
    })]
    #[ensures(match (^self).regions {
        Some(rv) => forall<i: Int> 0 <= i && i < rv@.len() ==> init_geo((^self).arena@[rv@[i]@],
            rbase(sb.data_start_offset@, sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i),
            rsize(sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, i)),
        None => false,
    })]
    pub fn initialize(&mut self, sb: Superblock, per_region: &Vec<Vec<SlabDescriptor>>) {
        proof_assert! { lemma_sb_hd(sb); sb_hd(sb) };
        let format_params = FormatParams {
            data_disk_size: sb.data_disk_size,
            slab_size: sb.slab_size,
            max_extent_size: sb.max_extent_size,
            sector_size: sb.sector_size,
            region_count: sb.region_count,
            metadata_alignment: sb.checkpoint_region_offset,
            instance_id: Some(sb.instance_id),
            metadata_disk_ns_id: sb.metadata_disk_ns_id,
            metadata_region_size: sb.checkpoint_region_offset + 2 * sb.checkpoint_region_size,
        };

        let data_start_offset = sb.data_start_offset;
        // `sb.data_disk_size.saturating_sub(data_start_offset)` (lib.rs:534); sb_sane: no saturation
        let usable_data_size = sb.data_disk_size - data_start_offset;
        let region_count = sb.region_count as usize;

        let n0 = snapshot! { self.arena@.len() };
        let a0 = snapshot! { self.arena };
        let mut region_vec: Vec<usize> = Vec::new();
        let empty: Vec<SlabDescriptor> = Vec::new();
        let mut i: usize = 0;
        #[invariant(i@ <= region_count@ && region_vec@.len() == i@)]
        #[invariant(self.arena@.len() == *n0 + i@)]
        #[invariant(arena_prefix(a0@, self.arena@, *n0))]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==> region_vec@[j]@ == *n0 + j)]
        #[invariant(forall<j: Int> 0 <= j && j < i@ ==>
            init_reg(self.arena@[*n0 + j], data_start_offset@, data_start_offset@ + usable_data_size@, dsel(per_region@, j),
                rbase(data_start_offset@, usable_data_size@, region_count@, j), rsize(usable_data_size@, region_count@, j), format_params))]
        while i < region_count {
            let (base, size) = region_extent(data_start_offset, usable_data_size, region_count, i);
            let descs = if i < per_region.len() { &per_region[i] } else { &empty };
            proof_assert! { descs@ == dsel(per_region@, i@) };
            proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok_sb(descs@[p], base@, size@, sb) };
            proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> { lemma_desc_ok_sb(descs@[p], base@, size@, sb); desc_geo_sb(descs@[p], base@, size@, sb) } };
            proof_assert! { forall<p: Int> 0 <= p && p < descs@.len() ==> desc_ok(descs@[p], base@, size@, format_params) };
            let region = rebuild_region(base, size, format_params, descs);
            proof_assert! { init_rg_ok(region, dsel(per_region@, i@)) };
            proof_assert! { rg_within(region, data_start_offset@, data_start_offset@ + usable_data_size@) };
            let ar0 = snapshot! { self.arena };
            let rg = snapshot! { region };
            self.arena.push(region);
            proof_assert! { self.arena@ == ar0@.push_back(*rg) };
            proof_assert! { self.arena@[*n0 + i@] == *rg };
            proof_assert! { forall<j: Int> 0 <= j && j < *n0 + i@ ==> self.arena@[j] == ar0@[j] };
            proof_assert! { init_geo(*rg, rbase(data_start_offset@, usable_data_size@, region_count@, i@), rsize(usable_data_size@, region_count@, i@)) };
            proof_assert! { init_reg(*rg, data_start_offset@, data_start_offset@ + usable_data_size@, dsel(per_region@, i@),
                rbase(data_start_offset@, usable_data_size@, region_count@, i@), rsize(usable_data_size@, region_count@, i@), format_params) };
            proof_assert! { forall<j: Int> 0 <= j && j < i@ ==> self.arena@[*n0 + j] == ar0@[*n0 + j] };
            proof_assert! { forall<j: Int> 0 <= j && j < i@ + 1 ==>
                init_reg(self.arena@[*n0 + j], data_start_offset@, data_start_offset@ + usable_data_size@, dsel(per_region@, j),
                    rbase(data_start_offset@, usable_data_size@, region_count@, j), rsize(usable_data_size@, region_count@, j), format_params) };
            region_vec.push(self.arena.len() - 1);
            i += 1;
        }
        proof_assert! { region_vec@.len() == region_count@ };
        proof_assert! { forall<j: Int> 0 <= j && j < region_vec@.len() ==> rg_inv(self.arena@[*n0 + j])
            && rg_within(self.arena@[*n0 + j], data_start_offset@, data_start_offset@ + usable_data_size@) };
        proof_assert! { forall<j: Int> 0 <= j && j < region_vec@.len() ==> init_rg_ok(self.arena@[region_vec@[j]@], dsel(per_region@, j)) };
        proof_assert! { forall<j: Int> 0 <= j && j < region_vec@.len() ==> self.arena@[region_vec@[j]@].format_params == format_params };
        proof_assert! { forall<j: Int> 0 <= j && j < region_vec@.len() ==> rg_within(self.arena@[*n0 + j], sb.data_start_offset@, sb.data_disk_size@) };
        proof_assert! { forall<j: Int> 0 <= j && j < region_vec@.len() ==> init_geo(self.arena@[region_vec@[j]@],
            rbase(sb.data_start_offset@, sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, j),
            rsize(sb.data_disk_size@ - sb.data_start_offset@, sb.region_count@, j)) };

        let shared = SharedState { format_params, checkpoint_seq: sb.checkpoint_seq, superblock: sb };
        self.publish_regions(region_vec);
        self.publish_shared(shared);
    }
}
