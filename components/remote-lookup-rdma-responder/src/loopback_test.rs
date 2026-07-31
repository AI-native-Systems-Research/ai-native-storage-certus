//! Single-host RDMA loopback integration tests (hardware-gated, `#[ignore]`d).
//!
//! These exercise the **real** `rdma_cm` accept path end-to-end on one machine:
//! `initialize()` → `RealCmSeam::bind` (real `rdma_bind_addr` port 0 +
//! `rdma_listen` + `rdma_get_src_port`) → a real inbound connect stamped with a
//! zyre UUID in `private_data` → `ConnectionEstablished { Some(peer) }` → a
//! `Disconnect` command → QP→ERROR teardown → `DisconnectAck`.
//!
//! A minimal `rdma_cm` **client** is stood up in-test (its connect-side calls are
//! declared here so the crate's `ffi` stays accept-only) to drive the connect.
//!
//! # Running
//!
//! ```bash
//! cargo test -p remote-lookup-rdma-responder -- --ignored loopback
//! # By default binds the first active RDMA device; pin a RoCE IP to override:
//! CERTUS_RDMA_TEST_IP=<roce-ip> cargo test -p remote-lookup-rdma-responder -- --ignored
//! ```
//!
//! Requires an active RDMA device with a routable IPv4 (RoCE/IB). The device is
//! chosen implicitly by `rdma_cm` from that IP's route — exactly as production.

use std::ffi::{c_void, CString};
use std::os::raw::c_int;
use std::ptr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use interfaces::{
    CacheKey, IMemoryTier, IRemoteLookupRdmaResponder, IRemoteLookupRdmaResponderAdmin,
    MemoryTierError, PeerId, ResponderCommand, ResponderEvent,
};

use crate::ffi;
use crate::RemoteLookupRdmaResponderComponent;

/// A minimal [`IMemoryTier`] that owns a heap pool and reports it via
/// [`pool_info`](IMemoryTier::pool_info). The responder only calls `pool_info()`
/// (to `ibv_reg_mr` the whole pool at `initialize`); every other method is an
/// unused stub. The pool memory is owned here, so it outlives the registered MR.
struct PoolMemoryTier {
    buf: Vec<u8>,
}

impl PoolMemoryTier {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0u8; len],
        }
    }
}

impl IMemoryTier for PoolMemoryTier {
    fn pool_info(&self) -> Option<(*mut u8, usize)> {
        Some((self.buf.as_ptr() as *mut u8, self.buf.len()))
    }

    // --- Unused stubs (never exercised by the responder's registrar path). ---
    fn initialize(
        &self,
        _pool_size: usize,
        _numa_node: Option<i32>,
    ) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn insert(&self, _key: CacheKey, _size: u32) -> Result<*mut u8, MemoryTierError> {
        Err(MemoryTierError::PoolFull)
    }
    fn get(&self, _key: CacheKey) -> Option<(*mut u8, u32)> {
        None
    }
    fn peek(&self, _key: CacheKey) -> Option<(*mut u8, u32)> {
        None
    }
    fn evict_next(&self) -> Option<CacheKey> {
        None
    }
    fn evict_next_for_key(&self, _key: CacheKey) -> Option<CacheKey> {
        None
    }
    fn oldest_keys(&self, _n: usize) -> Vec<CacheKey> {
        Vec::new()
    }
    fn remove(&self, _key: CacheKey) -> Result<(), MemoryTierError> {
        Ok(())
    }
    fn touch(&self, _key: CacheKey) {}
    fn batch_touch(&self, _keys: &[CacheKey]) {}
    fn contains(&self, _key: CacheKey) -> bool {
        false
    }
    fn capacity(&self) -> usize {
        self.buf.len()
    }
    fn used(&self) -> usize {
        0
    }
    fn is_dma_capable(&self) -> bool {
        false
    }
    fn clear(&self) -> Result<usize, MemoryTierError> {
        Ok(0)
    }
    fn telemetry_snapshot(&self) -> interfaces::MemoryTierTelemetrySnapshot {
        interfaces::MemoryTierTelemetrySnapshot::default()
    }
}

/// Build a responder with a heap pool bound to its `memory_tier` receptacle and
/// the test bind IP set, ready for `initialize()` to register the pool.
fn pooled_responder(pool_len: usize) -> Arc<RemoteLookupRdmaResponderComponent> {
    let comp = RemoteLookupRdmaResponderComponent::new_default();
    let mt: Arc<dyn IMemoryTier + Send + Sync> = Arc::new(PoolMemoryTier::new(pool_len));
    comp.memory_tier
        .connect(mt)
        .expect("connect memory_tier receptacle");
    comp.set_bind_ip(test_ip());
    comp
}

// Client-side CM event types (only needed by the in-test client).
const RDMA_CM_EVENT_ADDR_RESOLVED: c_int = 0;
const RDMA_CM_EVENT_ROUTE_RESOLVED: c_int = 2;
const RDMA_CM_EVENT_ESTABLISHED: c_int = 9;

