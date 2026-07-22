/* C shims for the RDMA responder.
 *
 * The queue-pair ERROR transition is the load-bearing safety step of
 * teardown-before-reclaim: a QP in IBV_QPS_ERR NAKs any late one-sided writes so
 * they cannot land in memory that is about to be reclaimed. It is exposed as a
 * one-argument helper so Rust need not bind the large `struct ibv_qp_attr`.
 */

#include <infiniband/verbs.h>
#include <string.h>

/* Transition `qp` into the ERROR state. Returns 0 on success, else the
 * errno-style return of ibv_modify_qp. The transition is legal from any QP
 * state and fails only on a fatal HCA/programming fault. */
int responder_qp_to_error(struct ibv_qp *qp) {
    struct ibv_qp_attr attr;
    memset(&attr, 0, sizeof(attr));
    attr.qp_state = IBV_QPS_ERR;
    return ibv_modify_qp(qp, &attr, IBV_QP_STATE);
}
