mod client;
mod ffi;
mod latency;
mod output;
mod rdma;
mod recv_bw;
mod send_bw;
mod server;
mod stats;
mod throughput;

use std::path::Path;
use std::process;

use clap::{Parser, Subcommand, ValueEnum};

use output::OutputFormat;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TestType {
    Write,
    Read,
    Send,
    Recv,
    Latency,
    All,
}

impl std::fmt::Display for TestType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TestType::Write => write!(f, "write"),
            TestType::Read => write!(f, "read"),
            TestType::Send => write!(f, "send"),
            TestType::Recv => write!(f, "recv"),
            TestType::Latency => write!(f, "latency"),
            TestType::All => write!(f, "all"),
        }
    }
}

#[derive(Parser)]
#[command(name = "rdma-test")]
#[command(about = "RDMA network throughput and latency benchmark tool")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    mode: Mode,

    /// RDMA device name (auto-detect if omitted)
    #[arg(short, long, global = true)]
    device: Option<String>,

    /// Port number for RDMA connection management
    #[arg(short, long, global = true, default_value_t = 7471)]
    port: u16,

    /// Message size in bytes (supports K, M, G suffixes, e.g. 4K, 4M, 1G)
    #[arg(short, long, global = true, default_value = "4K", value_parser = parse_size)]
    size: usize,

    /// Number of test iterations
    #[arg(short = 'n', long, global = true, default_value_t = 10000)]
    iterations: u64,

    /// Test type to run
    #[arg(short, long, global = true, default_value = "all")]
    test: TestType,

    /// Number of warmup iterations
    #[arg(short, long, global = true, default_value_t = 100)]
    warmup: u64,

    /// Output format
    #[arg(short, long, global = true, default_value = "human")]
    output: OutputFormat,
}

#[derive(Subcommand)]
enum Mode {
    /// Listen for incoming RDMA connections
    Server {
        /// Address to bind to
        #[arg(short, long, default_value = "0.0.0.0")]
        address: String,
    },
    /// Connect to a server and run benchmarks
    Client {
        /// Server address to connect to
        #[arg(short, long)]
        address: String,
    },
}

fn parse_size(s: &str) -> Result<usize, String> {
    let s = s.trim();
    let (num, multiplier) = if let Some(n) = s.strip_suffix('G') {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('g') {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('M') {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 1024 * 1024)
    } else if let Some(n) = s.strip_suffix('K') {
        (n, 1024)
    } else if let Some(n) = s.strip_suffix('k') {
        (n, 1024)
    } else {
        (s, 1)
    };
    let n: usize = num.parse().map_err(|e| format!("invalid size '{}': {}", s, e))?;
    Ok(n * multiplier)
}

struct DeviceInfo {
    name: String,
    transport: String,
    _state: String,
}

fn check_ibverbs_available() -> Vec<DeviceInfo> {
    let sysfs_path = Path::new("/sys/class/infiniband");

    println!("=== RDMA Device Check ===");

    let lib_found = Path::new("/usr/lib64/libibverbs.so.1").exists()
        || Path::new("/usr/lib/x86_64-linux-gnu/libibverbs.so.1").exists()
        || Path::new("/usr/lib64/libibverbs.so").exists()
        || Path::new("/usr/lib/libibverbs.so").exists();

    if !lib_found {
        eprintln!(
            "ERROR: libibverbs not found.\n\
             Install rdma-core-devel (RHEL/Fedora) or libibverbs-dev (Debian/Ubuntu)."
        );
        process::exit(1);
    }
    println!("  libibverbs: found");

    if !sysfs_path.exists() {
        eprintln!(
            "ERROR: No RDMA devices detected (/sys/class/infiniband not present).\n\
             Ensure an RDMA-capable NIC is installed and drivers are loaded.\n\
             For software testing, load the SoftRoCE module: modprobe rdma_rxe"
        );
        process::exit(1);
    }

    let entries: Vec<String> = match std::fs::read_dir(sysfs_path) {
        Ok(dir) => dir
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect(),
        Err(_) => Vec::new(),
    };

    if entries.is_empty() {
        eprintln!(
            "ERROR: No RDMA devices found in /sys/class/infiniband/.\n\
             Ensure RDMA drivers are loaded (mlx5_ib, rxe, etc.)."
        );
        process::exit(1);
    }

    println!("  Devices found: {}", entries.len());

    let mut devices = Vec::new();
    for dev_name in &entries {
        let link_layer = read_sysfs_attr(dev_name, "ports/1/link_layer");
        let state = read_sysfs_attr(dev_name, "ports/1/state");
        let transport = if link_layer.trim().contains("Ethernet") {
            "RoCE"
        } else {
            "InfiniBand"
        };
        let state_str = state.trim().to_string();

        output::print_device_info(dev_name, transport, &state_str);

        devices.push(DeviceInfo {
            name: dev_name.clone(),
            transport: transport.to_string(),
            _state: state_str,
        });
    }
    println!();

    devices
}

fn read_sysfs_attr(device: &str, attr: &str) -> String {
    let path = format!("/sys/class/infiniband/{}/{}", device, attr);
    std::fs::read_to_string(&path).unwrap_or_else(|_| "unknown".to_string())
}

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();

    let cli = Cli::parse();

    if cli.size == 0 {
        eprintln!("ERROR: Message size must be greater than 0");
        process::exit(2);
    }
    if cli.iterations == 0 {
        eprintln!("ERROR: Iteration count must be greater than 0");
        process::exit(2);
    }

    let devices = check_ibverbs_available();

    let active_device = if let Some(ref dev) = cli.device {
        devices
            .iter()
            .find(|d| d.name == *dev)
            .unwrap_or_else(|| {
                eprintln!(
                    "ERROR: Device '{}' not found. Available: {}",
                    dev,
                    devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")
                );
                process::exit(1);
            })
    } else {
        devices.first().unwrap_or_else(|| {
            eprintln!("ERROR: No RDMA devices available");
            process::exit(1);
        })
    };

    let device_name = active_device.name.clone();
    let transport = active_device.transport.clone();

    let result = match cli.mode {
        Mode::Server { ref address } => server::run(
            address,
            cli.port,
            cli.test,
            cli.size,
            cli.iterations,
            cli.warmup,
        ),
        Mode::Client { ref address } => client::run(
            address,
            cli.port,
            cli.test,
            cli.size,
            cli.iterations,
            cli.warmup,
            cli.output,
            &device_name,
            &transport,
        ),
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        process::exit(1);
    }
}
