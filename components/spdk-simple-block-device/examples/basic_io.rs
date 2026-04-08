//! Basic block I/O example.
//!
//! Wires the logger, SPDK environment, and simple block device components,
//! then demonstrates a write-read-verify cycle.
//!
//! # Prerequisites
//!
//! - NVMe device(s) bound to `vfio-pci`
//! - Hugepages allocated (e.g. `echo 1024 > /proc/sys/vm/nr_hugepages`)
//! - Run with sufficient permissions (root or vfio group member)
//!
//! ```bash
//! cargo run --example basic_io
//! ```

use component_framework::iunknown::query;
use example_logger::{ILogger, LoggerComponent};
use spdk_env::{ISPDKEnv, SPDKEnvComponent};
use spdk_simple_block_device::{default_device_state, IBlockDevice, SimpleBlockDevice};
use std::sync::atomic::AtomicBool;
use std::sync::RwLock;

fn main() {
    println!("=== Simple Block Device Example ===\n");

    // --- Instantiate components ---

    let logger = LoggerComponent::new();
    let env_comp = SPDKEnvComponent::new(RwLock::new(Vec::new()), AtomicBool::new(false));
    let bdev = SimpleBlockDevice::new(default_device_state());

    // --- Wire receptacles ---

    let ilogger = query::<dyn ILogger + Send + Sync>(&*logger).expect("ILogger not found");
    env_comp
        .logger
        .connect(ilogger)
        .expect("env logger connect failed");

    let ilogger2 = query::<dyn ILogger + Send + Sync>(&*logger).expect("ILogger not found");
    bdev.logger
        .connect(ilogger2)
        .expect("bdev logger connect failed");

    let ispdk_env =
        query::<dyn ISPDKEnv + Send + Sync>(&*env_comp).expect("ISPDKEnv not found");
    bdev.spdk_env
        .connect(ispdk_env)
        .expect("bdev spdk_env connect failed");

    println!("Components wired. Initializing SPDK environment...\n");

    // --- Initialize SPDK environment ---

    env_comp.init().expect("SPDK environment init failed");
    println!(
        "SPDK environment initialized. Found {} device(s).\n",
        env_comp.device_count()
    );
    for dev in env_comp.devices() {
        println!("  Device: {} ({})", dev.address, dev.device_type);
    }

    // --- Open block device ---

    println!("\nOpening block device...");
    bdev.open().expect("Block device open failed");

    let sector_size = bdev.sector_size() as usize;
    let num_sectors = bdev.num_sectors();
    println!(
        "Block device open: sector_size={sector_size}, num_sectors={num_sectors}, \
         capacity={}MB\n",
        (num_sectors * sector_size as u64) / (1024 * 1024)
    );

    // --- Write-Read-Verify cycle ---

    // Use the last sector to avoid disturbing partition tables / filesystem metadata.
    let test_lba = num_sectors - 1;
    println!("Writing test pattern to LBA {test_lba}...");

    // Create a test pattern: each byte is (offset % 251) to detect byte-shift errors.
    let mut write_buf = vec![0u8; sector_size];
    for (i, byte) in write_buf.iter_mut().enumerate() {
        *byte = (i % 251) as u8;
    }

    bdev.write_blocks(test_lba, &write_buf)
        .expect("Write failed");
    println!("Write complete.");

    // Read it back.
    println!("Reading back LBA {test_lba}...");
    let mut read_buf = vec![0u8; sector_size];
    bdev.read_blocks(test_lba, &mut read_buf)
        .expect("Read failed");
    println!("Read complete.");

    // Verify.
    if read_buf == write_buf {
        println!("Verification PASSED: read data matches written data.");
    } else {
        eprintln!("Verification FAILED: read data does not match written data!");
        let mismatches: Vec<_> = read_buf
            .iter()
            .zip(write_buf.iter())
            .enumerate()
            .filter(|(_, (r, w))| r != w)
            .take(10)
            .collect();
        for (i, (r, w)) in &mismatches {
            eprintln!("  byte[{i}]: read={r:#04x}, expected={w:#04x}");
        }
        std::process::exit(1);
    }

    // --- Close ---

    println!("\nClosing block device...");
    bdev.close().expect("Close failed");
    println!("Block device closed.");

    // SPDKEnvComponent::drop will call spdk_env_fini.
    println!("\n=== Done ===");
}
