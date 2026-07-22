//! Raw FFI bindings to rdma-core (`librdmacm` + `libibverbs`) and the few libc
//! syscalls (`epoll`, `eventfd`) the accept loop multiplexes on.
//!
//! The accept-side surface is bound: bind/listen/`rdma_get_src_port`,
//! accept/reject, the CM event channel, queue-pair creation/teardown, and the
//! `qp_to_error` shim (`src/wrapper.c`). The responder is also the **registrar**
//! for its own memory tier: it registers the whole pool once with `ibv_reg_mr`
//! (`REMOTE_WRITE`) so inbound one-sided writes are bounds-checked against its
//! protection domain. It never touches the value bytes themselves.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_uint};

// ibverbs constants
pub const IBV_QPT_RC: c_int = 2;
pub const IBV_QPS_ERR: c_int = 6;

// ibv_reg_mr access flags (bitmask).
pub const IBV_ACCESS_LOCAL_WRITE: c_int = 1;
pub const IBV_ACCESS_REMOTE_WRITE: c_int = 2;
pub const IBV_ACCESS_REMOTE_READ: c_int = 4;

// Port state (ibv_port_attr.state) — used to pick the first active device.
pub const IBV_PORT_ACTIVE: u32 = 4;

// rdmacm constants
pub const RDMA_PS_TCP: c_int = 0x0106;

pub const RDMA_CM_EVENT_CONNECT_REQUEST: c_int = 4;
pub const RDMA_CM_EVENT_ESTABLISHED: c_int = 9;
pub const RDMA_CM_EVENT_DISCONNECTED: c_int = 10;

pub const AF_INET: u16 = 2;

// epoll / eventfd / fcntl constants
pub const EPOLL_CLOEXEC: c_int = 0o2000000;
pub const EPOLL_CTL_ADD: c_int = 1;
pub const EPOLLIN: u32 = 0x001;
pub const EFD_NONBLOCK: c_int = 0o4000;
pub const EFD_CLOEXEC: c_int = 0o2000000;
pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;
pub const O_NONBLOCK: c_int = 0o4000;
pub const EAGAIN: c_int = 11;

