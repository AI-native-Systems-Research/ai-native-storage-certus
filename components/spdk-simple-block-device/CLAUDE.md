# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is the `spdk-simple-block-device` component of the **Certus** project. It provides synchronous block I/O over SPDK's user-space NVMe driver, exposed as a component-framework interface (`IBlockDevice`).

The component probes the first NVMe controller on the local PCIe bus, opens namespace 1, and wraps SPDK's async submit+poll NVMe commands into synchronous `read_blocks`/`write_blocks` calls with automatic DMA buffer management.

## Build Commands

```bash
cargo build -p spdk-simple-block-device          # Build
cargo test -p spdk-simple-block-device            # Unit tests (no hardware needed)
cargo clippy -p spdk-simple-block-device -- -D warnings  # Lint
cargo doc -p spdk-simple-block-device --no-deps   # Docs
```

The `basic_io` example requires real NVMe hardware bound to vfio-pci with hugepages:
```bash
cargo run --example basic_io
```

## Architecture

### Dependency Chain

```
spdk-sys          (raw FFI: env.h + nvme.h bindings via bindgen)
    |
spdk-env          (safe wrapper: ISPDKEnv init, VFIO checks, device enum)
    |
spdk-simple-block-device  (this crate: IBlockDevice sync read/write)
```

### Key Files

- `src/lib.rs` — `IBlockDevice` interface and `SimpleBlockDevice` component definition. Receptacles: `spdk_env: ISPDKEnv`, `logger: ILogger`.
- `src/io.rs` — Internal NVMe operations (`do_open`, `do_close`, `do_read`, `do_write`). Contains the synchronous I/O pattern: submit NVMe command with completion callback, then busy-poll `spdk_nvme_qpair_process_completions`.
- `src/error.rs` — `BlockDeviceError` enum with all error variants.
- `examples/basic_io.rs` — Full wiring example: logger -> env -> block device -> write/read/verify.

### Synchronous I/O Pattern

SPDK NVMe is async (submit + poll). This crate wraps it synchronously:
1. Allocate DMA buffer via `spdk_dma_zmalloc`
2. Copy user data in (write) or prepare empty buffer (read)
3. Submit `spdk_nvme_ns_cmd_{read,write}` with a callback that sets an `AtomicBool`
4. Busy-poll `spdk_nvme_qpair_process_completions` until done
5. Copy data out (read) and free DMA buffer

### Thread Safety

Raw SPDK pointers are stored in `Mutex<Option<InnerState>>`. The Mutex ensures the SPDK single-thread-per-qpair requirement is met.

## SPDK Prerequisites

Before running code that calls `open()`:
1. System deps: `../../deps/install_deps.sh`
2. SPDK built: `../../deps/build_spdk.sh`
3. NVMe devices bound to vfio-pci: `../../deps/spdk/scripts/setup.sh`
4. Hugepages allocated: `echo 1024 > /proc/sys/vm/nr_hugepages`
