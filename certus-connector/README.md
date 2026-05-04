# certus-connector

vLLM **OffloadingSpec** plugin for the Certus storage system. Implements vLLM's `OffloadingSpec` ABC so that `OffloadingConnectorScheduler` can offload KV cache blocks to tiered DRAM + raw NVMe storage via SPDK.

Single installable package providing both the native Rust engine (PyO3) and the Python vLLM adapter.

## How it fits into vLLM

```
vLLM OffloadingConnectorScheduler          ← vLLM's internal scheduler
  │                                           (we do NOT implement this)
  │  loads via kv_connector_extra_config:
  │    spec_module_path = "certus_connector.spec"
  │    spec_name = "CertusOffloadingSpec"
  │
  ▼
CertusOffloadingSpec (OffloadingSpec)       ← OUR plugin entry point
  │
  ├─ get_manager() → OffloadingManager     ← allocation / eviction decisions
  │     ├─ NativeCertusOffloadingManager       (production, backed by Rust)
  │     └─ CertusOffloadingManager             (mock, pure Python for testing)
  │
  └─ get_handlers() → OffloadingHandler    ← actual GPU ↔ storage DMA
        ├─ GpuToCertusHandler                  (store: GPU → DRAM staging → NVMe)
        └─ CertusToGpuHandler                  (load:  NVMe/DRAM → GPU)
```

This is the same plugin contract that llm-d's `SharedStorageOffloadingSpec` uses. The difference: llm-d uses POSIX files on shared storage, we use raw NVMe via SPDK with no filesystem.

## Rust engine (certus_native)

The Python handlers delegate to a Rust PyO3 extension module (`certus_native`) which assembles and wires the Certus component stack:

```
certus_native.CertusEngine                 ← PyO3 class (assembler, not a component)
  │
  │  instantiates & connects:
  │
  ├─ dispatcher        components/dispatcher/v0/       orchestrates cache ops
  ├─ dispatch-map      components/dispatch-map/v0/     key → location index
  ├─ gpu-services      components/gpu-services/v0/     CUDA DMA transfers
  └─ spdk-env          components/spdk-env/            SPDK environment init
```

These are reusable Rust components (defined with `define_component!`) that live in the repo under `components/`. The `CertusEngine` is the application-level assembler — it creates each component, connects their receptacles (typed dependency slots), and exposes the combined API to Python.

The dispatcher internally creates NVMe block devices and extent managers during `initialize()` based on the PCI addresses in config.

## Package contents

| Path | What |
|------|------|
| `src/lib.rs` | PyO3 module definition — `CertusEngine` class |
| `src/engine.rs` | Wires the Rust component stack (creates, connects, initializes) |
| `src/keys.rs` | OffloadKey (u64) to CacheKey mapping |
| `certus_connector/spec.py` | `CertusOffloadingSpec` — vLLM OffloadingSpec implementation |
| `certus_connector/manager.py` | Mock manager (pure Python, for testing without hardware) |
| `certus_connector/native_manager.py` | Production manager (thin proxy to `certus_native.CertusEngine`) |
| `certus_connector/handler.py` | Transfer handlers (GPU ↔ Certus I/O) |
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
