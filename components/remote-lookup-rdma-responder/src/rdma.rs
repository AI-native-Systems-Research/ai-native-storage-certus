//! Production `rdma_cm` accept path — the real implementation of the
//! [`CmListener`]/[`CmConnection`] seam.
//!
//! [`RealCmSeam::bind`] binds an ephemeral port on the supplied RoCE IPv4,
//! `rdma_listen`s, and reads the assigned port via `rdma_get_src_port`. Its
//! [`next_events`](CmListener::next_events) blocks in `epoll` over
//! `{cm channel fd, command eventfd, stop eventfd}` so `Disconnect` commands and
//! stop are serviced promptly and never block behind a pending accept (FR-004).
//! Commands arrive over the SPSC control channel, which has no pollable fd, so a
//! small **bridge thread** drains them into a queue and signals the command
//! eventfd.
//!
//! On `CONNECT_REQUEST` the loop reads the zyre UUID from the connect
//! `private_data`, creates the child queue pair, `rdma_accept`s, and surfaces a
//! [`CmEvent::ConnectRequest`]. [`RealCmConn::to_error`] drives that QP into the
//! ERROR state (asserted — fail-stop on a fatal fault); its `Drop` disconnects
//! and destroys the queue pair (best-effort).

use std::collections::VecDeque;
use std::ffi::CString;
use std::os::raw::{c_int, c_uint};
use std::ptr;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use component_core::channel::Receiver;
use interfaces::{Endpoint, LocalRegion, ResponderCommand};

use crate::connection::{CmConnection, CmEvent, CmListener};
use crate::ffi;

/// Listener backlog for `rdma_listen`.
const LISTEN_BACKLOG: c_int = 16;
/// Max epoll events drained per wake.
const MAX_EPOLL_EVENTS: usize = 8;

// epoll data tags identifying which fd woke us.
const TAG_CM: u64 = 1;
const TAG_CMD: u64 = 2;
const TAG_STOP: u64 = 3;
const TAG_ASYNC: u64 = 4;

/// Human-readable name for an `ibv_event_type` (device async events). Only the
/// events worth acting on during a transfer are named; the rest log as their
/// numeric code. QP_FATAL/QP_REQ_ERR/QP_ACCESS_ERR on a child QP are the
/// responder-side signature of an initiator transport `RETRY_EXC`.
fn async_event_name(etype: c_int) -> &'static str {
    match etype {
        0 => "CQ_ERR",
        1 => "QP_FATAL",
        2 => "QP_REQ_ERR",
        3 => "QP_ACCESS_ERR",
        4 => "COMM_EST",
        5 => "SQ_DRAINED",
        6 => "PATH_MIG",
        7 => "PATH_MIG_ERR",
        8 => "DEVICE_FATAL",
        9 => "PORT_ACTIVE",
        10 => "PORT_ERR",
        11 => "LID_CHANGE",
        12 => "PKEY_CHANGE",
        13 => "SM_CHANGE",
        14 => "SRQ_ERR",
        15 => "SRQ_LIMIT_REACHED",
        16 => "QP_LAST_WQE_REACHED",
        17 => "CLIENT_REREGISTER",
        18 => "GID_CHANGE",
        _ => "UNKNOWN",
    }
}

