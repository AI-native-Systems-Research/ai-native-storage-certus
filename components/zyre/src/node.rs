use crate::builder::NodeConfig;
use crate::error::ZyreError;
use crate::event::ZyreEvent;
use crate::ffi;
use crate::peer::PeerId;

/// A zyre peer node on the network.
///
/// Owns the underlying `zyre_t` C pointer. Implements `Send` (can be moved
/// between threads) but not `Sync` (the C API is not thread-safe for
/// concurrent access to a single node).
///
/// # Example
///
/// ```no_run
/// use zyre::{NodeConfig, ZyreNode, ZyreEvent};
///
/// let config = NodeConfig::builder().name("my-node").build();
/// let mut node = ZyreNode::new(config).unwrap();
/// node.start().unwrap();
/// node.join("cluster").unwrap();
///
/// // Receive events
/// loop {
///     match node.recv().unwrap() {
///         ZyreEvent::Shout { message, .. } => {
///             println!("got: {:?}", message);
///         }
///         ZyreEvent::Stop => break,
///         _ => {}
///     }
/// }
/// ```
pub struct ZyreNode {
    ptr: *mut ffi::zyre_t,
    started: bool,
}

// SAFETY: zyre_t can be moved between threads (ownership transfer).
// The C API is not thread-safe for concurrent access, so we do NOT impl Sync.
unsafe impl Send for ZyreNode {}

impl ZyreNode {
    /// Create a new ZyreNode with the given configuration.
    ///
    /// The node is created but not started. Call `start()` to begin
    /// discovery and messaging on the network.
    pub fn new(config: NodeConfig) -> Result<Self, ZyreError> {
        config.validate()?;

        let name_cstr;
        let ptr = unsafe {
            if let Some(ref name) = config.name {
                name_cstr = std::ffi::CString::new(name.as_str())
                    .map_err(|_| ZyreError::InvalidConfig("name contains null byte".into()))?;
                ffi::zyre_new(name_cstr.as_ptr())
            } else {
                ffi::zyre_new(std::ptr::null())
            }
        };

        if ptr.is_null() {
            return Err(ZyreError::CreateFailed);
        }

        let mut node = Self {
            ptr,
            started: false,
        };

        node.apply_config(&config)?;
        Ok(node)
    }

    fn apply_config(&mut self, config: &NodeConfig) -> Result<(), ZyreError> {
        unsafe {
            for (key, value) in &config.headers {
                let k = std::ffi::CString::new(key.as_str()).unwrap();
                let v = std::ffi::CString::new(value.as_str()).unwrap();
                ffi::zyre_set_header(self.ptr, k.as_ptr(), v.as_ptr());
            }

            if let Some(port) = config.port {
                ffi::zyre_set_port(self.ptr, port as libc::c_int);
            }

            if let Some(ref iface) = config.interface {
                let iface_c = std::ffi::CString::new(iface.as_str()).unwrap();
                ffi::zyre_set_interface(self.ptr, iface_c.as_ptr());
            }

            ffi::zyre_set_evasive_timeout(self.ptr, config.evasive_timeout_ms as libc::c_int);
            ffi::zyre_set_expired_timeout(self.ptr, config.expired_timeout_ms as libc::c_int);
            ffi::zyre_set_interval(self.ptr, config.beacon_interval_ms as libc::size_t);

            if let Some(ref gossip) = config.gossip {
                // In gossip mode the node must publish its own data endpoint,
                // which is distinct from the gossip hub endpoint. Validation
                // guarantees `config.endpoint` is present here.
                if let Some(ref endpoint) = config.endpoint {
                    let ep = std::ffi::CString::new(endpoint.as_str()).unwrap();
                    if ffi::zyre_set_endpoint(self.ptr, ep.as_ptr()) != 0 {
                        return Err(ZyreError::InvalidConfig(format!(
                            "failed to bind node endpoint '{endpoint}'"
                        )));
                    }
                }
                if let Some(ref bind_endpoint) = gossip.bind {
                    let ep = std::ffi::CString::new(bind_endpoint.as_str()).unwrap();
                    ffi::zyre_gossip_bind(self.ptr, ep.as_ptr());
                }
                for connect_endpoint in &gossip.connect {
                    let ep = std::ffi::CString::new(connect_endpoint.as_str()).unwrap();
                    ffi::zyre_gossip_connect(self.ptr, ep.as_ptr());
                }
            }
        }
        Ok(())
    }

    /// Start the node, beginning network discovery and messaging.
    pub fn start(&mut self) -> Result<(), ZyreError> {
        let rc = unsafe { ffi::zyre_start(self.ptr) };
        if rc != 0 {
            return Err(ZyreError::StartFailed("zyre_start returned error".into()));
        }
        self.started = true;
        Ok(())
    }

