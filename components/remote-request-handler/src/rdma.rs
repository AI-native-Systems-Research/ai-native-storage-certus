//! RDMA resource management and connection handling.
//!
//! Provides both real RDMA operations (via rdma-core FFI) and mock
//! implementations for unit testing without hardware.

use std::ffi::{c_void, CString};
use std::fmt;
use std::os::raw::c_int;
use std::ptr;

use anyhow::{bail, Context, Result};

use crate::ffi;

const MAX_CQ_ENTRIES: c_int = 128;
const MAX_SEND_WR: u32 = 128;
const MAX_RECV_WR: u32 = 64;
const MAX_RETRIES: u32 = 3;
const POLL_TIMEOUT_SECS: u64 = 10;

/// Errors from RDMA operations.
#[derive(Debug, Clone)]
pub enum RdmaError {
    ConnectionFailed(String),
    AllocationFailed(String),
    WriteFailed(String),
    SendRecvFailed(String),
    ResourceExhausted(String),
    EventError(String),
}

impl fmt::Display for RdmaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConnectionFailed(msg) => write!(f, "RDMA connection failed: {msg}"),
            Self::AllocationFailed(msg) => write!(f, "RDMA allocation failed: {msg}"),
            Self::WriteFailed(msg) => write!(f, "RDMA write failed: {msg}"),
            Self::SendRecvFailed(msg) => write!(f, "RDMA send/recv failed: {msg}"),
            Self::ResourceExhausted(msg) => write!(f, "RDMA resource exhausted: {msg}"),
            Self::EventError(msg) => write!(f, "RDMA event error: {msg}"),
        }
    }
}

impl std::error::Error for RdmaError {}

/// A registered memory region for RDMA operations.
///
/// Can own a buffer (allocated by `register_mr`) or borrow an external
/// pointer (registered by `register_existing_mr`).
pub struct MemoryRegion {
    mr: *mut ffi::ibv_mr,
    /// Owned buffer (None for externally-borrowed regions).
    pub buf: Vec<u8>,
    /// Address of the registered region (for borrowed regions, this is the external pointer).
    registered_addr: *const u8,
    /// Length of the registered region.
    registered_len: usize,
}

// SAFETY: MemoryRegion is only accessed from the thread that created it
// or via synchronization in Session. The underlying ibv_mr is thread-safe
// for the operations we perform (read addr/lkey/rkey).
unsafe impl Send for MemoryRegion {}

impl Drop for MemoryRegion {
    fn drop(&mut self) {
        if !self.mr.is_null() {
            // SAFETY: mr was allocated by ibv_reg_mr and is valid until deregistered.
            unsafe {
                ffi::ibv_dereg_mr(self.mr);
            }
        }
    }
}

impl MemoryRegion {
    pub fn addr(&self) -> u64 {
        self.registered_addr as u64
    }

    pub fn rkey(&self) -> u32 {
        // SAFETY: mr is valid (checked non-null in register_mr).
        unsafe { (*self.mr).rkey }
    }

    pub fn lkey(&self) -> u32 {
        // SAFETY: mr is valid (checked non-null in register_mr).
        unsafe { (*self.mr).lkey }
    }

    pub fn len(&self) -> usize {
        self.registered_len
    }

    pub fn is_empty(&self) -> bool {
        self.registered_len == 0
    }
}

/// An RDMA connection (server or client side).
pub struct RdmaConnection {
    cm_id: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
    qp: *mut ffi::ibv_qp,
    channel: *mut ffi::rdma_event_channel,
}

// SAFETY: RdmaConnection is accessed from a single session task.
// The underlying RDMA resources are thread-safe for our usage pattern
// (one poster, one poller, same thread).
unsafe impl Send for RdmaConnection {}