/// Discover the IPv4 of the first RDMA device with an active port — the default
/// bind address when none is configured. The RoCE IPv4 is read from the port's
/// IPv4-mapped (RoCE v2) GID. Returns an error if no active device with an IPv4
/// GID is present.
pub(crate) fn first_active_rdma_ipv4() -> Result<String, String> {
    // SAFETY: every enumeration call is null/return checked before use; each
    // opened device is closed and the device list freed on all paths.
    unsafe {
        let mut num: c_int = 0;
        let list = ffi::ibv_get_device_list(&mut num);
        if list.is_null() || num <= 0 {
            return Err("no RDMA devices found".into());
        }
        let mut found: Option<String> = None;
        for i in 0..num as isize {
            let dev = *list.offset(i);
            if dev.is_null() {
                continue;
            }
            let ctx = ffi::ibv_open_device(dev);
            if ctx.is_null() {
                continue;
            }
            // Probe the common physical ports for an ACTIVE one carrying an
            // IPv4-mapped (RoCE v2) GID.
            'ports: for port in 1u8..=2 {
                let mut attr: ffi::ibv_port_attr = std::mem::zeroed();
                if ffi::ibv_query_port(ctx, port, &mut attr) != 0
                    || attr.state != ffi::IBV_PORT_ACTIVE
                {
                    continue;
                }
                for idx in 0..attr.gid_tbl_len.max(0) {
                    let mut gid: ffi::ibv_gid = std::mem::zeroed();
                    if ffi::ibv_query_gid(ctx, port, idx, &mut gid) != 0 {
                        continue;
                    }
                    let r = gid.raw;
                    // RoCE v2 IPv4-mapped GID: 0:0:0:0:0:ffff:a.b.c.d
                    let is_v4 = r[..10].iter().all(|b| *b == 0) && r[10] == 0xff && r[11] == 0xff;
                    if is_v4 {
                        let ip = format!("{}.{}.{}.{}", r[12], r[13], r[14], r[15]);
                        if ip != "0.0.0.0" {
                            found = Some(ip);
                            break 'ports;
                        }
                    }
                }
            }
            ffi::ibv_close_device(ctx);
            if found.is_some() {
                break;
            }
        }
        ffi::ibv_free_device_list(list);
        found.ok_or_else(|| "no active RDMA device with an IPv4 (RoCE v2) GID found".to_string())
    }
}

/// A live accepted connection's queue pair (real rdma-core resources).
pub struct RealCmConn {
    /// The child `rdma_cm_id` produced by the connect request.
    id: *mut ffi::rdma_cm_id,
    /// Its RC queue pair (owned by `id`; destroyed via `rdma_destroy_qp`).
    qp: *mut ffi::ibv_qp,
}

// SAFETY: a RealCmConn is created on the accept-loop thread and moved into the
// ConnectionTable, which lives on that same thread. The raw rdma-core pointers
// are only ever dereferenced from there; no two threads touch them concurrently.
unsafe impl Send for RealCmConn {}

impl CmConnection for RealCmConn {
    fn to_error(&self) {
        // SAFETY: `qp` is a valid queue pair for the life of this connection.
        let ret = unsafe { ffi::responder_qp_to_error(self.qp) };
        // The ERROR transition is legal from any QP state and fails only on a
        // fatal HCA/programming fault — fail-stop rather than proceed (FR-008).
        assert_eq!(ret, 0, "QP→ERROR transition failed (ibv_modify_qp={ret})");
    }
}

impl Drop for RealCmConn {
    fn drop(&mut self) {
        // Best-effort teardown after the ERROR transition (FR-008): failures are
        // not fatal. Ordering: disconnect, destroy QP, destroy id.
        // SAFETY: pointers were produced by rdma-core and are released once here.
        unsafe {
            if !self.id.is_null() {
                ffi::rdma_disconnect(self.id);
                if !self.qp.is_null() {
                    ffi::rdma_destroy_qp(self.id);
                }
                ffi::rdma_destroy_id(self.id);
            }
        }
    }
}

/// Shared queue the bridge thread fills from the SPSC command channel.
type CommandQueue = Arc<Mutex<VecDeque<ResponderCommand>>>;

/// Production CM listener: an `rdma_cm` listener multiplexed with the command
/// inbox and a stop signal via `epoll`.
pub struct RealCmSeam {
    epfd: c_int,
    cmd_eventfd: c_int,
    stop_eventfd: c_int,
    channel: *mut ffi::rdma_event_channel,
    listen_id: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
    /// The whole memory-tier pool registered once in `pd` with `REMOTE_WRITE`;
    /// deregistered in `Drop` before the PD is freed.
    mr: *mut ffi::ibv_mr,
    cmd_queue: CommandQueue,
    bridge: Option<JoinHandle<()>>,
}

// SAFETY: after `bind` returns, the seam is owned solely by the accept-loop
// thread (moved into `run_accept_loop`). The bridge thread only touches the
// `Send` fields it was handed (the command receiver, the command queue, and the
// copied eventfd), never these raw pointers.
unsafe impl Send for RealCmSeam {}

