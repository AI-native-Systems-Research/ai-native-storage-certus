//! Async connection listener for RDMA CM events.
//!
//! Polls the rdma_cm event channel for incoming connection requests,
//! accepts new connections, and spawns per-session tasks.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::rdma::RdmaError;
use crate::session::Session;

/// Configuration for the connection listener.
pub struct ListenerConfig {
    /// TCP port for rdma_cm listener.
    pub port: u16,
    /// Maximum concurrent sessions.
    pub max_sessions: u32,
    /// Protocol version supported by this handler.
    pub protocol_version: u32,
}

/// Active session registry, tracking all connected sessions.
pub struct SessionRegistry {
    sessions: Mutex<HashMap<u64, Arc<Session>>>,
    next_id: AtomicU64,
    max_sessions: u32,
}

impl SessionRegistry {
    pub fn new(max_sessions: u32) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            max_sessions,
        }
    }

    pub async fn register(&self, session: Arc<Session>) -> Result<u64, RdmaError> {
        let mut sessions = self.sessions.lock().await;
        if sessions.len() >= self.max_sessions as usize {
            return Err(RdmaError::ResourceExhausted(
                "maximum sessions reached".into(),
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        sessions.insert(id, session);
        Ok(id)
    }

    pub async fn remove(&self, id: u64) -> Option<Arc<Session>> {
        self.sessions.lock().await.remove(&id)
    }

    pub async fn session_count(&self) -> usize {
        self.sessions.lock().await.len()
    }
}

/// The RDMA connection listener.
///
/// Listens for incoming connections on the configured port and manages
/// the lifecycle of sessions.
pub struct Listener {
    config: ListenerConfig,
    registry: Arc<SessionRegistry>,
}

impl Listener {
    pub fn new(config: ListenerConfig) -> Self {
        let registry = Arc::new(SessionRegistry::new(config.max_sessions));
        Self { config, registry }
    }

    /// Returns a reference to the session registry.
    pub fn registry(&self) -> &Arc<SessionRegistry> {
        &self.registry
    }

    /// Returns the configured port.
    pub fn port(&self) -> u16 {
        self.config.port
    }

    /// Returns the protocol version.
    pub fn protocol_version(&self) -> u32 {
        self.config.protocol_version
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionConfig;

    #[tokio::test]
    async fn session_registry_enforces_max() {
        let registry = SessionRegistry::new(2);
        let config = SessionConfig {
            protocol_version: 1,
            max_batch_size: 64,
        };

        let s1 = Arc::new(Session::new(config.clone()));
        let s2 = Arc::new(Session::new(config.clone()));
        let s3 = Arc::new(Session::new(config));

        assert!(registry.register(s1).await.is_ok());
        assert!(registry.register(s2).await.is_ok());
        assert!(registry.register(s3).await.is_err());
    }

    #[tokio::test]
    async fn session_registry_remove() {
        let registry = SessionRegistry::new(10);
        let config = SessionConfig {
            protocol_version: 1,
            max_batch_size: 64,
        };
        let session = Arc::new(Session::new(config));
        let id = registry.register(session).await.unwrap();
        assert_eq!(registry.session_count().await, 1);
        registry.remove(id).await;
        assert_eq!(registry.session_count().await, 0);
    }
}
