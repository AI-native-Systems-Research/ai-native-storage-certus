#![allow(non_camel_case_types, dead_code)]

use std::ffi::c_void;
use std::os::raw::{c_char, c_int};

// ibverbs constants
pub const IBV_ACCESS_LOCAL_WRITE: c_int = 1;
pub const IBV_ACCESS_REMOTE_WRITE: c_int = 2;
pub const IBV_ACCESS_REMOTE_READ: c_int = 4;

pub const IBV_QPT_RC: c_int = 2;

pub const IBV_QPS_RESET: c_int = 0;
pub const IBV_QPS_INIT: c_int = 1;
pub const IBV_QPS_RTR: c_int = 2;
pub const IBV_QPS_RTS: c_int = 3;
pub const IBV_QPS_SQD: c_int = 4;
pub const IBV_QPS_SQE: c_int = 5;
pub const IBV_QPS_ERR: c_int = 6;

pub const IBV_WR_SEND: c_int = 0;
pub const IBV_WR_RDMA_WRITE: c_int = 1;
pub const IBV_WR_RDMA_READ: c_int = 2;

pub const IBV_SEND_SIGNALED: c_int = 2;

pub const IBV_WC_SUCCESS: c_int = 0;

pub const IBV_PORT_ACTIVE: u8 = 4;

pub const IBV_LINK_LAYER_INFINIBAND: u8 = 1;
pub const IBV_LINK_LAYER_ETHERNET: u8 = 2;

// rdmacm constants
pub const RDMA_PS_TCP: c_int = 0x0106;

pub const RDMA_CM_EVENT_ADDR_RESOLVED: c_int = 0;
pub const RDMA_CM_EVENT_ADDR_ERROR: c_int = 1;
pub const RDMA_CM_EVENT_ROUTE_RESOLVED: c_int = 2;
pub const RDMA_CM_EVENT_ROUTE_ERROR: c_int = 3;
pub const RDMA_CM_EVENT_CONNECT_REQUEST: c_int = 4;
pub const RDMA_CM_EVENT_CONNECT_RESPONSE: c_int = 5;
pub const RDMA_CM_EVENT_CONNECT_ERROR: c_int = 6;
pub const RDMA_CM_EVENT_UNREACHABLE: c_int = 7;
pub const RDMA_CM_EVENT_REJECTED: c_int = 8;
pub const RDMA_CM_EVENT_ESTABLISHED: c_int = 9;
pub const RDMA_CM_EVENT_DISCONNECTED: c_int = 10;

