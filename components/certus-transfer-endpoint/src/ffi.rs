//! Verbs FFI declarations — **the single copy** (SC-008).
//!
//! `remote-lookup-rdma-initiator` and `remote-lookup-rdma-responder` each
//! declare their own `ffi.rs`, with 52 public names in common; this module
//! exists so a third copy is never made. It is the union of what both need —
//! the connect side of one and the accept side of the other — plus the
//! send/recv surface neither predecessor ever used: `post_recv`, the receive
//! work request and SGE types, and `IBV_WR_SEND` as a live opcode (research D3).
//!
//! # Not a verbatim copy
//!
//! Every struct below was measured against `verbs.h` / `rdma_cma.h` from
//! rdma-core 2510.0.11, and four predecessor definitions were wrong. They were
//! harmless there only because the wrong parts were never read or built in
//! Rust; this crate builds work requests in Rust, so they are fixed here:
//!
//! * `ibv_send_wr`'s `wr` union is 32 bytes (the atomic variant), not 24, so
//!   every field after it sat 8 bytes early. The total size still matched,
//!   because the tail padding absorbed the error.
//! * `IBV_WR_*` opcodes were numbered `SEND = 0, RDMA_WRITE = 1`. The real
//!   enum is `RDMA_WRITE = 0, RDMA_WRITE_WITH_IMM = 1, SEND = 2`: a send posted
//!   with the old value would have been an RDMA write.
//! * `ibv_port_attr` lacked `active_speed_ex` (52 bytes against 56). It is
//!   reached through [`certus_te_query_port`] rather than the exported
//!   `ibv_query_port`, which is the *compat* entry point the C macro bypasses.
//! * `rdma_cm_id`, `rdma_cm_event` and `ibv_qp` were padded to guessed sizes.
//!   They are now padded to their real ones, so their sizes can be asserted.
//!
//! # Layout check
//!
//! The `const` block at the bottom asserts the size of every struct and the
//! offset of every field Rust reads or writes, so a wrong Rust declaration
//! fails the build. It checks Rust against **pinned numbers**, not against the
//! header; the C half — the same numbers as `_Static_assert`s over the real
//! header — lives in `src/wrapper.c` (T008), so a header that changes under us
//! fails the build there. Change a number on one side, change it on both.
//!
//! # Inline verbs
//!
//! `ibv_post_send`, `ibv_post_recv`, `ibv_poll_cq` and the non-compat
//! `ibv_query_port` are `static inline` in C, so no symbol exists to bind.
//! They are reached through thin pass-through shims in `src/wrapper.c` (T008),
//! prefixed `certus_te_` so they cannot collide with a predecessor's shim when
//! both are linked into one binary. Work requests are built in Rust and passed
//! through unchanged, which is what allows chained lists and an accurate
//! `bad_wr` (T014's partial batch posting needs both).
//!
//! # Safety
//!
//! Every function here is `unsafe` and has the same caller contract as its C
//! counterpart: pointers must be valid for the access the C function makes,
//! objects must not be used after their destroy call, and an `rdma_cm_event`
//! must be acked exactly once. Each call site carries its own `// SAFETY:`
//! justification (constitution Principle V); a bare declaration has nothing to
//! justify on its own.

// Items are consumed by T009 onward; the whole surface is declared here at
// once so it is reviewed as one piece against the headers.
#![allow(non_camel_case_types, dead_code)]

use std::ffi::c_void;
use std::os::raw::{c_char, c_int};

// --- ibverbs constants -------------------------------------------------------

// `ibv_reg_mr` access flags (bitmask).
pub const IBV_ACCESS_LOCAL_WRITE: c_int = 1;
pub const IBV_ACCESS_REMOTE_WRITE: c_int = 2;
pub const IBV_ACCESS_REMOTE_READ: c_int = 4;

pub const IBV_QPT_RC: c_int = 2;

// `enum ibv_qp_state`.
pub const IBV_QPS_RESET: c_int = 0;
pub const IBV_QPS_INIT: c_int = 1;
pub const IBV_QPS_RTR: c_int = 2;
pub const IBV_QPS_RTS: c_int = 3;
pub const IBV_QPS_SQD: c_int = 4;
pub const IBV_QPS_SQE: c_int = 5;
pub const IBV_QPS_ERR: c_int = 6;

// `enum ibv_wr_opcode` — see the module docs: the predecessors had these wrong.
pub const IBV_WR_RDMA_WRITE: c_int = 0;
pub const IBV_WR_SEND: c_int = 2;

// `enum ibv_send_flags` (bitmask).
pub const IBV_SEND_SIGNALED: c_int = 2;