impl RealCmSeam {
    /// Bind an ephemeral port on `bind_ip`, `rdma_listen`, register the whole
    /// memory-tier pool `[pool_ptr, pool_ptr + pool_len)` in the listener's
    /// protection domain with `REMOTE_WRITE`, and set up the epoll multiplex.
    /// Returns the seam, the bound [`Endpoint`], the stop eventfd (which the actor
    /// writes to make the loop exit), and the pool-wide [`LocalRegion`] (base,
    /// `rkey`, length) for `remote-lookup` to advertise.
    ///
    /// The caller retains ownership of the pool memory; it must stay valid until
    /// this seam is dropped (which deregisters the MR). The bridge thread takes
    /// ownership of `command_rx` and exits when that channel closes.
    pub fn bind(
        bind_ip: &str,
        command_rx: Receiver<ResponderCommand>,
        pool_ptr: *mut u8,
        pool_len: usize,
    ) -> Result<(Self, Endpoint, c_int, LocalRegion), String> {
        // Default to the first active RDMA device when no bind IP is supplied.
        let resolved_ip = if bind_ip.trim().is_empty() {
            first_active_rdma_ipv4()?
        } else {
            bind_ip.to_string()
        };

        // SAFETY: every rdma-core / libc call below is checked for its documented
        // failure return before its result is used; pointers created here are
        // owned by the returned seam and released in its Drop.
        unsafe {
            let channel = ffi::rdma_create_event_channel();
            if channel.is_null() {
                return Err("rdma_create_event_channel failed".into());
            }

            let mut listen_id: *mut ffi::rdma_cm_id = ptr::null_mut();
            if ffi::rdma_create_id(channel, &mut listen_id, ptr::null_mut(), ffi::RDMA_PS_TCP) != 0
            {
                ffi::rdma_destroy_event_channel(channel);
                return Err("rdma_create_id failed".into());
            }

            let ip_c =
                CString::new(resolved_ip.as_str()).map_err(|_| "invalid bind IP".to_string())?;
            let mut sin = ffi::sockaddr_in {
                sin_family: ffi::AF_INET,
                sin_port: ffi::htons(0), // ephemeral: OS assigns the port
                sin_addr: ffi::in_addr {
                    s_addr: ffi::inet_addr(ip_c.as_ptr()),
                },
                sin_zero: [0; 8],
            };
            if ffi::rdma_bind_addr(
                listen_id,
                &mut sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            ) != 0
            {
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(channel);
                return Err(format!("rdma_bind_addr({resolved_ip}) failed"));
            }
            if ffi::rdma_listen(listen_id, LISTEN_BACKLOG) != 0 {
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(channel);
                return Err("rdma_listen failed".into());
            }

            // Ephemeral port, read back (network byte order → host).
            let port = ffi::ntohs(ffi::rdma_get_src_port(listen_id));
            if port == 0 {
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(channel);
                return Err("rdma_get_src_port returned 0".into());
            }

            // Binding to a specific IP associates the listener with its device,
            // so the verbs context is available now for the shared PD/CQ that
            // child queue pairs are created on.
            let ctx = (*listen_id).verbs;
            if ctx.is_null() {
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(channel);
                return Err("listen_id.verbs is null after bind".into());
            }
            let pd = ffi::ibv_alloc_pd(ctx);
            if pd.is_null() {
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(channel);
                return Err("ibv_alloc_pd failed".into());
            }
            let cq = ffi::ibv_create_cq(ctx, 16, ptr::null_mut(), ptr::null_mut(), 0);
            if cq.is_null() {
                ffi::ibv_dealloc_pd(pd);
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(channel);
                return Err("ibv_create_cq failed".into());
            }

            // Register the whole memory-tier pool once in this PD. A single
            // tier-wide MR means the NIC bounds-checks inbound one-sided writes at
            // tier granularity; the per-slot bound is enforced in software by
            // remote-lookup before it advertises each landing slot.
            let access = ffi::IBV_ACCESS_LOCAL_WRITE
                | ffi::IBV_ACCESS_REMOTE_WRITE
                | ffi::IBV_ACCESS_REMOTE_READ;
            let mr = ffi::ibv_reg_mr(pd, pool_ptr as *mut _, pool_len, access);
            if mr.is_null() {
                ffi::ibv_destroy_cq(cq);
                ffi::ibv_dealloc_pd(pd);
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(channel);
                return Err(format!(
                    "ibv_reg_mr failed for pool {pool_ptr:p} len {pool_len}"
                ));
            }
            let local_region = LocalRegion {
                addr: pool_ptr as u64,
                rkey: (*mr).rkey,
                length: pool_len,
            };

            // Make the CM event channel fd non-blocking so `rdma_get_cm_event`
            // returns EAGAIN (rather than blocking) once epoll has woken us.
            let cm_fd = (*channel).fd;
            let flags = ffi::fcntl(cm_fd, ffi::F_GETFL, 0);
            ffi::fcntl(cm_fd, ffi::F_SETFL, flags | ffi::O_NONBLOCK);

            let epfd = ffi::epoll_create1(ffi::EPOLL_CLOEXEC);
            let cmd_eventfd = ffi::eventfd(0, ffi::EFD_NONBLOCK | ffi::EFD_CLOEXEC);
            let stop_eventfd = ffi::eventfd(0, ffi::EFD_NONBLOCK | ffi::EFD_CLOEXEC);
            if epfd < 0 || cmd_eventfd < 0 || stop_eventfd < 0 {
                if epfd >= 0 {
                    ffi::close(epfd);
                }
                if cmd_eventfd >= 0 {
                    ffi::close(cmd_eventfd);
                }
                if stop_eventfd >= 0 {
                    ffi::close(stop_eventfd);
                }
                ffi::ibv_dereg_mr(mr);
                ffi::ibv_destroy_cq(cq);
                ffi::ibv_dealloc_pd(pd);
                ffi::rdma_destroy_id(listen_id);
                ffi::rdma_destroy_event_channel(channel);
                return Err("epoll/eventfd creation failed".into());
            }
            epoll_add(epfd, cm_fd, TAG_CM);
            epoll_add(epfd, cmd_eventfd, TAG_CMD);
            epoll_add(epfd, stop_eventfd, TAG_STOP);

            // Diagnostic: watch the device async-event fd so QP async errors on
            // child queue pairs (IBV_EVENT_QP_FATAL etc.) are observed the moment
            // the HCA raises them. The fd belongs to `ctx` and is NOT closed by
            // this seam (epoll drops it when `epfd` closes). Best-effort — if it
            // can't be made non-blocking we simply skip it.
            let async_fd = ffi::responder_async_fd(ctx);
            if async_fd >= 0 {
                let af = ffi::fcntl(async_fd, ffi::F_GETFL, 0);
                ffi::fcntl(async_fd, ffi::F_SETFL, af | ffi::O_NONBLOCK);
                epoll_add(epfd, async_fd, TAG_ASYNC);
            }

            // Bridge the SPSC command inbox (no fd) onto the command eventfd.
            let cmd_queue: CommandQueue = Arc::new(Mutex::new(VecDeque::new()));
            let bridge_queue = Arc::clone(&cmd_queue);
            let bridge = std::thread::Builder::new()
                .name("rdma-responder-cmd-bridge".into())
                .spawn(move || {
                    while let Ok(cmd) = command_rx.recv() {
                        bridge_queue
                            .lock()
                            .expect("cmd_queue poisoned")
                            .push_back(cmd);
                        // Signal the command eventfd so the accept loop's epoll wakes.
                        signal_eventfd(cmd_eventfd);
                    }
                })
                .map_err(|e| format!("bridge thread spawn failed: {e}"))?;

            let seam = RealCmSeam {
                epfd,
                cmd_eventfd,
                stop_eventfd,
                channel,
                listen_id,
                pd,
                cq,
                mr,
                cmd_queue,
                bridge: Some(bridge),
            };
            let endpoint = Endpoint {
                ip: resolved_ip.clone(),
                port,
            };
            Ok((seam, endpoint, stop_eventfd, local_region))
        }
    }

