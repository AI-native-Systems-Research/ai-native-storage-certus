use std::time::Instant;

use anyhow::Result;
use tracing::info;

use crate::rdma::{self, RdmaConnection};
use crate::stats::ThroughputStats;

pub fn run_client(
    conn: &RdmaConnection,
    size: usize,
    iterations: u64,
    warmup: u64,
) -> Result<ThroughputStats> {
    let send_mr = conn.register_mr(size)?;

    info!("Requesting remote MR info from server");
    let remote_info = rdma::exchange_mr_info_client(conn)?;
    info!(
        "Got remote MR: addr=0x{:x}, rkey=0x{:x}, size={}",
        remote_info.addr, remote_info.rkey, remote_info.size
    );

    info!("Running {} warmup iterations (RDMA Write)", warmup);
    for _ in 0..warmup {
        conn.rdma_write(&send_mr, size, &remote_info)?;
    }

    info!("Running {} throughput iterations (RDMA Write)", iterations);
    let start = Instant::now();

    for _ in 0..iterations {
        conn.rdma_write(&send_mr, size, &remote_info)?;
    }

    let elapsed = start.elapsed();
    let total_bytes = size as u64 * iterations;

    let mut done_mr = conn.register_mr(4)?;
    done_mr.buf[..4].copy_from_slice(b"DONE");
    conn.send_msg(&done_mr, 4)?;

    Ok(ThroughputStats::compute(total_bytes, iterations, elapsed))
}

pub fn run_server(conn: &RdmaConnection, size: usize) -> Result<()> {
    let recv_mr = conn.register_mr(size)?;

    info!("Sending MR info to client");
    rdma::exchange_mr_info_server(conn, &recv_mr)?;

    let mut done_mr = conn.register_mr(4)?;
    info!("Waiting for client completion signal");
    conn.recv_msg(&mut done_mr)?;

    info!("Throughput test complete (server side)");
    Ok(())
}
