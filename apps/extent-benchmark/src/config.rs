use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "extent-benchmark",
    about = "Benchmark extent manager operations (create/checkpoint/recover/remove)"
)]
pub struct BenchmarkConfig {
    /// Metadata NVMe device PCI address (e.g., 0000:04:00.0).
    /// When omitted an in-memory mock block device is used instead of real hardware.
    #[arg(long)]
    pub metadata_device: Option<String>,

    /// NVMe namespace ID for the metadata device.
    #[arg(long, default_value_t = 1)]
    pub metadata_ns_id: u32,

    /// Number of worker threads for Create and Remove phases.
    #[arg(long, default_value_t = 1)]
    pub threads: usize,

    /// Number of extents to create.
    /// Defaults to 100,000,000 when using real hardware, or 1,000,000 in mock mode.
    #[arg(long)]
    pub count: Option<u64>,

    /// Extent size class in bytes (default 128 KiB; must be a non-zero multiple of 4096).
    #[arg(long, default_value_t = 131072)]
    pub size_class: u32,

    /// Slab size in bytes (default 1 GiB; must be a non-zero multiple of size-class).
    #[arg(long, default_value_t = 1073741824)]
    pub slab_size: u64,

    /// Number of extent regions (default 32; must be a power of two).
    #[arg(long, default_value_t = 32)]
    pub region_count: u32,

    /// Override the logical data-disk size in bytes.
    /// When omitted the size is derived from --count, --size-class, and --slab-size.
    #[arg(long)]
    pub total_size: Option<u64>,
}

impl BenchmarkConfig {
    pub fn effective_count(&self) -> u64 {
        self.count.unwrap_or_else(|| {
            if self.metadata_device.is_some() {
                100_000_000
            } else {
                1_000_000
            }
        })
    }
}
