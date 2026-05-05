use std::time::Duration;

use crate::config::BenchmarkConfig;
use crate::worker::PhaseResult;

pub fn print_header(config: &BenchmarkConfig, count: u64, data_disk_size: u64) {
    println!("=== Extent Manager Benchmark ===");
    match &config.metadata_device {
        Some(addr) => println!("Metadata device: {} (ns_id={})", addr, config.metadata_ns_id),
        None => println!("Metadata device: (in-memory mock)"),
    }
    println!(
        "Threads: {} | Count: {} | Size class: {} B",
        config.threads, count, config.size_class
    );
    println!(
        "Slab size: {} B | Regions: {} | Data disk: {} B",
        config.slab_size, config.region_count, data_disk_size
    );
    println!();
}

pub fn print_phase(result: &PhaseResult) {
    println!("--- {} Phase ---", result.phase_name);
    println!("  Total ops:   {}", result.total_ops);
    println!("  Elapsed:     {:.3}s", result.elapsed.as_secs_f64());
    println!("  Throughput:  {:.0} ops/sec", result.ops_per_sec);
    let sample_note = if result.latency.count < result.total_ops {
        format!(" (1-in-{} sample)", result.total_ops / result.latency.count)
    } else {
        String::new()
    };
    println!("  Latency ({} samples{}):", result.latency.count, sample_note);
    println!("    min:  {:>8} us", result.latency.min.as_micros());
    println!("    mean: {:>8} us", result.latency.mean.as_micros());
    println!("    p50:  {:>8} us", result.latency.p50.as_micros());
    println!("    p99:  {:>8} us", result.latency.p99.as_micros());
    println!("    max:  {:>8} us", result.latency.max.as_micros());

    if result.per_thread.len() > 1 {
        println!("  Per-thread:");
        for w in &result.per_thread {
            println!(
                "    thread {}: {} ops, p50={} us, p99={} us",
                w.thread_id,
                w.ops_completed,
                w.latency.p50.as_micros(),
                w.latency.p99.as_micros(),
            );
        }
    }
    println!();
}

pub fn print_single_op(name: &str, elapsed: Duration) {
    println!("--- {} ---", name);
    println!("  Elapsed: {:.3}s", elapsed.as_secs_f64());
    println!();
}

pub fn print_enumerate(recovered: u64, expected: u64, elapsed: Duration) {
    println!("--- Enumerate ---");
    println!("  Extents: {}/{}", recovered, expected);
    println!("  Elapsed: {:.3}s", elapsed.as_secs_f64());
    if recovered != expected {
        println!("  WARNING: count mismatch (expected {expected}, got {recovered})");
    }
    println!();
}

pub fn print_summary(count: u64, create: &PhaseResult, remove: &PhaseResult) {
    println!("=== Summary ===");
    println!(
        "  Create: {} ops at {:.0} ops/sec",
        create.total_ops, create.ops_per_sec
    );
    println!(
        "  Remove: {} ops at {:.0} ops/sec",
        remove.total_ops, remove.ops_per_sec
    );
    println!("  Total extents: {}", count);
}
