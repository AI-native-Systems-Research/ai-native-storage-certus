//! Criterion benchmarks for GPU Services component.
//!
//! Requires `--features gpu` and NVIDIA GPU hardware to produce
//! meaningful results.

use criterion::{criterion_group, criterion_main, Criterion};

use component_core::query_interface;
use gpu_services::GpuServicesComponentV0;
use interfaces::IGpuServices;

fn bench_initialize(c: &mut Criterion) {
    c.bench_function("gpu_initialize", |b| {
        b.iter(|| {
            let component = GpuServicesComponentV0::new();
            let gpu = query_interface!(component, IGpuServices).unwrap();
            let _ = gpu.initialize();
            let _ = gpu.shutdown();
        });
    });
}

fn bench_get_devices(c: &mut Criterion) {
    let component = GpuServicesComponentV0::new();
    let gpu = query_interface!(component, IGpuServices).unwrap();
    if gpu.initialize().is_err() {
        eprintln!("Skipping bench_get_devices: CUDA init failed");
        return;
    }

    c.bench_function("gpu_get_devices", |b| {
        b.iter(|| {
            let _ = gpu.get_devices();
        });
    });

    let _ = gpu.shutdown();
}

fn bench_deserialize_ipc_handle(c: &mut Criterion) {
    use base64::Engine;

    let component = GpuServicesComponentV0::new();
    let gpu = query_interface!(component, IGpuServices).unwrap();
    if gpu.initialize().is_err() {
        eprintln!("Skipping bench_deserialize: CUDA init failed");
        return;
    }

    // Create a synthetic payload (won't produce a valid GPU pointer but
    // measures the decode + open path).
    let mut payload_bytes = [0u8; 72];
    payload_bytes[64..72].copy_from_slice(&4096u64.to_le_bytes());
    let payload = base64::engine::general_purpose::STANDARD.encode(payload_bytes);

    c.bench_function("gpu_deserialize_ipc_handle", |b| {
        b.iter(|| {
            // This will fail at cudaIpcOpenMemHandle (synthetic handle)
            // but measures the decode path.
            let _ = gpu.deserialize_ipc_handle(&payload);
        });
    });

    let _ = gpu.shutdown();
}

fn bench_verify_memory(c: &mut Criterion) {
    use base64::Engine;

    let component = GpuServicesComponentV0::new();
    let gpu = query_interface!(component, IGpuServices).unwrap();
    if gpu.initialize().is_err() {
        eprintln!("Skipping bench_verify_memory: CUDA init failed");
        return;
    }

    let mut payload_bytes = [0u8; 72];
    payload_bytes[64..72].copy_from_slice(&4096u64.to_le_bytes());
    let payload = base64::engine::general_purpose::STANDARD.encode(payload_bytes);

    let handle = match gpu.deserialize_ipc_handle(&payload) {
        Ok(h) => h,
        Err(_) => {
            eprintln!("Skipping bench_verify_memory: no valid IPC handle");
            let _ = gpu.shutdown();
            return;
        }
    };

    c.bench_function("gpu_verify_memory", |b| {
        b.iter(|| {
            let _ = gpu.verify_memory(&handle);
        });
    });

    let _ = gpu.shutdown();
}

fn bench_pin_unpin(c: &mut Criterion) {
    use base64::Engine;

    let component = GpuServicesComponentV0::new();
    let gpu = query_interface!(component, IGpuServices).unwrap();
    if gpu.initialize().is_err() {
        eprintln!("Skipping bench_pin_unpin: CUDA init failed");
        return;
    }

    let mut payload_bytes = [0u8; 72];
    payload_bytes[64..72].copy_from_slice(&4096u64.to_le_bytes());
    let payload = base64::engine::general_purpose::STANDARD.encode(payload_bytes);

    let handle = match gpu.deserialize_ipc_handle(&payload) {
        Ok(h) => h,
        Err(_) => {
            eprintln!("Skipping bench_pin_unpin: no valid IPC handle");
            let _ = gpu.shutdown();
            return;
        }
    };

    if gpu.verify_memory(&handle).is_err() {
        eprintln!("Skipping bench_pin_unpin: verify failed");
        let _ = gpu.shutdown();
        return;
    }

    c.bench_function("gpu_pin_unpin", |b| {
        b.iter(|| {
            let _ = gpu.pin_memory(&handle);
            let _ = gpu.unpin_memory(&handle);
        });
    });

    let _ = gpu.shutdown();
}

fn bench_create_dma_buffer(c: &mut Criterion) {
    use base64::Engine;

    let component = GpuServicesComponentV0::new();
    let gpu = query_interface!(component, IGpuServices).unwrap();
    if gpu.initialize().is_err() {
        eprintln!("Skipping bench_create_dma_buffer: CUDA init failed");
        return;
    }

    let mut payload_bytes = [0u8; 72];
    payload_bytes[64..72].copy_from_slice(&4096u64.to_le_bytes());
    let payload = base64::engine::general_purpose::STANDARD.encode(payload_bytes);

    // Attempt to get a handle and prepare it for DMA buffer creation.
    // On systems without a real GPU+IPC handle, this benchmark will skip.
    let handle = match gpu.deserialize_ipc_handle(&payload) {
        Ok(h) => h,
        Err(_) => {
            eprintln!("Skipping bench_create_dma_buffer: no valid IPC handle");
            let _ = gpu.shutdown();
            return;
        }
    };

    if gpu.verify_memory(&handle).is_err() || gpu.pin_memory(&handle).is_err() {
        eprintln!("Skipping bench_create_dma_buffer: verify/pin failed");
        let _ = gpu.shutdown();
        return;
    }

    c.bench_function("gpu_create_dma_buffer", |b| {
        b.iter(|| {
            // Re-deserialize each iteration since create_dma_buffer consumes the handle
            if let Ok(h) = gpu.deserialize_ipc_handle(&payload) {
                let _ = gpu.verify_memory(&h);
                let _ = gpu.pin_memory(&h);
                let _ = gpu.create_dma_buffer(h);
            }
        });
    });

    let _ = gpu.shutdown();
}

criterion_group!(
    benches,
    bench_initialize,
    bench_get_devices,
    bench_deserialize_ipc_handle,
    bench_verify_memory,
    bench_pin_unpin,
    bench_create_dma_buffer
);
criterion_main!(benches);