    /// Stop the node, signaling departure to peers.
    pub fn stop(&mut self) {
        if self.started {
            unsafe { ffi::zyre_stop(self.ptr) };
            self.started = false;
        }
    }

    /// Join a named group.
    pub fn join(&mut self, group: &str) -> Result<(), ZyreError> {
        if !self.started {
            return Err(ZyreError::NotStarted);
        }
        let group_c = std::ffi::CString::new(group)
            .map_err(|_| ZyreError::InvalidConfig("group name contains null byte".into()))?;
        unsafe { ffi::zyre_join(self.ptr, group_c.as_ptr()) };
        Ok(())
    }

    /// Leave a named group.
    pub fn leave(&mut self, group: &str) -> Result<(), ZyreError> {
        if !self.started {
            return Err(ZyreError::NotStarted);
        }
        let group_c = std::ffi::CString::new(group)
            .map_err(|_| ZyreError::InvalidConfig("group name contains null byte".into()))?;
        unsafe { ffi::zyre_leave(self.ptr, group_c.as_ptr()) };
        Ok(())
    }

    /// Send a message to all peers in a group.
    pub fn shout(&self, group: &str, data: &[u8]) -> Result<(), ZyreError> {
        if !self.started {
            return Err(ZyreError::NotStarted);
        }
        let group_c = std::ffi::CString::new(group)
            .map_err(|_| ZyreError::InvalidConfig("group name contains null byte".into()))?;
        let rc = unsafe {
            let mut msg = ffi::zmsg_new();
            ffi::zmsg_addmem(msg, data.as_ptr() as *const libc::c_void, data.len());
            ffi::zyre_shout(self.ptr, group_c.as_ptr(), &mut msg)
        };
        if rc != 0 {
            return Err(ZyreError::SendFailed);
        }
        Ok(())
    }

    /// Send a message directly to a specific peer.
    pub fn whisper(&self, peer: &PeerId, data: &[u8]) -> Result<(), ZyreError> {
        if !self.started {
            return Err(ZyreError::NotStarted);
        }
        let peer_c = std::ffi::CString::new(peer.as_str())
            .map_err(|_| ZyreError::InvalidConfig("peer id contains null byte".into()))?;
        let rc = unsafe {
            let mut msg = ffi::zmsg_new();
            ffi::zmsg_addmem(msg, data.as_ptr() as *const libc::c_void, data.len());
            ffi::zyre_whisper(self.ptr, peer_c.as_ptr(), &mut msg)
        };
        if rc != 0 {
            return Err(ZyreError::SendFailed);
        }
        Ok(())
    }

    /// Receive the next event from the network (blocking).
    pub fn recv(&self) -> Result<ZyreEvent, ZyreError> {
        if !self.started {
            return Err(ZyreError::NotStarted);
        }
        let event_ptr = unsafe { ffi::zyre_event_new(self.ptr) };
        if event_ptr.is_null() {
            return Err(ZyreError::RecvFailed);
        }
        let event = parse_event(event_ptr);
        unsafe { ffi::zyre_event_destroy(&mut (event_ptr as *mut _)) };
        event
    }

    /// Try to receive an event without blocking.
    ///
    /// Returns `Ok(None)` if no event is available.
    pub fn try_recv(&self) -> Result<Option<ZyreEvent>, ZyreError> {
        if !self.started {
            return Err(ZyreError::NotStarted);
        }
        let socket = unsafe { ffi::zyre_socket(self.ptr) };
        if socket.is_null() {
            return Err(ZyreError::RecvFailed);
        }
        // Use zpoller with zero timeout for non-blocking check
        let ready = unsafe {
            let poller = ffi::zpoller_new(
                socket as *mut libc::c_void,
                std::ptr::null_mut::<libc::c_void>(),
            );
            let result = ffi::zpoller_wait(poller, 0);
            let has_data = !result.is_null();
            ffi::zpoller_destroy(&mut (poller as *mut _));
            has_data
        };
        if !ready {
            return Ok(None);
        }
        self.recv().map(Some)
    }

    /// Get this node's UUID.
    pub fn uuid(&self) -> PeerId {
        let uuid_ptr = unsafe { ffi::zyre_uuid(self.ptr) };
        let uuid_str = unsafe { std::ffi::CStr::from_ptr(uuid_ptr) }
            .to_string_lossy()
            .into_owned();
        PeerId::new(uuid_str)
    }

    /// Get this node's name.
    pub fn name(&self) -> String {
        let name_ptr = unsafe { ffi::zyre_name(self.ptr) };
        unsafe { std::ffi::CStr::from_ptr(name_ptr) }
            .to_string_lossy()
            .into_owned()
    }

