use anyhow::Result;
use tracing::info;

use crate::latency;
use crate::rdma;
use crate::recv_bw;
use crate::send_bw;
use crate::throughput;
use crate::TestType;

pub fn run(
    address: &str,
    port: u16,
    test: TestType,
    size: usize,
    iterations: u64,
    warmup: u64,
) -> Result<()> {
    let conn = rdma::server_connect(address, port)?;

    match test {
        TestType::Write => {
            info!("Running RDMA Write test (server side)");
            throughput::run_server(&conn, size)?;
        }
        TestType::Read => {
            info!("Running RDMA Read test (server side)");
            throughput::run_server(&conn, size)?;
        }
        TestType::Send => {
            info!("Running Send throughput test (server side)");
            send_bw::run_server(&conn, size, iterations, warmup)?;
        }
        TestType::Recv => {
            info!("Running Recv throughput test (server side)");
            recv_bw::run_server(&conn, size, iterations, warmup)?;
        }
        TestType::Latency => {
            info!("Running latency test (server side)");
            latency::run_server(&conn, size, iterations, warmup)?;
        }
        TestType::All => {
            info!("Running RDMA Write test (server side)");
            throughput::run_server(&conn, size)?;
            info!("Running RDMA Read test (server side)");
            throughput::run_server(&conn, size)?;
            info!("Running Send throughput test (server side)");
            send_bw::run_server(&conn, size, iterations, warmup)?;
            info!("Running Recv throughput test (server side)");
            recv_bw::run_server(&conn, size, iterations, warmup)?;
            info!("Running latency test (server side)");
            latency::run_server(&conn, size, iterations, warmup)?;
        }
    }

    info!("Server finished");
    Ok(())
}
