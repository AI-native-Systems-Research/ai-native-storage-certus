//! Mirror of `BlockDeviceClient` (components/extent-manager/src/block_io.rs:6-159) and of
//! `ExtentManager::get_metadata_client` (lib.rs:185-209 at the pin).
//!
//! The client channel protocol (one `Command::WriteSync` / `ReadSync` per block, answered
//! by one completion, block_io.rs:82-104 / 125-151) is the trusted leaf pair
//! [`dev_write_sync`] / [`dev_read_sync`], whose contracts are the IBlockDevice per-block
//! contract over a ghost sector store keyed by (namespace, LBA): a write either completes
//! (the block now holds the buffer) or reports an error; a read either returns the block's
//! contents or an error. Every command also appends its (namespace, LBA) to a ghost access
//! log, so properties about WHERE the client reads and writes can be stated. DMA buffer
//! allocation (`alloc_buffer`, block_io.rs:42-47) is the trusted leaf [`dma_alloc`]: it may
//! fail; on success the buffer has the requested length (its contents are arbitrary).
use crate::model::component::*;
use crate::model::params::*;
use creusot_std::logic::FMap;
use crate::model::assume::*;
use creusot_std::prelude::*;

/// Ghost metadata block device: sector contents by (ns, lba), and the command log.
pub struct BDev {
    pub store: Snapshot<FMap<(Int, Int), Seq<u8>>>,
    pub log: Snapshot<Seq<(Int, Int)>>,
}

/// TRUSTED device leaf: send `WriteSync { ns_id, lba, buf }` and await its completion
/// (block_io.rs:82-104).
#[trusted]
#[ensures((*(^dev).log) == (*dev.log).push_back((ns@, lba@)))]
#[ensures(match result {
    Ok(()) => *(^dev).store == (*dev.store).insert((ns@, lba@), buf@),
    Err(e) => e == EmError::IoError,
})]
pub fn dev_write_sync(dev: &mut BDev, ns: u32, lba: u64, buf: &Vec<u8>) -> Result<(), EmError> {
    Ok(())
}

/// TRUSTED device leaf: send `ReadSync { ns_id, lba, buf }` and await its completion
/// (block_io.rs:125-151); `n` is the sector size.
#[trusted]
#[ensures((*(^dev).log) == (*dev.log).push_back((ns@, lba@)))]
#[ensures(*(^dev).store == *dev.store)]
#[ensures(match result {
    Ok(v) => v@.len() == n@ && match (*dev.store).get((ns@, lba@)) { Some(s) => v@ == s, None => true },
    Err(e) => e == EmError::IoError,
})]
pub fn dev_read_sync(dev: &mut BDev, ns: u32, lba: u64, n: usize) -> Result<Vec<u8>, EmError> {
    Ok(Vec::new())
}

/// TRUSTED leaf: `(self.alloc)(size, align, None)` (block_io.rs:42-47) — a DMA buffer of
/// `n` bytes, or an allocation error.
#[trusted]
#[ensures(match result { Ok(v) => v@.len() == n@, Err(e) => e == EmError::IoError })]
pub fn dma_alloc(n: usize) -> Result<Vec<u8>, EmError> {
    Ok(Vec::new())
}

/// Byte `k` of `data` zero-padded to any length.
#[logic(open)]
pub fn padded(data: Seq<u8>, k: Int) -> u8 {
    pearlite! { if k < data.len() { data[k] } else { 0u8 } }
}

/// Mirror of `BlockDeviceClient` (block_io.rs:6-12); the channels and the DMA allocator
/// are behind the trusted leaves above.
pub struct Client {
    pub sector_size: u32,
    pub ns_id: u32,
    pub base_lba: u64,
}