// Connect-side rdma_cm calls, declared locally so `crate::ffi` stays accept-only.
extern "C" {
    fn rdma_resolve_addr(
        id: *mut ffi::rdma_cm_id,
        src_addr: *mut ffi::sockaddr,
        dst_addr: *mut ffi::sockaddr,
        timeout_ms: c_int,
    ) -> c_int;
    fn rdma_resolve_route(id: *mut ffi::rdma_cm_id, timeout_ms: c_int) -> c_int;
    fn rdma_connect(id: *mut ffi::rdma_cm_id, conn_param: *mut ffi::rdma_conn_param) -> c_int;
}

fn test_ip() -> String {
    // Override with CERTUS_RDMA_TEST_IP; otherwise use the first active RDMA
    // device (same discovery the responder binds by default).
    std::env::var("CERTUS_RDMA_TEST_IP")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            crate::rdma::first_active_rdma_ipv4()
                .expect("no CERTUS_RDMA_TEST_IP set and no active RDMA device found")
        })
}

/// Block for one CM event of `expected` type on `channel`, acking it.
///
/// # Safety
/// `channel` must be a live event channel.
unsafe fn wait_event(channel: *mut ffi::rdma_event_channel, expected: c_int) -> Result<(), String> {
    let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
    if ffi::rdma_get_cm_event(channel, &mut event) != 0 {
        return Err("rdma_get_cm_event failed".into());
    }
    let got = (*event).event;
    ffi::rdma_ack_cm_event(event);
    if got != expected {
        return Err(format!(
            "unexpected CM event: got {got}, expected {expected}"
        ));
    }
    Ok(())
}

/// Live client-side RDMA resources, torn down together.
struct Client {
    channel: *mut ffi::rdma_event_channel,
    id: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
}

// SAFETY: the client is created on one thread and handed to the test thread via
// a join; only one thread ever touches these pointers at a time.
unsafe impl Send for Client {}

impl Client {
    /// Connect to `ip:port`, stamping `uuid` into the connect `private_data`.
    ///
    /// # Safety
    /// Must run on a thread that owns these raw resources exclusively.
    unsafe fn connect(ip: &str, port: u16, uuid: &[u8]) -> Result<Client, String> {
        let channel = ffi::rdma_create_event_channel();
        if channel.is_null() {
            return Err("client rdma_create_event_channel failed".into());
        }
        let mut id: *mut ffi::rdma_cm_id = ptr::null_mut();
        if ffi::rdma_create_id(channel, &mut id, ptr::null_mut(), ffi::RDMA_PS_TCP) != 0 {
            return Err("client rdma_create_id failed".into());
        }

        let ip_c = CString::new(ip).map_err(|_| "bad ip".to_string())?;
        let mut dst = ffi::sockaddr_in {
            sin_family: ffi::AF_INET,
            sin_port: ffi::htons(port),
            sin_addr: ffi::in_addr {
                s_addr: ffi::inet_addr(ip_c.as_ptr()),
            },
            sin_zero: [0; 8],
        };
        let mut src = ffi::sockaddr_in {
            sin_family: ffi::AF_INET,
            sin_port: 0,
            sin_addr: ffi::in_addr { s_addr: 0 },
            sin_zero: [0; 8],
        };
        if rdma_resolve_addr(
            id,
            &mut src as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            &mut dst as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            2000,
        ) != 0
        {
            return Err("rdma_resolve_addr failed".into());
        }
        wait_event(channel, RDMA_CM_EVENT_ADDR_RESOLVED)?;
        if rdma_resolve_route(id, 2000) != 0 {
            return Err("rdma_resolve_route failed".into());
        }
        wait_event(channel, RDMA_CM_EVENT_ROUTE_RESOLVED)?;

        let ctx = (*id).verbs;
        if ctx.is_null() {
            return Err("client verbs null".into());
        }
        let pd = ffi::ibv_alloc_pd(ctx);
        if pd.is_null() {
            return Err("client ibv_alloc_pd failed".into());
        }
        let cq = ffi::ibv_create_cq(ctx, 16, ptr::null_mut(), ptr::null_mut(), 0);
        if cq.is_null() {
            return Err("client ibv_create_cq failed".into());
        }
        let mut init = ffi::ibv_qp_init_attr {
            qp_context: ptr::null_mut(),
            send_cq: cq,
            recv_cq: cq,
            srq: ptr::null_mut(),
            cap: ffi::ibv_qp_cap {
                max_send_wr: 16,
                max_recv_wr: 16,
                max_send_sge: 1,
                max_recv_sge: 1,
                max_inline_data: 0,
            },
            qp_type: ffi::IBV_QPT_RC,
            sq_sig_all: 0,
        };
        if ffi::rdma_create_qp(id, pd, &mut init) != 0 {
            return Err("client rdma_create_qp failed".into());
        }

        let mut conn_param = ffi::rdma_conn_param {
            private_data: uuid.as_ptr() as *const c_void,
            private_data_len: uuid.len() as u8,
            responder_resources: 1,
            initiator_depth: 1,
            flow_control: 0,
            retry_count: 7,
            rnr_retry_count: 7,
            srq: 0,
            qp_num: 0,
        };
        if rdma_connect(id, &mut conn_param) != 0 {
            return Err("rdma_connect failed".into());
        }
        wait_event(channel, RDMA_CM_EVENT_ESTABLISHED)?;
        Ok(Client {
            channel,
            id,
            pd,
            cq,
        })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: pointers created in `connect`, released once here.
        unsafe {
            if !self.id.is_null() {
                ffi::rdma_disconnect(self.id);
                ffi::rdma_destroy_qp(self.id);
                ffi::rdma_destroy_id(self.id);
            }
            if !self.cq.is_null() {
                ffi::ibv_destroy_cq(self.cq);
            }
            if !self.pd.is_null() {
                ffi::ibv_dealloc_pd(self.pd);
            }
            if !self.channel.is_null() {
                ffi::rdma_destroy_event_channel(self.channel);
            }
        }
    }
}

