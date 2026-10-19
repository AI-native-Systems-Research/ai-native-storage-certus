/* C shims for certus-transfer-endpoint, and the C half of the FFI layout check.
 *
 * Built by build.rs only with the `rdma` feature (constitution Principle VIII).
 *
 * The shims exist because ibv_post_send, ibv_post_recv, ibv_poll_cq and the
 * non-compat ibv_query_port are static inline in verbs.h, so there is no symbol
 * to bind from Rust. Each forwards its arguments unchanged: work requests are
 * built in Rust (src/ffi.rs) so that chained lists and an accurate bad_wr reach
 * the caller. Names carry a certus_te_ prefix so they cannot collide with a
 * predecessor crate's shim when both are linked into one binary.
 */

#include <stddef.h>
#include <string.h>
#include <netinet/in.h>
#include <infiniband/verbs.h>
#include <rdma/rdma_cma.h>

/* --- layout check ---------------------------------------------------------
 *
 * The same numbers src/ffi.rs asserts about its Rust declarations, asserted
 * here about the real headers. Together they make a Rust/C disagreement a
 * build failure: Rust checks its declarations against these numbers, C checks
 * the headers against them. Change a number on one side, change it on both.
 * Pinned to rdma-core 2510.0.11 on x86_64.
 */
#define SIZE(t, n) _Static_assert(sizeof(t) == (n), "sizeof(" #t ") != " #n " -- update src/ffi.rs")
#define OFF(t, f, n) \
    _Static_assert(offsetof(t, f) == (n), "offsetof(" #t ", " #f ") != " #n " -- update src/ffi.rs")

SIZE(struct ibv_port_attr, 56);
OFF(struct ibv_port_attr, state, 0);
OFF(struct ibv_port_attr, link_layer, 46);
OFF(struct ibv_port_attr, port_cap_flags2, 48);
OFF(struct ibv_port_attr, active_speed_ex, 52);

SIZE(union ibv_gid, 16);

SIZE(struct ibv_mr, 48);
OFF(struct ibv_mr, lkey, 36);
OFF(struct ibv_mr, rkey, 40);

SIZE(struct ibv_qp_cap, 20);
SIZE(struct ibv_qp_init_attr, 64);
OFF(struct ibv_qp_init_attr, cap, 32);
OFF(struct ibv_qp_init_attr, qp_type, 52);
OFF(struct ibv_qp_init_attr, sq_sig_all, 56);

SIZE(struct ibv_qp, 160);
OFF(struct ibv_qp, handle, 48);
OFF(struct ibv_qp, qp_num, 52);
OFF(struct ibv_qp, state, 56);
OFF(struct ibv_qp, qp_type, 60);

SIZE(struct ibv_sge, 16);

SIZE(struct ibv_send_wr, 128);
OFF(struct ibv_send_wr, sg_list, 16);
OFF(struct ibv_send_wr, opcode, 28);
OFF(struct ibv_send_wr, send_flags, 32);
OFF(struct ibv_send_wr, imm_data, 36);
OFF(struct ibv_send_wr, wr, 40);
_Static_assert(sizeof(((struct ibv_send_wr *)0)->wr) == 32, "ibv_send_wr.wr != 32 -- update src/ffi.rs");
OFF(struct ibv_send_wr, qp_type, 72);

SIZE(struct ibv_recv_wr, 32);
OFF(struct ibv_recv_wr, num_sge, 24);

SIZE(struct ibv_wc, 48);
OFF(struct ibv_wc, byte_len, 20);
OFF(struct ibv_wc, qp_num, 28);
OFF(struct ibv_wc, wc_flags, 36);
OFF(struct ibv_wc, dlid_path_bits, 45);

SIZE(struct rdma_cm_id, 416);
OFF(struct rdma_cm_id, channel, 8);
OFF(struct rdma_cm_id, context, 16);
OFF(struct rdma_cm_id, qp, 24);

SIZE(struct rdma_cm_event, 80);
OFF(struct rdma_cm_event, event, 16);
OFF(struct rdma_cm_event, status, 20);
OFF(struct rdma_cm_event, param, 24);
_Static_assert(sizeof(((struct rdma_cm_event *)0)->param) == 56, "rdma_cm_event.param != 56 -- update src/ffi.rs");

SIZE(struct rdma_conn_param, 24);
OFF(struct rdma_conn_param, private_data_len, 8);
OFF(struct rdma_conn_param, rnr_retry_count, 13);
OFF(struct rdma_conn_param, qp_num, 16);

SIZE(struct sockaddr_in, 16);
SIZE(struct sockaddr, 16);

/* Enum values src/ffi.rs declares as constants. The predecessors had the
 * IBV_WR_* numbering wrong, which is why these are checked too. */
_Static_assert(IBV_WR_RDMA_WRITE == 0, "IBV_WR_RDMA_WRITE");
_Static_assert(IBV_WR_SEND == 2, "IBV_WR_SEND");
_Static_assert(IBV_SEND_SIGNALED == 2, "IBV_SEND_SIGNALED");
_Static_assert(IBV_WC_SUCCESS == 0, "IBV_WC_SUCCESS");
_Static_assert(IBV_WC_WR_FLUSH_ERR == 5, "IBV_WC_WR_FLUSH_ERR");
_Static_assert(IBV_WC_SEND == 0, "IBV_WC_SEND");
_Static_assert(IBV_WC_RDMA_WRITE == 1, "IBV_WC_RDMA_WRITE");
_Static_assert(IBV_WC_RECV == 128, "IBV_WC_RECV");
_Static_assert(IBV_QPT_RC == 2, "IBV_QPT_RC");
_Static_assert(IBV_QPS_ERR == 6, "IBV_QPS_ERR");
_Static_assert(IBV_PORT_ACTIVE == 4, "IBV_PORT_ACTIVE");
_Static_assert(IBV_ACCESS_REMOTE_WRITE == 2, "IBV_ACCESS_REMOTE_WRITE");
_Static_assert(RDMA_PS_TCP == 0x0106, "RDMA_PS_TCP");
_Static_assert(RDMA_CM_EVENT_DISCONNECTED == 10, "RDMA_CM_EVENT_DISCONNECTED");

/* --- pass-through shims ---------------------------------------------------- */

int certus_te_post_send(struct ibv_qp *qp, struct ibv_send_wr *wr, struct ibv_send_wr **bad_wr) {
    return ibv_post_send(qp, wr, bad_wr);
}

int certus_te_post_recv(struct ibv_qp *qp, struct ibv_recv_wr *wr, struct ibv_recv_wr **bad_wr) {
    return ibv_post_recv(qp, wr, bad_wr);
}

int certus_te_poll_cq(struct ibv_cq *cq, int num_entries, struct ibv_wc *wc) {
    return ibv_poll_cq(cq, num_entries, wc);
}

/* The verbs.h macro, not the exported compat symbol: fills the full struct,
 * including active_speed_ex. */
int certus_te_query_port(struct ibv_context *context, uint8_t port_num, struct ibv_port_attr *port_attr) {
    return ibv_query_port(context, port_num, port_attr);
}

/* Transition `qp` into ERROR. Legal from any state; afterwards the QP NAKs any
 * late one-sided write, which is what lets a landing slot be reclaimed after a
 * deadline (FR-023). Exposed as one argument so Rust need not bind the large
 * struct ibv_qp_attr. Returns 0 or the errno-style result of ibv_modify_qp. */
int certus_te_qp_to_error(struct ibv_qp *qp) {
    struct ibv_qp_attr attr;
    memset(&attr, 0, sizeof(attr));
    attr.qp_state = IBV_QPS_ERR;
    return ibv_modify_qp(qp, &attr, IBV_QP_STATE);
}