impl Drop for RdmaConnection {
    fn drop(&mut self) {
        // SAFETY: All pointers were allocated by rdma-core and are valid.
        unsafe {
            if !self.cm_id.is_null() {
                ffi::rdma_disconnect(self.cm_id);
                if !self.qp.is_null() {
                    ffi::rdma_destroy_qp(self.cm_id);
                }
                ffi::rdma_destroy_id(self.cm_id);
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

impl RdmaConnection {
    /// Register a new memory region for RDMA access (allocates buffer).
    pub fn register_mr(&self, size: usize) -> Result<MemoryRegion> {
        let mut buf = vec![0u8; size];
        let access = ffi::IBV_ACCESS_LOCAL_WRITE
            | ffi::IBV_ACCESS_REMOTE_WRITE
            | ffi::IBV_ACCESS_REMOTE_READ;

        let addr = buf.as_mut_ptr();
        // SAFETY: pd is valid, buf is a valid allocation of `size` bytes.
        let mr = unsafe { ffi::ibv_reg_mr(self.pd, addr as *mut c_void, size, access) };
        if mr.is_null() {
            bail!("ibv_reg_mr failed");
        }

        Ok(MemoryRegion {
            mr,
            buf,
            registered_addr: addr,
            registered_len: size,
        })
    }

    /// Register an existing memory address as an RDMA memory region.
    /// The caller retains ownership of the memory — it must remain valid
    /// until this MemoryRegion is dropped (which deregisters the MR).
    pub fn register_existing_mr(&self, addr: *const u8, len: usize) -> Result<MemoryRegion> {
        let access = ffi::IBV_ACCESS_LOCAL_WRITE
            | ffi::IBV_ACCESS_REMOTE_WRITE
            | ffi::IBV_ACCESS_REMOTE_READ;

        // SAFETY: pd is valid, addr points to `len` bytes of valid memory
        // owned by the caller (e.g., memory-tier pool).
        let mr = unsafe { ffi::ibv_reg_mr(self.pd, addr as *mut c_void, len, access) };
        if mr.is_null() {
            bail!("ibv_reg_mr failed for existing address {:p} len {}", addr, len);
        }

        Ok(MemoryRegion {
            mr,
            buf: Vec::new(),
            registered_addr: addr,
            registered_len: len,
        })
    }

    /// Send a message via RDMA Send (blocks until completion).
    pub fn send_msg(&self, mr: &MemoryRegion, len: usize) -> Result<()> {
        // SAFETY: qp and mr are valid, registered_addr is within registered region.
        let ret = unsafe {
            ffi::rdma_test_send_msg(
                self.qp,
                mr.registered_addr as *mut c_void,
                len as u32,
                mr.lkey(),
            )
        };
        if ret != 0 {
            bail!("ibv_post_send (SEND) failed: {}", ret);
        }
        self.poll_completion_with_retry()
    }

    /// Post a recv buffer and wait for a message (blocks until completion).
    /// Returns the number of bytes received.
    pub fn recv_msg(&self, mr: &mut MemoryRegion) -> Result<usize> {
        self.post_recv(mr)?;
        self.poll_completion_bytes()
    }

    /// Post a recv buffer without waiting for completion.
    pub fn post_recv(&self, mr: &mut MemoryRegion) -> Result<()> {
        // SAFETY: qp and mr are valid.
        let ret = unsafe {
            ffi::rdma_test_recv_msg(
                self.qp,
                mr.registered_addr as *mut c_void,
                mr.len() as u32,
                mr.lkey(),
            )
        };
        if ret != 0 {
            bail!("ibv_post_recv failed: {}", ret);
        }
        Ok(())
    }

    /// Perform an RDMA Write to remote memory (blocks until completion).
    pub fn rdma_write(
        &self,
        local_mr: &MemoryRegion,
        len: usize,
        remote_addr: u64,
        rkey: u32,
    ) -> Result<()> {
        // SAFETY: qp and local_mr are valid, registered_addr within MR bounds.
        let ret = unsafe {
            ffi::rdma_test_rdma_write(
                self.qp,
                local_mr.registered_addr as *mut c_void,
                len as u32,
                local_mr.lkey(),
                remote_addr,
                rkey,
            )
        };
        if ret != 0 {
            bail!("ibv_post_send (RDMA_WRITE) failed: {}", ret);
        }
        self.poll_completion_with_retry()
    }

    /// Perform an RDMA Write from an offset within a pre-registered MR.
    /// `local_addr` must be within the bounds of `pool_mr`.
    /// Blocks until completion (signaled).
    pub fn rdma_write_from_pool(
        &self,
        pool_mr: &MemoryRegion,
        local_addr: *const u8,
        len: usize,
        remote_addr: u64,
        rkey: u32,
    ) -> Result<()> {
        // SAFETY: local_addr is within the pool_mr's registered region.
        let ret = unsafe {
            ffi::rdma_test_rdma_write(
                self.qp,
                local_addr as *mut c_void,
                len as u32,
                pool_mr.lkey(),
                remote_addr,
                rkey,
            )
        };
        if ret != 0 {
            bail!("ibv_post_send (RDMA_WRITE pool) failed: {}", ret);
        }
        self.poll_completion_with_retry()
    }

    /// Post an RDMA Write from the pool MR without waiting for completion (unsignaled).
    /// The write is queued but no CQE is generated. Use `poll_completion` on
    /// a subsequent signaled write to ensure all prior unsignaled writes complete.
    pub fn post_rdma_write_unsignaled(
        &self,
        pool_mr: &MemoryRegion,
        local_addr: *const u8,
        len: usize,
        remote_addr: u64,
        rkey: u32,
    ) -> Result<()> {
        // SAFETY: local_addr is within the pool_mr's registered region.
        let ret = unsafe {
            ffi::rdma_test_rdma_write_unsignaled(
                self.qp,
                local_addr as *mut c_void,
                len as u32,
                pool_mr.lkey(),
                remote_addr,
                rkey,
            )
        };
        if ret != 0 {
            bail!("ibv_post_send (RDMA_WRITE unsignaled) failed: {}", ret);
        }
        Ok(())
    }

    /// Poll for a recv completion, returning the number of bytes received.
    fn poll_completion_bytes(&self) -> Result<usize> {
        let mut wc = ffi::ibv_wc {
            wr_id: 0,
            status: 0,
            opcode: 0,
            vendor_err: 0,
            byte_len: 0,
            imm_data: 0,
            qp_num: 0,
            src_qp: 0,
            wc_flags: 0,
            pkey_index: 0,
            slid: 0,
            sl: 0,
            dlid_path_bits: 0,
        };

        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(POLL_TIMEOUT_SECS);

        loop {
            // SAFETY: cq is valid, wc is a properly-sized buffer.
            let ret = unsafe { ffi::rdma_test_poll_cq(self.cq, 1, &mut wc) };
            if ret < 0 {
                bail!("ibv_poll_cq failed: {}", ret);
            }
            if ret > 0 {
                if wc.status != ffi::IBV_WC_SUCCESS {
                    bail!(
                        "work completion error: status={}, opcode={}, vendor_err={}",
                        wc.status,
                        wc.opcode,
                        wc.vendor_err
                    );
                }
                return Ok(wc.byte_len as usize);
            }
            if start.elapsed() > timeout {
                bail!("poll_completion timed out after {}s", POLL_TIMEOUT_SECS);
            }
            std::hint::spin_loop();
        }
    }

    /// Poll the completion queue for one entry with timeout.
    pub fn poll_completion(&self) -> Result<()> {
        let mut wc = ffi::ibv_wc {
            wr_id: 0,
            status: 0,
            opcode: 0,
            vendor_err: 0,
            byte_len: 0,
            imm_data: 0,
            qp_num: 0,
            src_qp: 0,
            wc_flags: 0,
            pkey_index: 0,
            slid: 0,
            sl: 0,
            dlid_path_bits: 0,
        };

        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(POLL_TIMEOUT_SECS);

        loop {
            // SAFETY: cq is valid, wc is a properly-sized buffer.
            let ret = unsafe { ffi::rdma_test_poll_cq(self.cq, 1, &mut wc) };
            if ret < 0 {
                bail!("ibv_poll_cq failed: {}", ret);
            }
            if ret > 0 {
                if wc.status != ffi::IBV_WC_SUCCESS {
                    bail!(
                        "work completion error: status={}, opcode={}, vendor_err={}",
                        wc.status,
                        wc.opcode,
                        wc.vendor_err
                    );
                }
                return Ok(());
            }
            if start.elapsed() > timeout {
                bail!("poll_completion timed out after {}s", POLL_TIMEOUT_SECS);
            }
            std::hint::spin_loop();
        }
    }

    fn poll_completion_with_retry(&self) -> Result<()> {
        for attempt in 0..MAX_RETRIES {
            match self.poll_completion() {
                Ok(()) => return Ok(()),
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    eprintln!("poll failed (attempt {}): {}, retrying", attempt + 1, e);
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    /// Drain any pending completions from the CQ.
    pub fn drain_cq(&self) {
        let mut wc = ffi::ibv_wc {
            wr_id: 0,
            status: 0,
            opcode: 0,
            vendor_err: 0,
            byte_len: 0,
            imm_data: 0,
            qp_num: 0,
            src_qp: 0,
            wc_flags: 0,
            pkey_index: 0,
            slid: 0,
            sl: 0,
            dlid_path_bits: 0,
        };
        loop {
            // SAFETY: cq is valid.
            let ret = unsafe { ffi::rdma_test_poll_cq(self.cq, 1, &mut wc) };
            if ret <= 0 {
                break;
            }
        }
    }
}

// --- Connection establishment ---

fn wait_for_event(
    channel: *mut ffi::rdma_event_channel,
    expected: c_int,
) -> Result<*mut ffi::rdma_cm_event> {
    let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
    // SAFETY: channel is valid, event is an out-pointer.
    let ret = unsafe { ffi::rdma_get_cm_event(channel, &mut event) };
    if ret != 0 {
        bail!("rdma_get_cm_event failed: {}", ret);
    }
    // SAFETY: event is valid after successful rdma_get_cm_event.
    let event_type = unsafe { (*event).event };
    if event_type != expected {
        unsafe { ffi::rdma_ack_cm_event(event) };
        bail!(
            "unexpected CM event: got {}, expected {}",
            event_type,
            expected
        );
    }
    Ok(event)
}

fn create_qp(
    cm_id: *mut ffi::rdma_cm_id,
    pd: *mut ffi::ibv_pd,
    cq: *mut ffi::ibv_cq,
) -> Result<*mut ffi::ibv_qp> {
    let mut init_attr = ffi::ibv_qp_init_attr {
        qp_context: ptr::null_mut(),
        send_cq: cq,
        recv_cq: cq,
        srq: ptr::null_mut(),
        cap: ffi::ibv_qp_cap {
            max_send_wr: MAX_SEND_WR,
            max_recv_wr: MAX_RECV_WR,
            max_send_sge: 1,
            max_recv_sge: 1,
            max_inline_data: 0,
        },
        qp_type: ffi::IBV_QPT_RC,
        sq_sig_all: 0,
    };

    // SAFETY: cm_id, pd are valid pointers.
    let ret = unsafe { ffi::rdma_create_qp(cm_id, pd, &mut init_attr) };
    if ret != 0 {
        bail!("rdma_create_qp failed: {}", ret);
    }

    // SAFETY: cm_id is valid after successful rdma_create_qp.
    let qp = unsafe { (*cm_id).qp };
    if qp.is_null() {
        bail!("rdma_create_qp returned success but QP is null");
    }
    Ok(qp)
}

/// Listen for RDMA connections on the given address and port.
/// Returns a listener handle (channel + listen_id) that can accept connections.
pub struct RdmaListener {
    channel: *mut ffi::rdma_event_channel,
    listen_id: *mut ffi::rdma_cm_id,
}

// SAFETY: RdmaListener is used from the listener task only.
unsafe impl Send for RdmaListener {}

impl Drop for RdmaListener {
    fn drop(&mut self) {
        // SAFETY: Both pointers were allocated by rdma-core.
        unsafe {
            if !self.listen_id.is_null() {
                ffi::rdma_destroy_id(self.listen_id);
            }
            if !self.channel.is_null() {
                ffi::rdma_destroy_event_channel(self.channel);
            }
        }
    }
}

impl RdmaListener {
    /// Bind and listen on the specified address and port.
    pub fn bind(addr: &str, port: u16) -> Result<Self> {
        // SAFETY: Creating RDMA event channel (no preconditions).
        let channel = unsafe { ffi::rdma_create_event_channel() };
        if channel.is_null() {
            bail!("rdma_create_event_channel failed");
        }

        let mut listen_id: *mut ffi::rdma_cm_id = ptr::null_mut();
        // SAFETY: channel is valid.
        let ret = unsafe {
            ffi::rdma_create_id(channel, &mut listen_id, ptr::null_mut(), ffi::RDMA_PS_TCP)
        };
        if ret != 0 {
            bail!("rdma_create_id failed: {}", ret);
        }

        let addr_c = CString::new(addr).context("invalid address")?;
        let mut sin = ffi::sockaddr_in {
            sin_family: ffi::AF_INET,
            // SAFETY: htons is a pure function.
            sin_port: unsafe { ffi::htons(port) },
            sin_addr: ffi::in_addr {
                s_addr: if addr == "0.0.0.0" {
                    0
                } else {
                    // SAFETY: addr_c is a valid C string.
                    unsafe { ffi::inet_addr(addr_c.as_ptr()) }
                },
            },
            sin_zero: [0; 8],
        };

        // SAFETY: listen_id is valid, sin is a properly initialized sockaddr_in.
        let ret = unsafe {
            ffi::rdma_bind_addr(
                listen_id,
                &mut sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            )
        };
        if ret != 0 {
            bail!("rdma_bind_addr failed: {}", ret);
        }

        // SAFETY: listen_id is bound.
        let ret = unsafe { ffi::rdma_listen(listen_id, 10) };
        if ret != 0 {
            bail!("rdma_listen failed: {}", ret);
        }

        Ok(RdmaListener { channel, listen_id })
    }

    /// Wait for and accept one incoming connection. Blocking call.
    pub fn accept(&self) -> Result<RdmaConnection> {
        let event = wait_for_event(self.channel, ffi::RDMA_CM_EVENT_CONNECT_REQUEST)?;
        // SAFETY: event is valid after wait_for_event success.
        let cm_id = unsafe { (*event).id };
        unsafe { ffi::rdma_ack_cm_event(event) };

        // SAFETY: cm_id from a CONNECT_REQUEST event has a valid verbs context.
        let ctx = unsafe { (*cm_id).verbs };
        if ctx.is_null() {
            bail!("no verbs context on CM ID");
        }

        // SAFETY: ctx is valid.
        let pd = unsafe { ffi::ibv_alloc_pd(ctx) };
        if pd.is_null() {
            bail!("ibv_alloc_pd failed");
        }

        // SAFETY: ctx is valid.
        let cq =
            unsafe { ffi::ibv_create_cq(ctx, MAX_CQ_ENTRIES, ptr::null_mut(), ptr::null_mut(), 0) };
        if cq.is_null() {
            bail!("ibv_create_cq failed");
        }

        let qp = create_qp(cm_id, pd, cq)?;

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

        // SAFETY: cm_id and conn_param are valid.
        let ret = unsafe { ffi::rdma_accept(cm_id, &mut conn_param) };
        if ret != 0 {
            bail!("rdma_accept failed: {}", ret);
        }

        let event = wait_for_event(self.channel, ffi::RDMA_CM_EVENT_ESTABLISHED)?;
        unsafe { ffi::rdma_ack_cm_event(event) };

        let conn = RdmaConnection {
            cm_id,
            pd,
            cq,
            qp,
            channel: ptr::null_mut(), // listener owns the channel
        };
        conn.drain_cq();
        Ok(conn)
    }
}

/// Connect to a remote RDMA handler as a client.
pub fn client_connect(addr: &str, port: u16) -> Result<RdmaConnection> {
    // SAFETY: No preconditions.
    let channel = unsafe { ffi::rdma_create_event_channel() };
    if channel.is_null() {
        bail!("rdma_create_event_channel failed");
    }

    let mut cm_id: *mut ffi::rdma_cm_id = ptr::null_mut();
    // SAFETY: channel is valid.
    let ret =
        unsafe { ffi::rdma_create_id(channel, &mut cm_id, ptr::null_mut(), ffi::RDMA_PS_TCP) };
    if ret != 0 {
        bail!("rdma_create_id failed: {}", ret);
    }

    let addr_c = CString::new(addr).context("invalid address")?;
    let mut sin = ffi::sockaddr_in {
        sin_family: ffi::AF_INET,
        sin_port: unsafe { ffi::htons(port) },
        sin_addr: ffi::in_addr {
            s_addr: unsafe { ffi::inet_addr(addr_c.as_ptr()) },
        },
        sin_zero: [0; 8],
    };

    let mut src_sin = ffi::sockaddr_in {
        sin_family: ffi::AF_INET,
        sin_port: 0,
        sin_addr: ffi::in_addr { s_addr: 0 },
        sin_zero: [0; 8],
    };

    // SAFETY: cm_id is valid, sin is initialized.
    let ret = unsafe {
        ffi::rdma_resolve_addr(
            cm_id,
            &mut src_sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            &mut sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            2000,
        )
    };
    if ret != 0 {
        bail!("rdma_resolve_addr failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ADDR_RESOLVED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };

    // SAFETY: cm_id has resolved address.
    let ret = unsafe { ffi::rdma_resolve_route(cm_id, 2000) };
    if ret != 0 {
        bail!("rdma_resolve_route failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ROUTE_RESOLVED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };

    // SAFETY: cm_id has resolved route, verbs context is available.
    let ctx = unsafe { (*cm_id).verbs };
    if ctx.is_null() {
        bail!("no verbs context on CM ID");
    }

    let pd = unsafe { ffi::ibv_alloc_pd(ctx) };
    if pd.is_null() {
        bail!("ibv_alloc_pd failed");
    }

    let cq =
        unsafe { ffi::ibv_create_cq(ctx, MAX_CQ_ENTRIES, ptr::null_mut(), ptr::null_mut(), 0) };
    if cq.is_null() {
        bail!("ibv_create_cq failed");
    }

    let qp = create_qp(cm_id, pd, cq)?;

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

    // SAFETY: cm_id and conn_param are valid.
    let ret = unsafe { ffi::rdma_connect(cm_id, &mut conn_param) };
    if ret != 0 {
        bail!("rdma_connect failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ESTABLISHED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };

    let conn = RdmaConnection {
        cm_id,
        pd,
        cq,
        qp,
        channel,
    };
    conn.drain_cq();
    Ok(conn)
}

/// Parameters for an RDMA Write operation (used by mock interface).
#[derive(Debug, Clone)]
pub struct WriteParams {
    pub local_addr: u64,
    pub local_length: u32,
    pub lkey: u32,
    pub remote_addr: u64,
    pub rkey: u32,
}

/// A completion event from the completion queue (used by mock interface).
#[derive(Debug, Clone)]
pub struct Completion {
    pub wr_id: u64,
    pub success: bool,
    pub bytes_transferred: u32,
}

/// Trait abstracting RDMA operations for testability.
pub trait RdmaOps: Send + Sync {
    fn post_write(&self, params: &WriteParams) -> std::result::Result<(), RdmaError>;
    fn post_send(&self, data: &[u8]) -> std::result::Result<(), RdmaError>;
    fn post_recv(&self, buf: &mut [u8]) -> std::result::Result<(), RdmaError>;
    fn poll_cq(&self) -> std::result::Result<Vec<Completion>, RdmaError>;
}

#[cfg(test)]
pub mod mock {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MockRdmaOps {
        pub writes: Mutex<Vec<WriteParams>>,
        pub sends: Mutex<Vec<Vec<u8>>>,
        pub completions: Mutex<Vec<Completion>>,
    }

    impl MockRdmaOps {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn push_completion(&self, completion: Completion) {
            self.completions.lock().unwrap().push(completion);
        }

        pub fn write_count(&self) -> usize {
            self.writes.lock().unwrap().len()
        }

        pub fn send_count(&self) -> usize {
            self.sends.lock().unwrap().len()
        }
    }

    impl RdmaOps for MockRdmaOps {
        fn post_write(&self, params: &WriteParams) -> std::result::Result<(), RdmaError> {
            self.writes.lock().unwrap().push(params.clone());
            Ok(())
        }

        fn post_send(&self, data: &[u8]) -> std::result::Result<(), RdmaError> {
            self.sends.lock().unwrap().push(data.to_vec());
            Ok(())
        }

        fn post_recv(&self, _buf: &mut [u8]) -> std::result::Result<(), RdmaError> {
            Ok(())
        }

        fn poll_cq(&self) -> std::result::Result<Vec<Completion>, RdmaError> {
            let mut completions = self.completions.lock().unwrap();
            let result = completions.drain(..).collect();
            Ok(result)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::*;
    use super::*;

    #[test]
    fn mock_post_write_records_params() {
        let ops = MockRdmaOps::new();
        let params = WriteParams {
            local_addr: 0x1000,
            local_length: 256,
            lkey: 1,
            remote_addr: 0x2000,
            rkey: 2,
        };
        ops.post_write(&params).unwrap();
        assert_eq!(ops.write_count(), 1);

        let writes = ops.writes.lock().unwrap();
        assert_eq!(writes[0].remote_addr, 0x2000);
        assert_eq!(writes[0].rkey, 2);
    }

    #[test]
    fn mock_post_send_records_data() {
        let ops = MockRdmaOps::new();
        ops.post_send(&[1, 2, 3, 4]).unwrap();
        assert_eq!(ops.send_count(), 1);

        let sends = ops.sends.lock().unwrap();
        assert_eq!(sends[0], vec![1, 2, 3, 4]);
    }

    #[test]
    fn mock_poll_cq_drains() {
        let ops = MockRdmaOps::new();
        ops.push_completion(Completion {
            wr_id: 1,
            success: true,
            bytes_transferred: 128,
        });
        ops.push_completion(Completion {
            wr_id: 2,
            success: false,
            bytes_transferred: 0,
        });

        let completions = ops.poll_cq().unwrap();
        assert_eq!(completions.len(), 2);
        assert!(completions[0].success);
        assert!(!completions[1].success);

        let completions = ops.poll_cq().unwrap();
        assert!(completions.is_empty());
    }
}