impl Client {
    /// Mirror of `BlockDeviceClient::write_blocks` (block_io.rs:49-108).
    #[requires(self.sector_size@ > 0)]
    #[requires(data@.len() + self.sector_size@ <= usize::MAX@)]
    #[requires(self.base_lba@ + lba@ + (data@.len() + self.sector_size@ - 1) / self.sector_size@ <= u64::MAX@)]
    #[requires((data@.len() + self.sector_size@ - 1) / self.sector_size@ * self.sector_size@ <= usize::MAX@)]
    #[ensures(forall<p: Int> 0 <= p && p < dev.log.len() ==> (*(^dev).log)[p] == (*dev.log)[p])]
    #[ensures((*dev.log).len() <= (*(^dev).log).len())]
    #[ensures((*(^dev).log).len() <= (*dev.log).len() + (data@.len() + self.sector_size@ - 1) / self.sector_size@)]
    #[ensures(forall<p: Int> dev.log.len() <= p && p < (^dev).log.len() ==>
        (*(^dev).log)[p] == (self.ns_id@, self.base_lba@ + lba@ + (p - dev.log.len())))]
    #[ensures(match result {
        Ok(()) => (*(^dev).log).len() == (*dev.log).len() + (data@.len() + self.sector_size@ - 1) / self.sector_size@
            && forall<i: Int> 0 <= i && i < (data@.len() + self.sector_size@ - 1) / self.sector_size@ ==>
                match (*(^dev).store).get((self.ns_id@, self.base_lba@ + lba@ + i)) {
                    Some(blk) => blk.len() == self.sector_size@
                        && forall<j: Int> 0 <= j && j < self.sector_size@ ==> blk[j] == padded(data@, i * self.sector_size@ + j),
                    None => false,
                },
        Err(_) => true,
    })]
    pub fn write_blocks(&self, dev: &mut BDev, lba: u64, data: &Vec<u8>) -> Result<(), EmError> {
        let ss = self.sector_size as usize;
        let num_blocks = (data.len() + ss - 1) / ss;
        proof_assert! { ss@ == self.sector_size@ };
        let buf_size = num_blocks * ss;
        proof_assert! { lemma_ceil(data@.len(), ss@); data@.len() <= buf_size@ };

        let mut buf = dma_alloc(buf_size)?;
        // `buf.as_mut_slice()[..data.len()].copy_from_slice(data)` + zero fill (block_io.rs:58-63)
        let mut k: usize = 0;
        #[invariant(k@ <= buf_size@ && buf@.len() == buf_size@)]
        #[invariant(forall<q: Int> 0 <= q && q < k@ ==> buf@[q] == padded(data@, q))]
        while k < buf_size {
            if k < data.len() {
                buf[k] = data[k];
            } else {
                buf[k] = 0;
            }
            k += 1;
        }

        let d0 = snapshot! { *dev };
        let mut i: usize = 0;
        #[invariant(i@ <= num_blocks@)]
        #[invariant(dev.log.len() == d0.log.len() + i@)]
        #[invariant(forall<p: Int> 0 <= p && p < d0.log.len() ==> (*dev.log)[p] == (*d0.log)[p])]
        #[invariant(forall<p: Int> d0.log.len() <= p && p < dev.log.len() ==>
            (*dev.log)[p] == (self.ns_id@, self.base_lba@ + lba@ + (p - d0.log.len())))]
        #[invariant(forall<b: Int> 0 <= b && b < i@ ==>
            match (*dev.store).get((self.ns_id@, self.base_lba@ + lba@ + b)) {
                Some(blk) => blk.len() == ss@ && forall<j: Int> 0 <= j && j < ss@ ==> blk[j] == padded(data@, b * ss@ + j),
                None => false,
            })]
        while i < num_blocks {
            let block_lba = self.base_lba + lba + i as u64;
            let block_start = i * ss;
            let block_end = block_start + ss;
            proof_assert! { lemma_mul_le_nb(i@, num_blocks@, ss@); block_end@ <= buf_size@ };
            let mut block_buf = dma_alloc(ss)?;
            let mut j: usize = 0;
            #[invariant(j@ <= ss@ && block_buf@.len() == ss@)]
            #[invariant(forall<q: Int> 0 <= q && q < j@ ==> block_buf@[q] == padded(data@, i@ * ss@ + q))]
            while j < ss {
                block_buf[j] = buf[block_start + j];
                j += 1;
            }
            let before = snapshot! { *dev };
            dev_write_sync(dev, self.ns_id, block_lba, &block_buf)?;
            proof_assert! { block_lba@ == self.base_lba@ + lba@ + i@ };
            proof_assert! { (*dev.store).get((self.ns_id@, block_lba@)) == Some(block_buf@) };
            proof_assert! { forall<b: Int> 0 <= b && b < i@ ==>
                (*dev.store).get((self.ns_id@, self.base_lba@ + lba@ + b)) == (*before.store).get((self.ns_id@, self.base_lba@ + lba@ + b)) };
            proof_assert! { forall<j: Int> 0 <= j && j < ss@ ==> block_buf@[j] == padded(data@, i@ * ss@ + j) };
            i += 1;
        }
        proof_assert! { ss@ == self.sector_size@ && num_blocks@ == (data@.len() + self.sector_size@ - 1) / self.sector_size@ };
        proof_assert! { forall<b: Int> 0 <= b && b < num_blocks@ ==>
            match (*dev.store).get((self.ns_id@, self.base_lba@ + lba@ + b)) {
                Some(blk) => blk.len() == self.sector_size@ && forall<j: Int> 0 <= j && j < self.sector_size@ ==> blk[j] == padded(data@, b * self.sector_size@ + j),
                None => false,
            } };
        Ok(())
    }

    /// Mirror of `BlockDeviceClient::read_blocks` (block_io.rs:110-155).
    #[requires(self.sector_size@ > 0)]
    #[requires(num_bytes@ + self.sector_size@ <= usize::MAX@)]
    #[requires(self.base_lba@ + lba@ + (num_bytes@ + self.sector_size@ - 1) / self.sector_size@ <= u64::MAX@)]
    #[ensures(forall<p: Int> 0 <= p && p < dev.log.len() ==> (*(^dev).log)[p] == (*dev.log)[p])]
    #[ensures((*dev.log).len() <= (*(^dev).log).len())]
    #[ensures(forall<p: Int> dev.log.len() <= p && p < (^dev).log.len() ==>
        (*(^dev).log)[p] == (self.ns_id@, self.base_lba@ + lba@ + (p - dev.log.len())))]
    #[ensures(*(^dev).store == *dev.store)]
    #[ensures(match result {
        Ok(v) => v@.len() == num_bytes@
            && forall<q: Int> 0 <= q && q < num_bytes@ ==>
                match (*dev.store).get((self.ns_id@, self.base_lba@ + lba@ + q / self.sector_size@)) {
                    Some(blk) => v@[q] == blk[q % self.sector_size@],
                    None => true,
                },
        Err(e) => e == EmError::IoError,
    })]
    pub fn read_blocks(&self, dev: &mut BDev, lba: u64, num_bytes: usize) -> Result<Vec<u8>, EmError> {
        let ss = self.sector_size as usize;
        let num_blocks = (num_bytes + ss - 1) / ss;
        let mut result: Vec<u8> = Vec::new();
        let d0 = snapshot! { *dev };
        let mut i: usize = 0;
        #[invariant(i@ <= num_blocks@)]
        #[invariant(*dev.store == *d0.store)]
        #[invariant(dev.log.len() == d0.log.len() + i@)]
        #[invariant(forall<p: Int> 0 <= p && p < d0.log.len() ==> (*dev.log)[p] == (*d0.log)[p])]
        #[invariant(forall<p: Int> d0.log.len() <= p && p < dev.log.len() ==>
            (*dev.log)[p] == (self.ns_id@, self.base_lba@ + lba@ + (p - d0.log.len())))]
        #[invariant(result@.len() <= num_bytes@)]
        #[invariant(result@.len() == if i@ * ss@ <= num_bytes@ { i@ * ss@ } else { num_bytes@ })]
        #[invariant(forall<q: Int> 0 <= q && q < result@.len() ==>
            match (*d0.store).get((self.ns_id@, self.base_lba@ + lba@ + q / ss@)) {
                Some(b) => result@[q] == b[q % ss@],
                None => true,
            })]
        while i < num_blocks {
            let block_lba = self.base_lba + lba + i as u64;
            proof_assert! { lemma_ceil_lt(num_bytes@, ss@, i@); i@ * ss@ < num_bytes@ };
            let blk = dev_read_sync(dev, self.ns_id, block_lba, ss)?;
            // `result.extend_from_slice(&locked.as_slice()[..remaining.min(sector_size)])`
            let remaining = num_bytes - result.len();
            let to_copy = if remaining < ss { remaining } else { ss };
            let mut j: usize = 0;
            let r0 = snapshot! { result@.len() };
            proof_assert! { *r0 == i@ * ss@ };
            #[invariant(j@ <= to_copy@ && result@.len() == *r0 + j@)]
            #[invariant(forall<q: Int> 0 <= q && q < result@.len() ==>
                match (*d0.store).get((self.ns_id@, self.base_lba@ + lba@ + q / ss@)) {
                    Some(b) => result@[q] == b[q % ss@],
                    None => true,
                })]
            while j < to_copy {
                proof_assert! { lemma_divmod_at(i@, ss@, j@); (*r0 + j@) / ss@ == i@ && (*r0 + j@) % ss@ == j@ };
                result.push(blk[j]);
                j += 1;
            }
            proof_assert! { lemma_ceil_next(num_bytes@, ss@, i@);
                result@.len() == if (i@ + 1) * ss@ <= num_bytes@ { (i@ + 1) * ss@ } else { num_bytes@ } };
            i += 1;
        }
        proof_assert! { lemma_ceil(num_bytes@, ss@); num_blocks@ * ss@ >= num_bytes@ };
        Ok(result)
    }
}

