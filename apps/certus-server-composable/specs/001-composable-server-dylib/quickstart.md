# Quickstart: certus-server-composable

## Prerequisites

- Linux system (RHEL/Fedora)
- Rust stable toolchain (MSRV 1.75)
- SPDK pre-built at `deps/spdk-build/` (for NVMe components)
- CUDA toolkit (for GPU services component)
- NVMe device(s) available (for production use)

## Build

```bash
# Build all component dylibs
cargo build --workspace --release

# Build certus-server-composable binary
cargo build -p certus-server-composable --release
```

## Create Configuration

Create a JSON configuration file (e.g., `config.json`):

```json
{
  "variables": {
    "num_ssd_devices": 2
  },
  "search_paths": ["./target/release"],
  "server": {
    "listen": "0.0.0.0:50051",
    "device_pci": ["0000:41:00.0", "0000:42:00.0"],
    "memory_tier_size": "2G",
    "format": false
  },
  "components": [
    { "name": "logger", "dylib": "liblogger.so" },
    { "name": "spdk-env", "dylib": "libspdk_env.so" },
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
    { "target": "dispatcher", "receptacle": "gpu_services", "source": "gpu-services" },
    { "target": "dispatcher", "receptacle": "spdk_env", "source": "spdk-env" }
  ]
}
```

## Run

```bash
# Start the server
./target/release/certus-server-composable --config config.json

# Override listen address via CLI
./target/release/certus-server-composable --config config.json --listen 127.0.0.1:9000

# Format extent managers (clean start)
./target/release/certus-server-composable --config config.json --format
```

## Verify

The server exposes the same gRPC interface as `certus-server`. Existing Python clients work unchanged:

```bash
# Check server is responding (using grpcurl or existing client)
grpcurl -plaintext localhost:50051 certus.dispatcher.v1.Dispatcher/Check
```

## Variable-Driven Deployment

Scale to 4 SSDs by changing only the variable:

```json
{
  "variables": { "num_ssd_devices": 4 },
  "components": [
    { "name": "block-device", "dylib": "libblock_device_spdk_nvme.so", "instances": "$num_ssd_devices" }
  ]
}
```

## Environment Variables

Set `CERTUS_LIB_PATH` to prepend additional search directories:

```bash
export CERTUS_LIB_PATH=/opt/certus/lib:/usr/local/lib/certus
./target/release/certus-server-composable --config config.json
```
