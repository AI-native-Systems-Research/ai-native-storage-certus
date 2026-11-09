//! Remote request handler component for the Certus storage system.
//!
//! An **outbound RDMA initiator**: given a remote host endpoint and a batch of
//! `(key, remote-region)` pairs, [`RemoteLookupRdmaInitiatorComponent`] connects to
//! the host (reusing an established connection), looks each key up in the local
//! memory tier, and — when the key is present and its size matches the region —
//! RDMA-writes the value directly into the remote host's memory.
//!
//! It is driven from the "server" side by the `remote-lookup` component: the
//! control-plane request (carrying the keys and the requester's remote memory
//! descriptors) arrives over zyre, and `remote-lookup` invokes
//! [`IRemoteLookupRdmaInitiator::push`] to satisfy it.
//!
//! # Architecture
//!
//! - **connection**: connection table + per-host state machine + the testable
//!   RDMA transport seam (real `rdma_cm` transport and, in tests, a mock).
//! - **rdma** / **ffi**: safe wrappers and FFI declarations over rdma-core.
//! - **telemetry**: optional metrics (feature-gated).
//!
//! # Examples
//!
//! ```no_run
//! use remote_lookup_rdma_initiator::RemoteLookupRdmaInitiatorComponent;
//! use interfaces::{IRemoteLookupRdmaInitiator, RemoteRegion};
//!
//! let handler = RemoteLookupRdmaInitiatorComponent::new_default();
//! // After binding the `logger` and `memory_tier` receptacles and initializing
//! // the memory-tier pool, push a value into a remote node's memory:
//! let items = [(42u64, RemoteRegion { addr: 0x1000, rkey: 7, length: 256 })];
//! let _ = handler.push("192.168.1.20:18515", &items);
//! ```

pub mod connection;
#[cfg(feature = "rdma")]
pub mod ffi;
#[cfg(feature = "rdma")]
pub mod rdma;
pub mod telemetry;

#[cfg(all(test, feature = "rdma"))]
mod loopback_test;

#[cfg(feature = "rdma")]
use std::sync::Arc;

use component_framework::define_component;
use interfaces::{
    CacheKey, ILogger, IMemoryTier, IRemoteLookupRdmaInitiator, PeerId, PushStatus,
    RemoteLookupRdmaInitiatorError, RemoteRegion,
};

#[cfg(feature = "rdma")]
use crate::connection::{ConnectionTable, ItemPlan, RealTransport};