/// `ceil(n / d) * d >= n`.
#[logic]
#[requires(d > 0 && n >= 0)]
#[ensures((n + d - 1) / d * d >= n)]
pub fn lemma_ceil(n: Int, d: Int) {
    pearlite! { crate::model::buddy::lemma_divmod(n + d - 1, d) }
}

/// Block `i < ceil(n / d)` starts inside the first `n` bytes (batch 5).
#[logic]
#[requires(d > 0 && n >= 0 && 0 <= i && i < (n + d - 1) / d)]
#[ensures(i * d < n)]
pub fn lemma_ceil_lt(n: Int, d: Int, i: Int) {
    pearlite! {
        crate::model::buddy::lemma_divmod(n + d - 1, d);
        crate::model::buddy::lemma_mul_le(i + 1, (n + d - 1) / d, d)
    }
}

/// After block `i` the copied prefix is `min((i + 1) * d, n)` (batch 5).
#[logic]
#[requires(d > 0 && n >= 0 && 0 <= i && i * d < n)]
#[ensures(((i + 1) * d <= n ==> (if n - i * d < d { n - i * d } else { d }) == d)
    && ((i + 1) * d > n ==> i * d + (if n - i * d < d { n - i * d } else { d }) == n))]
pub fn lemma_ceil_next(n: Int, d: Int, i: Int) {}

