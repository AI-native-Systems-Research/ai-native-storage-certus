use std::cell::Cell;

use crate::ffi;
use interfaces::{IZyreNode, NodeConfig, PeerId, ZyreError, ZyreEvent};

/// Lifecycle state of a [`ZyreNode`].
///
/// `stop()` moves `Running -> Draining` so the events already queued by the
/// zyre actor plus its final `Stop` sentinel stay readable. Consuming that
/// `Stop` moves `Draining -> Done`, after which no further `zyre_recv` is
/// issued: the actor has exited, so another receive would block forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Created,
    Running,
    Draining,
    Done,
}

/// A zyre peer node on the network — the concrete implementation of
/// [`interfaces::IZyreNode`].
///
/// Owns the underlying `zyre_t` C pointer. Implements `Send` (can be moved
/// between threads) but not `Sync` (the C API is not thread-safe for
/// concurrent access to a single node).
///
/// Constructed only via [`crate::ZyreComponent`]'s `IZyre::create_node`
/// factory, which returns it as a `Box<dyn IZyreNode>`; the constructor is
/// crate-private so callers cannot bypass the `IZyre` interface.
pub(crate) struct ZyreNode {
    ptr: *mut ffi::zyre_t,
    state: Cell<State>,
}

// SAFETY: zyre_t can be moved between threads (ownership transfer).
// The C API is not thread-safe for concurrent access, so we do NOT impl Sync.
// `Cell<State>` is `Send` (State is a Copy enum) but not `Sync`, which also
// reinforces the intended non-`Sync` guarantee.
unsafe impl Send for ZyreNode {}

impl ZyreNode {
    /// Create a new ZyreNode with the given configuration.
    ///
    /// The node is created but not started. Call `start()` to begin
    /// discovery and messaging on the network.
    pub(crate) fn new(config: NodeConfig) -> Result<Self, ZyreError> {
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
            state: Cell::new(State::Created),
        };

        node.apply_config(&config)?;
        Ok(node)
    }

    /// Guard for operations that require a running (started, not-yet-stopped)
    /// node. Returns [`ZyreError::NotStarted`] before `start()` and
    /// [`ZyreError::Stopped`] once the node has been stopped.
    fn ensure_running(&self) -> Result<(), ZyreError> {
        match self.state.get() {
            State::Running => Ok(()),
            State::Created => Err(ZyreError::NotStarted),
            State::Draining | State::Done => Err(ZyreError::Stopped),
        }
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
}

impl IZyreNode for ZyreNode {
    /// Start the node, beginning network discovery and messaging.
    fn start(&mut self) -> Result<(), ZyreError> {
        if self.state.get() != State::Created {
            return Err(ZyreError::StartFailed("node already started".into()));
        }
        let rc = unsafe { ffi::zyre_start(self.ptr) };
        if rc != 0 {
            return Err(ZyreError::StartFailed("zyre_start returned error".into()));
        }
        self.state.set(State::Running);
        Ok(())
    }

    /// Stop the node, signaling departure to peers.
    fn stop(&mut self) {
        // Only a running node can be stopped. `zyre_stop` blocks until the
        // actor has queued its final `["STOP", uuid, name]` sentinel on the
        // inbox, so after this returns the node is drainable, not gone — the
        // sentinel (and any events ahead of it) is still readable via recv().
        if self.state.get() == State::Running {
            unsafe { ffi::zyre_stop(self.ptr) };
            self.state.set(State::Draining);
        }
    }

    /// Join a named group.
    fn join(&mut self, group: &str) -> Result<(), ZyreError> {
        self.ensure_running()?;
        let group_c = std::ffi::CString::new(group)
            .map_err(|_| ZyreError::InvalidConfig("group name contains null byte".into()))?;
        unsafe { ffi::zyre_join(self.ptr, group_c.as_ptr()) };
        Ok(())
    }

    /// Leave a named group.
    fn leave(&mut self, group: &str) -> Result<(), ZyreError> {
        self.ensure_running()?;
        let group_c = std::ffi::CString::new(group)
            .map_err(|_| ZyreError::InvalidConfig("group name contains null byte".into()))?;
        unsafe { ffi::zyre_leave(self.ptr, group_c.as_ptr()) };
        Ok(())
    }

