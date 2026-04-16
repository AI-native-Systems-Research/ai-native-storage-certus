//! Extent Manager component for the Certus storage system.
//!
//! Manages fixed-size storage extents on NVMe SSDs with crash-consistent
//! metadata persistence. Uses bitmap-based space allocation with 4KiB-atomic
//! writes exploiting NVMe power-fail guarantees.
//!
//! # Architecture
//!
//! - `IExtentManager` interface: create, remove, lookup, iterate extents
//! - **Bitmap allocation**: one bit per slot per size class
//! - **On-disk format**: superblock + bitmap region + extent record region
//! - **Crash recovery**: orphan detection and cleanup on startup
//!
//! # Requirements
//!
//! This crate requires the `spdk` feature (enabled by default) and an
//! `IBlockDevice` provider (e.g., `block-device-spdk-nvme`).
//!
//! # Component lifecycle
//!
//! 1. Create: `ExtentManagerComponentV1::new_default()`
//! 2. Wire receptacles: connect `IBlockDevice` to `block_device`, `ILogger` to `logger`
//! 3. Initialize: `comp.initialize(sizes, slots, ns_id)` or `comp.open(ns_id)`
//! 4. Use via `IExtentManager` interface

pub mod bitmap;
pub mod block_device;
pub mod error;
pub mod metadata;
pub mod recovery;
pub mod superblock;

#[cfg(any(test, feature = "testing"))]
pub mod test_support;

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use component_framework::define_component;
use interfaces::{IBlockDevice, IExtentManager, IExtentManagerAdmin, ILogger};

use crate::block_device::DmaAllocFn;

use crate::bitmap::AllocationBitmap;
use crate::block_device::BlockDevice;
use crate::metadata::OnDiskExtentRecord;
use crate::superblock::{Superblock, SUPERBLOCK_LBA};

pub use crate::error::ExtentManagerError;
pub use crate::metadata::ExtentMetadata;
pub use crate::recovery::RecoveryResult;

/// Runtime configuration for the extent manager.
#[derive(Debug, Clone)]
pub struct ExtentManagerConfig {
    /// Configured extent sizes in bytes, indexed by size class.
    pub sizes: Vec<u32>,
    /// Maximum slots per size class.
    pub slots: Vec<u32>,
    /// NVMe namespace ID.
    pub ns_id: u32,
}

/// Internal state created during initialization.
///
/// Stored inside the component behind `RwLock<Option<...>>` so that
/// the outer lock is always read-locked during operations (concurrent
/// access) while the inner `RwLock`/`Mutex` provide fine-grained locking.
struct ExtentManagerState {
    bd: BlockDevice,
    superblock: Superblock,
    config: ExtentManagerConfig,
    index: RwLock<HashMap<u64, ExtentMetadata>>,
    bitmaps: Vec<Mutex<AllocationBitmap>>,
}

// Extent Manager component.
//
// Provides IExtentManager and requires IBlockDevice via receptacle.
define_component! {
    pub ExtentManagerComponentV1 {
        version: "0.1.0",
        provides: [IExtentManager, IExtentManagerAdmin],
        receptacles: {
            block_device: IBlockDevice,
            logger: ILogger,
        },
        fields: {
            state: RwLock<Option<ExtentManagerState>>,
            dma_alloc: Mutex<Option<DmaAllocFn>>,
        },
    }
}

impl ExtentManagerComponentV1 {
    /// Emit a debug-level log message.
    ///
    /// If the `logger` receptacle is connected, acknowledges connectivity;
    /// always writes to stderr with `[extent-manager]` prefix.
    fn log_debug(&self, msg: &str) {
        if let Ok(logger) = self.logger.get() {
            let _ = logger.name();
        }
        eprintln!("[extent-manager] {msg}");
    }

    /// Set a custom DMA allocator (used in tests to avoid SPDK dependency).
    ///
    /// If not set, the default SPDK hugepage allocator is used.
    fn set_dma_alloc(&self, alloc: DmaAllocFn) {
        *self.dma_alloc.lock().expect("dma_alloc lock poisoned") = Some(alloc);
    }

    /// Helper to connect to the IBlockDevice from the receptacle.
    fn connect_block_device(&self, ns_id: u32) -> Result<BlockDevice, ExtentManagerError> {
        let ibd = self.block_device.get().map_err(|_| {
            ExtentManagerError::NotInitialized(
                "block_device receptacle not connected — wire IBlockDevice before initializing"
                    .into(),
            )
        })?;

        let dma_alloc = self
            .dma_alloc
            .lock()
            .expect("dma_alloc lock poisoned")
            .clone();

        let bd = if let Some(alloc) = dma_alloc {
            BlockDevice::new_with_alloc(&ibd, ns_id, alloc)
        } else {
            BlockDevice::new(&ibd, ns_id)
        }
        .map_err(|e| ExtentManagerError::IoError(format!("block device connect failed: {e}")))?;

        self.log_debug(&format!(
            "block device connected: {} blocks, ns_id={}",
            bd.block_count(),
            ns_id
        ));
        Ok(bd)
    }

