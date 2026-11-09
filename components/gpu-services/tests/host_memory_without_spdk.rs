//! Regression test for spec-sync A-14 (2026-11-06): registering host memory when
//! no SPDK environment is active.
//!
//! `register_host_memory` pins a buffer with CUDA and registers it with SPDK, and
//! `unregister_host_memory` undoes both. In a process with a GPU but no SPDK
//! environment (memory-tier-only mode), the SPDK calls dereference an SPDK memory
//! map that does not exist, and the process crashes with SIGSEGV: before the fix,
//! in `spdk_mem_register` (the dispatcher's `initialize`) and in
//! `spdk_mem_unregister` (its `shutdown`). Found on node7 (A30) by J40.
//!
//! Each case runs in a child process, so a crash is a test failure, not a crash
//! of the test binary. Needs a real GPU:
//! `cargo test -p gpu-services --features p2p --test host_memory_without_spdk`

use std::process::Command;
use std::sync::Arc;

use component_core::query_interface;
use interfaces::{IGpuServices, ILogger};

const CHILD_ENV: &str = "A14_CHILD";
const SIZE: usize = 2 * 1024 * 1024;

/// The child: register, then unregister, a page-aligned buffer with no SPDK
/// environment initialized in this process.
fn child() {
    assert!(
        !interfaces::is_spdk_env_active(),
        "this test needs a process without an SPDK environment"
    );
    let logger: Arc<dyn ILogger + Send + Sync> = logger::LoggerComponent::new_default();
    let comp = gpu_services::GpuServicesComponent::new_default();
    comp.logger.connect(logger).unwrap();
    let gpu: Arc<dyn IGpuServices + Send + Sync> = query_interface!(comp, IGpuServices).unwrap();
    if let Err(e) = gpu.initialize() {
        // No GPU on this machine: nothing to check here (the parent sees a pass).
        eprintln!("SKIP: GPU initialize failed ({e}); this test needs a GPU");
        return;
    }

    // SAFETY: an anonymous private mapping of SIZE bytes, page-aligned, unmapped below.
    let ptr = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            SIZE,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
            -1,
            0,
        )
    };
    assert_ne!(ptr, libc::MAP_FAILED);

    gpu.register_host_memory(ptr, SIZE)
        .expect("register_host_memory must succeed without SPDK (CUDA pinning only)");
    gpu.unregister_host_memory(ptr, SIZE)
        .expect("unregister_host_memory must succeed without SPDK");

    // SAFETY: ptr/SIZE are the mapping created above.
    unsafe { libc::munmap(ptr, SIZE) };
    let _ = gpu.shutdown();
}

#[test]
fn register_and_unregister_host_memory_without_spdk_do_not_crash() {
    if std::env::var_os(CHILD_ENV).is_some() {
        child();
        return;
    }
    let out = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "register_and_unregister_host_memory_without_spdk_do_not_crash",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ENV, "1")
        .output()
        .expect("spawn the child test process");
    assert!(
        out.status.success(),
        "the child process failed ({:?}): register/unregister of host memory without an SPDK \
         environment must not crash\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}
