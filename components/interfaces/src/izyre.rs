//! IZyre interface definition and associated value types.
//!
//! `IZyre` is a **factory**: [`IZyre::create_node`] constructs an un-started
//! node returned as a [`IZyreNode`] handle. All peer-discovery and messaging
//! operations live on that handle. The value types the factory and handle
//! exchange ([`PeerId`], [`ZyreEvent`], [`NodeConfig`], [`GossipConfig`]) live
//! here in the `interfaces` crate so the interface can name them without
//! depending on the `zyre` implementation crate. The `zyre` crate re-exports
//! them for convenience.

use std::collections::HashMap;
use std::fmt;

/// Errors returned by `IZyre` and `IZyreNode` operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZyreError {
    /// `zyre_new()` returned null (resource exhaustion).
    CreateFailed,
    /// `zyre_start()` returned an error.
    StartFailed(String),
    /// An operation was attempted before the node was started.
    NotStarted,
    /// Configuration validation failed.
    InvalidConfig(String),
    /// A send operation (whisper/shout) failed.
    SendFailed,
    /// Receive returned unexpectedly (node stopped).
    RecvFailed,
}

impl fmt::Display for ZyreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateFailed => write!(f, "failed to create zyre node"),
            Self::StartFailed(reason) => write!(f, "failed to start zyre node: {reason}"),
            Self::NotStarted => write!(f, "node not started"),
            Self::InvalidConfig(reason) => write!(f, "invalid configuration: {reason}"),
            Self::SendFailed => write!(f, "send operation failed"),
            Self::RecvFailed => write!(f, "receive failed (node stopped)"),
        }
    }
}

impl std::error::Error for ZyreError {}

/// Unique identifier for a zyre peer (UUID string).
///
/// # Example
///
/// ```
/// use interfaces::PeerId;
///
/// let id = PeerId::from("abc-123");
/// assert_eq!(id.as_str(), "abc-123");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PeerId(String);

impl PeerId {
    /// Create a PeerId from a UUID string.
    pub fn new(uuid: impl Into<String>) -> Self {
        Self(uuid.into())
    }

    /// Return the UUID string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for PeerId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for PeerId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// A network event received from a zyre node.
///
/// Events are received via [`IZyreNode::recv`] or [`IZyreNode::try_recv`]. Each
/// variant carries the relevant peer, group, and message data as owned values.
///
/// # Example
///
/// ```
/// use interfaces::{ZyreEvent, PeerId};
///
/// let event = ZyreEvent::Shout {
///     peer: PeerId::from("uuid-123"),
///     name: "sender".into(),
///     group: "cluster".into(),
///     message: b"hello".to_vec(),
/// };
/// assert_eq!(event.group(), Some("cluster"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZyreEvent {
    /// A new peer was discovered on the network.
    Enter {
        peer: PeerId,
        name: String,
        headers: HashMap<String, String>,
        address: String,
    },
    /// A peer has left the network (graceful or timeout).
    Exit { peer: PeerId, name: String },
    /// A peer is being pinged (no response for evasive timeout).
    Evasive { peer: PeerId, name: String },
    /// A peer has not responded to pings (silent timeout reached).
    Silent { peer: PeerId, name: String },
    /// A peer joined a group.
    Join {
        peer: PeerId,
        name: String,
        group: String,
    },
    /// A peer left a group.
    Leave {
        peer: PeerId,
        name: String,
        group: String,
    },
    /// A direct message was received from a peer.
    Whisper {
        peer: PeerId,
        name: String,
        message: Vec<u8>,
    },
    /// A group message was received.
    Shout {
        peer: PeerId,
        name: String,
        group: String,
        message: Vec<u8>,
    },
    /// The local node has stopped.
    Stop,
}

impl ZyreEvent {
    /// Returns the peer ID associated with this event, if any.
    pub fn peer(&self) -> Option<&PeerId> {
        match self {
            Self::Enter { peer, .. }
            | Self::Exit { peer, .. }
            | Self::Evasive { peer, .. }
            | Self::Silent { peer, .. }
            | Self::Join { peer, .. }
            | Self::Leave { peer, .. }
            | Self::Whisper { peer, .. }
            | Self::Shout { peer, .. } => Some(peer),
            Self::Stop => None,
        }
    }