// Opaque types
#[repr(C)]
pub struct ibv_device {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct ibv_context {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct ibv_pd {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct ibv_cq {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct rdma_event_channel {
    _opaque: [u8; 0],
}

// ibv_port_attr - we only need a few fields so use a large buffer
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
pub struct ibv_mr {
    pub context: *mut ibv_context,
    pub pd: *mut ibv_pd,
    pub addr: *mut c_void,
    pub length: usize,
    pub handle: u32,
    pub lkey: u32,
    pub rkey: u32,
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
pub struct ibv_qp_cap {
    pub max_send_wr: u32,
    pub max_recv_wr: u32,
    pub max_send_sge: u32,
    pub max_recv_sge: u32,
    pub max_inline_data: u32,
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
pub struct ibv_sge {
    pub addr: u64,
    pub length: u32,
    pub lkey: u32,
}

#[repr(C)]
pub struct ibv_send_wr {
    pub wr_id: u64,
    pub next: *mut ibv_send_wr,
    pub sg_list: *mut ibv_sge,
    pub num_sge: c_int,
    pub opcode: c_int,
    pub send_flags: c_int,
    pub imm_data: u32,
    pub wr: ibv_send_wr_union,
    // qp_type union + bind union in the real struct
    pub _tail: [u64; 8],
}

#[repr(C)]
pub union ibv_send_wr_union {
    pub rdma: ibv_send_wr_rdma,
    pub _pad: [u64; 3],
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct ibv_send_wr_rdma {
    pub remote_addr: u64,
    pub rkey: u32,
}

#[repr(C)]
pub struct ibv_recv_wr {
    pub wr_id: u64,
    pub next: *mut ibv_recv_wr,
    pub sg_list: *mut ibv_sge,
    pub num_sge: c_int,
}

#[repr(C)]
pub struct ibv_wc {
    pub wr_id: u64,
    pub status: c_int,
    pub opcode: c_int,
    pub vendor_err: u32,
    pub byte_len: u32,
    pub imm_data: u32,
    pub qp_num: u32,
    pub src_qp: u32,
    pub wc_flags: c_int,
    pub pkey_index: u16,
    pub slid: u16,
    pub sl: u8,
    pub dlid_path_bits: u8,
}

// rdma_cm types
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

extern "C" {
    // Device management
    pub fn ibv_get_device_list(num_devices: *mut c_int) -> *mut *mut ibv_device;
    pub fn ibv_free_device_list(list: *mut *mut ibv_device);
    pub fn ibv_get_device_name(device: *mut ibv_device) -> *const c_char;
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

    // Protection domain
    pub fn ibv_alloc_pd(context: *mut ibv_context) -> *mut ibv_pd;
    pub fn ibv_dealloc_pd(pd: *mut ibv_pd) -> c_int;

    // Memory registration
    pub fn ibv_reg_mr(
        pd: *mut ibv_pd,
        addr: *mut c_void,
        length: usize,
        access: c_int,
    ) -> *mut ibv_mr;
    pub fn ibv_dereg_mr(mr: *mut ibv_mr) -> c_int;

    // Completion queue
    pub fn ibv_create_cq(
        context: *mut ibv_context,
        cqe: c_int,
        cq_context: *mut c_void,
        channel: *mut c_void,
        comp_vector: c_int,
    ) -> *mut ibv_cq;
    pub fn ibv_destroy_cq(cq: *mut ibv_cq) -> c_int;

    // Queue pair
    pub fn ibv_create_qp(pd: *mut ibv_pd, qp_init_attr: *mut ibv_qp_init_attr) -> *mut ibv_qp;
    pub fn ibv_destroy_qp(qp: *mut ibv_qp) -> c_int;

    // Wrappers for inline ibverbs functions (defined in wrapper.c)
    pub fn rdma_test_poll_cq(cq: *mut ibv_cq, num_entries: c_int, wc: *mut ibv_wc) -> c_int;

    // Higher-level C helper that constructs a proper RDMA-write work request.
    pub fn rdma_test_rdma_write(
        qp: *mut ibv_qp,
        buf: *mut c_void,
        len: u32,
        lkey: u32,
        remote_addr: u64,
        rkey: u32,
    ) -> c_int;

    // RDMA CM functions
    pub fn rdma_create_qp(
        id: *mut rdma_cm_id,
        pd: *mut ibv_pd,
        qp_init_attr: *mut ibv_qp_init_attr,
    ) -> c_int;
    pub fn rdma_destroy_qp(id: *mut rdma_cm_id);
    pub fn rdma_create_event_channel() -> *mut rdma_event_channel;
    pub fn rdma_destroy_event_channel(channel: *mut rdma_event_channel);
    pub fn rdma_create_id(
        channel: *mut rdma_event_channel,
        id: *mut *mut rdma_cm_id,
        context: *mut c_void,
        ps: c_int,
    ) -> c_int;
    pub fn rdma_destroy_id(id: *mut rdma_cm_id) -> c_int;
    pub fn rdma_resolve_addr(
        id: *mut rdma_cm_id,
        src_addr: *mut sockaddr,
        dst_addr: *mut sockaddr,
        timeout_ms: c_int,
    ) -> c_int;
    pub fn rdma_resolve_route(id: *mut rdma_cm_id, timeout_ms: c_int) -> c_int;
    pub fn rdma_connect(id: *mut rdma_cm_id, conn_param: *mut rdma_conn_param) -> c_int;
    pub fn rdma_disconnect(id: *mut rdma_cm_id) -> c_int;
    pub fn rdma_get_cm_event(
        channel: *mut rdma_event_channel,
        event: *mut *mut rdma_cm_event,
    ) -> c_int;
    pub fn rdma_ack_cm_event(event: *mut rdma_cm_event) -> c_int;

    // Helpers
    pub fn htons(hostshort: u16) -> u16;
    pub fn inet_addr(cp: *const c_char) -> u32;
}

pub const AF_INET: u16 = 2;