    /// Drain and process all currently-pending CM events (non-blocking).
    ///
    /// # Safety
    /// The listener/channel/PD/CQ pointers must be live (guaranteed for the
    /// seam's lifetime).
    unsafe fn drain_cm_events(&self) -> Vec<CmEvent> {
        let mut out = Vec::new();
        loop {
            let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
            if ffi::rdma_get_cm_event(self.channel, &mut event) != 0 {
                // EAGAIN → no more events queued; anything else → give up too.
                break;
            }
            let etype = (*event).event;
            match etype {
                ffi::RDMA_CM_EVENT_CONNECT_REQUEST => {
                    let child = (*event).id;
                    let private_data = read_private_data(&(*event).param.conn);
                    // Ack before accepting (rdma-core requires the event acked).
                    ffi::rdma_ack_cm_event(event);
                    match self.accept_child(child) {
                        Ok(qp) => out.push(CmEvent::ConnectRequest {
                            private_data,
                            conn: Box::new(RealCmConn { id: child, qp }),
                        }),
                        Err(message) => {
                            // Could not form the QP; reject and drop. This is a
                            // non-fatal accept-loop error (FR-016): reject the
                            // connect, then surface it so the accept loop emits
                            // `ResponderEvent::Error` and counts it.
                            ffi::rdma_reject(child, ptr::null(), 0);
                            out.push(CmEvent::AcceptError { message });
                        }
                    }
                }
                // ESTABLISHED / DISCONNECTED / others: ack and ignore — teardown is
                // command-driven, not peer-driven.
                _ => {
                    ffi::rdma_ack_cm_event(event);
                }
            }
        }
        out
    }

