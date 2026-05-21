/// Benchmark configuration parsed from CLI arguments.
use clap::Parser;
use std::fmt;

/// NVMe IO operation type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum OpType {
    /// All operations are reads.
    Read,
    /// All operations are writes.
    Write,
    /// 50/50 random mix of reads and writes.
    #[value(name = "rw")]
    ReadWrite,
}

impl fmt::Display for OpType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpType::Read => write!(f, "read"),
            OpType::Write => write!(f, "write"),
            OpType::ReadWrite => write!(f, "readwrite"),
        }
    }
}

/// IO submission mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum IoMode {
    /// Synchronous: each command blocks the actor until SPDK completion.
    Sync,
    /// Asynchronous: commands are submitted to SPDK and completed via callback.
    Async,
}

impl fmt::Display for IoMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IoMode::Sync => write!(f, "sync"),
            IoMode::Async => write!(f, "async"),
        }
    }
}

/// IO access pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Pattern {
    /// Uniform random LBA per operation.
    Random,
    /// Contiguous LBAs with per-thread non-overlapping regions.
    Sequential,
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pattern::Random => write!(f, "random"),
            Pattern::Sequential => write!(f, "sequential"),
        }
    }
}

/// Configuration for an IOPS benchmark run.
#[derive(Debug, Clone, Parser)]
#[command(name = "iops-benchmark", about = "NVMe IOPS benchmark using SPDK")]
pub struct BenchConfig {
    /// Operation type: read, write, or rw (mixed).
    #[arg(long, default_value = "read", value_enum)]
    pub op: OpType,

    /// IO block size(s) in bytes. Comma-separated list for mixed-size workloads
    /// (e.g. --block-size 4096,8192,16384). When multiple sizes are given, each
    /// IO randomly picks one.
    #[arg(long = "block-size", value_delimiter = ',', default_value = "4096")]
    pub block_sizes: Vec<usize>,

    /// Number of IOs to submit as a single batch. When > 1, commands are grouped
    /// into BatchSubmit messages. Each IO in a batch independently picks a random
    /// block size.
    #[arg(long, default_value_t = 1)]
    pub batch_size: u32,

    /// Outstanding IOs per thread.
    #[arg(long, default_value_t = 32)]
    pub queue_depth: u32,

    /// Number of concurrent client threads.
    #[arg(long, default_value_t = 1)]
    pub threads: u32,

    /// Test duration in seconds.
    #[arg(long, default_value_t = 10)]
    pub duration: u64,

    /// NVMe namespace ID.
    #[arg(long, default_value_t = 1)]
    pub ns_id: u32,

    /// NVMe controller PCI BDF address (e.g. 0000:03:00.0). Uses first device if omitted.
    #[arg(long)]
    pub pci_addr: Option<String>,

    /// IO access pattern: random or sequential.
    #[arg(long, default_value = "random", value_enum)]
    pub pattern: Pattern,

    /// IO submission mode: sync or async.
    #[arg(long, default_value = "async", value_enum)]
    pub io_mode: IoMode,

    /// Suppress per-second progress output.
    #[arg(long, default_value_t = false)]
    pub quiet: bool,

}

impl BenchConfig {
    /// Validate configuration against device properties.
    ///
    /// Returns `Ok(())` if valid, or `Err(message)` describing the problem.
    pub fn validate(
        &self,
        sector_size: u32,
        max_qd: u32,
        ns_list: &[interfaces::NamespaceInfo],
    ) -> Result<(), String> {
        if self.block_sizes.is_empty() {
            return Err("block-size must specify at least one size".into());
        }
        for &bs in &self.block_sizes {
            if bs == 0 {
                return Err("block-size must be > 0".into());
            }
            if bs % sector_size as usize != 0 {
                return Err(format!(
                    "block-size {} is not a multiple of device sector size {}",
                    bs, sector_size
                ));
            }
        }
        if self.threads < 1 {
            return Err("threads must be >= 1".into());
        }
        if self.duration < 1 {
            return Err("duration must be >= 1".into());
        }
        if self.queue_depth < 1 {
            return Err("queue-depth must be >= 1".into());
        }
        if self.batch_size < 1 {
            return Err("batch-size must be >= 1".into());
        }
        if self.batch_size > self.queue_depth {
            return Err(format!(
                "batch-size {} exceeds queue-depth {}",
                self.batch_size, self.queue_depth
            ));
        }
        if !ns_list.iter().any(|ns| ns.ns_id == self.ns_id) {
            let available: Vec<u32> = ns_list.iter().map(|ns| ns.ns_id).collect();
            return Err(format!(
                "namespace {} not found (available: {:?})",
                self.ns_id, available
            ));
        }
        let _ = max_qd; // Used by clamp_queue_depth
        Ok(())
    }

