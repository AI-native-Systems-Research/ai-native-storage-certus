use std::collections::HashMap;

use crate::error::ZyreError;

/// Configuration for gossip-based discovery (alternative to UDP beacon).
///
/// Use gossip when UDP broadcast is unavailable (e.g., across subnets).
///
/// # Example
///
/// ```
/// use zyre::GossipConfig;
///
/// // Hub node binds a gossip endpoint:
/// let hub = GossipConfig::bind("tcp://*:9999");
///
/// // Spoke nodes connect:
/// let spoke = GossipConfig::connect("tcp://hub-host:9999");
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Configuration for constructing a ZyreNode.
///
/// # Example
///
/// ```
/// use zyre::{NodeConfig, GossipConfig};
///
/// let config = NodeConfig::builder()
///     .name("my-node")
///     .header("role", "worker")
///     .port(5670)
///     .evasive_timeout_ms(3000)
///     .expired_timeout_ms(15000)
///     .build();
/// ```
#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub(crate) name: Option<String>,
    pub(crate) headers: HashMap<String, String>,
    pub(crate) port: Option<u16>,
    pub(crate) interface: Option<String>,
    pub(crate) evasive_timeout_ms: u32,
    pub(crate) expired_timeout_ms: u32,
    pub(crate) beacon_interval_ms: u32,
    pub(crate) endpoint: Option<String>,
    pub(crate) gossip: Option<GossipConfig>,
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
    /// Create a new builder for NodeConfig.
    pub fn builder() -> NodeConfigBuilder {
        NodeConfigBuilder::default()
    }

    /// Validate the configuration.
    pub(crate) fn validate(&self) -> Result<(), ZyreError> {
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
                    "gossip discovery requires an explicit node endpoint (set via .endpoint())"
                        .into(),
                ));
            }
        }
        Ok(())
    }
}

/// Builder for constructing [`NodeConfig`] with validated parameters.
///
/// Obtained via [`NodeConfig::builder()`].
#[derive(Debug, Default)]
pub struct NodeConfigBuilder {
    config: NodeConfig,
}

impl NodeConfigBuilder {
    /// Set the human-readable node name.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.config.name = Some(name.into());
        self
    }

    /// Add a header key-value pair shared during discovery.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.headers.insert(key.into(), value.into());
        self
    }

    /// Set the UDP beacon port (default: 5670).
    pub fn port(mut self, port: u16) -> Self {
        self.config.port = Some(port);
        self
    }

    /// Set the network interface for beaconing.
    pub fn interface(mut self, iface: impl Into<String>) -> Self {
        self.config.interface = Some(iface.into());
        self
    }

    /// Set the evasive timeout in milliseconds (default: 5000).
    pub fn evasive_timeout_ms(mut self, ms: u32) -> Self {
        self.config.evasive_timeout_ms = ms;
        self
    }

    /// Set the expired timeout in milliseconds (default: 30000).
    pub fn expired_timeout_ms(mut self, ms: u32) -> Self {
        self.config.expired_timeout_ms = ms;
        self
    }

    /// Set the beacon interval in milliseconds (default: 1000).
    pub fn beacon_interval_ms(mut self, ms: u32) -> Self {
        self.config.beacon_interval_ms = ms;
        self
    }

    /// Set this node's own data endpoint (its ZRE mailbox).
    ///
    /// Required when using gossip discovery: it must be unique per node and
    /// distinct from the gossip hub endpoint (`GossipConfig::bind`/`connect`).
    /// Any transport valid for both bind and connect works (`tcp://`,
    /// `ipc://`, `inproc://`); for `tcp://` use an address reachable by remote
    /// nodes as well as locally.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.config.endpoint = Some(endpoint.into());
        self
    }

    /// Use gossip-based discovery instead of UDP beaconing.
    ///
    /// Requires a node endpoint set via [`endpoint`](Self::endpoint).
    pub fn gossip(mut self, config: GossipConfig) -> Self {
        self.config.gossip = Some(config);
        self
    }

    /// Build and validate the configuration.
    pub fn build(self) -> NodeConfig {
        self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_valid() {
        let config = NodeConfig::builder().build();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn builder_sets_fields() {
        let config = NodeConfig::builder()
            .name("test-node")
            .header("role", "storage")
            .port(9999)
            .interface("eth0")
            .evasive_timeout_ms(3000)
            .expired_timeout_ms(20000)
            .beacon_interval_ms(500)
            .build();

        assert_eq!(config.name, Some("test-node".into()));
        assert_eq!(config.headers.get("role"), Some(&"storage".into()));
        assert_eq!(config.port, Some(9999));
        assert_eq!(config.interface, Some("eth0".into()));
        assert_eq!(config.evasive_timeout_ms, 3000);
        assert_eq!(config.expired_timeout_ms, 20000);
        assert_eq!(config.beacon_interval_ms, 500);
    }

    #[test]
    fn empty_name_is_invalid() {
        let config = NodeConfig::builder().name("").build();
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn zero_evasive_timeout_is_invalid() {
        let config = NodeConfig::builder().evasive_timeout_ms(0).build();
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn expired_must_exceed_evasive() {
        let config = NodeConfig::builder()
            .evasive_timeout_ms(5000)
            .expired_timeout_ms(5000)
            .build();
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn zero_port_is_invalid() {
        let config = NodeConfig::builder().port(0).build();
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn gossip_requires_bind_or_connect() {
        let gossip = GossipConfig {
            bind: None,
            connect: vec![],
        };
        let config = NodeConfig::builder().gossip(gossip).build();
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn gossip_bind_is_valid() {
        let config = NodeConfig::builder()
            .endpoint("tcp://127.0.0.1:9998")
            .gossip(GossipConfig::bind("tcp://*:9999"))
            .build();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn gossip_connect_is_valid() {
        let config = NodeConfig::builder()
            .endpoint("tcp://127.0.0.1:9998")
            .gossip(GossipConfig::connect("tcp://server:9999"))
            .build();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn gossip_without_endpoint_is_invalid() {
        let config = NodeConfig::builder()
            .gossip(GossipConfig::bind("tcp://*:9999"))
            .build();
        assert!(matches!(
            config.validate(),
            Err(ZyreError::InvalidConfig(_))
        ));
    }
}