    /// Returns the peer name associated with this event, if any.
    pub fn peer_name(&self) -> Option<&str> {
        match self {
            Self::Enter { name, .. }
            | Self::Exit { name, .. }
            | Self::Evasive { name, .. }
            | Self::Silent { name, .. }
            | Self::Join { name, .. }
            | Self::Leave { name, .. }
            | Self::Whisper { name, .. }
            | Self::Shout { name, .. } => Some(name),
            Self::Stop => None,
        }
    }

    /// Returns the group name if this is a group-related event.
    pub fn group(&self) -> Option<&str> {
        match self {
            Self::Join { group, .. } | Self::Leave { group, .. } | Self::Shout { group, .. } => {
                Some(group)
            }
            _ => None,
        }
    }
}

/// Configuration for gossip-based discovery (alternative to UDP beacon).
///
/// Use gossip when UDP broadcast is unavailable (e.g., across subnets).
///
/// # Example
///
/// ```
/// use interfaces::GossipConfig;
///
/// // Hub node binds a gossip endpoint:
/// let hub = GossipConfig::bind("tcp://*:9999");
///
/// // Spoke nodes connect:
/// let spoke = GossipConfig::connect("tcp://hub-host:9999");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct GossipConfig {
    /// Endpoint to bind as gossip server (e.g., "tcp://*:9999").
    pub bind: Option<String>,
    /// Endpoints to connect to as gossip client.
    pub connect: Vec<String>,
}

impl GossipConfig {
    /// Create a gossip config that binds a server endpoint.
    pub fn bind(endpoint: impl Into<String>) -> Self {
        Self {
            bind: Some(endpoint.into()),
            connect: Vec::new(),
        }
    }

    /// Create a gossip config that connects to an existing server.
    pub fn connect(endpoint: impl Into<String>) -> Self {
        Self {
            bind: None,
            connect: vec![endpoint.into()],
        }
    }

    fn validate(&self) -> Result<(), ZyreError> {
        if self.bind.is_none() && self.connect.is_empty() {
            return Err(ZyreError::InvalidConfig(
                "gossip config requires at least one of bind or connect".into(),
            ));
        }
        Ok(())
    }
}

/// Configuration for constructing a zyre node via [`IZyre::create_node`].
///
/// This struct is `#[non_exhaustive]`, so construct it from [`Default`] and set
/// the fields you need; new fields can be added without breaking callers.
///
/// # Example
///
/// ```
/// use interfaces::NodeConfig;
///
/// let mut config = NodeConfig::default();
/// config.name = Some("my-node".into());
/// config.headers.insert("role".into(), "worker".into());
/// config.port = Some(5670);
/// assert!(config.validate().is_ok());
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NodeConfig {
    /// Human-readable node name (announced during discovery).
    pub name: Option<String>,
    /// Header key-value pairs shared with peers during discovery.
    pub headers: HashMap<String, String>,
    /// UDP beacon port override (default: 5670 in zyre).
    pub port: Option<u16>,
    /// Network interface to beacon on.
    pub interface: Option<String>,
    /// Evasive timeout in milliseconds (default: 5000).
    pub evasive_timeout_ms: u32,
    /// Expired timeout in milliseconds (default: 30000). Must exceed evasive.
    pub expired_timeout_ms: u32,
    /// Beacon interval in milliseconds (default: 1000).
    pub beacon_interval_ms: u32,
    /// This node's own data endpoint (its ZRE mailbox). Required with gossip.
    pub endpoint: Option<String>,
    /// Use gossip-based discovery instead of UDP beaconing.
    pub gossip: Option<GossipConfig>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            name: None,
            headers: HashMap::new(),
            port: None,
            interface: None,
            evasive_timeout_ms: 5000,
            expired_timeout_ms: 30000,
            beacon_interval_ms: 1000,
            endpoint: None,
            gossip: None,
        }
    }
}

