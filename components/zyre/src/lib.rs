//! Safe, idiomatic Rust bindings for the zyre C library.
//!
//! Zyre provides zero-configuration peer discovery and group messaging
//! over local area networks. This crate wraps the C API with RAII
//! resource management, typed events, and a builder-based configuration.
//!
//! # Usage
//!
//! Create a node directly via `ZyreNode::new()`:
//!
//! ```no_run
//! use zyre::{NodeConfig, ZyreNode};
//!
//! let config = NodeConfig::builder().name("my-node").build();
//! let mut node = ZyreNode::new(config).unwrap();
//! node.start().unwrap();
//! node.join("my-group").unwrap();
//! ```
//!
//! Or obtain the `IZyre` component interface to check subsystem health:
//!
//! ```no_run
//! use interfaces::IZyre;
//! ```

mod builder;
mod error;
mod event;
#[allow(dead_code)]
mod ffi;
mod node;
mod peer;

pub use builder::{GossipConfig, NodeConfig, NodeConfigBuilder};
pub use event::ZyreEvent;
pub use interfaces::ZyreError;
pub use node::ZyreNode;
pub use peer::PeerId;

use component_framework::define_component;
use interfaces::IZyre;

define_component! {
    pub ZyreComponent {
        version: "0.1.0",
        provides: [IZyre],
        receptacles: {},
        fields: {},
    }
}

impl IZyre for ZyreComponent {
    fn ping(&self) -> Result<String, interfaces::ZyreError> {
        Ok("pong".to_string())
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
}
