---
name: design-artifacts-update
description: Update design artifacts in certus/design/ to reflect the current codebase
---

Update the architectural design documents in `/home/dwaddington/certus/design/` so they accurately reflect the current state of the code. The design directory is a separate working directory from the main repository.

## Design Artifacts

The following files in `/home/dwaddington/certus/design/` must be kept in sync with the code:

| File | Purpose |
|------|---------|
| `SYSTEM.md` | Master architecture reference — components, interfaces, data flows, concurrency model, design decisions |
| `certus-server-deployment.md` | Component topology diagram — receptacles, wiring, initialization order |
| `design-spec-put-flow.md` | Populate/write data path (GPU → DRAM → SSD) |
| `design-spec-hit-flow.md` | Lookup/read data paths (warm: DRAM → GPU, cold: SSD → DRAM → GPU) |

## Workflow

### 1. Gather Current Code State

Read the following source files to establish ground truth:

**Interface definitions** (canonical method signatures):
- `components/interfaces/src/idispatcher.rs` — `IDispatcher` trait
- `components/interfaces/src/idispatch_map.rs` — `IDispatchMap` trait
- `components/interfaces/src/imemory_tier.rs` — `IMemoryTier` trait
- `components/interfaces/src/iblock_device.rs` — `IBlockDevice` trait
- `components/interfaces/src/iextent_manager.rs` — `IExtentManager` trait
- `components/interfaces/src/igpu_services.rs` — `IGpuServices` trait
- `components/interfaces/src/ispdk_env.rs` — `ISPDKEnv` trait
- `components/interfaces/src/lib.rs` — Re-exports and shared types

**Component implementations** (fields, receptacles, internal structure):
- `components/dispatcher/src/lib.rs` — `define_component!` block and core logic
- `components/dispatcher-p2p/src/lib.rs` — P2P variant if applicable
- `components/dispatch-map/src/lib.rs`
- `components/memory-tier/src/lib.rs`
- `components/block-device-spdk-nvme/src/lib.rs`
- `components/extent-manager/src/lib.rs`
- `components/gpu-services/src/lib.rs`
- `components/spdk-env/src/lib.rs`
- `components/eviction-policy-lru/src/lib.rs`
- `components/remote-lookup/src/lib.rs`

**Server / integration layer**:
- `apps/certus-server/src/service.rs` — gRPC service (RPCs, IPC handle management)
- `apps/certus-server-yaml/src/main.rs` — YAML-configured server entry point

**Workspace structure**:
- Root `Cargo.toml` — workspace members and default-members

### 2. Read Each Design Artifact

Read all four `.md` files in `/home/dwaddington/certus/design/` in full.

### 3. Identify Drift

Compare the design documents against the code and note discrepancies:

- **Interface changes**: Methods added, removed, or renamed; signature changes (parameters, return types); new error variants
- **Component changes**: New/removed fields or receptacles in `define_component!` blocks; new components added to the workspace
- **Data flow changes**: Steps in the put/get paths that no longer match the implementation (e.g., new intermediate steps, removed steps, changed ordering)
- **Topology changes**: New components, changed wiring, new background threads/workers
- **Build/feature changes**: New cargo features, profiles, or conditional compilation gates

### 4. Update Each Document

Edit each `.md` file **in place**, preserving:
- The existing document structure, headings, and style
- ASCII diagrams (update them if topology changed)
- The level of detail and explanatory prose

Correct:
- Method lists and signatures
- Component field/receptacle enumerations
- Step-by-step flow descriptions
- Workspace layout listings
- Any factual claims about behavior that no longer match the code

**Do not** add speculative content or document unfinished/experimental features. Only document what the code currently implements.

### 5. Report

After updating, print a summary of changes made to each file:

```
Design Artifacts Updated:
─────────────────────────
SYSTEM.md:
  - Updated IDispatcher method list (added reserve_memory, populate_memory, memory_populated, release_memory; removed populate_async, populate_finalize)
  - Added new component: dispatcher-p2p
  - ...

certus-server-deployment.md:
  - Updated dispatcher receptacles
  - ...

design-spec-put-flow.md:
  - (no changes needed)

design-spec-hit-flow.md:
  - Updated cold path step 4 to reflect new pipeline ring behavior
  - ...
```

## Important Notes

- The design directory is at `/home/dwaddington/certus/design/`, NOT inside the main repo. It is a separate working directory.
- Preserve the existing writing style — these are human-authored design documents, not generated API docs.
- When in doubt about intent (e.g., a method exists but is clearly a placeholder/TODO), keep the existing design text and note the discrepancy in the report rather than documenting placeholder code.
- Do not modify the `.svg`, `.puml`, or `.pptx` files — only update the `.md` files.
- If a design document section describes a future/planned feature that isn't implemented yet, leave it as-is (it represents design intent).