// Opaque ibverbs/rdmacm types
#[repr(C)]
pub struct ibv_device {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct ibv_context {
    _opaque: [u8; 0],
}

// ibv_port_attr — full layout so `state`/`gid_tbl_len` sit at the right offsets
// (only those two are read, during first-active-device discovery).
#[repr(C)]
pub struct ibv_port_attr {
    pub state: u32,
    pub max_mtu: u32,
    pub active_mtu: u32,
    pub gid_tbl_len: c_int,
    pub port_cap_flags: u32,
    pub max_msg_sz: u32,
    pub bad_pkey_cntr: u32,
    pub qkey_viol_cntr: u32,
    pub pkey_tbl_len: u16,
    pub lid: u16,
    pub sm_lid: u16,
    pub lmc: u8,
    pub max_vl_num: u8,
    pub sm_sl: u8,
    pub subnet_timeout: u8,
    pub init_type_reply: u8,
    pub active_width: u8,
    pub active_speed: u8,
    pub phys_state: u8,
    pub link_layer: u8,
    pub flags: u8,
    pub port_cap_flags2: u16,
}

#[repr(C)]
pub struct ibv_gid {
    pub raw: [u8; 16],
}

#[repr(C)]
pub struct ibv_pd {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct ibv_cq {
    _opaque: [u8; 0],
}

/// `struct ibv_mr` — a registered memory region. Only the leading fields are
/// declared; the `rkey` (and `lkey`) are read after `ibv_reg_mr`.
#[repr(C)]
pub struct ibv_mr {
    pub context: *mut ibv_context,
    pub pd: *mut ibv_pd,
    pub addr: *mut c_void,
    pub length: usize,
    pub handle: u32,
    pub lkey: u32,
    pub rkey: u32,
}

/// `struct rdma_event_channel` — its first (and only) member is the pollable fd.
#[repr(C)]
pub struct rdma_event_channel {
    pub fd: c_int,
}

#[repr(C)]
pub struct ibv_qp_cap {
    pub max_send_wr: u32,
    pub max_recv_wr: u32,
    pub max_send_sge: u32,
    pub max_recv_sge: u32,
    pub max_inline_data: u32,
}

#[repr(C)]
pub struct ibv_qp_init_attr {
    pub qp_context: *mut c_void,
    pub send_cq: *mut ibv_cq,
    pub recv_cq: *mut ibv_cq,
    pub srq: *mut c_void,
    pub cap: ibv_qp_cap,
    pub qp_type: c_int,
    pub sq_sig_all: c_int,
}

#[repr(C)]
pub struct ibv_qp {
    pub context: *mut ibv_context,
    pub qp_context: *mut c_void,
    pub pd: *mut ibv_pd,
    pub send_cq: *mut ibv_cq,
    pub recv_cq: *mut ibv_cq,
    pub srq: *mut c_void,
    pub handle: u32,
    pub qp_num: u32,
    pub state: c_int,
    pub qp_type: c_int,
}

#[repr(C)]
pub struct rdma_cm_id {
    pub verbs: *mut ibv_context,
    pub channel: *mut rdma_event_channel,
    pub context: *mut c_void,
    pub qp: *mut ibv_qp,
    // route, ps, port_num, event, cq channels, cq, srq, pd, qp_type follow
    _tail: [u8; 256],
}

#[repr(C)]
pub struct rdma_cm_event {
    pub id: *mut rdma_cm_id,
    pub listen_id: *mut rdma_cm_id,
    pub event: c_int,
    pub status: c_int,
    pub param: rdma_conn_param_event,
}

#[repr(C)]
pub union rdma_conn_param_event {
    pub conn: rdma_conn_param,
    pub _pad: [u8; 256],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct rdma_conn_param {
    pub private_data: *const c_void,
    pub private_data_len: u8,
    pub responder_resources: u8,
    pub initiator_depth: u8,
    pub flow_control: u8,
    pub retry_count: u8,
    pub rnr_retry_count: u8,
    pub srq: u8,
    pub qp_num: u32,
}

#[repr(C)]
pub struct sockaddr_in {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: in_addr,
    pub sin_zero: [u8; 8],
}

#[repr(C)]
pub struct in_addr {
    pub s_addr: u32,
}

#[repr(C)]
pub struct sockaddr {
    pub sa_family: u16,
    pub sa_data: [u8; 14],
}

/// `struct epoll_event` is packed on x86_64.
#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct epoll_event {
    pub events: u32,
    pub data: u64,
}

extern "C" {
    // Device enumeration (first-active-device discovery when no bind IP is set).
    pub fn ibv_get_device_list(num_devices: *mut c_int) -> *mut *mut ibv_device;
    pub fn ibv_free_device_list(list: *mut *mut ibv_device);
    pub fn ibv_open_device(device: *mut ibv_device) -> *mut ibv_context;
    pub fn ibv_close_device(context: *mut ibv_context) -> c_int;
    pub fn ibv_query_port(
        context: *mut ibv_context,
        port_num: u8,
        port_attr: *mut ibv_port_attr,
    ) -> c_int;
    pub fn ibv_query_gid(
        context: *mut ibv_context,
        port_num: u8,
        index: c_int,
        gid: *mut ibv_gid,
    ) -> c_int;

    // Protection domain / completion queue / queue pair (accept side).
    pub fn ibv_alloc_pd(context: *mut ibv_context) -> *mut ibv_pd;
    pub fn ibv_dealloc_pd(pd: *mut ibv_pd) -> c_int;
    pub fn ibv_create_cq(
        context: *mut ibv_context,
        cqe: c_int,
        cq_context: *mut c_void,
        channel: *mut c_void,
        comp_vector: c_int,
    ) -> *mut ibv_cq;
    pub fn ibv_destroy_cq(cq: *mut ibv_cq) -> c_int;

    // Memory-region registration (registrar side): register the whole pool once.
    pub fn ibv_reg_mr(
        pd: *mut ibv_pd,
        addr: *mut c_void,
        length: usize,
        access: c_int,
    ) -> *mut ibv_mr;
    pub fn ibv_dereg_mr(mr: *mut ibv_mr) -> c_int;

    // RDMA CM (accept side).
    pub fn rdma_create_event_channel() -> *mut rdma_event_channel;
    pub fn rdma_destroy_event_channel(channel: *mut rdma_event_channel);
    pub fn rdma_create_id(
        channel: *mut rdma_event_channel,
        id: *mut *mut rdma_cm_id,
        context: *mut c_void,
        ps: c_int,
    ) -> c_int;
    pub fn rdma_destroy_id(id: *mut rdma_cm_id) -> c_int;
    pub fn rdma_bind_addr(id: *mut rdma_cm_id, addr: *mut sockaddr) -> c_int;
    pub fn rdma_listen(id: *mut rdma_cm_id, backlog: c_int) -> c_int;
    pub fn rdma_get_src_port(id: *mut rdma_cm_id) -> u16;
    pub fn rdma_accept(id: *mut rdma_cm_id, conn_param: *mut rdma_conn_param) -> c_int;
    pub fn rdma_reject(
        id: *mut rdma_cm_id,
        private_data: *const c_void,
        private_data_len: u8,
    ) -> c_int;
    pub fn rdma_disconnect(id: *mut rdma_cm_id) -> c_int;
    pub fn rdma_create_qp(
        id: *mut rdma_cm_id,
        pd: *mut ibv_pd,
        qp_init_attr: *mut ibv_qp_init_attr,
    ) -> c_int;
    pub fn rdma_destroy_qp(id: *mut rdma_cm_id);
    pub fn rdma_get_cm_event(
        channel: *mut rdma_event_channel,
        event: *mut *mut rdma_cm_event,
    ) -> c_int;
    pub fn rdma_ack_cm_event(event: *mut rdma_cm_event) -> c_int;

    // QP→ERROR shim (src/wrapper.c): ibv_modify_qp(qp, {qp_state=IBV_QPS_ERR}, IBV_QP_STATE).
    // Returns 0 on success, else the errno-style return of ibv_modify_qp.
    pub fn responder_qp_to_error(qp: *mut ibv_qp) -> c_int;

    // libc helpers.
    pub fn htons(hostshort: u16) -> u16;
    pub fn ntohs(netshort: u16) -> u16;
    pub fn inet_addr(cp: *const c_char) -> u32;

    // epoll / eventfd / fcntl (accept-loop multiplexing).
    pub fn epoll_create1(flags: c_int) -> c_int;
    pub fn epoll_ctl(epfd: c_int, op: c_int, fd: c_int, event: *mut epoll_event) -> c_int;
    pub fn epoll_wait(
        epfd: c_int,
        events: *mut epoll_event,
        maxevents: c_int,
        timeout: c_int,
    ) -> c_int;
    pub fn eventfd(initval: c_uint, flags: c_int) -> c_int;
    pub fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
    pub fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    pub fn close(fd: c_int) -> c_int;
    pub fn fcntl(fd: c_int, cmd: c_int, arg: c_int) -> c_int;
}
