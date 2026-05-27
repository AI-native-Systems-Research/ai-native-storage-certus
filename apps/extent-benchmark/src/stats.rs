use std::time::Duration;

pub struct LatencyStats {
    pub count: u64,
    pub min: Duration,
    pub max: Duration,
    pub mean: Duration,
    pub p50: Duration,
    pub p99: Duration,
}

pub fn compute_stats(samples: &mut [Duration]) -> LatencyStats {
    if samples.is_empty() {
        return LatencyStats {
            count: 0,
            min: Duration::ZERO,
            max: Duration::ZERO,
            mean: Duration::ZERO,
            p50: Duration::ZERO,
            p99: Duration::ZERO,
        };
    }
    samples.sort();
    let count = samples.len() as u64;
    let min = samples[0];
    let max = samples[samples.len() - 1];
    let sum: Duration = samples.iter().sum();
    let mean = sum / count as u32;
    let p50 = samples[samples.len() / 2];
    let p99 = samples[samples.len() * 99 / 100];
    LatencyStats {
        count,
        min,
        max,
        mean,
        p50,
        p99,
    }
}
