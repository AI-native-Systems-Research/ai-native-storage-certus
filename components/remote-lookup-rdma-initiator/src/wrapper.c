#include <infiniband/verbs.h>
#include <stdint.h>

int rdma_test_poll_cq(struct ibv_cq *cq, int num_entries, struct ibv_wc *wc) {
    return ibv_poll_cq(cq, num_entries, wc);
}

// Helper: post an RDMA Write using properly constructed C structs.
//
// Posting only — the caller reaps the completion separately, which is what lets
// a whole window of writes be outstanding at once. `wr_id` is echoed back in the
// work completion so the reaper can correlate each completion with the write
// that produced it; callers pass the write's index within its window.
//
// Every write stays IBV_SEND_SIGNALED: per-WR completions are what make that
// correlation possible.
int rdma_test_rdma_write(struct ibv_qp *qp, void *buf, uint32_t len,
                         uint32_t lkey, uint64_t remote_addr, uint32_t rkey,
                         uint64_t wr_id) {
    struct ibv_sge sge = {
        .addr = (uint64_t)buf,
        .length = len,
        .lkey = lkey,
    };
    struct ibv_send_wr wr = {
        .wr_id = wr_id,
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