    /// Send a multi-frame message to all peers in a group.
    pub fn shout_multi(&self, group: &str, frames: &[&[u8]]) -> Result<(), ZyreError> {
        if !self.started {
            return Err(ZyreError::NotStarted);
        }
        let group_c = std::ffi::CString::new(group)
            .map_err(|_| ZyreError::InvalidConfig("group name contains null byte".into()))?;
        let rc = unsafe {
            let mut msg = ffi::zmsg_new();
            for frame in frames {
                ffi::zmsg_addmem(msg, frame.as_ptr() as *const libc::c_void, frame.len());
            }
            ffi::zyre_shout(self.ptr, group_c.as_ptr(), &mut msg)
        };
        if rc != 0 {
            return Err(ZyreError::SendFailed);
        }
        Ok(())
    }

    /// Send a multi-frame message directly to a specific peer.
    pub fn whisper_multi(&self, peer: &PeerId, frames: &[&[u8]]) -> Result<(), ZyreError> {
        if !self.started {
            return Err(ZyreError::NotStarted);
        }
        let peer_c = std::ffi::CString::new(peer.as_str())
            .map_err(|_| ZyreError::InvalidConfig("peer id contains null byte".into()))?;
        let rc = unsafe {
            let mut msg = ffi::zmsg_new();
            for frame in frames {
                ffi::zmsg_addmem(msg, frame.as_ptr() as *const libc::c_void, frame.len());
            }
            ffi::zyre_whisper(self.ptr, peer_c.as_ptr(), &mut msg)
        };
        if rc != 0 {
            return Err(ZyreError::SendFailed);
        }
        Ok(())
    }

    /// Get the list of all known peers (by UUID).
    pub fn peers(&self) -> Vec<PeerId> {
        unsafe {
            let list = ffi::zyre_peers(self.ptr);
            let result = zlist_to_peer_ids(list);
            ffi::zlist_destroy(&mut (list as *mut _));
            result
        }
    }

    /// Get peers that belong to a specific group.
    pub fn peers_by_group(&self, group: &str) -> Vec<PeerId> {
        let group_c = std::ffi::CString::new(group).unwrap_or_default();
        unsafe {
            let list = ffi::zyre_peers_by_group(self.ptr, group_c.as_ptr());
            let result = zlist_to_peer_ids(list);
            ffi::zlist_destroy(&mut (list as *mut _));
            result
        }
    }

    /// Get the list of groups this node has joined.
    pub fn own_groups(&self) -> Vec<String> {
        unsafe {
            let list = ffi::zyre_own_groups(self.ptr);
            let result = zlist_to_strings(list);
            ffi::zlist_destroy(&mut (list as *mut _));
            result
        }
    }

    /// Get all groups known to this node (from all peers).
    pub fn peer_groups(&self) -> Vec<String> {
        unsafe {
            let list = ffi::zyre_peer_groups(self.ptr);
            let result = zlist_to_strings(list);
            ffi::zlist_destroy(&mut (list as *mut _));
            result
        }
    }

    /// Get the network address of a peer.
    pub fn peer_address(&self, peer: &PeerId) -> Option<String> {
        let peer_c = std::ffi::CString::new(peer.as_str()).ok()?;
        unsafe {
            let addr = ffi::zyre_peer_address(self.ptr, peer_c.as_ptr());
            if addr.is_null() {
                None
            } else {
                let s = std::ffi::CStr::from_ptr(addr)
                    .to_string_lossy()
                    .into_owned();
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            }
        }
    }

    /// Get the value of a specific header for a peer.
    pub fn peer_header_value(&self, peer: &PeerId, key: &str) -> Option<String> {
        let peer_c = std::ffi::CString::new(peer.as_str()).ok()?;
        let key_c = std::ffi::CString::new(key).ok()?;
        unsafe {
            let val = ffi::zyre_peer_header_value(self.ptr, peer_c.as_ptr(), key_c.as_ptr());
            if val.is_null() {
                None
            } else {
                let s = std::ffi::CStr::from_ptr(val).to_string_lossy().into_owned();
                Some(s)
            }
        }
    }
}

impl Drop for ZyreNode {
    fn drop(&mut self) {
        self.stop();
        unsafe { ffi::zyre_destroy(&mut self.ptr) };
    }
}

unsafe fn zlist_to_peer_ids(list: *mut ffi::zlist_t) -> Vec<PeerId> {
    let mut result = Vec::new();
    if list.is_null() {
        return result;
    }
    let mut item = ffi::zlist_first(list);
    while !item.is_null() {
        let s = std::ffi::CStr::from_ptr(item as *const libc::c_char)
            .to_string_lossy()
            .into_owned();
        result.push(PeerId::new(s));
        item = ffi::zlist_next(list);
    }
    result
}

