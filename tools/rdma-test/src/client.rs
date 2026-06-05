use anyhow::Result;
use tracing::info;

use crate::latency;
use crate::output::{self, OutputFormat, TestConfig, TestOutput, TestResults};
use crate::rdma;
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

    let mut throughput_result: Option<ThroughputStats> = None;
    let mut latency_result: Option<LatencyStats> = None;

    match test {
        TestType::Throughput => {
            info!("Running throughput test");
            let stats = throughput::run_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations);
            }
            throughput_result = Some(stats);
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
            info!("Running throughput test");
            let stats = throughput::run_client(&conn, size, iterations, warmup)?;
            if output_format == OutputFormat::Human {
                output::print_throughput_human(&stats, size, iterations);
            }
            throughput_result = Some(stats);

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
            test: format!("{:?}", test).to_lowercase(),
            config: TestConfig {
                message_size: size,
                iterations,
                warmup,
            },
            results: TestResults {
                throughput: throughput_result,
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
