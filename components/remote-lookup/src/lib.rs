//! Remote lookup component.
//!
//! Placeholder for performing remote cache lookups to other Certus nodes
//! on the network. Currently logs each request and returns `NotFound`.

use component_framework::define_component;
use interfaces::{CacheKey, ILogger, IRemoteLookup, IpcHandle, RemoteLookupError};

define_component! {
    pub RemoteLookupComponent {
        version: "0.1.0",
        provides: [IRemoteLookup],
        receptacles: {
            logger: ILogger,
        },
    }
}

impl IRemoteLookup for RemoteLookupComponent {
    /// Placeholder batch lookup — logs each entry and returns `NotFound`.
    ///
    /// # Examples
    ///
    /// ```
    /// use component_core::query_interface;
    /// use interfaces::{CacheKey, IpcHandle, IRemoteLookup, RemoteLookupError};
    /// use remote_lookup::RemoteLookupComponent;
    ///
    /// let comp = RemoteLookupComponent::new();
    /// let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
    ///     query_interface!(comp, IRemoteLookup).unwrap();
    ///
    /// let mut buf = vec![0u8; 4096];
    /// let entries: Vec<(CacheKey, IpcHandle)> = vec![
    ///     (1, IpcHandle { address: buf.as_mut_ptr(), size: 4096 }),
    ///     (2, IpcHandle { address: buf.as_mut_ptr(), size: 4096 }),
    /// ];
    /// let results = rl.batch_lookup(&entries);
    /// assert_eq!(results.len(), 2);
    /// assert_eq!(results[0], Err(RemoteLookupError::NotFound));
    /// ```
    fn batch_lookup(
        &self,
        entries: &[(CacheKey, IpcHandle)],
    ) -> Vec<Result<(), RemoteLookupError>> {
        entries
            .iter()
            .map(|(key, handle)| {
                if let Ok(logger) = self.logger.get() {
                    logger.info(&format!(
                        "remote-lookup: batch_lookup placeholder - key={key}, size={}",
                        handle.size
                    ));
                }
                Err(RemoteLookupError::NotFound)
            })
            .collect()
    }

    /// Placeholder for joining a cluster; we may need more parameters
    fn join_cluster(&self, endpoint: &str) -> Result<(), RemoteLookupError> {
        if let Ok(logger) = self.logger.get() {
            logger.info(&format!(
                "remote-lookup: join_cluster placeholder - endpoint={endpoint}"
            ));
        }
        Ok(())
    }

    /// Placeholder for leaving a cluster
    fn leave_cluster(&self) -> Result<(), RemoteLookupError> {
        if let Ok(logger) = self.logger.get() {
            logger.info("remote-lookup: leave_cluster placeholder");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;

    fn setup() -> std::sync::Arc<RemoteLookupComponent> {
        RemoteLookupComponent::new()
    }

    fn make_entries(count: usize) -> (Vec<u8>, Vec<(CacheKey, IpcHandle)>) {
        let mut buf = vec![0u8; 4096];
        let ptr = buf.as_mut_ptr();
        let entries: Vec<(CacheKey, IpcHandle)> = (0..count)
            .map(|i| {
                (
                    i as CacheKey,
                    IpcHandle {
                        address: ptr,
                        size: 4096,
                    },
                )
            })
            .collect();
        (buf, entries)
    }

    #[test]
    fn batch_lookup_returns_not_found_for_each_entry() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        let (_buf, entries) = make_entries(5);
        let results = rl.batch_lookup(&entries);

        assert_eq!(results.len(), 5);
        for r in &results {
            assert_eq!(*r, Err(RemoteLookupError::NotFound));
        }
    }

    #[test]
    fn batch_lookup_returns_empty_vec_for_empty_input() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        let results = rl.batch_lookup(&[]);
        assert!(results.is_empty());
    }

    #[test]
    fn batch_lookup_preserves_positional_order() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        let (_buf, entries) = make_entries(10);
        let results = rl.batch_lookup(&entries);

        assert_eq!(results.len(), entries.len());
    }

    #[test]
    fn join_cluster_succeeds() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        assert!(rl.join_cluster("192.168.1.10:9090").is_ok());
    }

    #[test]
    fn leave_cluster_succeeds() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        assert!(rl.leave_cluster().is_ok());
    }

    #[test]
    fn batch_lookup_accepts_cache_key_ipc_handle_slice() {
        let comp = setup();
        let rl: std::sync::Arc<dyn IRemoteLookup + Send + Sync> =
            query_interface!(comp, IRemoteLookup).unwrap();

        let mut buf = vec![0u8; 4096];
        let entries: &[(CacheKey, IpcHandle)] = &[(
            42,
            IpcHandle {
                address: buf.as_mut_ptr(),
                size: 4096,
            },
        )];
        let results = rl.batch_lookup(entries);
        assert_eq!(results.len(), 1);
    }
}
