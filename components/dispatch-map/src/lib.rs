//! DispatchMap component for the Certus storage system.
//!
//! Maps extent keys ([`CacheKey`]) to their current location — either a
//! memory-tier pointer or a block-device offset — with readers-writer
//! reference counting for concurrent access.
//!
//! Provides the [`IDispatchMap`] interface with receptacles for [`ILogger`]
//! and [`IExtentManager`].

mod entry;
mod state;

pub use state::DispatchMapState;

/// Returns the size of `DispatchEntry` in bytes (for benchmarks/assertions).
pub fn entry_size() -> usize {
    std::mem::size_of::<entry::DispatchEntry>()
}

use std::time::Duration;

use component_framework::define_component;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(2000);
use interfaces::{
    CacheKey, DispatchMapError, IDispatchMap, IEvictionPolicy, IExtentManager, ILogger,
    LookupResult,
};

use crate::entry::{DispatchEntry, Location};

define_component! {
    pub DispatchMapComponent {
        version: "0.2.0",
        provides: [IDispatchMap],
        receptacles: {
            logger: ILogger,
            extent_manager: IExtentManager,
            eviction_policy: IEvictionPolicy,
        },
        fields: {
            state: DispatchMapState,
        },
    }
}

impl DispatchMapComponent {
    /// Get or create the eviction pool. Creates on first call (during initialize).
    fn get_pool_id(&self) -> interfaces::PoolId {
        let mut pool_id = self.state.pool_id.lock().unwrap();
        if let Some(id) = *pool_id {
            return id;
        }
        let ep = self.eviction_policy.get().unwrap();
        let id = ep.create_pool();
        *pool_id = Some(id);
        id
    }
}

impl IDispatchMap for DispatchMapComponent {

    /// Recover dispatch map state from the extent manager's persisted extents.
    /// Each extent is re-inserted as a BlockDevice location with zero reference
    /// counts, restoring the map to a consistent view of committed storage.
    /// If no extent manager is bound, starts with an empty map.
    fn initialize(&self) -> Result<(), DispatchMapError> {
        let pool_id = self.get_pool_id();
        let ep = self.eviction_policy.get().map_err(|_| {
            DispatchMapError::NotInitialized("eviction_policy receptacle not connected".into())
        })?;

        let em = match self.extent_manager.get() {
            Ok(em) => em,
            Err(_) => {
                if let Ok(logger) = self.logger.get() {
                    logger.info("dispatch-map: no extent_manager bound, starting fresh");
                }
                return Ok(());
            }
        };

        if let Ok(logger) = self.logger.get() {
            logger.info("dispatch-map: beginning state recovery from extent manager");
        }

        let mut inner = self.state.inner.lock().unwrap();
        let mut count: u64 = 0;

        em.for_each_extent(&mut |extent| {
            let eviction_handle = ep.track(pool_id, extent.key).unwrap();
            let entry = DispatchEntry {
                location: Location::BlockDevice {
                    offset: extent.offset,
                },
                size_blocks: extent.size,
                read_ref: 0,
                write_ref: 0,
                eviction_handle,
            };
            inner.entries.insert(extent.key, entry);
            count += 1;
        });

        if let Ok(logger) = self.logger.get() {
            logger.info(&format!(
                "dispatch-map: state recovery complete — {count} extents restored"
            ));
        }
        Ok(())
    }

