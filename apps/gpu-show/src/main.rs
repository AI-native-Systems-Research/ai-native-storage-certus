use component_core::query_interface;
use gpu_services::GpuServicesComponentV0;
use interfaces::IGpuServices;

fn architecture_name(major: u32, minor: u32) -> &'static str {
    match (major, minor) {
        (7, 0) | (7, 2) => "Volta",
        (7, 5) => "Turing",
        (8, 0) => "Ampere",
        (8, 6) => "Ampere (GA10x)",
        (8, 7) => "Ampere (Orin)",
        (8, 9) => "Ada Lovelace",
        (9, 0) => "Hopper",
        (10, 0) => "Blackwell",
        _ => "Unknown",
    }
}

fn format_memory(bytes: u64) -> String {
    let gib = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gib >= 1.0 {
        format!("{:.1} GiB", gib)
    } else {
        let mib = bytes as f64 / (1024.0 * 1024.0);
        format!("{:.0} MiB", mib)
    }
}

fn main() {
    let component = GpuServicesComponentV0::new();
    let gpu = query_interface!(component, IGpuServices).expect("IGpuServices not available");

    if let Err(e) = gpu.initialize() {
        eprintln!("Failed to initialize GPU services: {}", e);
        std::process::exit(1);
    }

    let devices = match gpu.get_devices() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Failed to enumerate GPUs: {}", e);
            std::process::exit(1);
        }
    };

    println!("Found {} GPU(s):\n", devices.len());

    for dev in &devices {
        let arch = architecture_name(dev.compute_major, dev.compute_minor);
        println!("  [{}] {}", dev.device_index, dev.name);
        println!("      Architecture:       {} (sm_{}{})", arch, dev.compute_major, dev.compute_minor);
        println!("      Memory:             {}", format_memory(dev.memory_bytes));
        println!("      PCI Bus ID:         {}", dev.pci_bus_id);
        println!();
    }

    let _ = gpu.shutdown();
}