    /// Send a message to all peers in a group.
    fn shout(&self, group: &str, data: &[u8]) -> Result<(), ZyreError> {
        self.ensure_running()?;
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
    fn whisper(&self, peer: &PeerId, data: &[u8]) -> Result<(), ZyreError> {
        self.ensure_running()?;
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
    fn recv(&self) -> Result<ZyreEvent, ZyreError> {
        match self.state.get() {
            State::Created => return Err(ZyreError::NotStarted),
            // The `Stop` sentinel has already been delivered and the actor has
            // exited; another `zyre_recv` on the producerless inbox would block
            // forever, so report the terminal state instead.
            State::Done => return Err(ZyreError::Stopped),
            State::Running | State::Draining => {}
        }
        let event_ptr = unsafe { ffi::zyre_event_new(self.ptr) };
        if event_ptr.is_null() {
            return Err(ZyreError::RecvFailed);
        }
        let event = parse_event(event_ptr);
        unsafe { ffi::zyre_event_destroy(&mut (event_ptr as *mut _)) };
        // `Stop` is the terminal end-of-stream sentinel: after it, stop reading.
        if matches!(event, Ok(ZyreEvent::Stop)) {
            self.state.set(State::Done);
        }
        event
    }

    /// Try to receive an event without blocking.
    ///
    /// Returns `Ok(None)` if no event is available.
    fn try_recv(&self) -> Result<Option<ZyreEvent>, ZyreError> {
        match self.state.get() {
            State::Created => return Err(ZyreError::NotStarted),
            State::Done => return Ok(None),
            State::Running | State::Draining => {}
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
    fn uuid(&self) -> PeerId {
        let uuid_ptr = unsafe { ffi::zyre_uuid(self.ptr) };
        let uuid_str = unsafe { std::ffi::CStr::from_ptr(uuid_ptr) }
            .to_string_lossy()
            .into_owned();
        PeerId::new(uuid_str)
    }

    /// Get this node's name.
    fn name(&self) -> String {
        let name_ptr = unsafe { ffi::zyre_name(self.ptr) };
        unsafe { std::ffi::CStr::from_ptr(name_ptr) }
            .to_string_lossy()
            .into_owned()
    }

    /// Get the list of all known peers (by UUID).
    fn peers(&self) -> Vec<PeerId> {
        unsafe {
            let list = ffi::zyre_peers(self.ptr);
            let result = zlist_to_peer_ids(list);
            ffi::zlist_destroy(&mut (list as *mut _));
            result
        }
    }

    /// Get peers that belong to a specific group.
    fn peers_by_group(&self, group: &str) -> Vec<PeerId> {
        let group_c = std::ffi::CString::new(group).unwrap_or_default();
        unsafe {
            let list = ffi::zyre_peers_by_group(self.ptr, group_c.as_ptr());
            let result = zlist_to_peer_ids(list);
            ffi::zlist_destroy(&mut (list as *mut _));
            result
        }
    }

    /// Get the list of groups this node has joined.
    fn own_groups(&self) -> Vec<String> {
        unsafe {
            let list = ffi::zyre_own_groups(self.ptr);
            let result = zlist_to_strings(list);
            ffi::zlist_destroy(&mut (list as *mut _));
            result
        }
    }

    /// Get all groups known to this node (from all peers).
    fn peer_groups(&self) -> Vec<String> {
        unsafe {
            let list = ffi::zyre_peer_groups(self.ptr);
            let result = zlist_to_strings(list);
            ffi::zlist_destroy(&mut (list as *mut _));
            result
        }
    }

    /// Get the network address of a peer.
    fn peer_address(&self, peer: &PeerId) -> Option<String> {
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
    fn peer_header_value(&self, peer: &PeerId, key: &str) -> Option<String> {
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

    #[test]
    fn node_rejects_invalid_config() {
        let mut config = NodeConfig::default();
        config.name = Some(String::new());
        assert!(matches!(
            ZyreNode::new(config),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn node_rejects_zero_evasive_timeout() {
        let mut config = NodeConfig::default();
        config.evasive_timeout_ms = 0;
        assert!(matches!(
            ZyreNode::new(config),
            Err(ZyreError::InvalidConfig(_))
        ));
    }

    #[test]
    fn recv_before_start_is_not_started() {
        // A freshly created (un-started) node is in `Created`, so recv/try_recv
        // report `NotStarted` without touching the network.
        let node = ZyreNode::new(NodeConfig::default()).expect("create node");
        assert!(matches!(node.recv(), Err(ZyreError::NotStarted)));
        assert!(matches!(node.try_recv(), Err(ZyreError::NotStarted)));
    }
}
