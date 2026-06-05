use anyhow::Result;
use tracing::info;

use crate::latency;
use crate::rdma;
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
        TestType::Throughput => {
            info!("Running throughput test (server side)");
            throughput::run_server(&conn, size)?;
        }
        TestType::Latency => {
            info!("Running latency test (server side)");
            latency::run_server(&conn, size, iterations, warmup)?;
        }
        TestType::All => {
            info!("Running throughput test (server side)");
            throughput::run_server(&conn, size)?;
            info!("Running latency test (server side)");
            latency::run_server(&conn, size, iterations, warmup)?;
        }
    }

    info!("Server finished");
    Ok(())
}