impl NodeConfig {
    /// Validate the configuration. Called by [`IZyre::create_node`].
    ///
    /// # Errors
    ///
    /// Returns [`ZyreError::InvalidConfig`] if any field is out of range or the
    /// gossip configuration is inconsistent.
    pub fn validate(&self) -> Result<(), ZyreError> {
        if let Some(ref name) = self.name {
            if name.is_empty() {
                return Err(ZyreError::InvalidConfig("name must not be empty".into()));
            }
        }
        if self.evasive_timeout_ms == 0 {
            return Err(ZyreError::InvalidConfig(
                "evasive_timeout_ms must be > 0".into(),
            ));
        }
        if self.expired_timeout_ms <= self.evasive_timeout_ms {
            return Err(ZyreError::InvalidConfig(
                "expired_timeout_ms must be > evasive_timeout_ms".into(),
            ));
        }
        if let Some(port) = self.port {
            if port == 0 {
                return Err(ZyreError::InvalidConfig("port must be > 0".into()));
            }
        }
        if let Some(ref gossip) = self.gossip {
            gossip.validate()?;
            // In gossip mode zyre disables UDP beaconing, so the node must
            // publish its own data endpoint explicitly. This endpoint is the
            // node's mailbox and must be distinct from the gossip hub endpoint.
            if self.endpoint.is_none() {
                return Err(ZyreError::InvalidConfig(
                    "gossip discovery requires an explicit node endpoint (set config.endpoint)"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

/// A handle to a zyre peer node, returned by [`IZyre::create_node`].
///
/// This is `Send` but **not** `Sync`: the underlying zyre C API is not
/// thread-safe for concurrent access to a single node, though ownership can be
/// moved between threads. The node runs no threads of its own beyond those the
/// zyre C library manages internally; the caller drives event reception by
/// calling [`recv`](IZyreNode::recv) / [`try_recv`](IZyreNode::try_recv).
///
/// Deliberately a plain trait (not a `define_interface!` component interface):
/// component interfaces are `Send + Sync` with `&self`-only methods, which
/// would force a lock around this inherently single-threaded, `&mut self`
/// resource. As a returned handle it does not need runtime interface discovery.
pub trait IZyreNode: Send {
    /// Start the node, beginning network discovery and messaging.
    fn start(&mut self) -> Result<(), ZyreError>;

    /// Stop the node, signaling departure to peers.
    fn stop(&mut self);

    /// Join a named group.
    fn join(&mut self, group: &str) -> Result<(), ZyreError>;

    /// Leave a named group.
    fn leave(&mut self, group: &str) -> Result<(), ZyreError>;

    /// Send a message to all peers in a group.
    fn shout(&self, group: &str, data: &[u8]) -> Result<(), ZyreError>;

    /// Send a message directly to a specific peer.
    fn whisper(&self, peer: &PeerId, data: &[u8]) -> Result<(), ZyreError>;

    /// Receive the next event from the network (blocking).
    fn recv(&self) -> Result<ZyreEvent, ZyreError>;

    /// Try to receive an event without blocking. Returns `Ok(None)` if none.
    fn try_recv(&self) -> Result<Option<ZyreEvent>, ZyreError>;

    /// Get this node's UUID.
    fn uuid(&self) -> PeerId;

    /// Get this node's name.
    fn name(&self) -> String;

    /// Get the list of all known peers (by UUID).
    fn peers(&self) -> Vec<PeerId>;

    /// Get peers that belong to a specific group.
    fn peers_by_group(&self, group: &str) -> Vec<PeerId>;

    /// Get the list of groups this node has joined.
    fn own_groups(&self) -> Vec<String>;

    /// Get all groups known to this node (from all peers).
    fn peer_groups(&self) -> Vec<String>;

    /// Get the network address of a peer.
    fn peer_address(&self, peer: &PeerId) -> Option<String>;

    /// Get the value of a specific header for a peer.
    fn peer_header_value(&self, peer: &PeerId, key: &str) -> Option<String>;
}

component_macros::define_interface! {
    pub IZyre {
        /// Check if the zyre subsystem is available and healthy.
        fn ping(&self) -> Result<String, ZyreError>;

        /// Create a new, un-started node from `config`.
        ///
        /// The returned [`IZyreNode`] is the only entry point to node
        /// operations; call [`IZyreNode::start`] to begin discovery.
        fn create_node(&self, config: NodeConfig) -> Result<Box<dyn IZyreNode>, ZyreError>;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn zyre_error_display() {
        assert!(ZyreError::CreateFailed
            .to_string()
            .contains("failed to create zyre node"));
        assert!(ZyreError::InvalidConfig("bad".into())
            .to_string()
            .contains("bad"));
    }

    #[test]
    fn peer_id_display() {
        let id = PeerId::new("abc123");
        assert_eq!(id.to_string(), "abc123");
        assert_eq!(id.as_str(), "abc123");
    }

    #[test]
    fn peer_id_equality() {
        let a = PeerId::new("same");
        let b = PeerId::new("same");
        let c = PeerId::new("different");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn peer_id_hashable() {
        let mut set = HashSet::new();
        set.insert(PeerId::new("one"));
        set.insert(PeerId::new("two"));
        set.insert(PeerId::new("one"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn peer_id_from_conversions() {
        let from_str: PeerId = "hello".into();
        let from_string: PeerId = String::from("hello").into();
        assert_eq!(from_str, from_string);
    }

    #[test]
    fn event_peer_accessor() {
        let event = ZyreEvent::Enter {
            peer: PeerId::new("uuid-1"),
            name: "node-a".into(),
            headers: HashMap::new(),
            address: "tcp://192.168.1.1:9001".into(),
        };
        assert_eq!(event.peer(), Some(&PeerId::new("uuid-1")));
        assert_eq!(event.peer_name(), Some("node-a"));
    }

    #[test]
    fn stop_event_has_no_peer() {
        assert_eq!(ZyreEvent::Stop.peer(), None);
        assert_eq!(ZyreEvent::Stop.peer_name(), None);
        assert_eq!(ZyreEvent::Stop.group(), None);
    }

    #[test]
    fn group_accessor() {
        let event = ZyreEvent::Shout {
            peer: PeerId::new("uuid-2"),
            name: "node-b".into(),
            group: "cluster".into(),
            message: b"hello".to_vec(),
        };
        assert_eq!(event.group(), Some("cluster"));
    }

    #[test]
    fn whisper_has_no_group() {
        let event = ZyreEvent::Whisper {
            peer: PeerId::new("uuid-3"),
            name: "node-c".into(),
            message: vec![1, 2, 3],
        };
        assert_eq!(event.group(), None);
    }

    #[test]
    fn default_config_is_valid() {
        assert!(NodeConfig::default().validate().is_ok());
    }

    #[test]
    fn empty_name_is_invalid() {
        let mut config = NodeConfig::default();
        config.name = Some(String::new());
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn zero_evasive_timeout_is_invalid() {
        let mut config = NodeConfig::default();
        config.evasive_timeout_ms = 0;
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn expired_must_exceed_evasive() {
        let mut config = NodeConfig::default();
        config.evasive_timeout_ms = 5000;
        config.expired_timeout_ms = 5000;
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn zero_port_is_invalid() {
        let mut config = NodeConfig::default();
        config.port = Some(0);
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn gossip_requires_bind_or_connect() {
        let mut config = NodeConfig::default();
        config.endpoint = Some("tcp://127.0.0.1:9998".into());
        config.gossip = Some(GossipConfig {
            bind: None,
            connect: vec![],
        });
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn gossip_bind_is_valid() {
        let mut config = NodeConfig::default();
        config.endpoint = Some("tcp://127.0.0.1:9998".into());
        config.gossip = Some(GossipConfig::bind("tcp://*:9999"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn gossip_connect_is_valid() {
        let mut config = NodeConfig::default();
        config.endpoint = Some("tcp://127.0.0.1:9998".into());
        config.gossip = Some(GossipConfig::connect("tcp://server:9999"));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn gossip_without_endpoint_is_invalid() {
        let mut config = NodeConfig::default();
        config.gossip = Some(GossipConfig::bind("tcp://*:9999"));
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }
}
