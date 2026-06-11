use anyhow::Result;
use tracing::info;

use crate::latency;
use crate::output::{self, OutputFormat, TestConfig, TestOutput, TestResults};
use crate::rdma;
use crate::recv_bw;
use crate::send_bw;
use crate::stats::{LatencyStats, ThroughputStats};
use crate::throughput;
use crate::TestType;

#[allow(clippy::too_many_arguments)]
pub fn run(
    address: &str,
    port: u16,
    test: TestType,
    size: usize,
    iterations: u64,
    warmup: u64,
    output_format: OutputFormat,
    device_name: &str,
    transport: &str,
) -> Result<()> {
    let conn = rdma::client_connect(address, port)?;

    let mut write_result: Option<ThroughputStats> = None;
    let mut read_result: Option<ThroughputStats> = None;
    let mut send_result: Option<ThroughputStats> = None;
    let mut recv_result: Option<ThroughputStats> = None;
    let mut latency_result: Option<LatencyStats> = None;

    match test {
        TestType::Write => {
            info!("Running RDMA Write throughput test");
            let stats = throughput::run_write_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations, "RDMA Write");
            }
            write_result = Some(stats);
        }
        TestType::Read => {
            info!("Running RDMA Read throughput test");
            let stats = throughput::run_read_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations, "RDMA Read");
            }
            read_result = Some(stats);
        }
        TestType::Send => {
            info!("Running Send throughput test");
            let stats = send_bw::run_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations, "Send");
            }
            send_result = Some(stats);
        }
        TestType::Recv => {
            info!("Running Recv throughput test");
            let stats = recv_bw::run_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations, "Recv");
            }
            recv_result = Some(stats);
        }
        TestType::Latency => {
            info!("Running latency test");
            let stats = latency::run_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_latency_human(&stats, size);
            }
            latency_result = Some(stats);
        }
        TestType::All => {
            info!("Running RDMA Write throughput test");
            let stats = throughput::run_write_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations, "RDMA Write");
            }
            write_result = Some(stats);

            info!("Running RDMA Read throughput test");
            let stats = throughput::run_read_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations, "RDMA Read");
            }
            read_result = Some(stats);

            info!("Running Send throughput test");
            let stats = send_bw::run_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations, "Send");
            }
            send_result = Some(stats);

            info!("Running Recv throughput test");
            let stats = recv_bw::run_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations, "Recv");
            }
            recv_result = Some(stats);

            info!("Running latency test");
            let stats = latency::run_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_latency_human(&stats, size);
            }
            latency_result = Some(stats);
        }
    }

    if output_format == OutputFormat::Json {
        let test_output = TestOutput {
            device: device_name.to_string(),
            transport: transport.to_string(),
            test: test.to_string(),
            config: TestConfig {
                message_size: size,
                iterations,
                warmup,
            },
            results: TestResults {
                write: write_result,
                read: read_result,
                send: send_result,
                recv: recv_result,
                latency: latency_result,
            },
            partial: false,
            error: None,
        };
        output::print_json(&test_output);
    }

    info!("Client finished");
    Ok(())
}
