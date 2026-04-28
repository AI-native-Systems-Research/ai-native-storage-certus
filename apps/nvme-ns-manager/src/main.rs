//! Interactive NVMe namespace management tool using the block-device-spdk-nvme component.
//!
//! Provides a text-based interactive shell to list, create, and delete NVMe
//! namespaces on SPDK-attached devices.
//!
//! Run with `--help` for startup options.

use std::io::{self, BufRead, Write};

use clap::{Parser, ValueEnum};

use block_device_spdk_nvme::BlockDeviceSpdkNvmeComponentV1;
use block_device_spdk_nvme_v2::BlockDeviceSpdkNvmeComponentV2;
use component_core::binding::bind;
use component_core::iunknown::query;
use interfaces::{ClientChannels, Command, Completion};
use spdk_env::SPDKEnvComponent;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Driver {
    V1,
    V2,
}

#[derive(Parser)]
#[command(name = "nvme-ns-manager", about = "Interactive NVMe namespace manager via SPDK")]
struct Cli {
    /// NVMe controller PCI BDF address (e.g. 0000:03:00.0). Uses first device if omitted.
    #[arg(long)]
    pci_addr: Option<String>,

    /// Block device driver version.
    #[arg(long, default_value = "v2", value_enum)]
    driver: Driver,
}

fn main() {
    let cli = Cli::parse();

    // --- Component wiring ---
    let spdk_env_comp = SPDKEnvComponent::new_default();
    let block_dev: std::sync::Arc<dyn component_core::IUnknown> = match cli.driver {
        Driver::V1 => BlockDeviceSpdkNvmeComponentV1::new_default(),
        Driver::V2 => BlockDeviceSpdkNvmeComponentV2::new_default(),
    };

    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env").unwrap_or_else(|e| {
        eprintln!("error: failed to bind spdk_env: {e}");
        std::process::exit(2);
    });

    // --- Initialize SPDK environment ---
    let ienv =
        query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp).unwrap_or_else(|| {
            eprintln!("error: failed to query ISPDKEnv");
            std::process::exit(2);
        });
    if let Err(e) = ienv.init() {
        eprintln!("error: SPDK init failed: {e}");
        std::process::exit(2);
    }

    // --- Select device ---
    let devices = ienv.devices();
    if devices.is_empty() {
        eprintln!("error: no NVMe devices found");
        std::process::exit(2);
    }

    let device = if let Some(ref addr_str) = cli.pci_addr {
        match parse_pci_addr(addr_str) {
            Some(target) => {
                match devices.iter().find(|d| {
                    d.address.domain == target.domain
                        && d.address.bus == target.bus
                        && d.address.dev == target.dev
                        && d.address.func == target.func
                }) {
                    Some(d) => d,
                    None => {
                        eprintln!("error: no NVMe device found at PCI address {addr_str}");
                        std::process::exit(2);
                    }
                }
            }
            None => {
                eprintln!("error: invalid PCI address format: {addr_str}");
                std::process::exit(1);
            }
        }
    } else {
        &devices[0]
    };

    let driver_label = match cli.driver {
        Driver::V1 => "v1",
        Driver::V2 => "v2",
    };
    println!(
        "Device: {}  Driver: {}",
        device.address, driver_label
    );

    // --- Initialize block device ---
    let admin = query::<dyn interfaces::IBlockDeviceAdmin + Send + Sync>(&*block_dev)
        .unwrap_or_else(|| {
            eprintln!("error: failed to query IBlockDeviceAdmin");
            std::process::exit(2);
        });

    admin.set_pci_address(interfaces::PciAddress {
        domain: device.address.domain,
        bus: device.address.bus,
        dev: device.address.dev,
        func: device.address.func,
    });

    if let Err(e) = admin.initialize() {
        eprintln!("error: block device init failed: {e}");
        std::process::exit(2);
    }

    let ibd = query::<dyn interfaces::IBlockDevice + Send + Sync>(&*block_dev).unwrap_or_else(|| {
        eprintln!("error: failed to query IBlockDevice");
        std::process::exit(2);
    });

    let channels = ibd.connect_client().unwrap_or_else(|e| {
        eprintln!("error: failed to connect client: {e}");
        std::process::exit(2);
    });

    // --- Interactive menu ---
    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        println!();
        print_menu();
        let _ = write!(stdout, "Select [1-6]: ");
        let _ = stdout.flush();

        let line = match read_line(&stdin) {
            Some(l) => l,
            None => break,
        };

        match line.trim() {
            "1" => cmd_list(&channels),
            "2" => {
                let size_sectors = match prompt_value::<u64>(&stdin, &mut stdout, "Size in sectors") {
                    Some(v) => v,
                    None => continue,
                };
                cmd_create(&channels, size_sectors);
            }
            "3" => {
                cmd_create(&channels, 0);
            }
            "4" => {
                let ns_id = match prompt_value::<u32>(&stdin, &mut stdout, "Namespace ID") {
                    Some(v) => v,
                    None => continue,
                };
                let lbaf = prompt_with_default::<u8>(&stdin, &mut stdout, "LBAF index", 0);
                cmd_format(&channels, ns_id, lbaf);
            }
            "5" => {
                let ns_id = match prompt_value::<u32>(&stdin, &mut stdout, "Namespace ID") {
                    Some(v) => v,
                    None => continue,
                };
                cmd_delete(&channels, ns_id);
            }
            "6" => break,
            _ => eprintln!("Invalid selection."),
        }
    }

    println!();
}

fn print_menu() {
    println!("  1) List namespaces");
    println!("  2) Create namespace");
    println!("  3) Create namespace (use all remaining capacity)");
    println!("  4) Format namespace");
    println!("  5) Delete namespace");
    println!("  6) Quit");
}

