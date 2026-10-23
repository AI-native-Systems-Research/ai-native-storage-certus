//! Per-thread statistics and aggregated final report.

/// Statistics collected by a single worker thread.
#[derive(Debug, Default)]
pub(crate) struct ThreadResult {
    /// Number of successful read completions.
    pub read_ops: u64,
    /// Number of successful write completions.
    pub write_ops: u64,
    /// Number of IO errors encountered.
    pub errors: u64,
    /// Total bytes transferred (sum of each completed IO's block size).
    pub total_bytes: u64,
    /// Per-operation latency samples in nanoseconds.
    pub latencies_ns: Vec<u64>,
}

/// Aggregated benchmark results from all threads.
#[derive(Debug)]
pub(crate) struct FinalReport {
    /// Sum of read_ops across all threads.
    pub total_read_ops: u64,
    /// Sum of write_ops across all threads.
    pub total_write_ops: u64,
    /// Sum of errors across all threads.
    pub total_errors: u64,
    /// Actual measured duration in seconds.
    pub duration_secs: f64,
    /// Read IOPS.
    pub read_iops: f64,
    /// Write IOPS.
    pub write_iops: f64,
    /// Total IOPS (read + write).
    pub total_iops: f64,
    /// Throughput in MB/s.
    pub throughput_mbps: f64,
    /// Throughput in GB/s.
    pub throughput_gbps: f64,
    /// Minimum latency in microseconds.
    pub lat_min_us: f64,
    /// Mean latency in microseconds.
    pub lat_mean_us: f64,
    /// 50th percentile latency in microseconds.
    pub lat_p50_us: f64,
    /// 99th percentile latency in microseconds.
    pub lat_p99_us: f64,
    /// Maximum latency in microseconds.
    pub lat_max_us: f64,
}

impl FinalReport {
    /// Compute a final report from per-thread results.
    ///
    /// `results` contains one `ThreadResult` per worker thread.
    /// `duration_secs` is the actual measured benchmark duration.
    pub(crate) fn from_results(results: &[ThreadResult], duration_secs: f64) -> Self {
        let total_read_ops: u64 = results.iter().map(|r| r.read_ops).sum();
        let total_write_ops: u64 = results.iter().map(|r| r.write_ops).sum();
        let total_errors: u64 = results.iter().map(|r| r.errors).sum();
        let total_bytes: u64 = results.iter().map(|r| r.total_bytes).sum();

        let total_ops = total_read_ops + total_write_ops;
        let read_iops = if duration_secs > 0.0 {
            total_read_ops as f64 / duration_secs
        } else {
            0.0
        };
        let write_iops = if duration_secs > 0.0 {
            total_write_ops as f64 / duration_secs
        } else {
            0.0
        };
        let total_iops = if duration_secs > 0.0 {
            total_ops as f64 / duration_secs
        } else {
            0.0
        };
        let throughput_mbps = if duration_secs > 0.0 {
            total_bytes as f64 / duration_secs / 1_048_576.0
        } else {
            0.0
        };
        let throughput_gbps = throughput_mbps / 1_024.0;

        // Merge and sort all latency samples for percentile computation.
        let total_samples: usize = results.iter().map(|r| r.latencies_ns.len()).sum();
        let mut all_latencies: Vec<u64> = Vec::with_capacity(total_samples);
        for r in results {
            all_latencies.extend_from_slice(&r.latencies_ns);
        }
        all_latencies.sort_unstable();

        let (lat_min_us, lat_mean_us, lat_p50_us, lat_p99_us, lat_max_us) =
            if all_latencies.is_empty() {
                (0.0, 0.0, 0.0, 0.0, 0.0)
            } else {
                let min = all_latencies[0] as f64 / 1_000.0;
                let max = all_latencies[all_latencies.len() - 1] as f64 / 1_000.0;
                let sum: u64 = all_latencies.iter().sum();
                let mean = (sum as f64 / all_latencies.len() as f64) / 1_000.0;
                let p50 = percentile(&all_latencies, 50.0) / 1_000.0;
                let p99 = percentile(&all_latencies, 99.0) / 1_000.0;
                (min, mean, p50, p99, max)
            };

        Self {
            total_read_ops,
            total_write_ops,
            total_errors,
            duration_secs,
            read_iops,
            write_iops,
            total_iops,
            throughput_mbps,
            throughput_gbps,
            lat_min_us,
            lat_mean_us,
            lat_p50_us,
            lat_p99_us,
            lat_max_us,
        }
    }
}

