#!/usr/bin/env bash
# RDMA write bandwidth node2 -> node5, host and GPU memory on each end.
# node5 = server (destination), node2 = client. mlx5_0 port 1, RoCE v2 GID 3.
# GPUs: node2 GPU0 (41:00.0, NUMA0 = NIC's NUMA), node5 GPU0 (A1:00.0, NUMA1, NIC on NUMA0).
set -u
OPTS="-d mlx5_0 -i 1 -x 3 -s 1048576 -D 5 -q 4 --report_gbits -F"
run() {   # run <label> <server extra> <client extra>
    local label=$1 sx=$2 cx=$3
    ssh -o BatchMode=yes node5 "ib_write_bw $OPTS $sx" > /tmp/gdr-srv.log 2>&1 &
    sleep 3
    local out
    out=$(ib_write_bw $OPTS $cx 10.0.0.105 2>&1)
    wait
    local bw
    bw=$(echo "$out" | awk '/^ *1048576/ {print $4}')
    printf "%-28s %s Gb/s\n" "$label" "${bw:-FAILED}"
    [ -n "$bw" ] || { echo "$out" | tail -5; tail -5 /tmp/gdr-srv.log; }
}
run "host -> host"            ""             ""
run "node2 GPU0 -> host"      ""             "--use_cuda=0"
run "host -> node5 GPU"       "--use_cuda=0" ""
run "node2 GPU0 -> node5 GPU" "--use_cuda=0" "--use_cuda=0"
