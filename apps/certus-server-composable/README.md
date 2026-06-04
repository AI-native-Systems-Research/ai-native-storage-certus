# certus-server-composable

A runtime-configurable variant of `certus-server` that loads all components as dynamic libraries (`.so`) based on a JSON configuration file. Exposes the identical `certus.dispatcher.v1` gRPC API.

## How It Differs from certus-server

| | certus-server | certus-server-composable |
|---|---|---|
| Component wiring | Hard-coded in `main.rs` | Defined in JSON config |
| Adding components | Requires code change + recompile | Add dylib + update config |
| Hardware adaptation | CLI flags only | Variables in config (e.g., `$num_ssd_devices`) |
| Component loading | Static linking (rlib) | Dynamic loading (`.so` at runtime) |
| Binary dependencies | All components compiled in | Only `component-core`, `interfaces`, gRPC stack |

Both binaries expose the same gRPC interface and produce identical benchmark results (zero measurable overhead from dynamic loading).

## Building

All component dylibs and the binary **must** be built in a single `cargo build` invocation. This ensures consistent TypeIds across dylib boundaries for name-based interface binding.

```bash
# From the workspace root:
cargo build --release --workspace --features \
  "logger/dylib,spdk-env/dylib,gpu-services/dylib,block-device-spdk-nvme/dylib,extent-manager/dylib,dispatch-map/dylib,memory-tier/dylib,dispatcher/dylib"
```

This produces:
- `target/release/certus-server-composable` (the binary)
- `target/release/lib{logger,gpu_services,dispatch_map,memory_tier,dispatcher}.so` (component dylibs)

## Running

```bash
CERTUS_LIB_PATH=./target/release \
  ./target/release/certus-server-composable \
  --config apps/certus-server-composable/configs/example-dev.json \
  --format
```

### Required arguments

| Argument | Description |
|----------|-------------|
| `--config <path>` | Path to JSON configuration file (mandatory) |

### Optional arguments (override JSON config values)

| Argument | Description |
|----------|-------------|
| `--listen <addr>` | gRPC listen address (default: `0.0.0.0:50051`) |
| `--device-pci <addr>` | NVMe PCI address (repeatable) |
| `--drive-count <N>` | Auto-select first N NVMe drives |
| `--memory-tier-size <size>` | Memory pool size (e.g., `2G`, `256M`) |
| `--format` | Format extent managers on startup (destroys data) |
| `--tls-cert <path>` | TLS certificate file |
| `--tls-key <path>` | TLS private key file |
| `--poller-base-cpu <N>` | Base CPU core for NVMe poller pinning |

### Environment variables

| Variable | Description |
|----------|-------------|
| `CERTUS_LIB_PATH` | Colon-separated directories prepended to the dylib search path |

## JSON Configuration

```json
{
  "variables": { "num_ssd_devices": 4 },
  "search_paths": ["./target/release"],
  "server": {
    "listen": "0.0.0.0:50051",
    "drive_count": 4,
    "memory_tier_size": "2G",
    "format": true,
    "poller_base_cpu": 4
  },
  "components": [
    { "name": "logger", "dylib": "liblogger.so" },
    { "name": "gpu-services", "dylib": "libgpu_services.so" },
    { "name": "dispatch-map", "dylib": "libdispatch_map.so" },
    { "name": "memory-tier", "dylib": "libmemory_tier.so" },
    { "name": "dispatcher", "dylib": "libdispatcher.so" }
  ],
  "bindings": [
    { "target": "gpu-services", "receptacle": "logger", "source": "logger" },
    { "target": "dispatch-map", "receptacle": "logger", "source": "logger" },
    { "target": "memory-tier", "receptacle": "logger", "source": "logger" },
    { "target": "dispatcher", "receptacle": "logger", "source": "logger" },
    { "target": "dispatcher", "receptacle": "dispatch_map", "source": "dispatch-map" },
    { "target": "dispatcher", "receptacle": "memory_tier", "source": "memory-tier" },
    { "target": "dispatcher", "receptacle": "gpu_services", "source": "gpu-services" }
  ]
}
```

### Configuration sections

- **variables** — Named integer values substituted into `instances` fields (e.g., `"instances": "$num_ssd_devices"`)
- **search_paths** — Ordered directories for dylib resolution
- **server** — Server parameters (all overridable by CLI)
- **components** — Which dylibs to load and how many instances to create
- **bindings** — How to wire component receptacles to interface providers
- **init_order** — (optional) Explicit initialization order; defaults to topological sort of binding dependencies

### SPDK components

Due to SPDK's process-global state, block-device, extent-manager, and spdk-env are linked statically into `libdispatcher.so`. The dispatcher creates and manages NVMe drives internally using `server.device_pci` or `server.drive_count` from the config.

## Prerequisites

- Linux (RHEL/Fedora)
- Rust stable toolchain (MSRV 1.75)
- SPDK pre-built at `deps/spdk-build/`
- CUDA toolkit (for GPU services)
- NVMe devices bound to vfio-pci
- Hugepages configured (1GB pages recommended)
- `memlock` set to unlimited

## Running the benchmark

```bash
python3 apps/python/certus-api-bench.py \
  --server localhost:50051 \
  --clients 1 \
  --num-objects 16 \
  --iterations 10 \
  --block-size 4M
```

## Architecture

```
┌─────────────────────────────────────────────────┐
│           certus-server-composable              │
│  (binary: CLI, config parser, gRPC server)      │
├─────────────────────────────────────────────────┤
│  JSON Config → Resolver → Loader → Binder       │
│       ↓            ↓         ↓        ↓         │
│  Topo Sort    Search Path  dlopen  connect_raw  │
└───────┬─────────────┬────────┬────────┬─────────┘
        │             │        │        │
   ┌────▼──┐   ┌─────▼──┐  ┌──▼───┐  ┌─▼──────────┐
   │logger │   │gpu-svc │  │disp- │  │dispatcher  │
   │  .so  │   │  .so   │  │map.so│  │    .so     │
   └───────┘   └────────┘  └──────┘  │(+spdk-env) │
                                      │(+block-dev)│
                                      │(+extent-mg)│
                                      └────────────┘
```

Components bind to each other via name-based interface matching across dylib boundaries. The dispatcher dylib bundles all SPDK-dependent components internally.
