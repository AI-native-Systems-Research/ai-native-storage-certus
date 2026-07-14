//! Safe, idiomatic Rust bindings for the zyre C library.
//!
//! Zyre provides zero-configuration peer discovery and group messaging
//! over local area networks. This crate wraps the C API with RAII
//! resource management, typed events, and configuration structs.
//!
//! # Usage
//!
//! Obtain the [`IZyre`] component interface and use it as a node factory.
//! Node operations live on the returned [`IZyreNode`] handle:
//!
//! ```no_run
//! use component_core::query_interface;
//! use interfaces::{IZyre, IZyreNode, NodeConfig};
//! use zyre::ZyreComponent;
//!
//! let comp = ZyreComponent::new();
//! let izyre = query_interface!(comp, IZyre).unwrap();
//!
//! let mut config = NodeConfig::default();
//! config.name = Some("my-node".into());
//!
//! let mut node = izyre.create_node(config).unwrap();
//! node.start().unwrap();
//! node.join("my-group").unwrap();
//! ```

#[allow(dead_code)]
mod ffi;
mod node;

pub use interfaces::{GossipConfig, IZyre, IZyreNode, NodeConfig, PeerId, ZyreError, ZyreEvent};

use component_framework::define_component;
use node::ZyreNode;

define_component! {
    pub ZyreComponent {
        version: "0.1.0",
        provides: [IZyre],
        receptacles: {},
        fields: {},
    }
}

impl IZyre for ZyreComponent {
    fn ping(&self) -> Result<String, ZyreError> {
        Ok("pong".to_string())
    }

    fn create_node(&self, config: NodeConfig) -> Result<Box<dyn IZyreNode>, ZyreError> {
        Ok(Box::new(ZyreNode::new(config)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use component_core::query_interface;
    use std::sync::Arc;

    #[test]
    fn component_provides_izyre() {
        let comp = ZyreComponent::new();
        let iface: Option<Arc<dyn IZyre + Send + Sync>> = query_interface!(comp, IZyre);
        assert!(iface.is_some());
    }

    #[test]
    fn ping_returns_pong() {
        let comp = ZyreComponent::new();
        let iface: Arc<dyn IZyre + Send + Sync> = query_interface!(comp, IZyre).unwrap();
        assert_eq!(iface.ping().unwrap(), "pong");
    }

    #[test]
    fn create_node_rejects_invalid_config() {
        let comp = ZyreComponent::new();
        let iface: Arc<dyn IZyre + Send + Sync> = query_interface!(comp, IZyre).unwrap();
        let mut config = NodeConfig::default();
        config.name = Some(String::new());
        assert!(matches!(
            iface.create_node(config),
            Err(ZyreError::InvalidConfig(_))
        ));
    }
}
