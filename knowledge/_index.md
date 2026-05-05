---
title: "Certus Component Index"
updated: 2026-05-05
---

# Certus Component Index

AI-native storage for LLM inference KV cache offloading. Raw NVMe via SPDK, no filesystem.

**[Status & Guard Rails](_status.md)** | **[Dependency Graph](graph.html)** | **[README](README.md)**

---

## Components by Dependency Depth

Level N depends only on levels < N. Build bottom-up; don't ship level N until levels below are green.

### Level 0 — No Dependencies

| Component | Path | Domain | Status | Spec |
|-----------|------|--------|--------|------|
| [component-framework](../components/component-framework/README.md) | `components/component-framework/` | Foundation | done | [spec](../components/component-framework/specs/) |
| [spdk-sys](../components/spdk-sys/README.md) | `components/spdk-sys/` | SPDK | done | -- |

### Level 1 — Depends on Level 0

| Component | Path | Domain | Status | Deps |
|-----------|------|--------|--------|------|
| [interfaces](../components/interfaces/README.md) | `components/interfaces/` | Foundation | done | component-framework |
| [spdk-env](../components/spdk-env/README.md) | `components/spdk-env/` | SPDK | done | spdk-sys |

### Level 2 — Depends on Level 1

| Component | Path | Domain | Status | Deps |
|-----------|------|--------|--------|------|
| [block-device-spdk-nvme v2](../components/block-device-spdk-nvme/v2/README.md) | `components/block-device-spdk-nvme/v2/` | SPDK | done | spdk-env |
| [gpu-services v0](../components/gpu-services/v0/README.md) | `components/gpu-services/v0/` | GPU | done | (none) |

### Level 3 — Depends on Level 2

| Component | Path | Domain | Status | Deps |
|-----------|------|--------|--------|------|
| [extent-manager v2](../components/extent-manager/v2/README.md) | `components/extent-manager/v2/` | Storage | done | block-device-spdk-nvme |

### Level 4 — Depends on Level 3

| Component | Path | Domain | Status | Deps | Spec |
|-----------|------|--------|--------|------|------|
| [dispatch-map v0](../components/dispatch-map/v0/README.md) | `components/dispatch-map/v0/` | Cache | done | extent-manager | [spec](../components/dispatch-map/v0/specs/001-dispatch-map/spec.md) |

### Level 5 — Depends on Levels 1, 2, 4

| Component | Path | Domain | Status | Deps | Spec |
|-----------|------|--------|--------|------|------|
| [dispatcher v0](../components/dispatcher/v0/README.md) | `components/dispatcher/v0/` | Cache | needs-work (2 bugs) | dispatch-map, gpu-services, spdk-env | [spec](../components/dispatcher/v0/specs/001-dispatcher-cache-interface/spec.md) |

### Level 6 — Application (assembler)

| Component | Path | Domain | Status | Deps |
|-----------|------|--------|--------|------|
| [certus-connector](../certus-connector/README.md) | `certus-connector/` | Connector | in-progress | dispatcher, dispatch-map, gpu-services, spdk-env |

---

## Dependency DAG

```
Level 6: certus-connector (assembler — creates & wires all below)
           |
Level 5: dispatcher
           ├── dispatch-map        (receptacle: IDispatchMap)
           ├── gpu-services        (receptacle: IGpuServices)
           └── spdk-env            (receptacle: ISPDKEnv)
           |
Level 4: dispatch-map
           └── extent-manager      (receptacle: IExtentManager)
           |
Level 3: extent-manager
           └── block-device-spdk-nvme  (receptacle: IBlockDevice)
           |
Level 2: block-device-spdk-nvme       gpu-services
           └── spdk-env                (no receptacles)
           |
Level 1: spdk-env                      interfaces
           └── spdk-sys                └── component-framework
           |
Level 0: spdk-sys                      component-framework
```

Note: dispatcher also creates block-device and extent-manager instances internally
during `initialize()`, but those are runtime children, not receptacle-wired deps.

---

## Legend

- **done** — all interface methods implemented and tested
- **in-progress** — actively being developed, partial implementation
- **needs-work** — implemented but has known bugs that must be fixed before integration
