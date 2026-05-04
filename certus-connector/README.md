# certus-connector

vLLM storage connector for the Certus system. Single installable package providing both the native Rust engine and the Python vLLM plugin.

## Architecture

```
vLLM OffloadingConnectorScheduler
  │
  ├─ certus_connector.spec.CertusOffloadingSpec    ← vLLM plugin entry point
  │     │
  │     ├─ get_manager() → NativeCertusOffloadingManager (production)
  │     │                 → CertusOffloadingManager      (mock/testing)
  │     │
  │     └─ get_handlers() → GpuToCertusHandler, CertusToGpuHandler
  │
  └─ certus_native (Rust PyO3 extension)
        │
        └─ CertusEngine
              ├─ dispatcher     (orchestrates cache operations)
              ├─ dispatch-map   (key→location index)
              ├─ gpu-services   (CUDA DMA)
              └─ spdk-env       (NVMe I/O via SPDK)
```

**Python** (`certus_connector/`) — thin vLLM adapter. Converts OffloadKey bytes to u64, constructs PrepareStoreOutput, emits OffloadingEvents.

**Rust** (`src/`) — owns all state: index, allocation, eviction (LRU), DMA transfers, NVMe I/O. Exposed to Python as `certus_native.CertusEngine`.

## Package contents

| Path | What |
|------|------|
| `src/lib.rs` | PyO3 module — `CertusEngine` class definition |
| `src/engine.rs` | Wires Rust component stack (dispatcher, dispatch-map, gpu-services, spdk-env) |
| `src/keys.rs` | OffloadKey (u64) ↔ CacheKey mapping |
| `certus_connector/spec.py` | `CertusOffloadingSpec` — vLLM OffloadingSpec implementation |
| `certus_connector/manager.py` | Mock manager (Python-only, for testing without hardware) |
| `certus_connector/native_manager.py` | Production manager (thin proxy to `certus_native`) |
| `certus_connector/handler.py` | Transfer handlers (GPU↔Certus I/O) |
| `certus_connector/mediums.py` | `CertusLoadStoreSpec` medium definition |

## Build

Requires SPDK and CUDA for full native build. Without hardware, the mock manager path works for development/testing.

```bash
# Python tests (no hardware needed)
python3 -m pytest tests/ -v

# Full build (requires SPDK + CUDA)
pip install -e .

# Rust type-check only (will fail at spdk-sys link without SPDK libs)
cargo check -p certus-connector
```

## vLLM configuration

```json
{
    "spec_name": "CertusOffloadingSpec",
    "spec_module_path": "certus_connector.spec",
    "data_pci_addrs": ["0000:02:00.0"],
    "metadata_pci_addr": "0000:01:00.0",
    "slab_size_bytes": 131072,
    "dram_cache_bytes": 8589934592,
    "io_queue_depth": 128
}
```

Set `"use_native": false` to force the mock manager (for testing without hardware).
