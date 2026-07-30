//! Single-host RDMA loopback integration test (hardware-gated, `#[ignore]`d).
//!
//! This exercises the real outbound data path end-to-end on one machine:
//! `ConnectionTable::push` → `RealTransport::connect` → `rdma::client_connect`
//! → register the pool as an MR → a window of `post_write_from_pool` calls
//! followed by one `reap`. Because an RDMA write
//! is one-sided and needs the destination `rkey` at write time, the responder
//! (accept) side must register its buffer and publish the `rkey` *before* the
//! initiator connects. The accept side normally belongs to the `remote-lookup`
//! component; here a minimal `rdma_cm` responder is stood up as **test-only
//! scaffolding** so this crate's outbound path can be verified against a real
//! peer without a second host.
//!
//! # Running
//!
//! ```bash
//! cargo test -p remote-lookup-rdma-initiator -- --ignored loopback
//! # Optionally pin the local RoCE IP (otherwise auto-detected):
//! CERTUS_RDMA_TEST_IP=<roce-ip> cargo test -p remote-lookup-rdma-initiator -- --ignored
//! ```
//!
//! Requires an active RDMA device with a routable IPv4 (RoCE or IB). The device
//! is chosen implicitly by `rdma_cm` from that IP's route — exactly as the
//! production path does — so no device is opened by name.
//!
//! # Limitations
//!
//! The responder's CM event waits are blocking; on the happy path the initiator's
//! connect unblocks `accept`. If setup fails after the `rkey` is published, the
//! initiator may block waiting for an established connection — acceptable for a
//! manually-run `#[ignore]` test.

use std::ffi::{c_void, CString};
use std::os::raw::c_int;
use std::ptr;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;

use interfaces::{ILogger, PushStatus};

use crate::connection::{ConnectionTable, ItemPlan, RealTransport};
use crate::ffi;
use crate::telemetry::TelemetryCollector;

// These `rdma_cm` accept-side symbols were removed from the production FFI (the
// accept side lives in `remote-lookup`). They are re-declared here, test-only,
// purely to stand up a local responder. librdmacm is already linked by build.rs.
extern "C" {
    fn rdma_bind_addr(id: *mut ffi::rdma_cm_id, addr: *mut ffi::sockaddr) -> c_int;
    fn rdma_listen(id: *mut ffi::rdma_cm_id, backlog: c_int) -> c_int;
    fn rdma_accept(id: *mut ffi::rdma_cm_id, conn_param: *mut ffi::rdma_conn_param) -> c_int;
}

const TEST_PORT: u16 = 18515;
const PAYLOAD_LEN: usize = 4096;
/// Number of separate writes the payload is split into, so the push under test is
/// a real window rather than a single request. Must divide `PAYLOAD_LEN`.
const WRITES: usize = 8;
/// The initiator's zyre PeerId, stamped into the connect `private_data` and
/// asserted on the accept side (D2 identity correlation).
const INITIATOR_UUID: &str = "uuid-initiator-loopback";

/// Discards all log output.
struct NullLogger;
impl ILogger for NullLogger {
    fn info(&self, _msg: &str) {}
    fn warn(&self, _msg: &str) {}
    fn error(&self, _msg: &str) {}
    fn debug(&self, _msg: &str) {}
}

/// Find an active RDMA device's IPv4, honoring a `CERTUS_RDMA_TEST_IP` override.
///
/// Walks `/sys/class/infiniband/*/ports/*/state` for an `ACTIVE` port, maps the
/// device to its netdev, and reads that netdev's IPv4 via `ip`.
fn detect_roce_ipv4() -> Option<String> {
    if let Ok(ip) = std::env::var("CERTUS_RDMA_TEST_IP") {
        if !ip.trim().is_empty() {
            return Some(ip.trim().to_string());
        }
    }
    let devs = std::fs::read_dir("/sys/class/infiniband").ok()?;
    for dev in devs.flatten() {
        let dev_path = dev.path();
        let Ok(ports) = std::fs::read_dir(dev_path.join("ports")) else {
            continue;
        };
        for port in ports.flatten() {
            let state = std::fs::read_to_string(port.path().join("state")).unwrap_or_default();
            // e.g. "4: ACTIVE"
            if !state.trim_start().starts_with('4') {
                continue;
            }
            let Ok(mut nets) = std::fs::read_dir(dev_path.join("device/net")) else {
                continue;
            };
            let Some(net) = nets
                .next()
                .and_then(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
            else {
                continue;
            };
            if let Some(ip) = ipv4_of(&net) {
                return Some(ip);
            }
        }
    }
    None
}

/// Return the first IPv4 assigned to `netdev`, via `ip -o -4 addr show dev`.
fn ipv4_of(netdev: &str) -> Option<String> {
    let out = std::process::Command::new("ip")
        .args(["-o", "-4", "addr", "show", "dev", netdev])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut toks = line.split_whitespace();
        while let Some(t) = toks.next() {
            if t == "inet" {
                if let Some(cidr) = toks.next() {
                    return cidr.split('/').next().map(|s| s.to_string());
                }
            }
        }
    }
    None
}

