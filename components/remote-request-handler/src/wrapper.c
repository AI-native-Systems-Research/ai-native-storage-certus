#include <infiniband/verbs.h>
#include <stdint.h>
int rdma_test_post_send(struct ibv_qp *qp, struct ibv_send_wr *wr,
                        struct ibv_send_wr **bad_wr) {
    return ibv_post_send(qp, wr, bad_wr);
}

int rdma_test_post_recv(struct ibv_qp *qp, struct ibv_recv_wr *wr,
                        struct ibv_recv_wr **bad_wr) {
    return ibv_post_recv(qp, wr, bad_wr);
}

int rdma_test_poll_cq(struct ibv_cq *cq, int num_entries, struct ibv_wc *wc) {
    return ibv_poll_cq(cq, num_entries, wc);
}

// Helper: post a send using properly constructed C structs
int rdma_test_send_msg(struct ibv_qp *qp, void *buf, uint32_t len, uint32_t lkey) {
    struct ibv_sge sge = {
        .addr = (uint64_t)buf,
        .length = len,
        .lkey = lkey,
    };
    struct ibv_send_wr wr = {
        .sg_list = &sge,
        .num_sge = 1,
        .opcode = IBV_WR_SEND,
        .send_flags = IBV_SEND_SIGNALED,
    };
    struct ibv_send_wr *bad_wr = NULL;
    return ibv_post_send(qp, &wr, &bad_wr);
}

// Helper: post a recv using properly constructed C structs
int rdma_test_recv_msg(struct ibv_qp *qp, void *buf, uint32_t len, uint32_t lkey) {
    struct ibv_sge sge = {
        .addr = (uint64_t)buf,
        .length = len,
        .lkey = lkey,
    };
    struct ibv_recv_wr wr = {
        .sg_list = &sge,
        .num_sge = 1,
    };
    struct ibv_recv_wr *bad_wr = NULL;
    return ibv_post_recv(qp, &wr, &bad_wr);
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

// Helper: post an RDMA Write without signaling completion
int rdma_test_rdma_write_unsignaled(struct ibv_qp *qp, void *buf, uint32_t len,
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
        .send_flags = 0,  // no IBV_SEND_SIGNALED
        .wr.rdma = {
            .remote_addr = remote_addr,
            .rkey = rkey,
        },
    };
    struct ibv_send_wr *bad_wr = NULL;
    return ibv_post_send(qp, &wr, &bad_wr);
}

// Helper: post an RDMA Read using properly constructed C structs
int rdma_test_rdma_read(struct ibv_qp *qp, void *buf, uint32_t len,
                        uint32_t lkey, uint64_t remote_addr, uint32_t rkey) {
    struct ibv_sge sge = {
        .addr = (uint64_t)buf,
        .length = len,
        .lkey = lkey,
    };
    struct ibv_send_wr wr = {
        .sg_list = &sge,
        .num_sge = 1,
        .opcode = IBV_WR_RDMA_READ,
        .send_flags = IBV_SEND_SIGNALED,
        .wr.rdma = {
            .remote_addr = remote_addr,
            .rkey = rkey,
        },
    };
    struct ibv_send_wr *bad_wr = NULL;
    return ibv_post_send(qp, &wr, &bad_wr);
}