// `enum ibv_wc_status`. A flush error is what every work request still queued
// reports once its queue pair is forced to ERROR, so draining after a deadline
// (FR-023) must recognise it as expected rather than as a fresh fault.
pub const IBV_WC_SUCCESS: c_int = 0;
pub const IBV_WC_WR_FLUSH_ERR: c_int = 5;

// `enum ibv_wc_opcode`. Receive completions have `IBV_WC_RECV` set, so a
// completion is a receive iff `opcode & IBV_WC_RECV != 0`.
pub const IBV_WC_SEND: c_int = 0;
pub const IBV_WC_RDMA_WRITE: c_int = 1;
pub const IBV_WC_RECV: c_int = 1 << 7;

// `enum ibv_port_state`; `ibv_port_attr::state` is that enum, hence `u32`.
pub const IBV_PORT_ACTIVE: u32 = 4;

pub const IBV_LINK_LAYER_INFINIBAND: u8 = 1;
pub const IBV_LINK_LAYER_ETHERNET: u8 = 2;

// --- rdmacm constants --------------------------------------------------------

pub const RDMA_PS_TCP: c_int = 0x0106;

// `enum rdma_cm_event_type`.
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

pub const AF_INET: u16 = 2;

// --- opaque types ------------------------------------------------------------

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

// --- ibverbs structs ---------------------------------------------------------

/// Full layout, including `active_speed_ex`; filled by [`certus_te_query_port`].
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
    pub active_speed_ex: u32,
}

/// `union ibv_gid`; only the raw view is declared. Aligned to 8 like the C
/// union, whose other member is a pair of `u64`s.
#[repr(C, align(8))]
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

/// Only ever handled by pointer; the leading fields are declared so `qp_num`
/// can be read, and the tail (a mutex, a condvar, `events_completed`) is
/// padding sized so the total matches C.
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
    _tail: [u64; 12],
}

#[repr(C)]
pub struct ibv_sge {
    pub addr: u64,
    pub length: u32,
    pub lkey: u32,
}

/// Send work request. Built in Rust and passed unchanged through
/// [`certus_te_post_send`], so every field up to and including `wr` must sit
/// exactly where C expects it — see the layout check.
#[repr(C)]
pub struct ibv_send_wr {
    pub wr_id: u64,
    pub next: *mut ibv_send_wr,
    pub sg_list: *mut ibv_sge,
    pub num_sge: c_int,
    pub opcode: c_int,
    pub send_flags: c_int,
    /// `imm_data` / `invalidate_rkey` union; unused (no `*_WITH_IMM` opcodes).
    pub imm_data: u32,
    pub wr: ibv_send_wr_union,
    /// `qp_type.xrc.remote_srqn`; unused for RC.
    pub qp_type: u32,
    /// The `bind_mw` / `tso` union; unused, must be zero.
    _tail: [u64; 6],
}

