// DmaBuffer is Send but not Sync; Arc<DmaBuffer> is required by Command::WriteSync API.
#![allow(clippy::arc_with_non_send_sync)]

//! Criterion benchmarks for batch write throughput at varying batch sizes.
//!
//! Measures throughput at batch sizes 1, 8, 32, 128 for 4KB blocks using
//! crossbeam bounded channels (64 slots) as the SPSC transport.
//!
//! Hardware-dependent benchmarks use runtime detection via
//! `spdk_env::checks` — they are silently skipped when no SPDK hardware
//#![cfg(feature = "spdk")]
//! is available.
//!
//! Run with: `cargo bench --bench throughput`

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use block_device_spdk_nvme::Command;

/// Benchmark batch command construction throughput.
///
/// Measures the cost of constructing a `BatchSubmit` command with
/// varying numbers of operations.
fn batch_construction_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_construction");

    for &batch_size in &[1usize, 8, 32, 128] {
        let bytes = batch_size as u64 * 4096;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                b.iter(|| {
                    let ops: Vec<Command> = (0..size)
                        .map(|i| Command::WriteZeros {
                            ns_id: 1,
                            lba: i as u64,
                            num_blocks: 8,
                        })
                        .collect();
                    let _cmd = Command::BatchSubmit { ops };
                });
            },
        );
    }
    group.finish();
}

/// Benchmark full batch write throughput with SPDK hardware.
///
/// When hardware is unavailable, the benchmark group is skipped entirely.
/// When available, measures real batch write throughput at varying batch
/// sizes (1, 8, 32, 128 x 4KB blocks).
fn batch_write_throughput(c: &mut Criterion) {
    use block_device_spdk_nvme::{BlockDeviceSpdkNvmeComponentV1, IBlockDevice};
    use component_core::binding::bind;
    use component_core::iunknown::query;
    use spdk_env::SPDKEnvComponent;

    // Runtime hardware detection.
    if spdk_env::checks::check_vfio_available().is_err()
        || spdk_env::checks::check_hugepages().is_err()
    {
        eprintln!("batch_write_throughput: skipping — no SPDK hardware available");
        return;
    }

    let spdk_env_comp = SPDKEnvComponent::new_default();
    let block_dev = BlockDeviceSpdkNvmeComponentV1::new_default();

    bind(&*spdk_env_comp, "ISPDKEnv", &*block_dev, "spdk_env").expect("bind spdk_env");

    let ienv =
        query::<dyn spdk_env::ISPDKEnv + Send + Sync>(&*spdk_env_comp).expect("ISPDKEnv query");
    if let Err(e) = ienv.init() {
        eprintln!("batch_write_throughput: skipping — SPDK init failed: {e}");
        return;
    }

    let devices = ienv.devices();
    if devices.is_empty() {
        eprintln!("batch_write_throughput: skipping — no NVMe devices found");
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
        eprintln!("batch_write_throughput: skipping — block device init failed: {e}");
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
        eprintln!("batch_write_throughput: skipping — no namespaces");
        return;
    }
    let ns = &namespaces[0];
    let sector_size = ns.sector_size as usize;
    let ns_id = ns.ns_id;

    let mut group = c.benchmark_group("batch_write_throughput");
    for &batch_size in &[1usize, 8, 32, 128] {
        let bytes = batch_size as u64 * sector_size as u64;
        group.throughput(Throughput::Bytes(bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            &batch_size,
            |b, &size| {
                // Pre-allocate DMA buffers for the batch.
                let bufs: Vec<std::sync::Arc<interfaces::DmaBuffer>> = (0..size)
                    .map(|_| {
                        std::sync::Arc::new(
                            interfaces::DmaBuffer::new(sector_size, sector_size, None)
                                .expect("DMA alloc"),
                        )
                    })
                    .collect();

                b.iter(|| {
                    let ops: Vec<Command> = bufs
                        .iter()
                        .enumerate()
                        .map(|(i, buf)| Command::WriteSync {
                            ns_id,
                            lba: i as u64,
                            buf: std::sync::Arc::clone(buf),
                        })
                        .collect();
                    let batch = Command::BatchSubmit { ops };

                    channels.command_tx.send(batch).expect("send batch");

                    // Collect all completions.
                    for _ in 0..size {
                        let completion = channels.completion_rx.recv().expect("recv");
                        match completion {
                            block_device_spdk_nvme::Completion::WriteDone { result, .. } => {
                                result.expect("batch write failed")
                            }
                            other => panic!("expected WriteDone, got {other:?}"),
                        }
                    }
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    batch_construction_throughput,
    batch_write_throughput,
);
criterion_main!(benches);