fn read_line(stdin: &io::Stdin) -> Option<String> {
    let mut buf = String::new();
    match stdin.lock().read_line(&mut buf) {
        Ok(0) => None,
        Ok(_) => Some(buf),
        Err(_) => None,
    }
}

fn prompt_value<T: std::str::FromStr>(
    stdin: &io::Stdin,
    stdout: &mut io::Stdout,
    label: &str,
) -> Option<T> {
    let _ = write!(stdout, "  {label}: ");
    let _ = stdout.flush();
    let line = read_line(stdin)?;
    match line.trim().parse::<T>() {
        Ok(v) => Some(v),
        Err(_) => {
            eprintln!("error: invalid input");
            None
        }
    }
}

fn prompt_with_default<T: std::str::FromStr + std::fmt::Display>(
    stdin: &io::Stdin,
    stdout: &mut io::Stdout,
    label: &str,
    default: T,
) -> T {
    let _ = write!(stdout, "  {label} [default {default}]: ");
    let _ = stdout.flush();
    match read_line(stdin) {
        Some(line) if line.trim().is_empty() => default,
        Some(line) => line.trim().parse::<T>().unwrap_or(default),
        None => default,
    }
}

fn cmd_list(channels: &ClientChannels) {
    if let Err(e) = channels.command_tx.send(Command::NsProbe) {
        eprintln!("error: failed to send NsProbe: {e}");
        return;
    }

    match channels.completion_rx.recv() {
        Ok(Completion::NsProbeResult { namespaces }) => {
            if namespaces.is_empty() {
                println!("No active namespaces.");
                return;
            }
            println!(
                "{:<8} {:>16} {:>14} {:>12}",
                "NS ID", "Sectors", "Sector Size", "Capacity"
            );
            println!("{}", "-".repeat(54));
            for ns in &namespaces {
                let capacity_bytes = ns.num_sectors as u128 * ns.sector_size as u128;
                let capacity_str = format_capacity(capacity_bytes);
                println!(
                    "{:<8} {:>16} {:>11} B {:>12}",
                    ns.ns_id, ns.num_sectors, ns.sector_size, capacity_str
                );
            }
            println!("\n{} namespace(s) found.", namespaces.len());
        }
        Ok(Completion::Error { error, .. }) => {
            eprintln!("error: {error}");
        }
        Ok(other) => {
            eprintln!("error: unexpected completion: {other:?}");
        }
        Err(e) => {
            eprintln!("error: failed to receive completion: {e}");
        }
    }
}

fn cmd_create(channels: &ClientChannels, size_sectors: u64) {
    if let Err(e) = channels.command_tx.send(Command::NsCreate { size_sectors }) {
        eprintln!("error: failed to send NsCreate: {e}");
        return;
    }

    match channels.completion_rx.recv() {
        Ok(Completion::NsCreated { ns_id }) => {
            if size_sectors == 0 {
                println!("Namespace created: ns_id={ns_id} (all remaining capacity)");
            } else {
                println!("Namespace created: ns_id={ns_id}, size={size_sectors} sectors");
            }
        }
        Ok(Completion::Error { error, .. }) => {
            eprintln!("error: namespace create failed: {error}");
        }
        Ok(other) => {
            eprintln!("error: unexpected completion: {other:?}");
        }
        Err(e) => {
            eprintln!("error: failed to receive completion: {e}");
        }
    }
}

fn cmd_format(channels: &ClientChannels, ns_id: u32, lbaf: u8) {
    if let Err(e) = channels.command_tx.send(Command::NsFormat { ns_id, lbaf }) {
        eprintln!("error: failed to send NsFormat: {e}");
        return;
    }

    match channels.completion_rx.recv() {
        Ok(Completion::NsFormatted { ns_id }) => {
            println!("Namespace {ns_id} formatted with lbaf={lbaf}.");
        }
        Ok(Completion::Error { error, .. }) => {
            eprintln!("error: namespace format failed: {error}");
        }
        Ok(other) => {
            eprintln!("error: unexpected completion: {other:?}");
        }
        Err(e) => {
            eprintln!("error: failed to receive completion: {e}");
        }
    }
}

fn cmd_delete(channels: &ClientChannels, ns_id: u32) {
    if let Err(e) = channels.command_tx.send(Command::NsDelete { ns_id }) {
        eprintln!("error: failed to send NsDelete: {e}");
        return;
    }

    match channels.completion_rx.recv() {
        Ok(Completion::NsDeleted { ns_id }) => {
            println!("Namespace {ns_id} deleted.");
        }
        Ok(Completion::Error { error, .. }) => {
            eprintln!("error: namespace delete failed: {error}");
        }
        Ok(other) => {
            eprintln!("error: unexpected completion: {other:?}");
        }
        Err(e) => {
            eprintln!("error: failed to receive completion: {e}");
        }
    }
}

fn format_capacity(bytes: u128) -> String {
    const KIB: u128 = 1024;
    const MIB: u128 = 1024 * KIB;
    const GIB: u128 = 1024 * MIB;
    const TIB: u128 = 1024 * GIB;

    if bytes >= TIB {
        format!("{:.2} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.2} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.2} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn parse_pci_addr(s: &str) -> Option<interfaces::PciAddress> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() != 3 {
        return None;
    }

    let domain = u32::from_str_radix(parts[0], 16).ok()?;
    let bus = u8::from_str_radix(parts[1], 16).ok()?;

    let dev_func: Vec<&str> = parts[2].split('.').collect();
    if dev_func.len() != 2 {
        return None;
    }

    let dev = u8::from_str_radix(dev_func[0], 16).ok()?;
    let func = u8::from_str_radix(dev_func[1], 16).ok()?;

    Some(interfaces::PciAddress {
        domain,
        bus,
        dev,
        func,
    })
}
