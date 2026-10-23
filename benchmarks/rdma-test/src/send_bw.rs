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
    let send_mr = conn.register_mr(size)?;

    // Wait for server to signal it has pre-posted recvs
    let mut ready_mr = conn.register_mr(4)?;
    conn.recv_msg(&mut ready_mr)?;
    info!("Server ready, starting Send throughput");

    info!("Running {} warmup + {} iterations (Send)", warmup, iterations);
    for _ in 0..warmup {
        conn.send_msg(&send_mr, size)?;
    }

    let start = Instant::now();
    for _ in 0..iterations {
        conn.send_msg(&send_mr, size)?;
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
    let mut recv_mr = conn.register_mr(size)?;
    let total = warmup + iterations;

    // Pre-post a window of recvs before signaling client
    let window = RECV_WINDOW.min(total);
    for _ in 0..window {
        conn.post_recv_wr(&mut recv_mr)?;
    }

    // Signal client we're ready
    let ready_mr = conn.register_mr(4)?;
    conn.send_msg(&ready_mr, 4)?;

    info!(
        "Receiving {} messages (Send throughput server)",
        total
    );

    let remaining = total.saturating_sub(window);
    for _ in 0..remaining {
        conn.poll_completion_with_retry()?;
        conn.post_recv_wr(&mut recv_mr)?;
    }
    for _ in 0..window {
        conn.poll_completion_with_retry()?;
    }

    info!("Send throughput test complete (server side)");
    Ok(())
}
