#include <infiniband/verbs.h>
#include <stdint.h>

int rdma_test_poll_cq(struct ibv_cq *cq, int num_entries, struct ibv_wc *wc) {
    return ibv_poll_cq(cq, num_entries, wc);
}

// Helper: post an RDMA Write using properly constructed C structs
int rdma_test_rdma_write(struct ibv_qp *qp, void *buf, uint32_t len,
                         uint32_t lkey, uint64_t remote_addr, uint32_t rkey) {
    struct ibv_sge sge = {
        .addr = (uint64_t)buf,
        .length = len,
        .lkey = lkey,
    };
    struct ibv_send_wr wr = {
        .sg_list = &sge,
        .num_sge = 1,
        .opcode = IBV_WR_RDMA_WRITE,
        .send_flags = IBV_SEND_SIGNALED,
        .wr.rdma = {
            .remote_addr = remote_addr,
            .rkey = rkey,
        },
    };
    struct ibv_send_wr *bad_wr = NULL;
    return ibv_post_send(qp, &wr, &bad_wr);
}
