use std::sync::Arc;
use std::time::Duration;

use component_core::query_interface;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dispatch_map::{DispatchMapComponentV0, DispatchMapState};
use interfaces::{DmaAllocFn, DmaBuffer, IDispatchMap};

fn mock_dma_alloc() -> DmaAllocFn {
    Arc::new(|size, _align, _numa| {
        let layout = std::alloc::Layout::from_size_align(size, 4096).unwrap();
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            return Err("allocation failed".into());
        }
        unsafe {
            DmaBuffer::from_raw(
                ptr as *mut std::ffi::c_void,
                size,
                mock_free as unsafe extern "C" fn(*mut std::ffi::c_void),
                -1,
            )
        }
        .map_err(|e| e.to_string())
    })
}

unsafe extern "C" fn mock_free(ptr: *mut std::ffi::c_void) {
    if !ptr.is_null() {
        unsafe {
            std::alloc::dealloc(
                ptr as *mut u8,
                std::alloc::Layout::from_size_align_unchecked(1, 1),
            );
        }
    }
}

fn bench_lookup_no_contention(c: &mut Criterion) {
    let comp = DispatchMapComponentV0::new(DispatchMapState::new());
    let dm = query_interface!(comp, IDispatchMap).unwrap();
    dm.set_dma_alloc(mock_dma_alloc());

    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();

    c.bench_function("lookup_no_contention", |b| {
        b.iter(|| {
            let result = dm.lookup(black_box(1), Duration::from_millis(10)).unwrap();
            dm.release_read(black_box(1)).unwrap();
            black_box(result);
        });
    });
}

fn bench_ref_ops_throughput(c: &mut Criterion) {
    let comp = DispatchMapComponentV0::new(DispatchMapState::new());
    let dm = query_interface!(comp, IDispatchMap).unwrap();
    dm.set_dma_alloc(mock_dma_alloc());

    let _ = dm.create_staging(1, 1).unwrap();
    dm.release_write(1).unwrap();

    c.bench_function("take_release_read", |b| {
        b.iter(|| {
            dm.take_read(black_box(1), Duration::from_millis(10))
                .unwrap();
            dm.release_read(black_box(1)).unwrap();
        });
    });

    c.bench_function("take_release_write", |b| {
        b.iter(|| {
            dm.take_write(black_box(1), Duration::from_millis(10))
                .unwrap();
            dm.release_write(black_box(1)).unwrap();
        });
    });
}

fn bench_entry_size(c: &mut Criterion) {
    use dispatch_map::entry_size;

    c.bench_function("entry_size_check", |b| {
        b.iter(|| {
            let size = entry_size();
            assert!(size <= 48, "DispatchEntry is {size} bytes, expected ≤ 48");
            black_box(size);
        });
    });
}

criterion_group!(
    benches,
    bench_lookup_no_contention,
    bench_ref_ops_throughput,
    bench_entry_size
);
criterion_main!(benches);