/// `(i * d + j) / d == i` and `(i * d + j) % d == j` for `0 <= j < d` (batch 5).
#[logic]
#[requires(d > 0 && i >= 0 && 0 <= j && j < d)]
#[ensures((i * d + j) / d == i && (i * d + j) % d == j)]
pub fn lemma_divmod_at(i: Int, d: Int, j: Int) {
    pearlite! { crate::model::buddy::lemma_divmod(i * d + j, d) }
}

/// `(i + 1) * d <= nb * d` for `i < nb`.
#[logic]
#[requires(0 <= i && i < nb && d > 0)]
#[ensures(i * d + d <= nb * d)]
pub fn lemma_mul_le_nb(i: Int, nb: Int, d: Int) {
    pearlite! { crate::model::buddy::lemma_mul_le(i + 1, nb, d) }
}

impl ExtentManager {
    /// Mirror of `get_metadata_client` (lib.rs:185-209): the client carries the namespace,
    /// the device's sector size and the CURRENT `metadata_base_lba` (lib.rs:200). The
    /// `connect_client` / `sector_size(ns)` queries are folded into the `sector_size`
    /// argument (their failures return before any I/O).
    #[ensures(result.base_lba == self.metadata_base_lba && result.ns_id == ns_id && result.sector_size == sector_size)]
    pub fn get_metadata_client(&self, ns_id: u32, sector_size: u32) -> Client {
        Client { sector_size, ns_id, base_lba: self.metadata_base_lba }
    }
}
