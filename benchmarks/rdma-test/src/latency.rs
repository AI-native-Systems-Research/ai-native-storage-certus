use std::time::Instant;

use anyhow::Result;
use tracing::info;

use crate::rdma::RdmaConnection;
use crate::stats::LatencyStats;

pub fn run_client(
    conn: &RdmaConnection,
    size: usize,
    iterations: u64,
    warmup: u64,
) -> Result<LatencyStats> {
    let send_mr = conn.register_mr(size)?;
    let mut recv_mr = conn.register_mr(size)?;

    info!("Running {} warmup iterations (Send/Recv)", warmup);
    for _ in 0..warmup {
        conn.send_msg(&send_mr, size)?;
        conn.recv_msg(&mut recv_mr)?;
    }

    info!("Running {} latency iterations (Send/Recv)", iterations);
    let mut samples = Vec::with_capacity(iterations as usize);

    for _ in 0..iterations {
        let start = Instant::now();
        conn.send_msg(&send_mr, size)?;
        conn.recv_msg(&mut recv_mr)?;
        let rtt = start.elapsed();
        samples.push(rtt / 2);
    }

    let stats = LatencyStats::compute(&mut samples).expect("should have samples");
    Ok(stats)
}

pub fn run_server(conn: &RdmaConnection, size: usize, iterations: u64, warmup: u64) -> Result<()> {
    let send_mr = conn.register_mr(size)?;
    let mut recv_mr = conn.register_mr(size)?;

    let total = warmup + iterations;

    info!("Server echoing {} messages ({}+{} warmup)", total, iterations, warmup);
    for _ in 0..total {
        conn.recv_msg(&mut recv_mr)?;
        conn.send_msg(&send_mr, size)?;
    }

    info!("Latency test complete (server side)");
    Ok(())
}
