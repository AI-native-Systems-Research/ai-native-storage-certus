//! Extended metadata store component for the Certus storage system.
//!
//! Provides key-value metadata storage with logging support.
//! Built with the component framework using `define_component!`.
//!
//! # Quick start
//!
//! ```
//! use extended_metadata_store::ExtendedMetadataStoreComponent;
//! use interfaces::IExtendedMetadataStore;
//! use component_core::query_interface;
//!
//! let component = ExtendedMetadataStoreComponent::new_default();
//! let store = query_interface!(component, IExtendedMetadataStore).unwrap();
//! store.put("key1", b"value1").unwrap();
//! let val = store.get("key1").unwrap();
//! assert_eq!(val, b"value1");
//! ```

use component_framework::define_component;
use interfaces::{ExtendedMetadataStoreError, IExtendedMetadataStore, ILogger};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

pub mod on_disk;

#[cfg(feature = "testing")]
pub mod block_io;

#[cfg(feature = "testing")]
pub mod flush;

#[cfg(feature = "testing")]
pub mod recovery;

#[cfg(feature = "testing")]
pub mod test_support;

define_component! {
    pub ExtendedMetadataStoreComponent {
        version: "0.1.0",
        provides: [IExtendedMetadataStore],
        receptacles: {
            logger: ILogger,
        },
        fields: {
            store: RwLock<HashMap<String, Vec<u8>>>,
            dirty_count: AtomicU64,
            flush_seq: AtomicU64,
            // Optional durable-flush trigger installed by the wiring layer
            // (see `attach_flush_trigger`). When present, `force_flush()`
            // invokes it to make data durable; when absent (pure in-memory
            // mode) `force_flush()` is a no-op. Type-erased (see `FlushTrigger`)
            // so the field type does not depend on the `testing`/`spdk`-gated
            // FlushManager.
            flush_trigger: RwLock<Option<FlushTrigger>>,
        },
    }
}

/// Maximum value size: 128 KiB.
pub const MAX_VALUE_SIZE: usize = 128 * 1024;

/// Type-erased durable-flush trigger installed by the wiring layer via
/// [`ExtendedMetadataStoreComponent::attach_flush_trigger`]. Invoking it makes
/// all pending writes durable and blocks until complete.
pub type FlushTrigger = Box<dyn Fn() -> Result<(), String> + Send + Sync>;

impl ExtendedMetadataStoreComponent {
    /// Get the current dirty count (mutations since last flush).
    pub fn dirty_count(&self) -> u64 {
        self.dirty_count.load(Ordering::Relaxed)
    }

    /// Get the current flush sequence number.
    pub fn flush_seq(&self) -> u64 {
        self.flush_seq.load(Ordering::Relaxed)
    }

    /// Load entries into the store (used during recovery).
    pub fn load_entries(&self, entries: Vec<(String, Vec<u8>)>) {
        let mut map = self.store.write().unwrap();
        map.clear();
        for (k, v) in entries {
            map.insert(k, v);
        }
        self.dirty_count.store(0, Ordering::Relaxed);
    }