    /// Initialize a new extent manager on a fresh block device.
    ///
    /// Writes the superblock and empty bitmaps. The device must have
    /// enough blocks to hold the metadata regions.
    ///
    /// Must be called after wiring the `block_device` receptacle.
    ///
    /// # Errors
    ///
    /// Returns an error if receptacles are not wired, the device is too small,
    /// or I/O fails.
    fn initialize(
        &self,
        sizes: &[u32],
        slots: &[u32],
        ns_id: u32,
    ) -> Result<(), ExtentManagerError> {
        self.log_debug(&format!(
            "initializing: {} size classes, ns_id={}",
            sizes.len(),
            ns_id
        ));

        let bd = self.connect_block_device(ns_id)?;
        let sb = Superblock::new(sizes, slots)?;

        self.log_debug(&format!(
            "device: {} blocks available, {} required",
            bd.block_count(),
            sb.total_blocks_required()
        ));

        if bd.block_count() < sb.total_blocks_required() {
            return Err(ExtentManagerError::IoError(format!(
                "device too small: need {} blocks, have {}",
                sb.total_blocks_required(),
                bd.block_count()
            )));
        }

        // Write superblock.
        let sb_block = sb.serialize();
        bd.write_block(SUPERBLOCK_LBA, &sb_block)
            .map_err(ExtentManagerError::IoError)?;

        // Initialize and persist empty bitmaps.
        let mut bitmaps = Vec::with_capacity(sizes.len());
        for class in 0..sb.num_size_classes() {
            let bm = AllocationBitmap::new(sb.slots_for_class(class).unwrap());
            let lba = sb.bitmap_lba_for_class(class).unwrap();
            bm.persist(&bd, lba).map_err(ExtentManagerError::IoError)?;
            bitmaps.push(Mutex::new(bm));
        }

        let inner = ExtentManagerState {
            bd,
            superblock: sb,
            config: ExtentManagerConfig {
                sizes: sizes.to_vec(),
                slots: slots.to_vec(),
                ns_id,
            },
            index: RwLock::new(HashMap::new()),
            bitmaps,
        };

        *self.state.write().expect("state lock poisoned") = Some(inner);
        self.log_debug("initialized successfully");
        Ok(())
    }

    /// Open an existing extent manager from a block device, performing
    /// crash recovery.
    ///
    /// Reads the superblock, loads bitmaps, scans records, and cleans
    /// orphans.
    ///
    /// Must be called after wiring the `block_device` receptacle.
    ///
    /// # Errors
    ///
    /// Returns an error if the superblock is invalid, receptacles are not
    /// wired, or I/O fails.
    fn open(&self, ns_id: u32) -> Result<RecoveryResult, ExtentManagerError> {
        self.log_debug(&format!("opening existing store, ns_id={ns_id}"));

        let bd = self.connect_block_device(ns_id)?;

        let mut sb_block = [0u8; 4096];
        bd.read_block(SUPERBLOCK_LBA, &mut sb_block)
            .map_err(ExtentManagerError::IoError)?;
        let sb = Superblock::deserialize(&sb_block)?;

        let (index, bm_list, stats) = recovery::recover(&bd, &sb)?;

        self.log_debug(&format!(
            "recovery complete: {} extents loaded, {} orphans cleaned",
            stats.extents_loaded, stats.orphans_cleaned
        ));

        let sizes: Vec<u32> = (0..sb.num_size_classes())
            .map(|c| sb.size_for_class(c).unwrap())
            .collect();
        let slots: Vec<u32> = (0..sb.num_size_classes())
            .map(|c| sb.slots_for_class(c).unwrap())
            .collect();

        let bitmaps: Vec<Mutex<AllocationBitmap>> = bm_list.into_iter().map(Mutex::new).collect();

        let inner = ExtentManagerState {
            bd,
            superblock: sb,
            config: ExtentManagerConfig {
                sizes,
                slots,
                ns_id,
            },
            index: RwLock::new(index),
            bitmaps,
        };

        *self.state.write().expect("state lock poisoned") = Some(inner);
        Ok(stats)
    }

    /// Get a read lock on the initialized state.
    fn get_state(
        &self,
    ) -> Result<std::sync::RwLockReadGuard<'_, Option<ExtentManagerState>>, ExtentManagerError>
    {
        let guard = self.state.read().expect("state lock poisoned");
        if guard.is_none() {
            return Err(ExtentManagerError::NotInitialized(
                "call initialize() or open() before using the extent manager".into(),
            ));
        }
        Ok(guard)
    }
}

