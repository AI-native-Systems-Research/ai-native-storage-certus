/// Human-readable output formatting for the multi-device benchmark.
use crate::config::OpType;
use crate::stats::FinalReport;
use crate::DeviceContext;

use crate::config::BenchConfig;

/// Print the configuration summary to stdout.
pub fn print_config(config: &BenchConfig, device_contexts: &[DeviceContext]) {
    println!("=== IOPS Benchmark (Multi-Device) ===");
    println!("Devices:      {}", device_contexts.len());
    for (i, ctx) in device_contexts.iter().enumerate() {
        println!(
            "  [{i}] {}  ns={} ({} sectors, {}B sectors, NUMA {}, actor CPU {})",
            ctx.pci_addr_str,
            ctx.ns_info.ns_id,
            ctx.ns_info.num_sectors,
            ctx.ns_info.sector_size,
            ctx.numa_node,
            ctx.actor_cpu,
        );
    }
    println!("Operation:    {}", config.op);
    println!("IO mode:      {}", config.io_mode);
    println!("Pattern:      {}", config.pattern);
    if config.block_sizes.len() == 1 {
        println!("Block size:   {} bytes", config.block_sizes[0]);
    } else {
        let sizes: Vec<String> = config.block_sizes.iter().map(|s| s.to_string()).collect();
        println!("Block sizes:  [{}] bytes (random)", sizes.join(", "));
    }
    if config.batch_size > 1 {
        println!("Batch size:   {}", config.batch_size);
    }
    println!("Queue depth:  {}", config.queue_depth);
    println!("Threads:      {}", config.threads);
    println!("Duration:     {} seconds", config.duration);
}

/// Print a per-second progress line to stderr.
///
/// Shows per-device aggregate IOPS when multiple devices are in use.
pub fn print_progress(
    elapsed_secs: u64,
    total_iops: u64,
    per_thread_iops: &[u64],
    thread_device_map: &[usize],
    num_devices: usize,
    throughput_mbps: f64,
) {
    if num_devices > 1 {
        let mut per_device_iops = vec![0u64; num_devices];
        for (t, &iops) in per_thread_iops.iter().enumerate() {
            per_device_iops[thread_device_map[t]] += iops;
        }
        let parts: Vec<String> = per_device_iops
            .iter()
            .enumerate()
            .map(|(i, &iops)| format!("D{}:{}", i, iops))
            .collect();
        eprintln!(
            "[{:3}s] {} IOPS  {:.1} MB/s  ({})",
            elapsed_secs,
            total_iops,
            throughput_mbps,
            parts.join(", ")
        );
    } else if per_thread_iops.len() > 1 {
        let parts: Vec<String> = per_thread_iops
            .iter()
            .enumerate()
            .map(|(i, &iops)| format!("T{}:{}", i, iops))
            .collect();
        eprintln!(
            "[{:3}s] {} IOPS  {:.1} MB/s  ({})",
            elapsed_secs,
            total_iops,
            throughput_mbps,
            parts.join(", ")
        );
    } else {
        eprintln!(
            "[{:3}s] {} IOPS  {:.1} MB/s",
            elapsed_secs, total_iops, throughput_mbps
        );
    }
}

/// Print the final benchmark report to stdout.
pub fn print_final(
    report: &FinalReport,
    op_type: OpType,
    thread_results: &[crate::stats::ThreadResult],
    device_contexts: &[DeviceContext],
    num_devices: usize,
) {
    println!("=== Results ===");
    println!("Duration:     {:.2} seconds", report.duration_secs);

    // Per-device breakdown.
    if num_devices > 1 {
        println!();
        println!("Per-device IOPS:");
        for (dev_idx, ctx) in device_contexts.iter().enumerate() {
            let dev_results: Vec<&crate::stats::ThreadResult> = thread_results
                .iter()
                .filter(|r| r.device_idx == dev_idx)
                .collect();
            let dev_ops: u64 = dev_results.iter().map(|r| r.read_ops + r.write_ops).sum();
            let dev_iops = if report.duration_secs > 0.0 {
                dev_ops as f64 / report.duration_secs
            } else {
                0.0
            };
            let dev_bytes: u64 = dev_results.iter().map(|r| r.total_bytes).sum();
            let dev_mbps = if report.duration_secs > 0.0 {
                dev_bytes as f64 / report.duration_secs / 1_048_576.0
            } else {
                0.0
            };
            let threads = dev_results.len();
            println!(
                "  Device {:2} ({}):  {} IOPS  {:.1} MB/s  ({} threads)",
                dev_idx,
                ctx.pci_addr_str,
                format_count(dev_iops as u64),
                dev_mbps,
                threads,
            );
        }
    }

    // Per-thread breakdown.
    if thread_results.len() > 1 {
        println!();
        println!("Per-thread IOPS:");
        for (i, tr) in thread_results.iter().enumerate() {
            let thread_ops = tr.read_ops + tr.write_ops;
            let thread_iops = if report.duration_secs > 0.0 {
                thread_ops as f64 / report.duration_secs
            } else {
                0.0
            };
            let dev_label = if num_devices > 1 {
                format!(" [dev {}]", tr.device_idx)
            } else {
                String::new()
            };
            if op_type == OpType::ReadWrite {
                let read_iops = tr.read_ops as f64 / report.duration_secs;
                let write_iops = tr.write_ops as f64 / report.duration_secs;
                println!(
                    "  Thread {:2}{}:  {} IOPS  (read: {}, write: {})",
                    i,
                    dev_label,
                    format_count(thread_iops as u64),
                    format_count(read_iops as u64),
                    format_count(write_iops as u64),
                );
            } else {
                println!(
                    "  Thread {:2}{}:  {} IOPS",
                    i,
                    dev_label,
                    format_count(thread_iops as u64)
                );
            }
        }
        println!();
    }

    if op_type == OpType::ReadWrite {
        println!(
            "Read ops:     {}  ({} IOPS)",
            format_count(report.total_read_ops),
            format_count(report.read_iops as u64)
        );
        println!(
            "Write ops:    {}  ({} IOPS)",
            format_count(report.total_write_ops),
            format_count(report.write_iops as u64)
        );
    }

    let total_ops = report.total_read_ops + report.total_write_ops;
    println!("Total ops:    {}", format_count(total_ops));
    println!("Total IOPS:   {}", format_count(report.total_iops as u64));
    println!(
        "Throughput:   {:.1} MB/s ({:.2} GB/s)",
        report.throughput_mbps, report.throughput_gbps
    );
    println!("Errors:       {}", report.total_errors);
    println!();
    println!("Latency (us):");
    println!("  min:    {:.1}", report.lat_min_us);
    println!("  mean:   {:.1}", report.lat_mean_us);
    println!("  p50:    {:.1}", report.lat_p50_us);
    println!("  p99:    {:.1}", report.lat_p99_us);
    println!("  max:    {:.1}", report.lat_max_us);
}

/// Format a number with comma separators.
fn format_count(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_count_small() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
    }

    #[test]
    fn format_count_with_commas() {
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(1_000_000), "1,000,000");
        assert_eq!(format_count(1_478_234), "1,478,234");
    }
}