/// A no-op logger used when the `logger` receptacle is unbound, so that a
/// missing (optional) logger never turns a `push` into an error.
#[cfg(feature = "rdma")]
pub(crate) struct NoopLogger;
#[cfg(feature = "rdma")]
impl ILogger for NoopLogger {
    fn error(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn info(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

define_component! {
    pub RemoteLookupRdmaInitiatorComponent {
        version: "0.1.0",
        provides: [IRemoteLookupRdmaInitiator],
        receptacles: {
            logger: ILogger,
            memory_tier: IMemoryTier,
        },
        fields: {
            conn_table: std::sync::OnceLock<std::sync::Arc<connection::ConnectionTable>>,
            telemetry: std::sync::Arc<telemetry::TelemetryCollector>,
            local_peer_id: std::sync::Mutex<Option<PeerId>>,
        },
    }
}

impl RemoteLookupRdmaInitiatorComponent {
    /// Access the telemetry collector (a no-op unless the `telemetry` feature
    /// is enabled).
    pub fn telemetry(&self) -> &telemetry::TelemetryCollector {
        &self.telemetry
    }
}

impl RemoteLookupRdmaInitiatorComponent {
    /// Return the connection table, building it lazily from the memory-tier
    /// pool on first use.
    ///
    /// The pool must be initialized (so its base/size are known and can be
    /// registered as an RDMA memory region on each connection); otherwise this
    /// returns [`RemoteLookupRdmaInitiatorError::NotInitialized`].
    #[cfg(feature = "rdma")]
    fn table(
        &self,
        memory_tier: &(dyn IMemoryTier + Send + Sync),
    ) -> Result<Arc<ConnectionTable>, RemoteLookupRdmaInitiatorError> {
        if let Some(table) = self.conn_table.get() {
            return Ok(Arc::clone(table));
        }
        let (base, size) = memory_tier.pool_info().ok_or_else(|| {
            RemoteLookupRdmaInitiatorError::NotInitialized(
                "memory-tier pool not initialized".into(),
            )
        })?;
        let peer_bytes = self
            .local_peer_id
            .lock()
            .expect("local_peer_id lock poisoned")
            .as_ref()
            .map(|p| p.as_str().as_bytes().to_vec())
            .unwrap_or_default();
        // The connection threads outlive any single call, so the table owns a logger
        // rather than borrowing one per push.
        let logger: Arc<dyn ILogger + Send + Sync> = match self.logger.get() {
            Ok(l) => l,
            Err(_) => Arc::new(NoopLogger),
        };
        let built = Arc::new(ConnectionTable::new(
            Arc::new(RealTransport::new(base, size, peer_bytes)),
            Arc::clone(&self.telemetry),
            logger,
        ));
        // If another thread won the race, its table is the canonical one and
        // ours is dropped; either way `get()` returns the winner.
        let _ = self.conn_table.set(built);
        Ok(Arc::clone(self.conn_table.get().unwrap()))
    }
}

#[cfg(feature = "rdma")]
impl RemoteLookupRdmaInitiatorComponent {
    /// Resolve each item against the local memory tier, returning the connection
    /// table and a per-item plan.
    ///
    /// Absent keys and size mismatches get a terminal status here and never reach the
    /// wire; a match becomes a planned write from the pool pointer `peek` returns.
    /// Deliberately `peek` and not `get`, so serving a remote request does not
    /// refresh local LRU position.
    fn plan(
        &self,
        items: &[(CacheKey, RemoteRegion)],
    ) -> Result<(Arc<ConnectionTable>, Vec<ItemPlan>), RemoteLookupRdmaInitiatorError> {
        let memory_tier = self.memory_tier.get().map_err(|_| {
            RemoteLookupRdmaInitiatorError::NotInitialized("memory_tier receptacle unbound".into())
        })?;
        let table = self.table(memory_tier.as_ref())?;

        let mut resolved = Vec::with_capacity(items.len());
        for (key, region) in items {
            match memory_tier.peek(*key) {
                None => resolved.push(ItemPlan::Done(PushStatus::KeyNotFound)),
                Some((ptr, size)) => {
                    if size != region.length {
                        resolved.push(ItemPlan::Done(PushStatus::SizeMismatch));
                    } else {
                        resolved.push(ItemPlan::Write {
                            local: ptr as *const u8,
                            len: size as usize,
                            remote_addr: region.addr,
                            rkey: region.rkey,
                        });
                    }
                }
            }
        }
        Ok((table, resolved))
    }
}

impl IRemoteLookupRdmaInitiator for RemoteLookupRdmaInitiatorComponent {
    #[cfg(not(feature = "rdma"))]
    fn push_async(
        &self,
        _endpoint: &str,
        _items: &[(CacheKey, RemoteRegion)],
        _on_complete: interfaces::PushCompletion,
    ) -> Result<(), RemoteLookupRdmaInitiatorError> {
        // Built without the real rdma-core path; the outbound RDMA push is
        // unavailable. (The submit/completion engine is still unit-tested over the
        // mock transport in `connection`.) Returning `Err` drops the callback
        // without invoking it, which the interface documents as equivalent for a
        // callback that releases its resources on drop.
        Err(RemoteLookupRdmaInitiatorError::NotInitialized(
            "remote-lookup-rdma-initiator built without the `rdma` feature".into(),
        ))
    }

    #[cfg(feature = "rdma")]
    fn push_async(
        &self,
        endpoint: &str,
        items: &[(CacheKey, RemoteRegion)],
        on_complete: interfaces::PushCompletion,
    ) -> Result<(), RemoteLookupRdmaInitiatorError> {
        let (table, resolved) = self.plan(items)?;
        table.push_async(endpoint, resolved, on_complete)
    }

    #[cfg(not(feature = "rdma"))]
    fn push(
        &self,
        _endpoint: &str,
        _items: &[(CacheKey, RemoteRegion)],
    ) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError> {
        Err(RemoteLookupRdmaInitiatorError::NotInitialized(
            "remote-lookup-rdma-initiator built without the `rdma` feature".into(),
        ))
    }

    #[cfg(feature = "rdma")]
    fn push(
        &self,
        endpoint: &str,
        items: &[(CacheKey, RemoteRegion)],
    ) -> Result<Vec<PushStatus>, RemoteLookupRdmaInitiatorError> {
        let (table, resolved) = self.plan(items)?;
        table.push(endpoint, resolved)
    }

    #[cfg(not(feature = "rdma"))]
    fn connect(&self, _endpoint: &str) -> Result<(), RemoteLookupRdmaInitiatorError> {
        // Built without the real rdma-core path; warming is unavailable.
        Err(RemoteLookupRdmaInitiatorError::NotInitialized(
            "remote-lookup-rdma-initiator built without the `rdma` feature".into(),
        ))
    }

    #[cfg(feature = "rdma")]
    fn connect(&self, endpoint: &str) -> Result<(), RemoteLookupRdmaInitiatorError> {
        let memory_tier = self.memory_tier.get().map_err(|_| {
            RemoteLookupRdmaInitiatorError::NotInitialized("memory_tier receptacle unbound".into())
        })?;
        let table = self.table(memory_tier.as_ref())?;
        table.connect(endpoint)
    }

    fn disconnect(&self, endpoint: &str) {
        if let Some(table) = self.conn_table.get() {
            table.disconnect(endpoint);
        }
    }

    fn disconnect_all(&self) {
        if let Some(table) = self.conn_table.get() {
            table.disconnect_all();
        }
    }

    fn set_local_peer_id(&self, peer: PeerId) {
        *self
            .local_peer_id
            .lock()
            .expect("local_peer_id lock poisoned") = Some(peer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_without_memory_tier_is_not_initialized() {
        let component = RemoteLookupRdmaInitiatorComponent::new_default();
        let items = [(
            1u64,
            RemoteRegion {
                addr: 0,
                rkey: 0,
                length: 0,
            },
        )];
        let err = component.push("10.0.0.1:5000", &items).unwrap_err();
        assert!(matches!(
            err,
            RemoteLookupRdmaInitiatorError::NotInitialized(_)
        ));
    }

    #[test]
    fn invalid_endpoint_requires_bound_memory_tier_first() {
        // With no memory tier bound, the receptacle check fires before endpoint
        // parsing, so this reports NotInitialized rather than InvalidEndpoint.
        let component = RemoteLookupRdmaInitiatorComponent::new_default();
        let err = component.push("not-an-endpoint", &[]).unwrap_err();
        assert!(matches!(
            err,
            RemoteLookupRdmaInitiatorError::NotInitialized(_)
        ));
    }

    #[test]
    fn disconnect_before_any_push_is_noop() {
        let component = RemoteLookupRdmaInitiatorComponent::new_default();
        component.disconnect("10.0.0.1:5000");
        component.disconnect_all();
    }

    #[test]
    fn set_local_peer_id_is_stored() {
        // The stamped id is consumed when the (rdma-only) connection table is built;
        // here we assert the setter is accepted and overwrites idempotently.
        let component = RemoteLookupRdmaInitiatorComponent::new_default();
        component.set_local_peer_id(PeerId::new("uuid-a"));
        component.set_local_peer_id(PeerId::new("uuid-b"));
        assert_eq!(
            component
                .local_peer_id
                .lock()
                .unwrap()
                .as_ref()
                .map(|p| p.as_str().to_string()),
            Some("uuid-b".to_string())
        );
    }
}