impl ExtentManagerState {
    fn create_extent(
        &self,
        key: u64,
        size_class: u32,
        filename: Option<&str>,
        data_crc: Option<u32>,
    ) -> Result<ExtentMetadata, ExtentManagerError> {
        if size_class >= self.superblock.num_size_classes() {
            return Err(ExtentManagerError::InvalidSizeClass(size_class));
        }

        // Check for duplicate key (write lock because we'll insert).
        let mut index = self.index.write().unwrap();
        if index.contains_key(&key) {
            return Err(ExtentManagerError::DuplicateKey(key));
        }

        // Allocate a slot from the bitmap.
        let mut bm = self.bitmaps[size_class as usize].lock().unwrap();
        let slot = bm
            .find_first_free()
            .ok_or(ExtentManagerError::OutOfSpace { size_class })?;

        let extent_size = self.superblock.size_for_class(size_class).unwrap();
        let global = self.superblock.global_slot(size_class, slot);
        let offset_blocks = global;

        let meta = ExtentMetadata {
            key,
            size_class,
            extent_size,
            ns_id: self.config.ns_id,
            offset_blocks,
            filename: filename.map(|s| s.to_string()),
            data_crc,
        };

        // Step 1: Write the extent record block atomically.
        let record = OnDiskExtentRecord::serialize(&meta)?;
        let record_lba = self.superblock.record_lba(global);
        self.bd
            .write_block(record_lba, record.as_bytes())
            .map_err(ExtentManagerError::IoError)?;

        // Step 2: Flip the bitmap bit and persist the bitmap block atomically.
        bm.set(slot);
        let bm_lba = self.superblock.bitmap_lba_for_class(size_class).unwrap();
        bm.persist_block_for_slot(&self.bd, bm_lba, slot)
            .map_err(ExtentManagerError::IoError)?;

        // Step 3: Update the in-memory index.
        index.insert(key, meta.clone());

        Ok(meta)
    }

    fn remove_extent(&self, key: u64) -> Result<(), ExtentManagerError> {
        let mut index = self.index.write().unwrap();
        let meta = index
            .get(&key)
            .ok_or(ExtentManagerError::KeyNotFound(key))?
            .clone();

        let class = meta.size_class;
        let global = self.superblock.global_slot(class, 0);
        let slot = (meta.offset_blocks - global) as u32;

        let mut bm = self.bitmaps[class as usize].lock().unwrap();
        bm.clear(slot);
        let bm_lba = self.superblock.bitmap_lba_for_class(class).unwrap();
        bm.persist_block_for_slot(&self.bd, bm_lba, slot)
            .map_err(ExtentManagerError::IoError)?;

        index.remove(&key);
        Ok(())
    }

    fn lookup_extent(&self, key: u64) -> Result<ExtentMetadata, ExtentManagerError> {
        let index = self.index.read().unwrap();
        index
            .get(&key)
            .cloned()
            .ok_or(ExtentManagerError::KeyNotFound(key))
    }

    fn extent_count(&self) -> u64 {
        let index = self.index.read().unwrap();
        index.len() as u64
    }
}

impl IExtentManager for ExtentManagerComponentV1 {
    fn create_extent(
        &self,
        key: u64,
        size_class: u32,
        filename: &str,
        data_crc: u32,
        has_crc: bool,
    ) -> Result<Vec<u8>, ExtentManagerError> {
        let guard = self.get_state()?;
        let state = guard.as_ref().unwrap();

        let fname = if filename.is_empty() {
            None
        } else {
            Some(filename)
        };
        let crc = if has_crc { Some(data_crc) } else { None };

        let meta = state.create_extent(key, size_class, fname, crc)?;
        self.log_debug(&format!(
            "created extent key={key} class={size_class} offset={}",
            meta.offset_blocks
        ));
        Ok(meta.to_bytes())
    }

    fn remove_extent(&self, key: u64) -> Result<(), ExtentManagerError> {
        let guard = self.get_state()?;
        let state = guard.as_ref().unwrap();
        state.remove_extent(key)?;
        self.log_debug(&format!("removed extent key={key}"));
        Ok(())
    }

    fn lookup_extent(&self, key: u64) -> Result<Vec<u8>, ExtentManagerError> {
        let guard = self.get_state()?;
        let state = guard.as_ref().unwrap();
        let meta = state.lookup_extent(key)?;
        Ok(meta.to_bytes())
    }

    fn extent_count(&self) -> u64 {
        let guard = self.state.read().expect("state lock poisoned");
        match guard.as_ref() {
            Some(state) => state.extent_count(),
            None => 0,
        }
    }
}

impl interfaces::IExtentManagerAdmin for ExtentManagerComponentV1 {
    fn set_dma_alloc(&self, alloc: interfaces::spdk_types::DmaAllocFn) {
        self.set_dma_alloc(alloc);
    }

    fn initialize(
        &self,
        sizes: Vec<u32>,
        slots: Vec<u32>,
        ns_id: u32,
    ) -> Result<(), interfaces::NvmeBlockError> {
        self.initialize(&sizes, &slots, ns_id)
            .map_err(|e| interfaces::NvmeBlockError::NotSupported(e.to_string()))
    }

    fn open(&self, ns_id: u32) -> Result<interfaces::RecoveryResult, interfaces::NvmeBlockError> {
        let stats = self
            .open(ns_id)
            .map_err(|e| interfaces::NvmeBlockError::NotSupported(e.to_string()))?;
        Ok(interfaces::RecoveryResult {
            extents_loaded: stats.extents_loaded,
            orphans_cleaned: stats.orphans_cleaned,
            corrupt_records: stats.corrupt_records,
        })
    }
}
