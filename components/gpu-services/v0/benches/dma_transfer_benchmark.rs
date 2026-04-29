//! Criterion benchmarks measuring CPU↔GPU DMA transfer throughput.
//!
//! Requires `--features gpu` and NVIDIA GPU hardware.
//! Measures cudaMemcpy performance across:
//! - Multiple transfer sizes (4 KiB to 64 MiB)
//! - Both directions (Host→Device, Device→Host)
//! - All available GPU devices
//! - Pageable vs pinned host memory

use criterion::{
    criterion_group, criterion_main, BenchmarkId, Criterion, SamplingMode, Throughput,
};
use std::ffi::c_void;
use std::os::raw::c_int;

use gpu_services::cuda_ffi;

const TRANSFER_SIZES: &[usize] = &[
    4 * 1024,            // 4 KiB
    64 * 1024,           // 64 KiB
    256 * 1024,          // 256 KiB
    1024 * 1024,         // 1 MiB
    4 * 1024 * 1024,     // 4 MiB
    16 * 1024 * 1024,    // 16 MiB
    64 * 1024 * 1024,    // 64 MiB
];

fn size_label(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{} MiB", bytes / (1024 * 1024))
    } else {
        format!("{} KiB", bytes / 1024)
    }
}

fn get_device_count() -> c_int {
    let mut count: c_int = 0;
    // SAFETY: count is a valid pointer to a stack-allocated c_int.
    let err = unsafe { cuda_ffi::cudaGetDeviceCount(&mut count) };
    if err != cuda_ffi::CUDA_SUCCESS {
        0
    } else {
        count
    }
}

fn get_device_name(device: c_int) -> String {
    // SAFETY: zeroed memory is a valid representation for cudaDeviceProp.
    let mut prop: cuda_ffi::cudaDeviceProp = unsafe { std::mem::zeroed() };
    // SAFETY: prop is a valid pointer; device is in range.
    let err = unsafe { cuda_ffi::cudaGetDeviceProperties(&mut prop, device) };
    if err != cuda_ffi::CUDA_SUCCESS {
        return format!("GPU{}", device);
    }
    // SAFETY: cudaGetDeviceProperties null-terminates prop.name.
    let cstr = unsafe { std::ffi::CStr::from_ptr(prop.name.as_ptr()) };
    cstr.to_string_lossy().into_owned()
}

/// Allocate GPU memory on a specific device.
fn gpu_alloc(device: c_int, size: usize) -> *mut c_void {
    // SAFETY: device is a valid device index.
    unsafe { cuda_ffi::cudaSetDevice(device) };
    let mut ptr: *mut c_void = std::ptr::null_mut();
    // SAFETY: ptr is a valid pointer to a local variable.
    let err = unsafe { cuda_ffi::cudaMalloc(&mut ptr, size) };
    if err != cuda_ffi::CUDA_SUCCESS {
        std::ptr::null_mut()
    } else {
        ptr
    }
}

/// Benchmark Host→Device transfers using pageable host memory.
fn bench_host_to_device_pageable(c: &mut Criterion) {
    let device_count = get_device_count();
    if device_count == 0 {
        eprintln!("Skipping H2D pageable: no GPU available");
        return;
    }

    let mut group = c.benchmark_group("dma_h2d_pageable");
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(20);

    for device in 0..device_count {
        let name = get_device_name(device);

        for &size in TRANSFER_SIZES {
            let dev_ptr = gpu_alloc(device, size);
            if dev_ptr.is_null() {
                continue;
            }

            let src = vec![0xABu8; size];
            group.throughput(Throughput::Bytes(size as u64));

            let bench_id = BenchmarkId::new(
                format!("{}_{}", name.replace(' ', "_"), size_label(size)),
                size,
            );

            group.bench_with_input(bench_id, &size, |b, &_sz| {
                b.iter(|| {
                    // SAFETY: dev_ptr is valid device memory of `size` bytes;
                    // src is a valid host buffer.
                    unsafe {
                        cuda_ffi::cudaMemcpy(
                            dev_ptr,
                            src.as_ptr() as *const c_void,
                            size,
                            cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
                        );
                    }
                });
            });

            // SAFETY: dev_ptr was allocated by cudaMalloc.
            unsafe { cuda_ffi::cudaFree(dev_ptr) };
        }
    }

    group.finish();
}

/// Benchmark Device→Host transfers using pageable host memory.
fn bench_device_to_host_pageable(c: &mut Criterion) {
    let device_count = get_device_count();
    if device_count == 0 {
        eprintln!("Skipping D2H pageable: no GPU available");
        return;
    }

    let mut group = c.benchmark_group("dma_d2h_pageable");
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(20);

    for device in 0..device_count {
        let name = get_device_name(device);

        for &size in TRANSFER_SIZES {
            let dev_ptr = gpu_alloc(device, size);
            if dev_ptr.is_null() {
                continue;
            }

            let mut dst = vec![0u8; size];
            group.throughput(Throughput::Bytes(size as u64));

            let bench_id = BenchmarkId::new(
                format!("{}_{}", name.replace(' ', "_"), size_label(size)),
                size,
            );

            group.bench_with_input(bench_id, &size, |b, &_sz| {
                b.iter(|| {
                    // SAFETY: dst is a valid host buffer of `size` bytes;
                    // dev_ptr is valid device memory.
                    unsafe {
                        cuda_ffi::cudaMemcpy(
                            dst.as_mut_ptr() as *mut c_void,
                            dev_ptr as *const c_void,
                            size,
                            cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
                        );
                    }
                });
            });

            // SAFETY: dev_ptr was allocated by cudaMalloc.
            unsafe { cuda_ffi::cudaFree(dev_ptr) };
        }
    }

    group.finish();
}