    /// Largest block size in the configured set.
    pub fn max_block_size(&self) -> usize {
        *self.block_sizes.iter().max().unwrap()
    }

    /// Clamp queue depth to device maximum, printing a warning if clamped.
    pub fn clamp_queue_depth(&mut self, max_qd: u32) {
        if self.queue_depth > max_qd {
            eprintln!(
                "warning: queue-depth {} exceeds device maximum {}, clamping to {}",
                self.queue_depth, max_qd, max_qd
            );
            self.queue_depth = max_qd;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interfaces::NamespaceInfo;

    fn sample_ns_list() -> Vec<NamespaceInfo> {
        vec![NamespaceInfo {
            ns_id: 1,
            num_sectors: 1_000_000,
            sector_size: 512,
        }]
    }

    #[test]
    fn valid_config_passes() {
        let config = BenchConfig {
            op: OpType::Read,
            block_sizes: vec![4096],
            batch_size: 1,
            queue_depth: 32,
            threads: 1,
            duration: 10,
            ns_id: 1,
            pci_addr: None,
            pattern: Pattern::Random,
            io_mode: IoMode::Async,
            quiet: false,
        };
        assert!(config.validate(512, 256, &sample_ns_list()).is_ok());
    }

    #[test]
    fn block_size_not_multiple_of_sector_size() {
        let config = BenchConfig {
            op: OpType::Read,
            block_sizes: vec![1000],
            batch_size: 1,
            queue_depth: 32,
            threads: 1,
            duration: 10,
            ns_id: 1,
            pci_addr: None,
            pattern: Pattern::Random,
            io_mode: IoMode::Async,
            quiet: false,
        };
        let err = config.validate(512, 256, &sample_ns_list()).unwrap_err();
        assert!(err.contains("not a multiple of device sector size"));
    }

    #[test]
    fn invalid_namespace_fails() {
        let config = BenchConfig {
            op: OpType::Read,
            block_sizes: vec![4096],
            batch_size: 1,
            queue_depth: 32,
            threads: 1,
            duration: 10,
            ns_id: 5,
            pci_addr: None,
            pattern: Pattern::Random,
            io_mode: IoMode::Async,
            quiet: false,
        };
        let err = config.validate(512, 256, &sample_ns_list()).unwrap_err();
        assert!(err.contains("namespace 5 not found"));
    }

    #[test]
    fn queue_depth_clamping() {
        let mut config = BenchConfig {
            op: OpType::Read,
            block_sizes: vec![4096],
            batch_size: 1,
            queue_depth: 512,
            threads: 1,
            duration: 10,
            ns_id: 1,
            pci_addr: None,
            pattern: Pattern::Random,
            io_mode: IoMode::Async,
            quiet: false,
        };
        config.clamp_queue_depth(256);
        assert_eq!(config.queue_depth, 256);
    }

    #[test]
    fn queue_depth_no_clamp_when_within_limit() {
        let mut config = BenchConfig {
            op: OpType::Read,
            block_sizes: vec![4096],
            batch_size: 1,
            queue_depth: 32,
            threads: 1,
            duration: 10,
            ns_id: 1,
            pci_addr: None,
            pattern: Pattern::Random,
            io_mode: IoMode::Async,
            quiet: false,
        };
        config.clamp_queue_depth(256);
        assert_eq!(config.queue_depth, 32);
    }

    #[test]
    fn batch_size_exceeds_queue_depth() {
        let config = BenchConfig {
            op: OpType::Read,
            block_sizes: vec![4096],
            batch_size: 64,
            queue_depth: 32,
            threads: 1,
            duration: 10,
            ns_id: 1,
            pci_addr: None,
            pattern: Pattern::Random,
            io_mode: IoMode::Async,
            quiet: false,
        };
        let err = config.validate(512, 256, &sample_ns_list()).unwrap_err();
        assert!(err.contains("batch-size 64 exceeds queue-depth 32"));
    }

    #[test]
    fn op_type_display() {
        assert_eq!(format!("{}", OpType::Read), "read");
        assert_eq!(format!("{}", OpType::Write), "write");
        assert_eq!(format!("{}", OpType::ReadWrite), "readwrite");
    }

    #[test]
    fn pattern_display() {
        assert_eq!(format!("{}", Pattern::Random), "random");
        assert_eq!(format!("{}", Pattern::Sequential), "sequential");
    }

    #[test]
    fn io_mode_display() {
        assert_eq!(format!("{}", IoMode::Sync), "sync");
        assert_eq!(format!("{}", IoMode::Async), "async");
    }

}
