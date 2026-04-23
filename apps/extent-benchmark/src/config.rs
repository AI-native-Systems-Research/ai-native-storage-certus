use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "extent-benchmark",
    about = "Benchmark extent manager create/lookup/remove operations"
)]
pub struct BenchmarkConfig {
    #[arg(long, help = "NVMe device PCI address (e.g., 0000:03:00.0)")]
    pub device: String,

    #[arg(long, default_value_t = 1, help = "NVMe namespace ID")]
    pub ns_id: u32,

    #[arg(long, default_value_t = 1, help = "Number of worker threads")]
    pub threads: usize,

    #[arg(long, default_value_t = 10000, help = "Operations per benchmark phase")]
    pub count: u64,

    #[arg(
        long,
        default_value_t = 131072,
        help = "Extent size class in bytes (default 128 KiB)"
    )]
    pub size_class: u32,

    #[arg(
        long,
        default_value_t = 1073741824,
        help = "Slab size in bytes (default 1 GiB)"
    )]
    pub slab_size: u64,

    #[arg(
        long,
        help = "Total managed space in bytes (auto-detect from device if omitted)"
    )]
    pub total_size: Option<u64>,
}
