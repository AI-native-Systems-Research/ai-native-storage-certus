use serde::Serialize;

use crate::stats::{LatencyStats, ThroughputStats};

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Debug, Serialize)]
pub struct TestOutput {
    pub device: String,
    pub transport: String,
    pub test: String,
    pub config: TestConfig,
    pub results: TestResults,
    pub partial: bool,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestConfig {
    pub message_size: usize,
    pub iterations: u64,
    pub warmup: u64,
}

#[derive(Debug, Serialize)]
pub struct TestResults {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub throughput: Option<ThroughputStats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency: Option<LatencyStats>,
}

pub fn print_device_info(name: &str, transport: &str, state: &str) {
    println!("    {} ({}) - {}", name, transport, state);
}

pub fn print_throughput_human(stats: &ThroughputStats, message_size: usize, iterations: u64) {
    println!("\n=== RDMA Throughput Test (RDMA Write) ===");
    println!("  Message size: {} bytes", message_size);
    println!("  Iterations:   {}", iterations);
    println!("  Elapsed:      {:.3} s", stats.elapsed_seconds);
    println!("  Bandwidth:    {:.2} GB/s", stats.bandwidth_gbps);
    println!("  Message rate: {:.2} Mmsg/s", stats.message_rate_mpps);
    println!(
        "  Total data:   {:.2} MB",
        stats.total_bytes as f64 / 1_048_576.0
    );
}

pub fn print_latency_human(stats: &LatencyStats, message_size: usize) {
    println!("\n=== RDMA Latency Test (Send/Recv) ===");
    println!("  Message size: {} bytes", message_size);
    println!("  Samples: {}", stats.samples);
    println!("  Min:     {:.2} us", stats.min_us);
    println!("  Max:     {:.2} us", stats.max_us);
    println!("  Mean:    {:.2} us", stats.mean_us);
    println!("  Median:  {:.2} us", stats.median_us);
    println!("  P95:     {:.2} us", stats.p95_us);
    println!("  P99:     {:.2} us", stats.p99_us);
    println!("  Jitter:  {:.2} us (stddev)", stats.jitter_us);
}

pub fn print_json(output: &TestOutput) {
    println!("{}", serde_json::to_string_pretty(output).unwrap());
}