/// RDMA resources owned by the responder thread, torn down together.
struct ResponderResources {
    channel: *mut ffi::rdma_event_channel,
    listen_id: *mut ffi::rdma_cm_id,
    child: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
    mr: *mut ffi::ibv_mr,
}

impl ResponderResources {
    /// Release every RDMA resource. Called after the initiator's write completes.
    ///
    /// # Safety
    ///
    /// All pointers were allocated by rdma-core / ibverbs and are released
    /// exactly once here; the responder thread owns them for its whole lifetime.
    unsafe fn destroy(self) {
        ffi::rdma_disconnect(self.child);
        ffi::rdma_destroy_qp(self.child);
        ffi::ibv_dereg_mr(self.mr);
        ffi::ibv_destroy_cq(self.cq);
        ffi::ibv_dealloc_pd(self.pd);
        ffi::rdma_destroy_id(self.child);
        ffi::rdma_destroy_id(self.listen_id);
        ffi::rdma_destroy_event_channel(self.channel);
    }
}

/// Block for one CM event of the expected type, acking (and erroring) otherwise.
///
/// # Safety
///
/// `channel` must be a live event channel. On `Ok`, the returned event is still
/// unacked and must be acked by the caller.
unsafe fn wait_event(
    channel: *mut ffi::rdma_event_channel,
    expected: c_int,
) -> Result<*mut ffi::rdma_cm_event, String> {
    let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
    if ffi::rdma_get_cm_event(channel, &mut event) != 0 {
        return Err("rdma_get_cm_event failed".into());
    }
    let got = (*event).event;
    if got != expected {
        ffi::rdma_ack_cm_event(event);
        return Err(format!(
            "unexpected CM event: got {got}, expected {expected}"
        ));
    }
    Ok(event)
}

/// Stand up a local responder: bind/listen on `ip`, pre-register the destination
/// buffer, publish its `rkey`, accept one connection, and hold everything open
/// until signalled. Runs on its own thread (raw pointers stay off the main one).
fn run_responder(
    ip: String,
    dst_addr: usize,
    dst_len: usize,
    rkey_tx: mpsc::Sender<Result<u32, String>>,
    done_rx: mpsc::Receiver<()>,
) {
    // SAFETY: each rdma-core/ibverbs call is checked for the documented failure
    // return (null pointer or non-zero status) before its result is used.
    let setup = || -> Result<ResponderResources, String> {
        unsafe {
            let channel = ffi::rdma_create_event_channel();
            if channel.is_null() {
                return Err("rdma_create_event_channel failed".into());
            }
            let mut listen_id: *mut ffi::rdma_cm_id = ptr::null_mut();
            if ffi::rdma_create_id(channel, &mut listen_id, ptr::null_mut(), ffi::RDMA_PS_TCP) != 0
            {
                return Err("rdma_create_id failed".into());
            }

            let ip_c = CString::new(ip.as_str()).map_err(|_| "invalid IP string".to_string())?;
            let mut sin = ffi::sockaddr_in {
                sin_family: ffi::AF_INET,
                sin_port: ffi::htons(TEST_PORT),
                sin_addr: ffi::in_addr {
                    s_addr: ffi::inet_addr(ip_c.as_ptr()),
                },
                sin_zero: [0; 8],
            };
            if rdma_bind_addr(
                listen_id,
                &mut sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            ) != 0
            {
                return Err("rdma_bind_addr failed".into());
            }
            if rdma_listen(listen_id, 1) != 0 {
                return Err("rdma_listen failed".into());
            }

            // Binding to a specific IP associates the listener with its device, so
            // the verbs context is available for pre-registration before any
            // connection exists.
            let ctx = (*listen_id).verbs;
            if ctx.is_null() {
                return Err("listen_id.verbs is null after bind; cannot pre-register".into());
            }
            let pd = ffi::ibv_alloc_pd(ctx);
            if pd.is_null() {
                return Err("ibv_alloc_pd failed".into());
            }
            let access = ffi::IBV_ACCESS_LOCAL_WRITE | ffi::IBV_ACCESS_REMOTE_WRITE;
            let mr = ffi::ibv_reg_mr(pd, dst_addr as *mut c_void, dst_len, access);
            if mr.is_null() {
                return Err("ibv_reg_mr(dst) failed".into());
            }
            // Publish the rkey so the initiator can build its RemoteRegion. The
            // subsequent accept unblocks once that initiator connects.
            let rkey = (*mr).rkey;
            rkey_tx
                .send(Ok(rkey))
                .map_err(|_| "main thread gone".to_string())?;

            let event = wait_event(channel, ffi::RDMA_CM_EVENT_CONNECT_REQUEST)?;
            let child = (*event).id;
            // Verify the initiator stamped its PeerId into the connect private_data
            // (D2): read it before acking (the ack frees the event).
            let param = &(*event).param.conn;
            let pd_len = param.private_data_len as usize;
            let stamped: Vec<u8> = if pd_len == 0 || param.private_data.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(param.private_data as *const u8, pd_len).to_vec()
            };
            ffi::rdma_ack_cm_event(event);
            let stamped = std::str::from_utf8(&stamped)
                .ok()
                .map(|s| s.trim_matches('\0').trim().to_string());
            assert_eq!(
                stamped.as_deref(),
                Some(INITIATOR_UUID),
                "initiator must stamp its PeerId into the connect private_data"
            );

            // The child connection must share the listener's device context for
            // the pre-registered MR's rkey to be valid on its queue pair.
            if (*child).verbs != ctx {
                return Err("child cm_id verbs differs from listener context".into());
            }

            let cq = ffi::ibv_create_cq(ctx, 16, ptr::null_mut(), ptr::null_mut(), 0);
            if cq.is_null() {
                return Err("ibv_create_cq failed".into());
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
            if ffi::rdma_create_qp(child, pd, &mut init) != 0 {
                return Err("rdma_create_qp failed".into());
            }

            let mut conn_param = ffi::rdma_conn_param {
                private_data: ptr::null(),
                private_data_len: 0,
                responder_resources: 1,
                initiator_depth: 1,
                flow_control: 0,
                retry_count: 7,
                rnr_retry_count: 7,
                srq: 0,
                qp_num: 0,
            };
            if rdma_accept(child, &mut conn_param) != 0 {
                return Err("rdma_accept failed".into());
            }
            let event = wait_event(channel, ffi::RDMA_CM_EVENT_ESTABLISHED)?;
            ffi::rdma_ack_cm_event(event);

            Ok(ResponderResources {
                channel,
                listen_id,
                child,
                pd,
                cq,
                mr,
            })
        }
    };

    let resources = match setup() {
        Ok(r) => r,
        Err(e) => {
            let _ = rkey_tx.send(Err(e));
            return;
        }
    };

    // Keep the connection and MR alive until the initiator confirms the write.
    let _ = done_rx.recv();
    // SAFETY: resources were allocated above and are dropped exactly once here.
    unsafe { resources.destroy() };
}