/// The `wr` union of `ibv_send_wr`. Sized by its largest C member, `atomic`
/// (three `u64`s and a `u32`), which only the padding view represents.
#[repr(C)]
pub union ibv_send_wr_union {
    pub rdma: ibv_send_wr_rdma,
    _pad: [u64; 4],
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

// --- rdma_cm structs ---------------------------------------------------------

/// Only ever handled by pointer. The four leading pointers are declared; the
/// rest (route, port space, `port_num`, the id's own `pd`, ...) is padding, and
/// anything in it must be read through a C accessor, never declared here by
/// guesswork.
#[repr(C)]
pub struct rdma_cm_id {
    pub verbs: *mut ibv_context,
    pub channel: *mut rdma_event_channel,
    pub context: *mut c_void,
    pub qp: *mut ibv_qp,
    _tail: [u64; 48],
}

#[repr(C)]
pub struct rdma_cm_event {
    pub id: *mut rdma_cm_id,
    pub listen_id: *mut rdma_cm_id,
    pub event: c_int,
    pub status: c_int,
    pub param: rdma_conn_param_event,
}

/// The `param` union of `rdma_cm_event`: `conn` or `ud` (`rdma_ud_param`, the
/// larger, which only the padding view represents).
#[repr(C)]
pub union rdma_conn_param_event {
    pub conn: rdma_conn_param,
    _pad: [u64; 7],
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

// --- socket address structs --------------------------------------------------

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

// --- functions ---------------------------------------------------------------

extern "C" {
    // Device management. `ibv_query_port` is deliberately absent: the exported
    // symbol is the compat entry point; use `certus_te_query_port`.
    pub fn ibv_get_device_list(num_devices: *mut c_int) -> *mut *mut ibv_device;
    pub fn ibv_free_device_list(list: *mut *mut ibv_device);
    pub fn ibv_get_device_name(device: *mut ibv_device) -> *const c_char;
    pub fn ibv_open_device(device: *mut ibv_device) -> *mut ibv_context;
    pub fn ibv_close_device(context: *mut ibv_context) -> c_int;
    pub fn ibv_query_gid(
        context: *mut ibv_context,
        port_num: u8,
        index: c_int,
        gid: *mut ibv_gid,
    ) -> c_int;

    // Protection domain — owned by the endpoint, one per device (D17).
    pub fn ibv_alloc_pd(context: *mut ibv_context) -> *mut ibv_pd;
    pub fn ibv_dealloc_pd(pd: *mut ibv_pd) -> c_int;

    // Memory registration.
    pub fn ibv_reg_mr(
        pd: *mut ibv_pd,
        addr: *mut c_void,
        length: usize,
        access: c_int,
    ) -> *mut ibv_mr;
    pub fn ibv_dereg_mr(mr: *mut ibv_mr) -> c_int;

    // Completion queue.
    pub fn ibv_create_cq(
        context: *mut ibv_context,
        cqe: c_int,
        cq_context: *mut c_void,
        channel: *mut c_void,
        comp_vector: c_int,
    ) -> *mut ibv_cq;
    pub fn ibv_destroy_cq(cq: *mut ibv_cq) -> c_int;

    // Queue pair, created on a CM id with the endpoint's PD (D17).
    pub fn rdma_create_qp(
        id: *mut rdma_cm_id,
        pd: *mut ibv_pd,
        qp_init_attr: *mut ibv_qp_init_attr,
    ) -> c_int;
    pub fn rdma_destroy_qp(id: *mut rdma_cm_id);

    // Connection manager: ids and events.
    pub fn rdma_create_event_channel() -> *mut rdma_event_channel;
    pub fn rdma_destroy_event_channel(channel: *mut rdma_event_channel);
    pub fn rdma_create_id(
        channel: *mut rdma_event_channel,
        id: *mut *mut rdma_cm_id,
        context: *mut c_void,
        ps: c_int,
    ) -> c_int;
    pub fn rdma_destroy_id(id: *mut rdma_cm_id) -> c_int;
    pub fn rdma_get_cm_event(
        channel: *mut rdma_event_channel,
        event: *mut *mut rdma_cm_event,
    ) -> c_int;
    pub fn rdma_ack_cm_event(event: *mut rdma_cm_event) -> c_int;

    // Connection manager: active side.
    pub fn rdma_resolve_addr(
        id: *mut rdma_cm_id,
        src_addr: *mut sockaddr,
        dst_addr: *mut sockaddr,
        timeout_ms: c_int,
    ) -> c_int;
    pub fn rdma_resolve_route(id: *mut rdma_cm_id, timeout_ms: c_int) -> c_int;
    pub fn rdma_connect(id: *mut rdma_cm_id, conn_param: *mut rdma_conn_param) -> c_int;

    // Connection manager: passive side. `rdma_reject`'s private data is how the
    // simultaneous-connect loser is told why (D23).
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

    // Pass-through shims for static-inline verbs (src/wrapper.c, T008). Each
    // forwards its arguments unchanged and returns the inline call's result.
    pub fn certus_te_post_send(
        qp: *mut ibv_qp,
        wr: *mut ibv_send_wr,
        bad_wr: *mut *mut ibv_send_wr,
    ) -> c_int;
    pub fn certus_te_post_recv(
        qp: *mut ibv_qp,
        wr: *mut ibv_recv_wr,
        bad_wr: *mut *mut ibv_recv_wr,
    ) -> c_int;
    pub fn certus_te_poll_cq(cq: *mut ibv_cq, num_entries: c_int, wc: *mut ibv_wc) -> c_int;
    pub fn certus_te_query_port(
        context: *mut ibv_context,
        port_num: u8,
        port_attr: *mut ibv_port_attr,
    ) -> c_int;

    // Shim over `ibv_modify_qp(.., IBV_QP_STATE)` so Rust need not bind the
    // large `ibv_qp_attr`. Legal from any state; this is the step that makes a
    // late write impossible before a landing slot is reclaimed (FR-023).
    pub fn certus_te_qp_to_error(qp: *mut ibv_qp) -> c_int;

    // The QP's state as the device reports it, via `ibv_query_qp`; negative
    // errno on failure. `ibv_qp::state` is only libibverbs' cache of the last
    // state *software* set, so it misses the device forcing a QP to ERROR.
    pub fn certus_te_query_qp_state(qp: *mut ibv_qp) -> c_int;

    // libc byte-order and address helpers.
    pub fn htons(hostshort: u16) -> u16;
    pub fn ntohs(netshort: u16) -> u16;
    pub fn inet_addr(cp: *const c_char) -> u32;
}

// --- layout check ------------------------------------------------------------

/// Byte offset of a field, usable in `const`. `core::mem::offset_of!` would do,
/// but it is stable only from Rust 1.77 and the workspace MSRV is 1.75.
macro_rules! field_offset {
    ($t:ty, $f:ident) => {{
        let u = ::core::mem::MaybeUninit::<$t>::uninit();
        let base = u.as_ptr();
        // SAFETY: `addr_of!` takes the field's address without reading it or
        // creating a reference, so the uninitialised value is never observed,
        // and both pointers lie within the same allocation, as `offset_from`
        // requires.
        let off = unsafe {
            (::core::ptr::addr_of!((*base).$f) as *const u8).offset_from(base as *const u8)
        };
        off as usize
    }};
}

// Pinned to rdma-core 2510.0.11 on x86_64. The same numbers are
// `_Static_assert`ed against the real headers in src/wrapper.c (T008).
const _: () = {
    use core::mem::size_of;

    assert!(size_of::<ibv_port_attr>() == 56);
    assert!(field_offset!(ibv_port_attr, state) == 0);
    assert!(field_offset!(ibv_port_attr, link_layer) == 46);
    assert!(field_offset!(ibv_port_attr, port_cap_flags2) == 48);
    assert!(field_offset!(ibv_port_attr, active_speed_ex) == 52);

    assert!(size_of::<ibv_gid>() == 16);

    assert!(size_of::<ibv_mr>() == 48);
    assert!(field_offset!(ibv_mr, lkey) == 36);
    assert!(field_offset!(ibv_mr, rkey) == 40);

    assert!(size_of::<ibv_qp_cap>() == 20);
    assert!(size_of::<ibv_qp_init_attr>() == 64);
    assert!(field_offset!(ibv_qp_init_attr, cap) == 32);
    assert!(field_offset!(ibv_qp_init_attr, qp_type) == 52);
    assert!(field_offset!(ibv_qp_init_attr, sq_sig_all) == 56);

    assert!(size_of::<ibv_qp>() == 160);
    assert!(field_offset!(ibv_qp, handle) == 48);
    assert!(field_offset!(ibv_qp, qp_num) == 52);
    assert!(field_offset!(ibv_qp, state) == 56);
    assert!(field_offset!(ibv_qp, qp_type) == 60);

    assert!(size_of::<ibv_sge>() == 16);

    assert!(size_of::<ibv_send_wr>() == 128);
    assert!(field_offset!(ibv_send_wr, sg_list) == 16);
    assert!(field_offset!(ibv_send_wr, opcode) == 28);
    assert!(field_offset!(ibv_send_wr, send_flags) == 32);
    assert!(field_offset!(ibv_send_wr, imm_data) == 36);
    assert!(field_offset!(ibv_send_wr, wr) == 40);
    assert!(size_of::<ibv_send_wr_union>() == 32);
    assert!(field_offset!(ibv_send_wr, qp_type) == 72);

    assert!(size_of::<ibv_recv_wr>() == 32);
    assert!(field_offset!(ibv_recv_wr, num_sge) == 24);

    assert!(size_of::<ibv_wc>() == 48);
    assert!(field_offset!(ibv_wc, byte_len) == 20);
    assert!(field_offset!(ibv_wc, qp_num) == 28);
    assert!(field_offset!(ibv_wc, wc_flags) == 36);
    assert!(field_offset!(ibv_wc, dlid_path_bits) == 45);

    assert!(size_of::<rdma_cm_id>() == 416);
    assert!(field_offset!(rdma_cm_id, channel) == 8);
    assert!(field_offset!(rdma_cm_id, context) == 16);
    assert!(field_offset!(rdma_cm_id, qp) == 24);

    assert!(size_of::<rdma_cm_event>() == 80);
    assert!(field_offset!(rdma_cm_event, event) == 16);
    assert!(field_offset!(rdma_cm_event, status) == 20);
    assert!(field_offset!(rdma_cm_event, param) == 24);
    assert!(size_of::<rdma_conn_param_event>() == 56);

    assert!(size_of::<rdma_conn_param>() == 24);
    assert!(field_offset!(rdma_conn_param, private_data_len) == 8);
    assert!(field_offset!(rdma_conn_param, rnr_retry_count) == 13);
    assert!(field_offset!(rdma_conn_param, qp_num) == 16);

    assert!(size_of::<sockaddr_in>() == 16);
    assert!(size_of::<sockaddr>() == 16);
};