#[test]
#[ignore = "requires an RDMA NIC"]
fn loopback_bind_reports_ephemeral_port() {
    // SC-001: after initialize(), local_endpoint() reports the bound IP and a
    // non-placeholder (OS-assigned) ephemeral port.
    let comp = pooled_responder(1 << 20);
    comp.initialize().expect("real bind/listen on the NIC");
    let ep = comp.local_endpoint().expect("endpoint");
    assert_eq!(ep.ip, test_ip());
    assert_ne!(ep.port, 0, "ephemeral port must be OS-assigned, not 0");
    comp.shutdown().expect("shutdown");
}

#[test]
#[ignore = "requires an RDMA NIC"]
fn loopback_two_listeners_get_distinct_ports() {
    // SC-004: two co-resident instances on one NIC bind distinct ephemeral ports.
    let a = pooled_responder(1 << 20);
    let b = pooled_responder(1 << 20);
    a.initialize().expect("bind a");
    b.initialize().expect("bind b");
    let pa = a.local_endpoint().unwrap().port;
    let pb = b.local_endpoint().unwrap().port;
    assert_ne!(pa, 0);
    assert_ne!(pb, 0);
    assert_ne!(pa, pb, "co-resident listeners must not collide");
    a.shutdown().expect("shutdown a");
    b.shutdown().expect("shutdown b");
}

#[test]
#[ignore = "requires an RDMA NIC"]
fn loopback_connect_correlates_uuid_then_teardown() {
    // SC-005 + SC-002 + FR-008 on real hardware: a stamped connect correlates to
    // Some(peer); a Disconnect drives QP→ERROR before the single DisconnectAck.
    let uuid = "uuid-loopback-01";
    let comp = pooled_responder(1 << 20);
    comp.initialize().expect("bind/listen");
    let ch = comp.open_control_channel().expect("open control channel");
    let ep = comp.local_endpoint().expect("endpoint");

    // Drive a real inbound connect from a client thread, stamped with the UUID.
    let ip = ep.ip.clone();
    let port = ep.port;
    let uuid_bytes = uuid.as_bytes().to_vec();
    let client = thread::spawn(move || {
        // SAFETY: this thread exclusively owns the client resources it creates.
        unsafe { Client::connect(&ip, port, &uuid_bytes) }
    });

    // The responder accepts and reports the correlated peer.
    match ch.event_rx.recv().expect("connection event") {
        ResponderEvent::ConnectionEstablished { node } => {
            assert_eq!(
                node,
                Some(PeerId::new(uuid)),
                "UUID must correlate to Some(peer)"
            );
        }
        other => panic!("expected ConnectionEstablished, got {other:?}"),
    }
    let client = client
        .join()
        .expect("client thread")
        .expect("client connect");

    // Tear down: Disconnect → QP→ERROR (asserted, inside disconnect) → one ack.
    ch.command_tx
        .send(ResponderCommand::Disconnect {
            node: PeerId::new(uuid),
        })
        .expect("send disconnect");
    match ch.event_rx.recv().expect("ack") {
        ResponderEvent::DisconnectAck { node } => assert_eq!(node, PeerId::new(uuid)),
        other => panic!("expected DisconnectAck, got {other:?}"),
    }

    // Give the peer a moment to observe the disconnect, then tear everything down.
    thread::sleep(Duration::from_millis(50));
    drop(client);
    drop(ch);
    comp.shutdown().expect("shutdown");
}

#[test]
#[ignore = "requires an RDMA NIC"]
fn loopback_registers_pool_and_exposes_rkey() {
    // The responder is the registrar: initialize() registers the whole bound
    // memory-tier pool with ibv_reg_mr and local_region() exposes a real,
    // non-zero rkey spanning the whole pool.
    const POOL: usize = 1 << 20; // 1 MiB
    let comp = pooled_responder(POOL);
    comp.initialize().expect("bind + register the pool");

    let region = comp.local_region().expect("local_region after init");
    assert_ne!(region.rkey, 0, "a real ibv_reg_mr yields a non-zero rkey");
    assert_eq!(region.length, POOL, "the region spans the whole pool");
    assert_ne!(region.addr, 0, "the region carries the pool base address");

    comp.shutdown().expect("shutdown");
}