/// Compute the value at a given percentile from a sorted array.
///
/// Uses nearest-rank method. `pct` is in range [0, 100].
fn percentile(sorted: &[u64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0] as f64;
    }
    let rank = (pct / 100.0) * (sorted.len() - 1) as f64;
    let lower = rank.floor() as usize;
    let upper = rank.ceil() as usize;
    if lower == upper {
        sorted[lower] as f64
    } else {
        let frac = rank - lower as f64;
        sorted[lower] as f64 * (1.0 - frac) + sorted[upper] as f64 * frac
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iops_calculation() {
        let results = vec![ThreadResult {
            read_ops: 1000,
            write_ops: 0,
            errors: 0,
            total_bytes: 1000 * 4096,
            latencies_ns: vec![1000; 1000],
        }];
        let report = FinalReport::from_results(&results, 10.0);
        assert!((report.total_iops - 100.0).abs() < 0.01);
        assert_eq!(report.total_read_ops, 1000);
        assert_eq!(report.total_write_ops, 0);
    }

    #[test]
    fn throughput_calculation() {
        let results = vec![ThreadResult {
            read_ops: 10_000,
            write_ops: 0,
            errors: 0,
            total_bytes: 10_000 * 4096,
            latencies_ns: vec![5000; 10_000],
        }];
        let report = FinalReport::from_results(&results, 10.0);
        // 10000 ops * 4096 bytes / 10s / 1MB = 3.90625 MB/s
        assert!((report.throughput_mbps - 3.90625).abs() < 0.01);
    }

    #[test]
    fn percentile_accuracy() {
        let results = vec![ThreadResult {
            read_ops: 10,
            write_ops: 0,
            errors: 0,
            total_bytes: 10 * 4096,
            latencies_ns: vec![1000, 2000, 3000, 4000, 5000, 6000, 7000, 8000, 9000, 10000],
        }];
        let report = FinalReport::from_results(&results, 1.0);

        assert!((report.lat_min_us - 1.0).abs() < 0.01);
        assert!((report.lat_max_us - 10.0).abs() < 0.01);
        assert!((report.lat_mean_us - 5.5).abs() < 0.01);
        assert!((report.lat_p50_us - 5.5).abs() < 0.01);
        assert!((report.lat_p99_us - 9.91).abs() < 0.01);
    }

    #[test]
    fn empty_results() {
        let results: Vec<ThreadResult> = vec![];
        let report = FinalReport::from_results(&results, 10.0);
        assert_eq!(report.total_iops, 0.0);
        assert_eq!(report.lat_min_us, 0.0);
        assert_eq!(report.lat_max_us, 0.0);
    }

    #[test]
    fn multi_thread_aggregation() {
        let results = vec![
            ThreadResult {
                read_ops: 500,
                write_ops: 300,
                errors: 1,
                total_bytes: 800 * 4096,
                latencies_ns: vec![1000, 2000, 3000],
            },
            ThreadResult {
                read_ops: 600,
                write_ops: 400,
                errors: 2,
                total_bytes: 1000 * 4096,
                latencies_ns: vec![4000, 5000, 6000],
            },
        ];
        let report = FinalReport::from_results(&results, 10.0);
        assert_eq!(report.total_read_ops, 1100);
        assert_eq!(report.total_write_ops, 700);
        assert_eq!(report.total_errors, 3);
    }
}