#[test]
#[ignore = "requires an active RoCE/IB device; run with --ignored"]
fn loopback_push_writes_into_remote_buffer() {
    let Some(ip) = detect_roce_ipv4() else {
        eprintln!("no active RDMA IPv4 found (set CERTUS_RDMA_TEST_IP); skipping loopback test");
        return;
    };
    eprintln!("loopback test using RDMA IP {ip}:{TEST_PORT}");

    // Source (buffer A): a known pattern registered as the connection's pool MR.
    let src: Vec<u8> = (0..PAYLOAD_LEN).map(|i| (i % 251) as u8).collect();
    // Destination (buffer B): zeroed; the responder registers it for remote write.
    let mut dst: Vec<u8> = vec![0u8; PAYLOAD_LEN];
    let dst_addr = dst.as_mut_ptr() as usize;

    let (rkey_tx, rkey_rx) = mpsc::channel::<Result<u32, String>>();
    let (done_tx, done_rx) = mpsc::channel::<()>();

    let responder = {
        let ip = ip.clone();
        thread::spawn(move || run_responder(ip, dst_addr, PAYLOAD_LEN, rkey_tx, done_rx))
    };

    let rkey = match rkey_rx.recv() {
        Ok(Ok(k)) => k,
        Ok(Err(e)) => panic!("responder setup failed: {e}"),
        Err(_) => panic!("responder thread died before publishing rkey"),
    };

    // Drive the real outbound path at the ConnectionTable/RealTransport level.
    let table = ConnectionTable::new(
        Arc::new(RealTransport::new(
            src.as_ptr() as *mut u8,
            src.len(),
            INITIATOR_UUID.as_bytes().to_vec(),
        )),
        Arc::new(TelemetryCollector::new()),
        Arc::new(NullLogger),
    );
    let endpoint = format!("{ip}:{TEST_PORT}");
    // Split the payload into WRITES pieces and push them as one window, so this
    // exercises a genuinely pipelined push (many requests posted before any
    // completion is reaped) rather than the degenerate window of one. Each piece
    // has its own local offset and remote address, so a window that mismatched
    // sources to destinations would corrupt the comparison below rather than
    // silently pass.
    let piece = PAYLOAD_LEN / WRITES;
    let resolved: Vec<ItemPlan> = (0..WRITES)
        .map(|i| ItemPlan::Write {
            // SAFETY-adjacent: offset stays inside `src`, which is the registered
            // pool MR for this connection.
            local: unsafe { src.as_ptr().add(i * piece) },
            len: piece,
            remote_addr: (dst_addr + i * piece) as u64,
            rkey,
        })
        .collect();
    let statuses = table
        .push(&endpoint, resolved)
        .expect("push returns statuses");
    assert_eq!(
        statuses,
        vec![PushStatus::Success; WRITES],
        "every RDMA write in the window should report Success"
    );

    // The initiator's signaled completions guarantee every piece reached buffer B.
    assert_eq!(
        dst, src,
        "destination buffer must equal source after the RDMA window"
    );

    let _ = done_tx.send(());
    responder.join().expect("responder thread panicked");

    eprintln!("loopback test OK: {PAYLOAD_LEN} bytes in {WRITES} writes, verified");
}
