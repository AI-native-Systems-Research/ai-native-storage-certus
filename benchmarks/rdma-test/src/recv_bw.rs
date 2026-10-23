use std::time::Instant;

use anyhow::Result;
use tracing::info;

use crate::rdma::RdmaConnection;
use crate::stats::ThroughputStats;

const RECV_WINDOW: u64 = 32;

pub fn run_client(
    conn: &RdmaConnection,
    size: usize,
    iterations: u64,
    warmup: u64,
) -> Result<ThroughputStats> {
    let mut recv_mr = conn.register_mr(size)?;
    let total = warmup + iterations;

    // Pre-post recv window
    let window = RECV_WINDOW.min(total);
    for _ in 0..window {
        conn.post_recv_wr(&mut recv_mr)?;
    }

    // Signal server we're ready to receive
    let ready_mr = conn.register_mr(4)?;
    conn.send_msg(&ready_mr, 4)?;

    info!("Running {} warmup + {} iterations (Recv)", warmup, iterations);

    // Receive warmup
    for _ in 0..warmup {
        conn.poll_completion_with_retry()?;
        conn.post_recv_wr(&mut recv_mr)?;
    }

    // Timed main run
    let start = Instant::now();

    let repost_count = iterations.saturating_sub(window);
    for _ in 0..repost_count {
        conn.poll_completion_with_retry()?;
        conn.post_recv_wr(&mut recv_mr)?;
    }
    for _ in 0..window.min(iterations) {
        conn.poll_completion_with_retry()?;
    }

    let elapsed = start.elapsed();
    let total_bytes = size as u64 * iterations;

    Ok(ThroughputStats::compute(total_bytes, iterations, elapsed))
}

pub fn run_server(
    conn: &RdmaConnection,
    size: usize,
    iterations: u64,
    warmup: u64,
) -> Result<()> {
    let send_mr = conn.register_mr(size)?;
    let total = warmup + iterations;

    // Wait for client to signal ready (it has pre-posted recvs)
    let mut signal_mr = conn.register_mr(4)?;
    conn.recv_msg(&mut signal_mr)?;

    info!("Sending {} messages (Recv throughput server)", total);
    for _ in 0..total {
        conn.send_msg(&send_mr, size)?;
    }

    info!("Recv throughput test complete (server side)");
    Ok(())
}
