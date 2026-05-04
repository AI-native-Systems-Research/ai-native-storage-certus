//! DispatchMap component for the Certus storage system.
//!
//! Maps extent keys ([`CacheKey`]) to their current location — either an
//! in-memory DMA staging buffer or a block-device offset — with
//! readers-writer reference counting for concurrent access.
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

use std::sync::Arc;
use std::time::Duration;

use component_framework::define_component;

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(100);
use interfaces::{
    CacheKey, DispatchMapError, DmaAllocFn, DmaBuffer, IDispatchMap, IExtentManager, ILogger,
    LookupResult,
};

use crate::entry::{DispatchEntry, Location};

define_component! {
    pub DispatchMapComponentV0 {
        version: "0.1.0",
        provides: [IDispatchMap],
        receptacles: {
            logger: ILogger,
            extent_manager: IExtentManager,
        },
        fields: {
            state: DispatchMapState,
        },
    }
}

impl IDispatchMap for DispatchMapComponentV0 {
    fn set_dma_alloc(&self, alloc: DmaAllocFn) {
        let mut dma = self.state.dma_alloc.lock().unwrap();
        *dma = Some(alloc);
        if let Ok(logger) = self.logger.get() {
            logger.info("dispatch-map: DMA allocator set");
        }
    }

    /// Recover dispatch map state from the extent manager's persisted extents.
    /// Each extent is re-inserted as a BlockDevice location with zero reference
    /// counts, restoring the map to a consistent view of committed storage.
    fn initialize(&self) -> Result<(), DispatchMapError> {
        if let Ok(logger) = self.logger.get() {
            logger.info("dispatch-map: beginning state recovery from extent manager");
        }

        let em = self
            .extent_manager
            .get()
            .map_err(|_| DispatchMapError::NotInitialized("extent_manager not bound".into()))?;

        let mut inner = self.state.inner.lock().unwrap();
        let mut count: u64 = 0;

        // Walk all persisted extents and rebuild the in-memory dispatch entries.
        // Staging buffers are not recovered — only committed block-device locations.
        em.for_each_extent(&mut |extent| {
            let entry = DispatchEntry {
                location: Location::BlockDevice {
                    offset: extent.offset,
                },
                size_blocks: extent.size,
                read_ref: 0,
                write_ref: 0,
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

    fn create_staging(&self, key: CacheKey, size: u32) -> Result<Arc<DmaBuffer>, DispatchMapError> {
        if size == 0 {
            return Err(DispatchMapError::InvalidSize);
        }

        let alloc_fn = {
            let dma = self.state.dma_alloc.lock().unwrap();
            dma.clone()
                .ok_or_else(|| DispatchMapError::NotInitialized("DMA allocator not set".into()))?
        };

        let mut inner = self.state.inner.lock().unwrap();
        if inner.entries.contains_key(&key) {
            return Err(DispatchMapError::AlreadyExists(key));
        }

        let byte_size = size as usize * 4096;
        let buf = alloc_fn(byte_size, 4096, None).map_err(DispatchMapError::AllocationFailed)?;
        let buffer = Arc::new(buf);

        let entry = DispatchEntry {
            location: Location::Staging {
                buffer: Arc::clone(&buffer),
            },
            size_blocks: size,
            read_ref: 0,
            write_ref: 1,
        };

        inner.entries.insert(key, entry);

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!(
                "dispatch-map: created staging for key {key}, size {size} blocks"
            ));
        }

        Ok(buffer)
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

        entry.read_ref += 1;

        let result = match &entry.location {
            Location::Staging { buffer } => LookupResult::Staging {
                buffer: Arc::clone(buffer),
            },
            Location::BlockDevice { offset } => LookupResult::BlockDevice { offset: *offset },
        };

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

        if !matches!(entry.location, Location::Staging { .. }) {
            return Err(DispatchMapError::InvalidState(
                "entry is not in staging state".into(),
            ));
        }

        entry.location = Location::BlockDevice { offset };

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!(
                "dispatch-map: converted key {key} to storage at offset {offset}"
            ));
        }

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

        entry.read_ref += 1;
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
        entry.read_ref += 1;
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

        inner.entries.remove(&key);

        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("dispatch-map: removed key {key}"));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    fn mock_dma_alloc() -> DmaAllocFn {
        Arc::new(|size, _align, _numa| {
            let layout = std::alloc::Layout::from_size_align(size, 4096).unwrap();
            // SAFETY: Test-only allocation with valid layout.
            let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
            if ptr.is_null() {
                return Err("allocation failed".into());
            }
            // SAFETY: ptr is valid heap memory from alloc_zeroed with matching layout.
            unsafe {
                DmaBuffer::from_raw(
                    ptr as *mut std::ffi::c_void,
                    size,
                    mock_free as unsafe extern "C" fn(*mut std::ffi::c_void),
                    -1,
                )
            }
            .map_err(|e| e.to_string())
        })
    }