/// Benchmark Host→Device transfers using pinned (page-locked) host memory.
fn bench_host_to_device_pinned(c: &mut Criterion) {
    let device_count = get_device_count();
    if device_count == 0 {
        eprintln!("Skipping H2D pinned: no GPU available");
        return;
    }

    let mut group = c.benchmark_group("dma_h2d_pinned");
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(20);

    for device in 0..device_count {
        let name = get_device_name(device);

        for &size in TRANSFER_SIZES {
            let dev_ptr = gpu_alloc(device, size);
            if dev_ptr.is_null() {
                continue;
            }

            // Allocate page-aligned host memory and pin it
            let layout = std::alloc::Layout::from_size_align(size, 4096).unwrap();
            // SAFETY: layout has non-zero size.
            let host_ptr = unsafe { std::alloc::alloc(layout) };
            if host_ptr.is_null() {
                unsafe { cuda_ffi::cudaFree(dev_ptr) };
                continue;
            }

            // Fill with pattern
            unsafe { std::ptr::write_bytes(host_ptr, 0xCD, size) };

            // Pin the host memory for DMA
            // SAFETY: host_ptr is a valid, aligned allocation of `size` bytes.
            let err =
                unsafe { cuda_ffi::cudaHostRegister(host_ptr as *mut c_void, size, 0) };
            if err != cuda_ffi::CUDA_SUCCESS {
                unsafe {
                    std::alloc::dealloc(host_ptr, layout);
                    cuda_ffi::cudaFree(dev_ptr);
                }
                continue;
            }

            group.throughput(Throughput::Bytes(size as u64));

            let bench_id = BenchmarkId::new(
                format!("{}_{}", name.replace(' ', "_"), size_label(size)),
                size,
            );

            group.bench_with_input(bench_id, &size, |b, &_sz| {
                b.iter(|| {
                    // SAFETY: dev_ptr is valid device memory; host_ptr is pinned host memory.
                    unsafe {
                        cuda_ffi::cudaMemcpy(
                            dev_ptr,
                            host_ptr as *const c_void,
                            size,
                            cuda_ffi::CUDA_MEMCPY_HOST_TO_DEVICE,
                        );
                    }
                });
            });

            // SAFETY: host_ptr was registered with cudaHostRegister.
            unsafe { cuda_ffi::cudaHostUnregister(host_ptr as *mut c_void) };
            // SAFETY: host_ptr was allocated with std::alloc::alloc(layout).
            unsafe { std::alloc::dealloc(host_ptr, layout) };
            // SAFETY: dev_ptr was allocated by cudaMalloc.
            unsafe { cuda_ffi::cudaFree(dev_ptr) };
        }
    }

    group.finish();
}

/// Benchmark Device→Host transfers using pinned (page-locked) host memory.
fn bench_device_to_host_pinned(c: &mut Criterion) {
    let device_count = get_device_count();
    if device_count == 0 {
        eprintln!("Skipping D2H pinned: no GPU available");
        return;
    }

    let mut group = c.benchmark_group("dma_d2h_pinned");
    group.sampling_mode(SamplingMode::Flat);
    group.sample_size(20);

    for device in 0..device_count {
        let name = get_device_name(device);

        for &size in TRANSFER_SIZES {
            let dev_ptr = gpu_alloc(device, size);
            if dev_ptr.is_null() {
                continue;
            }

            // Allocate page-aligned host memory and pin it
            let layout = std::alloc::Layout::from_size_align(size, 4096).unwrap();
            // SAFETY: layout has non-zero size.
            let host_ptr = unsafe { std::alloc::alloc(layout) };
            if host_ptr.is_null() {
                unsafe { cuda_ffi::cudaFree(dev_ptr) };
                continue;
            }

            // SAFETY: host_ptr is a valid, aligned allocation of `size` bytes.
            let err =
                unsafe { cuda_ffi::cudaHostRegister(host_ptr as *mut c_void, size, 0) };
            if err != cuda_ffi::CUDA_SUCCESS {
                unsafe {
                    std::alloc::dealloc(host_ptr, layout);
                    cuda_ffi::cudaFree(dev_ptr);
                }
                continue;
            }

            group.throughput(Throughput::Bytes(size as u64));

            let bench_id = BenchmarkId::new(
                format!("{}_{}", name.replace(' ', "_"), size_label(size)),
                size,
            );

            group.bench_with_input(bench_id, &size, |b, &_sz| {
                b.iter(|| {
                    // SAFETY: host_ptr is pinned host memory; dev_ptr is valid device memory.
                    unsafe {
                        cuda_ffi::cudaMemcpy(
                            host_ptr as *mut c_void,
                            dev_ptr as *const c_void,
                            size,
                            cuda_ffi::CUDA_MEMCPY_DEVICE_TO_HOST,
                        );
                    }
                });
            });

            // SAFETY: host_ptr was registered with cudaHostRegister.
            unsafe { cuda_ffi::cudaHostUnregister(host_ptr as *mut c_void) };
            // SAFETY: host_ptr was allocated with std::alloc::alloc(layout).
            unsafe { std::alloc::dealloc(host_ptr, layout) };
            // SAFETY: dev_ptr was allocated by cudaMalloc.
            unsafe { cuda_ffi::cudaFree(dev_ptr) };
        }
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_host_to_device_pageable,
    bench_device_to_host_pageable,
    bench_host_to_device_pinned,
    bench_device_to_host_pinned
);
criterion_main!(benches);