unsafe fn zlist_to_strings(list: *mut ffi::zlist_t) -> Vec<String> {
    let mut result = Vec::new();
    if list.is_null() {
        return result;
    }
    let mut item = ffi::zlist_first(list);
    while !item.is_null() {
        let s = std::ffi::CStr::from_ptr(item as *const libc::c_char)
            .to_string_lossy()
            .into_owned();
        result.push(s);
        item = ffi::zlist_next(list);
    }
    result
}

fn parse_event(event_ptr: *mut ffi::zyre_event_t) -> Result<ZyreEvent, ZyreError> {
    let event_type = unsafe {
        let t = ffi::zyre_event_type(event_ptr);
        if t.is_null() {
            return Err(ZyreError::RecvFailed);
        }
        std::ffi::CStr::from_ptr(t).to_string_lossy().into_owned()
    };

    let peer_uuid = unsafe {
        let p = ffi::zyre_event_peer_uuid(event_ptr);
        if p.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };

    let peer_name = unsafe {
        let n = ffi::zyre_event_peer_name(event_ptr);
        if n.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(n).to_string_lossy().into_owned()
        }
    };

    let peer = PeerId::new(peer_uuid);

    match event_type.as_str() {
        "ENTER" => {
            let address = unsafe {
                let a = ffi::zyre_event_peer_addr(event_ptr);
                if a.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(a).to_string_lossy().into_owned()
                }
            };
            let headers = parse_headers(event_ptr);
            Ok(ZyreEvent::Enter {
                peer,
                name: peer_name,
                headers,
                address,
            })
        }
        "EXIT" => Ok(ZyreEvent::Exit {
            peer,
            name: peer_name,
        }),
        "EVASIVE" => Ok(ZyreEvent::Evasive {
            peer,
            name: peer_name,
        }),
        "SILENT" => Ok(ZyreEvent::Silent {
            peer,
            name: peer_name,
        }),
        "JOIN" => {
            let group = parse_group(event_ptr);
            Ok(ZyreEvent::Join {
                peer,
                name: peer_name,
                group,
            })
        }
        "LEAVE" => {
            let group = parse_group(event_ptr);
            Ok(ZyreEvent::Leave {
                peer,
                name: peer_name,
                group,
            })
        }
        "WHISPER" => {
            let message = parse_message(event_ptr);
            Ok(ZyreEvent::Whisper {
                peer,
                name: peer_name,
                message,
            })
        }
        "SHOUT" => {
            let group = parse_group(event_ptr);
            let message = parse_message(event_ptr);
            Ok(ZyreEvent::Shout {
                peer,
                name: peer_name,
                group,
                message,
            })
        }
        "STOP" => Ok(ZyreEvent::Stop),
        _ => Err(ZyreError::RecvFailed),
    }
}

fn parse_group(event_ptr: *mut ffi::zyre_event_t) -> String {
    unsafe {
        let g = ffi::zyre_event_group(event_ptr);
        if g.is_null() {
            String::new()
        } else {
            std::ffi::CStr::from_ptr(g).to_string_lossy().into_owned()
        }
    }
}

fn parse_message(event_ptr: *mut ffi::zyre_event_t) -> Vec<u8> {
    unsafe {
        let msg = ffi::zyre_event_get_msg(event_ptr);
        if msg.is_null() {
            return Vec::new();
        }
        let frame = ffi::zmsg_first(msg);
        if frame.is_null() {
            return Vec::new();
        }
        let data = ffi::zframe_data(frame);
        let size = ffi::zframe_size(frame);
        if data.is_null() || size == 0 {
            return Vec::new();
        }
        std::slice::from_raw_parts(data as *const u8, size).to_vec()
    }
}

fn parse_headers(event_ptr: *mut ffi::zyre_event_t) -> std::collections::HashMap<String, String> {
    let mut headers = std::collections::HashMap::new();
    unsafe {
        let zhash = ffi::zyre_event_headers(event_ptr);
        if zhash.is_null() {
            return headers;
        }
        let mut item = ffi::zhash_first(zhash);
        while !item.is_null() {
            let key_ptr = ffi::zhash_cursor(zhash);
            if !key_ptr.is_null() {
                let key = std::ffi::CStr::from_ptr(key_ptr)
                    .to_string_lossy()
                    .into_owned();
                let value = std::ffi::CStr::from_ptr(item as *const libc::c_char)
                    .to_string_lossy()
                    .into_owned();
                headers.insert(key, value);
            }
            item = ffi::zhash_next(zhash);
        }
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builder::NodeConfig;

    #[test]
    fn node_rejects_invalid_config() {
        let config = NodeConfig::builder().name("").build();
        assert!(matches!(
            ZyreNode::new(config),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn node_rejects_zero_evasive_timeout() {
        let config = NodeConfig::builder().evasive_timeout_ms(0).build();
        assert!(matches!(
            ZyreNode::new(config),
            Err(ZyreError::InvalidConfig(_))
        ));
    }
}
