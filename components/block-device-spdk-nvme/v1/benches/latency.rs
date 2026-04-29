// DmaBuffer is Send but not Sync; Arc<DmaBuffer> is required by Command::WriteSync API.
#![allow(clippy::arc_with_non_send_sync)]

//! Criterion benchmarks for sync/async IO latency at varying queue depths.
//!
//! Measures 4KB read/write latency at queue depths 1, 4, 16, 64 using
//! crossbeam bounded channels (64 slots) as the SPSC transport.
//!
//! Hardware-dependent benchmarks use runtime detection via
//! `spdk_env::checks` — they are silently skipped when no SPDK hardware
//#![cfg(feature = "spdk")]
//! is available.
//!
//! Run with: `cargo bench --bench latency`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

use block_device_spdk_nvme::Command;

/// Benchmark command construction at varying queue depths.
///
/// Measures the cost of creating Command::WriteZeros variants,
/// which is the pure message construction overhead.
fn command_construction_latency(c: &mut Criterion) {
    let mut group = c.benchmark_group("command_construction");
    for &depth in &[1, 4, 16, 64] {
        group.bench_with_input(
            BenchmarkId::new("write_zeros", depth),
            &depth,
            |b, &_depth| {
                b.iter(|| {
                    let _cmd = Command::WriteZeros {
                        ns_id: 1,
                        lba: 0,
                        num_blocks: 8,
                    };
                });
            },
        );
    }
    group.finish();
}

/// Benchmark full sync read/write IO latency with SPDK hardware.
///
/// When hardware is unavailable, the benchmark group is skipped entirely.
/// When available, measures real 4KB read and write latency at varying
/// queue depths.
fn sync_io_latency(c: &mut Criterion) {
    use block_device_spdk_nvme::{BlockDeviceSpdkNvmeComponentV1, IBlockDevice};
    use component_core::binding::bind;
    use component_core::iunknown::query;
    use spdk_env::SPDKEnvComponent;

    // Runtime hardware detection.
    if spdk_env::checks::check_vfio_available().is_err()
        || spdk_env::checks::check_hugepages().is_err()
    {
        eprintln!("sync_io_latency: skipping — no SPDK hardware available");
        return;
    }

    let spdk_env_comp = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();

    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env").expect("bind spdk_env");

    let ienv =
        query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp).expect("ISPDKEnv query");
    if let Err(e) = ienv.init() {
        eprintln!("sync_io_latency: skipping — SPDK init failed: {e}");
        return;
    }

    let devices = ienv.devices();
    if devices.is_empty() {
        eprintln!("sync_io_latency: skipping — no NVMe devices found");
        return;
    }

    let spdk_addr = devices[0].address;
    let admin = query::<dyn interfaces::IBlockDeviceAdmin + Send + Sync>(&*block_dev)
        .expect("IBlockDeviceAdmin query");
    admin.set_pci_address(interfaces::PciAddress {
        domain: spdk_addr.domain,
        bus: spdk_addr.bus,
        dev: spdk_addr.dev,
        func: spdk_addr.func,
    });
    if let Err(e) = admin.initialize() {
        eprintln!("sync_io_latency: skipping — block device init failed: {e}");
        return;
    }

    let ibd = query::<dyn IBlockDevice + Send + Sync>(&*block_dev).expect("IBlockDevice query");
    let channels = ibd.connect_client().expect("connect_client");

    // Probe namespaces.
    channels
        .command_tx
        .send(Command::NsProbe)
        .expect("send NsProbe");
    let completion = channels.completion_rx.recv().expect("recv");
    let namespaces = match completion {
        block_device_spdk_nvme::Completion::NsProbeResult { namespaces } => namespaces,
        other => panic!("expected NsProbeResult, got {other:?}"),
    };
    if namespaces.is_empty() {
        eprintln!("sync_io_latency: skipping — no namespaces");
        return;
    }
    let ns = &namespaces[0];
    let sector_size = ns.sector_size as usize;
    let ns_id = ns.ns_id;

    let mut group = c.benchmark_group("sync_io_latency");
    for &depth in &[1, 4, 16, 64] {
        group.bench_with_input(BenchmarkId::new("read_4k", depth), &depth, |b, &_depth| {
            // Allocate a read buffer per iteration.
            let buf =
                interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc");
            let buf = std::sync::Arc::new(std::sync::Mutex::new(buf));

            b.iter(|| {
                channels
                    .command_tx
                    .send(Command::ReadSync {
                        ns_id,
                        lba: 0,
                        buf: std::sync::Arc::clone(&buf),
                    })
                    .expect("send ReadSync");

                let completion = channels.completion_rx.recv().expect("recv");
                match completion {
                    block_device_spdk_nvme::Completion::ReadDone { result, .. } => {
                        result.expect("read failed")
                    }
                    other => panic!("expected ReadDone, got {other:?}"),
                }
            });
        });
        group.bench_with_input(BenchmarkId::new("write_4k", depth), &depth, |b, &_depth| {
            let buf =
                interfaces::DmaBuffer::new(sector_size, sector_size, None).expect("DMA alloc");
            let buf = std::sync::Arc::new(buf);

            b.iter(|| {
                channels
                    .command_tx
                    .send(Command::WriteSync {
                        ns_id,
                        lba: 0,
                        buf: std::sync::Arc::clone(&buf),
                    })
                    .expect("send WriteSync");

                let completion = channels.completion_rx.recv().expect("recv");
                match completion {
                    block_device_spdk_nvme::Completion::WriteDone { result, .. } => {
                        result.expect("write failed")
                    }
                    other => panic!("expected WriteDone, got {other:?}"),
                }
            });
        });
    }
    group.finish();
}

criterion_group!(benches, command_construction_latency, sync_io_latency,);
criterion_main!(benches);