    /// Drain and log all pending device async events (diagnostic, non-blocking).
    /// A child QP raising `QP_FATAL`/`QP_REQ_ERR`/`QP_ACCESS_ERR` here is the
    /// responder-side signature of the initiator seeing a transport `RETRY_EXC`,
    /// so surfacing it pins the failure to this side of the connection.
    ///
    /// # Safety
    /// `self.listen_id` must be live (guaranteed for the seam's lifetime); its
    /// `verbs` context owns the async-event queue for every child QP.
    unsafe fn drain_async_events(&self) {
        let ctx = (*self.listen_id).verbs;
        if ctx.is_null() {
            return;
        }
        loop {
            let mut qp_num: c_uint = 0;
            let etype = ffi::responder_drain_async_event(ctx, &mut qp_num);
            if etype < 0 {
                break; // nothing queued
            }
            eprintln!(
                "remote-lookup-rdma-responder: async event {} ({}) qp_num={}",
                etype,
                async_event_name(etype),
                qp_num
            );
        }
    }

    /// Create the child queue pair on the shared PD/CQ and accept the connection.
    ///
    /// # Safety
    /// `child` is a valid connect-request cm_id; `self.pd`/`self.cq` are live.
    unsafe fn accept_child(&self, child: *mut ffi::rdma_cm_id) -> Result<*mut ffi::ibv_qp, String> {
        let mut init = ffi::ibv_qp_init_attr {
            qp_context: ptr::null_mut(),
            send_cq: self.cq,
            recv_cq: self.cq,
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
        if ffi::rdma_create_qp(child, self.pd, &mut init) != 0 {
            return Err("rdma_create_qp failed for inbound connect".into());
        }
        let qp = (*child).qp;
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
        if ffi::rdma_accept(child, &mut conn_param) != 0 {
            ffi::rdma_destroy_qp(child);
            return Err("rdma_accept failed for inbound connect".into());
        }
        Ok(qp)
    }
}

impl CmListener for RealCmSeam {
    fn next_events(&self) -> Vec<CmEvent> {
        let mut events = [ffi::epoll_event { events: 0, data: 0 }; MAX_EPOLL_EVENTS];
        loop {
            // SAFETY: epfd is a valid epoll instance; `events` is a valid buffer.
            let n = unsafe {
                ffi::epoll_wait(
                    self.epfd,
                    events.as_mut_ptr(),
                    MAX_EPOLL_EVENTS as c_int,
                    -1, // block until ready
                )
            };
            if n < 0 {
                // Interrupted syscall (EINTR) or similar → retry.
                continue;
            }
            let mut out = Vec::new();
            for ev in events.iter().take(n as usize) {
                let tag = ev.data;
                match tag {
                    TAG_STOP => return vec![CmEvent::Stop],
                    TAG_CMD => {
                        drain_eventfd(self.cmd_eventfd);
                        let mut q = self.cmd_queue.lock().expect("cmd_queue poisoned");
                        while let Some(cmd) = q.pop_front() {
                            out.push(CmEvent::Command(cmd));
                        }
                    }
                    TAG_CM => {
                        // SAFETY: seam pointers are live for its whole lifetime.
                        out.extend(unsafe { self.drain_cm_events() });
                    }
                    TAG_ASYNC => {
                        // SAFETY: listen_id (and its verbs ctx) is live for the
                        // seam's lifetime. Produces no CmEvent — logs only.
                        unsafe { self.drain_async_events() };
                    }
                    _ => {}
                }
            }
            if !out.is_empty() {
                return out;
            }
            // Spurious wake (e.g. only ESTABLISHED/DISCONNECTED acked) → keep
            // waiting rather than returning an empty batch.
        }
    }
}

impl Drop for RealCmSeam {
    fn drop(&mut self) {
        // The bridge thread exits when the command channel closes; detach it
        // (its only live resources are Arc-shared). Then release listener state.
        // SAFETY: all pointers/fds were created in `bind` and are released once.
        unsafe {
            ffi::close(self.epfd);
            ffi::close(self.cmd_eventfd);
            ffi::close(self.stop_eventfd);
            // MR must be deregistered before its PD is deallocated.
            if !self.mr.is_null() {
                ffi::ibv_dereg_mr(self.mr);
            }
            if !self.cq.is_null() {
                ffi::ibv_destroy_cq(self.cq);
            }
            if !self.pd.is_null() {
                ffi::ibv_dealloc_pd(self.pd);
            }
            if !self.listen_id.is_null() {
                ffi::rdma_destroy_id(self.listen_id);
            }
            if !self.channel.is_null() {
                ffi::rdma_destroy_event_channel(self.channel);
            }
        }
        drop(self.bridge.take());
    }
}

/// Write 1 to an eventfd to wake any epoll waiting on it (used for stop).
pub fn signal_eventfd(fd: c_int) {
    let one: u64 = 1;
    // SAFETY: `fd` is a valid eventfd owned by the actor for the seam's life.
    unsafe {
        ffi::write(
            fd,
            &one as *const u64 as *const _,
            std::mem::size_of::<u64>(),
        );
    }
}

/// Add `fd` to `epfd`'s interest set for readability, tagged with `tag`.
fn epoll_add(epfd: c_int, fd: c_int, tag: u64) {
    let mut ev = ffi::epoll_event {
        events: ffi::EPOLLIN,
        data: tag,
    };
    // SAFETY: epfd and fd are valid; ev is a valid epoll_event.
    unsafe {
        ffi::epoll_ctl(epfd, ffi::EPOLL_CTL_ADD, fd, &mut ev);
    }
}

/// Drain an eventfd counter (a level-triggered readiness signal).
fn drain_eventfd(fd: c_int) {
    let mut buf: u64 = 0;
    // SAFETY: `fd` is a valid non-blocking eventfd; buf is 8 bytes.
    unsafe {
        ffi::read(
            fd,
            &mut buf as *mut u64 as *mut _,
            std::mem::size_of::<u64>(),
        );
    }
}

/// Copy the connect `private_data` bytes out of a `rdma_conn_param`. The
/// connection table resolves them to a `PeerId` (or `None`).
///
/// # Safety
/// `param` must reference a live connect-request parameter block.
unsafe fn read_private_data(param: &ffi::rdma_conn_param) -> Option<Vec<u8>> {
    let len = param.private_data_len as usize;
    if len == 0 || param.private_data.is_null() {
        return None;
    }
    let bytes = std::slice::from_raw_parts(param.private_data as *const u8, len);
    Some(bytes.to_vec())
}
