//! Criterion benchmarks for read/write throughput on a kernel block device.
//!
//! Measures sequential read and write throughput at 1, 8, 32, 128 × 4KB blocks.
//!
//! Run with: `cargo bench --bench throughput -p block-device-kernel`
//!
//! Set `BENCH_DEVICE_PATH` to override the default /dev/nvme0n1:
//!   BENCH_DEVICE_PATH=/dev/sda cargo bench --bench throughput -p block-device-kernel

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};

use interfaces::Command;

fn bench_device() -> String {
    std::env::var("BENCH_DEVICE_PATH").unwrap_or_else(|_| "/dev/nvme0n1".to_string())
}

/// Benchmark write throughput with actual block device IO.
fn write_throughput(c: &mut Criterion) {
    use block_device_kernel::BlockDeviceKernelComponent;
    use interfaces::IBlockDevice;

    let device = bench_device();
    let comp = BlockDeviceKernelComponent::create(&device, 4096, 4096);
    comp.initialize().expect("init failed");
    let channels = comp.connect_client().expect("connect failed");

    let mut group = c.benchmark_group("write_throughput");

    for &num_blocks in &[1u32, 8, 32, 128] {
        let total_bytes = num_blocks as u64 * 4096;
        group.throughput(Throughput::Bytes(total_bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(num_blocks),
            &num_blocks,
            |b, &blocks| {
                b.iter(|| {
                    channels
                        .command_tx
                        .send(Command::WriteZeros {
                            ns_id: 1,
                            lba: 0,
                            num_blocks: blocks,
                        })
                        .unwrap();
                    let _completion = channels.completion_rx.recv().unwrap();
                });
            },
        );
    }

    group.finish();
    comp.shutdown().expect("shutdown failed");
}

/// Benchmark read throughput with actual block device IO.
fn read_throughput(c: &mut Criterion) {
    use block_device_kernel::BlockDeviceKernelComponent;
    use interfaces::{DmaBuffer, IBlockDevice};
    use std::sync::{Arc, Mutex};

    let device = bench_device();
    let comp = BlockDeviceKernelComponent::create(&device, 4096, 4096);
    comp.initialize().expect("init failed");
    let channels = comp.connect_client().expect("connect failed");

    // Pre-fill so reads return real data
    channels
        .command_tx
        .send(Command::WriteZeros {
            ns_id: 1,
            lba: 0,
            num_blocks: 128,
        })
        .unwrap();
    let _ = channels.completion_rx.recv().unwrap();

    let mut group = c.benchmark_group("read_throughput");

    for &num_blocks in &[1u32, 8, 32, 128] {
        let total_bytes = num_blocks as u64 * 4096;
        group.throughput(Throughput::Bytes(total_bytes));
        group.bench_with_input(
            BenchmarkId::from_parameter(num_blocks),
            &num_blocks,
            |b, &blocks| {
                let buf_size = blocks as usize * 4096;
                let buf = unsafe {
                    let mut ptr: *mut std::ffi::c_void = std::ptr::null_mut();
                    let ret = libc::posix_memalign(&mut ptr, 512, buf_size);
                    assert_eq!(ret, 0, "posix_memalign failed");
                    std::ptr::write_bytes(ptr as *mut u8, 0, buf_size);
                    DmaBuffer::from_raw(ptr, buf_size, libc_free, -1).unwrap()
                };
                let buf_arc = Arc::new(Mutex::new(buf));

                b.iter(|| {
                    channels
                        .command_tx
                        .send(Command::ReadSync {
                            ns_id: 1,
                            lba: 0,
                            buf: Arc::clone(&buf_arc),
                        })
                        .unwrap();
                    let _completion = channels.completion_rx.recv().unwrap();
                });
            },
        );
    }

    group.finish();
    comp.shutdown().expect("shutdown failed");
}

unsafe extern "C" fn libc_free(ptr: *mut std::ffi::c_void) {
    libc::free(ptr);
}

criterion_group!(benches, write_throughput, read_throughput);
criterion_main!(benches);
