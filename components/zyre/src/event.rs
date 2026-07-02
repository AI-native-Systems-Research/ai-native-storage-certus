use std::collections::HashMap;

use crate::peer::PeerId;

/// A network event received from the zyre node.
///
/// Events are received via [`ZyreNode::recv()`](crate::ZyreNode::recv) or
/// [`ZyreNode::try_recv()`](crate::ZyreNode::try_recv). Each variant carries
/// the relevant peer, group, and message data.
///
/// # Example
///
/// ```
/// use zyre::{ZyreEvent, PeerId};
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
