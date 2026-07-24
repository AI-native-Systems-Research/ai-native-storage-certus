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

/* The device async-event fd for `ctx`. Diagnostic instrumentation adds this fd
 * to the accept loop's epoll set so QP async errors (e.g. IBV_EVENT_QP_FATAL)
 * on a child queue pair are observed the moment the HCA raises them, rather
 * than only inferred from the initiator's transport retry exhaustion. */
int responder_async_fd(struct ibv_context *ctx) {
    return ctx->async_fd;
}

/* Drain one pending async event from `ctx` (the fd must be readable / O_NONBLOCK
 * so this does not block). Returns the ibv_event_type on success, or -1 when no
 * event is queued. For QP-scoped events `*out_qp_num` receives the offending
 * QP's number (0 otherwise). The event is acked before returning. */
int responder_drain_async_event(struct ibv_context *ctx, unsigned int *out_qp_num) {
    struct ibv_async_event ev;
    if (ibv_get_async_event(ctx, &ev) != 0) {
        return -1; /* EAGAIN: nothing queued */
    }
    unsigned int qp_num = 0;
    switch (ev.event_type) {
        case IBV_EVENT_QP_FATAL:
        case IBV_EVENT_QP_REQ_ERR:
        case IBV_EVENT_QP_ACCESS_ERR:
        case IBV_EVENT_COMM_EST:
        case IBV_EVENT_SQ_DRAINED:
        case IBV_EVENT_PATH_MIG:
        case IBV_EVENT_PATH_MIG_ERR:
        case IBV_EVENT_QP_LAST_WQE_REACHED:
            if (ev.element.qp) {
                qp_num = ev.element.qp->qp_num;
            }
            break;
        default:
            break;
    }
    if (out_qp_num) {
        *out_qp_num = qp_num;
    }
    int t = ev.event_type;
    ibv_ack_async_event(&ev);
    return t;
}
