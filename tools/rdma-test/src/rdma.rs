use std::ffi::{c_void, CString};
use std::os::raw::c_int;
use std::ptr;

use anyhow::{bail, Context, Result};
use tracing::{debug, info};

use crate::ffi;

const MAX_CQ_ENTRIES: c_int = 128;
const MAX_SEND_WR: u32 = 64;
const MAX_RECV_WR: u32 = 64;
const MAX_RETRIES: u32 = 3;

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct RemoteMrInfo {
    pub addr: u64,
    pub rkey: u32,
    pub size: u32,
}

pub struct RdmaConnection {
    pub cm_id: *mut ffi::rdma_cm_id,
    pub pd: *mut ffi::ibv_pd,
    pub cq: *mut ffi::ibv_cq,
    pub qp: *mut ffi::ibv_qp,
    channel: *mut ffi::rdma_event_channel,
}

unsafe impl Send for RdmaConnection {}

impl Drop for RdmaConnection {
    fn drop(&mut self) {
        unsafe {
            if !self.cm_id.is_null() {
                ffi::rdma_disconnect(self.cm_id);
                if !self.qp.is_null() {
                    ffi::ibv_destroy_qp(self.qp);
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

pub struct MemoryRegion {
    pub mr: *mut ffi::ibv_mr,
    pub buf: Vec<u8>,
}

impl Drop for MemoryRegion {
    fn drop(&mut self) {
        if !self.mr.is_null() {
            unsafe {
                ffi::ibv_dereg_mr(self.mr);
            }
        }
    }
}

impl MemoryRegion {
    pub fn addr(&self) -> u64 {
        self.buf.as_ptr() as u64
    }

    pub fn rkey(&self) -> u32 {
        unsafe { (*self.mr).rkey }
    }

    pub fn lkey(&self) -> u32 {
        unsafe { (*self.mr).lkey }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }
}

impl RdmaConnection {
    pub fn register_mr(&self, size: usize) -> Result<MemoryRegion> {
        let mut buf = vec![0u8; size];
        let access = ffi::IBV_ACCESS_LOCAL_WRITE
            | ffi::IBV_ACCESS_REMOTE_WRITE
            | ffi::IBV_ACCESS_REMOTE_READ;

        let mr = unsafe {
            ffi::ibv_reg_mr(
                self.pd,
                buf.as_mut_ptr() as *mut c_void,
                size,
                access,
            )
        };
        if mr.is_null() {
            bail!("ibv_reg_mr failed");
        }

        Ok(MemoryRegion { mr, buf })
    }

    pub fn post_send_wr(
        &self,
        mr: &MemoryRegion,
        len: usize,
        opcode: c_int,
        remote: Option<&RemoteMrInfo>,
    ) -> Result<()> {
        let mut sge = ffi::ibv_sge {
            addr: mr.addr(),
            length: len as u32,
            lkey: mr.lkey(),
        };

        let wr_union = if let Some(r) = remote {
            ffi::ibv_send_wr_union {
                rdma: ffi::ibv_send_wr_rdma {
                    remote_addr: r.addr,
                    rkey: r.rkey,
                },
            }
        } else {
            ffi::ibv_send_wr_union { _pad: [0; 3] }
        };

        let mut wr = ffi::ibv_send_wr {
            wr_id: 0,
            next: ptr::null_mut(),
            sg_list: &mut sge,
            num_sge: 1,
            opcode,
            send_flags: ffi::IBV_SEND_SIGNALED,
            imm_data: 0,
            wr: wr_union,
        };

        let mut bad_wr: *mut ffi::ibv_send_wr = ptr::null_mut();
        let ret = unsafe { ffi::rdma_test_post_send(self.qp, &mut wr, &mut bad_wr) };
        if ret != 0 {
            bail!("ibv_post_send failed: {}", ret);
        }
        Ok(())
    }

    pub fn post_recv_wr(&self, mr: &mut MemoryRegion) -> Result<()> {
        let mut sge = ffi::ibv_sge {
            addr: mr.addr(),
            length: mr.len() as u32,
            lkey: mr.lkey(),
        };

        let mut wr = ffi::ibv_recv_wr {
            wr_id: 0,
            next: ptr::null_mut(),
            sg_list: &mut sge,
            num_sge: 1,
        };

        let mut bad_wr: *mut ffi::ibv_recv_wr = ptr::null_mut();
        let ret = unsafe { ffi::rdma_test_post_recv(self.qp, &mut wr, &mut bad_wr) };
        if ret != 0 {
            bail!("ibv_post_recv failed: {}", ret);
        }
        Ok(())
    }

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

        loop {
            let ret = unsafe { ffi::rdma_test_poll_cq(self.cq, 1, &mut wc) };
            if ret < 0 {
                bail!("ibv_poll_cq failed: {}", ret);
            }
            if ret > 0 {
                if wc.status != ffi::IBV_WC_SUCCESS {
                    bail!("Work completion error: status={}", wc.status);
                }
                return Ok(());
            }
        }
    }

    pub fn poll_completion_with_retry(&self) -> Result<()> {
        for attempt in 0..MAX_RETRIES {
            match self.poll_completion() {
                Ok(()) => return Ok(()),
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    tracing::warn!("Poll failed (attempt {}): {}, retrying", attempt + 1, e);
                }
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    pub fn send_msg(&self, mr: &MemoryRegion, len: usize) -> Result<()> {
        self.post_send_wr(mr, len, ffi::IBV_WR_SEND, None)?;
        self.poll_completion_with_retry()
    }

    pub fn recv_msg(&self, mr: &mut MemoryRegion) -> Result<()> {
        self.post_recv_wr(mr)?;
        self.poll_completion_with_retry()
    }

    pub fn rdma_write(&self, mr: &MemoryRegion, len: usize, remote: &RemoteMrInfo) -> Result<()> {
        self.post_send_wr(mr, len, ffi::IBV_WR_RDMA_WRITE, Some(remote))?;
        self.poll_completion_with_retry()
    }
}

fn wait_for_event(
    channel: *mut ffi::rdma_event_channel,
    expected: c_int,
) -> Result<*mut ffi::rdma_cm_event> {
    let mut event: *mut ffi::rdma_cm_event = ptr::null_mut();
    let ret = unsafe { ffi::rdma_get_cm_event(channel, &mut event) };
    if ret != 0 {
        bail!("rdma_get_cm_event failed: {}", ret);
    }
    let event_type = unsafe { (*event).event };
    if event_type != expected {
        unsafe { ffi::rdma_ack_cm_event(event) };
        bail!(
            "Unexpected CM event: got {}, expected {}",
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

    let qp = unsafe { ffi::ibv_create_qp(pd, &mut init_attr) };
    if qp.is_null() {
        bail!("ibv_create_qp failed");
    }

    unsafe {
        (*cm_id).qp = qp;
    }
    Ok(qp)
}

pub fn server_connect(addr: &str, port: u16) -> Result<RdmaConnection> {
    let channel = unsafe { ffi::rdma_create_event_channel() };
    if channel.is_null() {
        bail!("rdma_create_event_channel failed");
    }

    let mut listen_id: *mut ffi::rdma_cm_id = ptr::null_mut();
    let ret =
        unsafe { ffi::rdma_create_id(channel, &mut listen_id, ptr::null_mut(), ffi::RDMA_PS_TCP) };
    if ret != 0 {
        bail!("rdma_create_id failed: {}", ret);
    }

    let addr_c = CString::new(addr).context("Invalid address")?;
    let mut sin = ffi::sockaddr_in {
        sin_family: ffi::AF_INET,
        sin_port: unsafe { ffi::htons(port) },
        sin_addr: ffi::in_addr {
            s_addr: if addr == "0.0.0.0" {
                0
            } else {
                unsafe { ffi::inet_addr(addr_c.as_ptr()) }
            },
        },
        sin_zero: [0; 8],
    };

    let ret = unsafe {
        ffi::rdma_bind_addr(
            listen_id,
            &mut sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
        )
    };
    if ret != 0 {
        bail!("rdma_bind_addr failed: {}", ret);
    }

    let ret = unsafe { ffi::rdma_listen(listen_id, 1) };
    if ret != 0 {
        bail!("rdma_listen failed: {}", ret);
    }

    info!("Server listening on {}:{}", addr, port);

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_CONNECT_REQUEST)?;
    let cm_id = unsafe { (*event).id };
    unsafe { ffi::rdma_ack_cm_event(event) };

    info!("Client connection request received");

    let ctx = unsafe { (*cm_id).verbs };
    if ctx.is_null() {
        bail!("No verbs context on CM ID");
    }

    let pd = unsafe { ffi::ibv_alloc_pd(ctx) };
    if pd.is_null() {
        bail!("ibv_alloc_pd failed");
    }

    let cq = unsafe { ffi::ibv_create_cq(ctx, MAX_CQ_ENTRIES, ptr::null_mut(), ptr::null_mut(), 0) };
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

    let ret = unsafe { ffi::rdma_accept(cm_id, &mut conn_param) };
    if ret != 0 {
        bail!("rdma_accept failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ESTABLISHED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };

    unsafe { ffi::rdma_destroy_id(listen_id) };

    info!("Connection established (server)");

    Ok(RdmaConnection {
        cm_id,
        pd,
        cq,
        qp,
        channel,
    })
}

pub fn client_connect(addr: &str, port: u16) -> Result<RdmaConnection> {
    let channel = unsafe { ffi::rdma_create_event_channel() };
    if channel.is_null() {
        bail!("rdma_create_event_channel failed");
    }

    let mut cm_id: *mut ffi::rdma_cm_id = ptr::null_mut();
    let ret =
        unsafe { ffi::rdma_create_id(channel, &mut cm_id, ptr::null_mut(), ffi::RDMA_PS_TCP) };
    if ret != 0 {
        bail!("rdma_create_id failed: {}", ret);
    }

    let addr_c = CString::new(addr).context("Invalid address")?;
    let mut sin = ffi::sockaddr_in {
        sin_family: ffi::AF_INET,
        sin_port: unsafe { ffi::htons(port) },
        sin_addr: ffi::in_addr {
            s_addr: unsafe { ffi::inet_addr(addr_c.as_ptr()) },
        },
        sin_zero: [0; 8],
    };

    info!("Resolving address {}:{}", addr, port);

    let ret = unsafe {
        ffi::rdma_resolve_addr(
            cm_id,
            ptr::null_mut(),
            &mut sin as *mut ffi::sockaddr_in as *mut ffi::sockaddr,
            2000,
        )
    };
    if ret != 0 {
        bail!("rdma_resolve_addr failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ADDR_RESOLVED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };

    debug!("Address resolved, resolving route");

    let ret = unsafe { ffi::rdma_resolve_route(cm_id, 2000) };
    if ret != 0 {
        bail!("rdma_resolve_route failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ROUTE_RESOLVED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };

    debug!("Route resolved, creating resources");

    let ctx = unsafe { (*cm_id).verbs };
    if ctx.is_null() {
        bail!("No verbs context on CM ID");
    }

    let pd = unsafe { ffi::ibv_alloc_pd(ctx) };
    if pd.is_null() {
        bail!("ibv_alloc_pd failed");
    }

    let cq = unsafe { ffi::ibv_create_cq(ctx, MAX_CQ_ENTRIES, ptr::null_mut(), ptr::null_mut(), 0) };
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

    let ret = unsafe { ffi::rdma_connect(cm_id, &mut conn_param) };
    if ret != 0 {
        bail!("rdma_connect failed: {}", ret);
    }

    let event = wait_for_event(channel, ffi::RDMA_CM_EVENT_ESTABLISHED)?;
    unsafe { ffi::rdma_ack_cm_event(event) };

    info!("Connection established (client)");

    Ok(RdmaConnection {
        cm_id,
        pd,
        cq,
        qp,
        channel,
    })
}

pub fn exchange_mr_info_server(conn: &RdmaConnection, mr: &MemoryRegion) -> Result<()> {
    let info = RemoteMrInfo {
        addr: mr.addr(),
        rkey: mr.rkey(),
        size: mr.len() as u32,
    };

    let info_bytes: &[u8] = unsafe {
        std::slice::from_raw_parts(
            &info as *const RemoteMrInfo as *const u8,
            std::mem::size_of::<RemoteMrInfo>(),
        )
    };

    let mut send_mr = conn.register_mr(std::mem::size_of::<RemoteMrInfo>())?;
    send_mr.buf[..info_bytes.len()].copy_from_slice(info_bytes);
    conn.send_msg(&send_mr, info_bytes.len())?;
    Ok(())
}

pub fn exchange_mr_info_client(conn: &RdmaConnection) -> Result<RemoteMrInfo> {
    let mut recv_mr = conn.register_mr(std::mem::size_of::<RemoteMrInfo>())?;
    conn.recv_msg(&mut recv_mr)?;

    let info: RemoteMrInfo = unsafe { std::ptr::read(recv_mr.buf.as_ptr() as *const RemoteMrInfo) };
    Ok(info)
}