    /// Snapshot current entries (for flushing). Returns a clone under read lock.
    pub fn snapshot_entries(&self) -> Vec<(String, Vec<u8>)> {
        let map = self.store.read().unwrap();
        map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Reset dirty count after a successful flush.
    pub fn mark_flushed(&self, seq: u64) {
        self.flush_seq.store(seq, Ordering::Relaxed);
        self.dirty_count.store(0, Ordering::Relaxed);
    }

    /// Install a durable-flush trigger used by [`IExtendedMetadataStore::force_flush`].
    ///
    /// The wiring layer (which owns the `FlushManager`/`BlockDeviceClient`)
    /// passes a closure that makes all pending writes durable and blocks until
    /// complete — typically `move || flush_manager.trigger_flush()`. Once
    /// installed, callers holding only the `IExtendedMetadataStore` interface
    /// get real durability from `force_flush()`. Replaces any previously
    /// installed trigger.
    pub fn attach_flush_trigger(&self, trigger: FlushTrigger) {
        *self.flush_trigger.write().unwrap() = Some(trigger);
    }

    /// Initialize the store from a BlockDeviceClient.
    ///
    /// Reads the partition, recovers existing data or formats fresh.
    /// Returns the recovered Superblock and any warnings.
    #[cfg(feature = "testing")]
    pub fn initialize_from_client(
        &self,
        client: &block_io::BlockDeviceClient,
        total_sectors: u64,
    ) -> Result<(on_disk::Superblock, Vec<String>), String> {
        use recovery::{format_partition, recover_from_disk};

        let result = recover_from_disk(client)?;
        let mut warnings = result.warnings;

        let superblock = if result.superblock.partition_sectors == 0 {
            // Fresh format needed
            let sb = format_partition(client, total_sectors)?;
            warnings.push("partition formatted fresh".into());
            sb
        } else {
            result.superblock
        };

        // Load recovered entries
        if !result.entries.is_empty() {
            self.load_entries(result.entries);
        }
        self.flush_seq
            .store(superblock.flush_seq, Ordering::Relaxed);

        // Log warnings via ILogger
        if let Ok(logger) = self.logger.get() {
            for w in &warnings {
                logger.debug(&format!("extended-metadata-store: recovery: {w}"));
            }
        }

        Ok((superblock, warnings))
    }
}

impl IExtendedMetadataStore for ExtendedMetadataStoreComponent {
    fn put(&self, key: &str, value: &[u8]) -> Result<(), ExtendedMetadataStoreError> {
        if value.len() > MAX_VALUE_SIZE {
            return Err(ExtendedMetadataStoreError::ValueTooLarge);
        }
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("extended-metadata-store: put key={key}"));
        }
        let mut map = self.store.write().unwrap();
        map.insert(key.to_string(), value.to_vec());
        drop(map);
        self.dirty_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Vec<u8>, ExtendedMetadataStoreError> {
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("extended-metadata-store: get key={key}"));
        }
        let map = self.store.read().unwrap();
        map.get(key)
            .cloned()
            .ok_or(ExtendedMetadataStoreError::NotFound)
    }

    fn delete(&self, key: &str) -> Result<(), ExtendedMetadataStoreError> {
        if let Ok(logger) = self.logger.get() {
            logger.debug(&format!("extended-metadata-store: delete key={key}"));
        }
        let mut map = self.store.write().unwrap();
        map.remove(key);
        drop(map);
        self.dirty_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn iterate_all(&self) -> Result<Vec<(String, Vec<u8>)>, ExtendedMetadataStoreError> {
        if let Ok(logger) = self.logger.get() {
            logger.debug("extended-metadata-store: iterate_all");
        }
        let map = self.store.read().unwrap();
        Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }

    fn force_flush(&self) -> Result<(), ExtendedMetadataStoreError> {
        if let Ok(logger) = self.logger.get() {
            logger.debug("extended-metadata-store: force_flush");
        }
        // If the wiring layer installed a durable-flush trigger (via
        // `attach_flush_trigger`), invoke it and block until it completes so
        // the interface's "returns when all data is durable" contract holds.
        // In pure in-memory mode (no trigger installed) there is no durable
        // backing store, so this is a no-op.
        let guard = self.flush_trigger.read().unwrap();
        match guard.as_ref() {
            Some(trigger) => trigger().map_err(ExtendedMetadataStoreError::StorageError),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    fn setup() -> std::sync::Arc<ExtendedMetadataStoreComponent> {
        ExtendedMetadataStoreComponent::new_default()
    }

    #[test]
    fn put_and_get() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        store.put("key1", b"value1").unwrap();
        assert_eq!(store.get("key1").unwrap(), b"value1");
    }

    #[test]
    fn get_not_found() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        assert_eq!(
            store.get("nonexistent"),
            Err(ExtendedMetadataStoreError::NotFound)
        );
    }

    #[test]
    fn delete_existing_key() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        store.put("k", b"v").unwrap();
        assert!(store.delete("k").is_ok());
        assert_eq!(store.get("k"), Err(ExtendedMetadataStoreError::NotFound));
    }

    #[test]
    fn delete_nonexistent_is_ok() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        assert!(store.delete("missing").is_ok());
    }

    #[test]
    fn put_overwrites_existing() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        store.put("k", b"v1").unwrap();
        store.put("k", b"v2").unwrap();
        assert_eq!(store.get("k").unwrap(), b"v2");
    }

    #[test]
    fn iterate_all_returns_all_entries() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        store.put("a", b"1").unwrap();
        store.put("b", b"2").unwrap();
        store.put("c", b"3").unwrap();

        let mut entries = store.iterate_all().unwrap();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ("a".to_string(), b"1".to_vec()));
        assert_eq!(entries[1], ("b".to_string(), b"2".to_vec()));
        assert_eq!(entries[2], ("c".to_string(), b"3".to_vec()));
    }

    #[test]
    fn force_flush_succeeds() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        store.put("k", b"v").unwrap();
        assert!(store.force_flush().is_ok());
    }

    #[test]
    fn put_value_too_large() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        let big_value = vec![0u8; MAX_VALUE_SIZE + 1];
        assert_eq!(
            store.put("k", &big_value),
            Err(ExtendedMetadataStoreError::ValueTooLarge)
        );
    }

    #[test]
    fn dirty_count_increments() {
        let comp = setup();
        let store: std::sync::Arc<dyn IExtendedMetadataStore + Send + Sync> =
            query_interface!(comp, IExtendedMetadataStore).unwrap();

        assert_eq!(comp.dirty_count(), 0);
        store.put("a", b"1").unwrap();
        assert_eq!(comp.dirty_count(), 1);
        store.put("b", b"2").unwrap();
        assert_eq!(comp.dirty_count(), 2);
        store.delete("a").unwrap();
        assert_eq!(comp.dirty_count(), 3);
    }
}
