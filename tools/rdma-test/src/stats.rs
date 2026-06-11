use std::time::Duration;

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct LatencyStats {
    pub min_us: f64,
    pub max_us: f64,
    pub mean_us: f64,
    pub median_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub jitter_us: f64,
    pub samples: u64,
}

impl LatencyStats {
    pub fn compute(samples: &mut [Duration]) -> Option<Self> {
        if samples.is_empty() {
            return None;
        }

        samples.sort();
        let count = samples.len();
        let min = samples[0];
        let max = samples[count - 1];
        let median = samples[count / 2];
        let p95 = samples[((count as f64 * 0.95) as usize).min(count - 1)];
        let p99 = samples[((count as f64 * 0.99) as usize).min(count - 1)];

        let sum: Duration = samples.iter().sum();
        let mean = sum / count as u32;

        let mean_nanos = mean.as_nanos() as f64;
        let variance: f64 = samples
            .iter()
            .map(|s| {
                let diff = s.as_nanos() as f64 - mean_nanos;
                diff * diff
            })
            .sum::<f64>()
            / count as f64;
        let stddev_nanos = variance.sqrt();

        Some(Self {
            min_us: min.as_nanos() as f64 / 1000.0,
            max_us: max.as_nanos() as f64 / 1000.0,
            mean_us: mean.as_nanos() as f64 / 1000.0,
            median_us: median.as_nanos() as f64 / 1000.0,
            p95_us: p95.as_nanos() as f64 / 1000.0,
            p99_us: p99.as_nanos() as f64 / 1000.0,
            jitter_us: stddev_nanos / 1000.0,
            samples: count as u64,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct ThroughputStats {
    pub bandwidth_gbps: f64,
    pub message_rate_mpps: f64,
    pub total_bytes: u64,
    pub elapsed_seconds: f64,
}

impl ThroughputStats {
    pub fn compute(total_bytes: u64, iterations: u64, elapsed: Duration) -> Self {
        let secs = elapsed.as_secs_f64();
        Self {
            bandwidth_gbps: total_bytes as f64 / secs / 1_000_000_000.0,
            message_rate_mpps: iterations as f64 / secs / 1_000_000.0,
            total_bytes,
            elapsed_seconds: secs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_latency_stats_basic() {
        let mut samples: Vec<Duration> = (1..=100)
            .map(|i| Duration::from_nanos(i * 1000))
            .collect();
        let stats = LatencyStats::compute(&mut samples).unwrap();
        assert_eq!(stats.samples, 100);
        assert!((stats.min_us - 1.0).abs() < 0.01);
        assert!((stats.max_us - 100.0).abs() < 0.01);
        assert!((stats.median_us - 51.0).abs() < 1.0);
    }

    #[test]
    fn test_latency_stats_empty() {
        let mut samples: Vec<Duration> = vec![];
        assert!(LatencyStats::compute(&mut samples).is_none());
    }

    #[test]
    fn test_throughput_stats() {
        let stats = ThroughputStats::compute(
            10_000_000_000,
            1_000_000,
            Duration::from_secs(1),
        );
        assert!((stats.bandwidth_gbps - 10.0).abs() < 0.01);
        assert!((stats.message_rate_mpps - 1.0).abs() < 0.01);
    }
}
