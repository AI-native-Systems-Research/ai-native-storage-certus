//! Example: Initialize the SPDK environment and enumerate VFIO-attached devices.
//!
//! Demonstrates the construct-wire-init lifecycle of the SPDKEnvComponent:
//! 1. Create the component
//! 2. Create and wire the logger
//! 3. Call init() to perform VFIO checks and SPDK initialization
//! 4. Query discovered devices via ISPDKEnv
//!
//! # Requirements
//!
//! - VFIO enabled with `vfio-pci` kernel module loaded
//! - At least one device bound to vfio-pci (optional — empty list is valid)
//! - Hugepages configured
//! - `/dev/vfio/` accessible by current user (no root needed if permissions set)
//!
//! # Usage
//!
//! ```bash
//! cargo run -p spdk-env --example spdk-env-example
//! ```

use component_framework::iunknown::{query, IUnknown};
use console_logger::{ILogger, LoggerComponent};
use spdk_env::{ISPDKEnv, SPDKEnvComponent};

fn main() {
    println!("=== SPDK Environment Example ===\n");

    // 1. Create the SPDKEnv component.
    let spdk_comp = SPDKEnvComponent::new(
        std::sync::RwLock::new(Vec::new()),
        std::sync::atomic::AtomicBool::new(false),
    );
    println!("SPDKEnv component: version={}", spdk_comp.version());

    // 2. Create the logger component and wire it.
    let logger_comp = LoggerComponent::new();
    let ilogger = query::<dyn ILogger + Send + Sync>(&*logger_comp)
        .expect("ILogger not found on LoggerComponent");
    spdk_comp
        .logger
        .connect(ilogger)
        .expect("Failed to connect logger receptacle");
    println!("Logger connected: {}", logger_comp.version());

    // 3. Query ISPDKEnv interface.
    let env = query::<dyn ISPDKEnv + Send + Sync>(&*spdk_comp)
        .expect("ISPDKEnv not found on SPDKEnvComponent");

    // 4. Initialize (performs VFIO checks, SPDK init, device enumeration).
    println!("\nInitializing SPDK environment...");
    match env.init() {
        Ok(()) => {
            println!("Initialization successful!\n");

            // 5. Query devices.
            let devices = env.devices();
            println!("Discovered {} VFIO-attached device(s):", devices.len());
            for dev in &devices {
                println!(
                    "  {} - vendor:{:04x} device:{:04x} class:{:06x} type:{} numa:{}",
                    dev.address,
                    dev.id.vendor_id,
                    dev.id.device_id,
                    dev.id.class_id,
                    dev.device_type,
                    dev.numa_node,
                );
            }

            if devices.is_empty() {
                println!("  (no devices bound to vfio-pci)");
            }
        }
        Err(e) => {
            eprintln!("Initialization failed: {e}");
            eprintln!("\nThis is expected if VFIO is not configured on this system.");
            std::process::exit(1);
        }
    }

    println!("\n=== Done ===");
    // Component drop will call spdk_env_fini() automatically.
}