    fn lookup(&self, key: CacheKey) -> Result<LookupResult, DispatchMapError> {
        let satisfied = self
            .state
            .wait_for(DEFAULT_TIMEOUT, |inner| match inner.entries.get(&key) {
                None => true,
                Some(e) => e.write_ref == 0,
            });

        let mut inner = self.state.inner.lock().unwrap();
        let entry = match inner.entries.get_mut(&key) {
            None => return Ok(LookupResult::NotExist),
            Some(e) => e,
        };

        if !satisfied && entry.write_ref > 0 {
            return Err(DispatchMapError::Timeout(key));
        }

        entry.read_ref = entry
            .read_ref
            .checked_add(1)
            .ok_or(DispatchMapError::RefCountOverflow(key))?;
        let handle = entry.eviction_handle;

        let result = match &entry.location {
            Location::BlockDevice { offset } => LookupResult::BlockDevice { offset: *offset },
            Location::MemoryTier { pointer, size, .. } => LookupResult::MemoryTier {
                pointer: *pointer,
                size: *size,
            },
        };

        drop(inner);
        if let Ok(ep) = self.eviction_policy.get() {
            let _ = ep.touch(handle);
        }

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("dispatch-map: lookup key {key} → {result:?}"));
        }

        Ok(result)
    }

    fn convert_to_storage(&self, key: CacheKey, offset: u64) -> Result<(), DispatchMapError> {
        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        match &mut entry.location {
            Location::MemoryTier { ssd_offset, .. } => {
                *ssd_offset = Some(offset);
            }
            Location::BlockDevice { .. } => {
                return Err(DispatchMapError::InvalidState(
                    "entry is already in block-device state".into(),
                ));
            }
        }

        if entry.read_ref > 0 {
            entry.read_ref -= 1;
        }

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!(
                "dispatch-map: converted key {key} to storage at offset {offset}"
            ));
        }

        drop(inner);
        self.state.condvar.notify_all();

        Ok(())
    }

    fn take_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let satisfied = self.state.wait_for(DEFAULT_TIMEOUT, |inner| {
            inner.entries.get(&key).map_or(true, |e| e.write_ref == 0)
        });

        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        if !satisfied && entry.write_ref > 0 {
            return Err(DispatchMapError::Timeout(key));
        }

        entry.read_ref = entry
            .read_ref
            .checked_add(1)
            .ok_or(DispatchMapError::RefCountOverflow(key))?;
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("dispatch-map: take_read key {key}"));
        }
        Ok(())
    }

    fn take_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let satisfied = self.state.wait_for(DEFAULT_TIMEOUT, |inner| {
            inner
                .entries
                .get(&key)
                .map_or(true, |e| e.read_ref == 0 && e.write_ref == 0)
        });

        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        if !satisfied && (entry.read_ref > 0 || entry.write_ref > 0) {
            return Err(DispatchMapError::Timeout(key));
        }

        entry.write_ref = 1;
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("dispatch-map: take_write key {key}"));
        }
        Ok(())
    }

    fn release_read(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        if entry.read_ref == 0 {
            return Err(DispatchMapError::RefCountUnderflow(key));
        }

        entry.read_ref -= 1;
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("dispatch-map: release_read key {key}"));
        }
        drop(inner);
        self.state.condvar.notify_all();
        Ok(())
    }

    fn release_write(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        if entry.write_ref == 0 {
            return Err(DispatchMapError::RefCountUnderflow(key));
        }

        entry.write_ref = 0;
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("dispatch-map: release_write key {key}"));
        }
        drop(inner);
        self.state.condvar.notify_all();
        Ok(())
    }

    fn downgrade_reference(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        if entry.write_ref == 0 {
            return Err(DispatchMapError::NoWriteReference(key));
        }

        entry.write_ref = 0;
        entry.read_ref = entry
            .read_ref
            .checked_add(1)
            .ok_or(DispatchMapError::RefCountOverflow(key))?;
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("dispatch-map: downgrade_reference key {key}"));
        }
        drop(inner);
        self.state.condvar.notify_all();
        Ok(())
    }

    fn remove(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        if entry.read_ref > 0 || entry.write_ref > 0 {
            return Err(DispatchMapError::ActiveReferences(key));
        }

        let handle = entry.eviction_handle;
        inner.entries.remove(&key);

        if let Ok(ep) = self.eviction_policy.get() {
            let _ = ep.remove(handle);
        }

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("dispatch-map: removed key {key}"));
        }

        Ok(())
    }

    fn touch(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        let handle = entry.eviction_handle;
        drop(inner);
        if let Ok(ep) = self.eviction_policy.get() {
            let _ = ep.touch(handle);
        }
        Ok(())
    }

    fn entry_size(&self, key: CacheKey) -> Result<u32, DispatchMapError> {
        let inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;
        Ok(entry.size_blocks * 4096)
    }

    fn oldest_keys(&self, n: usize) -> Vec<CacheKey> {
        let pool_id = self.get_pool_id();
        if let Ok(ep) = self.eviction_policy.get() {
            ep.peek_oldest(pool_id, n)
        } else {
            Vec::new()
        }
    }

    fn create_memory_tier_entry(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        if size == 0 {
            return Err(DispatchMapError::InvalidSize);
        }

        let pool_id = self.get_pool_id();
        let ep = self.eviction_policy.get().unwrap();

        let mut inner = self.state.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }

        let eviction_handle = ep.track(pool_id, key).unwrap();
        let entry = DispatchEntry {
            location: Location::MemoryTier {
                pointer,
                size,
                ssd_offset: None,
            },
            size_blocks: size.div_ceil(4096),
            read_ref: 0,
            write_ref: 1,
            eviction_handle,
        };

        inner.entries.insert(key, entry);

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!(
                "dispatch-map: created memory-tier entry for key {key}, size {size}"
            ));
        }

        Ok(())
    }

    fn convert_memory_tier_to_block(&self, key: CacheKey) -> Result<(), DispatchMapError> {
        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        match &entry.location {
            Location::MemoryTier {
                ssd_offset: Some(offset),
                ..
            } => {
                let offset = *offset;
                entry.location = Location::BlockDevice { offset };
            }
            Location::MemoryTier {
                ssd_offset: None, ..
            } => {
                return Err(DispatchMapError::InvalidState(
                    "memory-tier entry has no SSD offset (write-through not complete)".into(),
                ));
            }
            _ => {
                return Err(DispatchMapError::InvalidState(
                    "entry is not in memory-tier state".into(),
                ));
            }
        }

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!(
                "dispatch-map: converted memory-tier key {key} to block device"
            ));
        }

        drop(inner);
        self.state.condvar.notify_all();

        Ok(())
    }

    fn promote_block_to_memory_tier(
        &self,
        key: CacheKey,
        pointer: *mut u8,
        size: u32,
    ) -> Result<(), DispatchMapError> {
        if size == 0 {
            return Err(DispatchMapError::InvalidSize);
        }

        let mut inner = self.state.inner.lock().unwrap();
        let entry = inner
            .entries
            .get_mut(&key)
            .ok_or(DispatchMapError::KeyNotFound(key))?;

        match &entry.location {
            Location::BlockDevice { offset } => {
                // In-place flip: keep the eviction handle and all refs (the
                // entry may be pinned by an in-flight load). Retain the SSD
                // offset so the promoted entry stays demotable without a reread.
                let offset = *offset;
                entry.location = Location::MemoryTier {
                    pointer,
                    size,
                    ssd_offset: Some(offset),
                };
                entry.size_blocks = size.div_ceil(4096);
            }
            Location::MemoryTier { .. } => {
                return Err(DispatchMapError::InvalidState(
                    "entry is already in memory-tier state".into(),
                ));
            }
        }

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!(
                "dispatch-map: promoted block-device key {key} to memory-tier in place, size {size}"
            ));
        }

        drop(inner);
        self.state.condvar.notify_all();

        Ok(())
    }

    fn is_evictable(&self, key: CacheKey) -> bool {
        let inner = self.state.inner.lock().unwrap();
        match inner.entries.get(&key) {
            Some(entry) => {
                entry.read_ref == 0
                    && entry.write_ref == 0
                    && matches!(
                        entry.location,
                        Location::MemoryTier { ssd_offset: Some(_), .. }
                    )
            }
            None => false,
        }
    }

    fn recover_extent(
        &self,
        key: CacheKey,
        offset: u64,
        size_blocks: u32,
    ) -> Result<(), DispatchMapError> {
        let pool_id = self.get_pool_id();
        let ep = self.eviction_policy.get().unwrap();

        let mut inner = self.state.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }
        let eviction_handle = ep.track(pool_id, key).unwrap();
        let entry = DispatchEntry {
            location: Location::BlockDevice { offset },
            size_blocks,
            read_ref: 0,
            write_ref: 0,
            eviction_handle,
        };
        inner.entries.insert(key, entry);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use component_core::query_interface;

    fn setup_component() -> Arc<DispatchMapComponent> {
        let ep_comp = eviction_policy_lru::EvictionPolicyLruComponent::new_default();
        let ep: Arc<dyn IEvictionPolicy + Send + Sync> =
            query_interface!(ep_comp, IEvictionPolicy).unwrap();

        let c = DispatchMapComponent::new(DispatchMapState::new());
        c.eviction_policy.connect(ep).unwrap();
        c
    }

    /// Helper: create a memory-tier entry (replaces the old create_staging test helper).
    fn create_entry(dm: &Arc<dyn IDispatchMap + Send + Sync>, key: CacheKey) {
        let ptr = Box::into_raw(vec![0u8; 4096].into_boxed_slice()) as *mut u8;
        dm.create_memory_tier_entry(key, ptr, 4096).unwrap();
    }

    // --- US4: Reference counting ---

    #[test]
    fn take_read_increments_ref() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.release_write(1).unwrap();
        dm.take_read(1).unwrap();
        dm.release_read(1).unwrap();
    }

    #[test]
    fn take_write_blocks_on_readers() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.release_write(1).unwrap();
        dm.take_read(1).unwrap();
        let err = dm.take_write(1);
        assert!(matches!(err, Err(DispatchMapError::Timeout(1))));
        dm.release_read(1).unwrap();
    }

    #[test]
    fn release_read_underflow() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.release_write(1).unwrap();
        let err = dm.release_read(1);
        assert!(matches!(err, Err(DispatchMapError::RefCountUnderflow(1))));
    }

    #[test]
    fn release_write_underflow() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.release_write(1).unwrap();
        let err = dm.release_write(1);
        assert!(matches!(err, Err(DispatchMapError::RefCountUnderflow(1))));
    }

    #[test]
    fn downgrade_reference_happy_path() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.downgrade_reference(1).unwrap();
        dm.release_read(1).unwrap();
    }

    #[test]
    fn downgrade_without_write_ref() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.release_write(1).unwrap();
        let err = dm.downgrade_reference(1);
        assert!(matches!(err, Err(DispatchMapError::NoWriteReference(1))));
    }

    // --- US2: Lookup ---

    #[test]
    fn lookup_not_exist() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let result = dm.lookup(99).unwrap();
        assert!(matches!(result, LookupResult::NotExist));
    }

    #[test]
    fn lookup_memory_tier() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let mut buf = [0u8; 8192];
        dm.create_memory_tier_entry(1, buf.as_mut_ptr(), 8192)
            .unwrap();
        dm.release_write(1).unwrap();
        let result = dm.lookup(1).unwrap();
        match result {
            LookupResult::MemoryTier { pointer, size } => {
                assert_eq!(pointer, buf.as_mut_ptr());
                assert_eq!(size, 8192);
            }
            _ => panic!("expected MemoryTier"),
        }
        dm.release_read(1).unwrap();
    }

    #[test]
    fn lookup_block_device() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.convert_to_storage(1, 8192).unwrap();
        dm.release_write(1).unwrap();
        dm.take_write(1).unwrap();
        dm.convert_memory_tier_to_block(1).unwrap();
        dm.release_write(1).unwrap();
        let result = dm.lookup(1).unwrap();
        match result {
            LookupResult::BlockDevice { offset } => {
                assert_eq!(offset, 8192);
            }
            _ => panic!("expected BlockDevice"),
        }
        dm.release_read(1).unwrap();
    }

    // --- US3: Convert to storage ---

    #[test]
    fn convert_to_storage_happy_path() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.convert_to_storage(1, 4096).unwrap();
    }

    #[test]
    fn convert_to_storage_key_not_found() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let err = dm.convert_to_storage(99, 0);
        assert!(matches!(err, Err(DispatchMapError::KeyNotFound(99))));
    }

    #[test]
    fn convert_to_storage_already_block_device() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.convert_to_storage(1, 4096).unwrap();
        dm.release_write(1).unwrap();
        dm.take_write(1).unwrap();
        dm.convert_memory_tier_to_block(1).unwrap();
        let err = dm.convert_to_storage(1, 8192);
        assert!(matches!(err, Err(DispatchMapError::InvalidState(_))));
    }

    // --- US6: Remove ---

    #[test]
    fn remove_happy_path() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.release_write(1).unwrap();
        dm.remove(1).unwrap();
        let result = dm.lookup(1).unwrap();
        assert!(matches!(result, LookupResult::NotExist));
    }

    #[test]
    fn remove_active_references() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        let err = dm.remove(1);
        assert!(matches!(err, Err(DispatchMapError::ActiveReferences(1))));
    }

    #[test]
    fn remove_key_not_found() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let err = dm.remove(99);
        assert!(matches!(err, Err(DispatchMapError::KeyNotFound(99))));
    }

    // --- oldest_keys ---

    #[test]
    fn oldest_keys_empty_map() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        assert!(dm.oldest_keys(5).is_empty());
    }

    #[test]
    fn oldest_keys_fewer_than_n() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 10);
        create_entry(&dm, 11);
        let keys = dm.oldest_keys(5);
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&10));
        assert!(keys.contains(&11));
    }

    #[test]
    fn oldest_keys_respects_creation_order() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        create_entry(&dm, 2);
        create_entry(&dm, 3);
        let keys = dm.oldest_keys(2);
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], 1);
        assert_eq!(keys[1], 2);
    }

    #[test]
    fn oldest_keys_lookup_updates_timestamp() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        create_entry(&dm, 1);
        dm.release_write(1).unwrap();
        create_entry(&dm, 2);
        dm.release_write(2).unwrap();
        create_entry(&dm, 3);
        dm.release_write(3).unwrap();

        let _ = dm.lookup(1).unwrap();
        dm.release_read(1).unwrap();

        let keys = dm.oldest_keys(2);
        assert_eq!(keys.len(), 2);
        assert!(keys.contains(&2));
        assert!(keys.contains(&3));
        assert!(!keys.contains(&1));
    }

    // --- Memory-tier entry methods ---

    #[test]
    fn create_memory_tier_entry_happy_path() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let mut buf = [0u8; 4096];
        dm.create_memory_tier_entry(1, buf.as_mut_ptr(), 4096)
            .unwrap();
        // Should be visible via lookup (blocks until write ref released).
        dm.release_write(1).unwrap();
        let result = dm.lookup(1).unwrap();
        match result {
            LookupResult::MemoryTier { pointer, size } => {
                assert_eq!(pointer, buf.as_mut_ptr());
                assert_eq!(size, 4096);
            }
            _ => panic!("expected MemoryTier"),
        }
        dm.release_read(1).unwrap();
    }

    #[test]
    fn create_memory_tier_entry_duplicate() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let mut buf = [0u8; 4096];
        dm.create_memory_tier_entry(1, buf.as_mut_ptr(), 4096)
            .unwrap();
        let err = dm.create_memory_tier_entry(1, buf.as_mut_ptr(), 4096);
        assert!(matches!(err, Err(DispatchMapError::AlreadyExists(1))));
    }

    #[test]
    fn convert_to_storage_on_memory_tier_sets_ssd_offset() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let mut buf = [0u8; 4096];
        dm.create_memory_tier_entry(1, buf.as_mut_ptr(), 4096)
            .unwrap();
        // convert_to_storage sets ssd_offset but keeps it as MemoryTier.
        dm.convert_to_storage(1, 8192).unwrap();
        // Still shows as MemoryTier on lookup (not BlockDevice).
        dm.release_write(1).unwrap();
        let result = dm.lookup(1).unwrap();
        assert!(matches!(result, LookupResult::MemoryTier { .. }));
        dm.release_read(1).unwrap();
    }

    #[test]
    fn convert_memory_tier_to_block_happy_path() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let mut buf = [0u8; 4096];
        dm.create_memory_tier_entry(1, buf.as_mut_ptr(), 4096)
            .unwrap();
        dm.convert_to_storage(1, 8192).unwrap();
        dm.release_write(1).unwrap();
        dm.take_write(1).unwrap();
        dm.convert_memory_tier_to_block(1).unwrap();
        dm.release_write(1).unwrap();
        let result = dm.lookup(1).unwrap();
        match result {
            LookupResult::BlockDevice { offset } => assert_eq!(offset, 8192),
            _ => panic!("expected BlockDevice after convert_memory_tier_to_block"),
        }
        dm.release_read(1).unwrap();
    }

    #[test]
    fn convert_memory_tier_to_block_without_ssd_offset_fails() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let mut buf = [0u8; 4096];
        dm.create_memory_tier_entry(1, buf.as_mut_ptr(), 4096)
            .unwrap();
        dm.release_write(1).unwrap();
        dm.take_write(1).unwrap();
        let err = dm.convert_memory_tier_to_block(1);
        assert!(matches!(err, Err(DispatchMapError::InvalidState(_))));
    }
}