    unsafe extern "C" fn mock_free(ptr: *mut std::ffi::c_void) {
        if !ptr.is_null() {
            // SAFETY: ptr was allocated with alloc_zeroed in mock_dma_alloc with 4096 alignment.
            // We use a size of 1 here because global alloc doesn't track size; this is
            // test-only code and the OS reclaims the full allocation.
            unsafe {
                std::alloc::dealloc(
                    ptr as *mut u8,
                    std::alloc::Layout::from_size_align_unchecked(1, 1),
                );
            }
        }
    }

    fn setup_component() -> Arc<DispatchMapComponentV0> {
        let c = DispatchMapComponentV0::new(DispatchMapState::new());
        let dm = query_interface!(c, IDispatchMap).unwrap();
        dm.set_dma_alloc(mock_dma_alloc());
        c
    }

    // --- US4: Reference counting ---

    #[test]
    fn take_read_increments_ref() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.release_write(1).unwrap();
        dm.take_read(1).unwrap();
        // read_ref should be 1
        dm.release_read(1).unwrap();
    }

    #[test]
    fn take_write_blocks_on_readers() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.release_write(1).unwrap();
        dm.take_read(1).unwrap();
        // write should timeout because read_ref > 0
        let err = dm.take_write(1);
        assert!(matches!(err, Err(DispatchMapError::Timeout(1))));
        dm.release_read(1).unwrap();
    }

    #[test]
    fn release_read_underflow() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.release_write(1).unwrap();
        let err = dm.release_read(1);
        assert!(matches!(err, Err(DispatchMapError::RefCountUnderflow(1))));
    }

    #[test]
    fn release_write_underflow() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.release_write(1).unwrap();
        let err = dm.release_write(1);
        assert!(matches!(err, Err(DispatchMapError::RefCountUnderflow(1))));
    }

    #[test]
    fn downgrade_reference_happy_path() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.downgrade_reference(1).unwrap();
        // now read_ref=1, write_ref=0; release the read
        dm.release_read(1).unwrap();
    }

    #[test]
    fn downgrade_without_write_ref() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.release_write(1).unwrap();
        let err = dm.downgrade_reference(1);
        assert!(matches!(err, Err(DispatchMapError::NoWriteReference(1))));
    }

    // --- US1: Staging ---

    #[test]
    fn create_staging_happy_path() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let buf = dm.create_staging(42, 4).unwrap();
        assert_eq!(buf.len(), 4 * 4096);
    }

    #[test]
    fn create_staging_size_zero() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let err = dm.create_staging(1, 0);
        assert!(matches!(err, Err(DispatchMapError::InvalidSize)));
    }

    #[test]
    fn create_staging_duplicate_key() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        let err = dm.create_staging(1, 1);
        assert!(matches!(err, Err(DispatchMapError::AlreadyExists(1))));
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
    fn lookup_staging() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let buf = dm.create_staging(1, 2).unwrap();
        dm.release_write(1).unwrap();
        let result = dm.lookup(1).unwrap();
        match result {
            LookupResult::Staging { buffer } => {
                assert_eq!(buffer.as_ptr(), buf.as_ptr());
                assert_eq!(buffer.len(), 2 * 4096);
            }
            _ => panic!("expected Staging"),
        }
        dm.release_read(1).unwrap();
    }

    #[test]
    fn lookup_block_device() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.release_write(1).unwrap();
        dm.take_write(1).unwrap();
        dm.convert_to_storage(1, 8192).unwrap();
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
        let _ = dm.create_staging(1, 1).unwrap();
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
    fn convert_to_storage_already_converted() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.convert_to_storage(1, 4096).unwrap();
        let err = dm.convert_to_storage(1, 8192);
        assert!(matches!(err, Err(DispatchMapError::InvalidState(_))));
    }

    // --- US6: Remove ---

    #[test]
    fn remove_happy_path() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
        dm.release_write(1).unwrap();
        dm.remove(1).unwrap();
        let result = dm.lookup(1).unwrap();
        assert!(matches!(result, LookupResult::NotExist));
    }

    #[test]
    fn remove_active_references() {
        let c = setup_component();
        let dm = query_interface!(c, IDispatchMap).unwrap();
        let _ = dm.create_staging(1, 1).unwrap();
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
}
