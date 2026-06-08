---
name: profile-optimization-targets
description: Analyze the system component map and source code to discover ALL possible optimization targets — do not ask the user, discover them from the architecture.
argument-hint: "<profile-yaml> [output-dir]"
---

The purpose of this skill is to read the system's component map (YAML profile), trace through the source code, and discover all performance-relevant optimization targets that are supported by evidence from the component graph, source code, metrics, or diagnostic KB. Do NOT ask the user what to optimize — your job is to FIND and REPORT what CAN be optimized.

**Constraints:**
- This skill must NOT modify source code. It only writes analysis/configuration outputs.
- Output valid JSON only (no comments, trailing commas, markdown fences, or prose in JSON files).
- Do not report a target if it cannot be tied to code evidence, measured data, or a KB rule.

## Inputs

- First argument: path to the profile YAML (e.g. `apps/certus-server-yaml/profiles/full.yaml`)
- Second argument (optional): output directory (default: `./optimization_profile`)

If no profile path given, scan `apps/*/profiles/*.yaml` and list available profiles — then use the most complete one (largest component count).

## Prerequisites

**Read `knowledge/inference-storage-diagnostics/diagnostic-kb.md` FIRST.**

This KB is your reasoning engine. It contains symptom→diagnosis→experiment→action rules for every type of bottleneck. You MUST use it throughout this skill:
- Step 5: confirm targets using KB diagnostic rules (don't report without grounding)
- Step 6: use KB experiment suggestions to determine ceilings
- Step 7: use KB models to calculate expected improvement (show the math)
- Every target must reference which KB section grounded the diagnosis

## Process

### Step 1: Detect Platform Context

Read system information FIRST — you need this for gap analysis later:
- `lspci -vvv` for NVMe devices (link speed/width) and GPU
- `nvidia-smi -q` for GPU details
- Kernel modules (`lsmod | grep nvidia`)
- Hugepage configuration (`cat /proc/meminfo | grep Huge`)
- NUMA topology (`numactl --hardware` or `lscpu`)
- IOMMU/VFIO setup

### Step 2: Read the Component Map

Read the specified profile YAML. If multiple profiles exist, note them in the output — different profiles may have different optimization opportunities (e.g. a minimal profile won't have GPU components). This YAML defines:
- All components and their provided interfaces
- Wiring (which component plugs into which receptacle)
- Init order (dependency chain)
- Exports (what the system exposes externally)

Identify ALL critical paths through the system (read path, write path, eviction path, batch path, etc.) by tracing data/control flow through the wiring. There is no single "goal" — discover every performance-relevant flow.

### Step 3: Read Interface Definitions

For each component, find and read its interface/contract (the `provides` field in the YAML points to the interface name). Understand:
- What methods are available on each interface
- What types flow across component boundaries
- What feature gates or conditional compilation exists

### Step 4: Read Implementation Files

For each component, find its implementation source (the `crate` field in the YAML points to the source directory). Read:
- The main implementation files
- Focus on functions that handle data flow (not setup/teardown/logging)
- Identify hot-path functions, allocation decisions, and architectural choices

If crate path is missing or ambiguous, search `Cargo.toml` / workspace members and component names. If an interface name is not found directly, grep for trait/interface definitions.

### Step 5: Discover ALL Optimization Targets

Actively search for optimization targets using these discovery methods:

**Method 1: Gap Analysis** — Compare theoretical hardware ceiling (from Step 1) vs current code path.
- Calculate: what could the hardware do if software were perfect?
- Measure/estimate: what does the current code achieve?
- A large gap (e.g. 7x) usually indicates architectural waste or a missing fast path, not just parameter tuning.

**Method 2: Unused Capabilities** — grep for loaded modules, available functions, feature gates that are compiled but never called on the hot path.
- Loaded kernel modules that no code invokes (e.g. nvidia-peermem loaded but no P2P DMA)
- `#[cfg(feature = "X")]` code that's compiled but never reached from the main path
- Interface methods that exist but aren't called by the orchestrator

**Method 3: Serialization Points** — Find where parallelism is artificially limited.
- Single Mutex/RwLock protecting shared state under concurrent access
- Sequential loops that could be parallel (per-drive, per-client, per-stream)
- Blocking waits (condvar, channel recv) where async alternatives exist
- Thread::scope barriers that force join before proceeding

**Method 4: Hardcoded Constants** — Find arbitrary values that should be knobs.
- `const RING_SIZE: usize = 8` — why 8? Should it scale with drive count?
- `queue_depth = 16` — hardware supports 128+, why limit?
- `timeout = 50ms` — is this tuned or arbitrary?

**Method 5: Allocation Waste** — Find per-operation allocations that could be pooled/reused.
- DmaBuffer created per transfer instead of from a pre-allocated ring
- Vec/HashMap growing under load instead of pre-sized
- Lock acquisition just to read a rarely-changing value

**Method 6: Data Copy Elimination** — Find unnecessary data movement.
- Host bounce buffer between two DMA-capable devices (SSD→DRAM→GPU when SSD→GPU is possible)
- memcpy between buffers that could be aliased/shared
- Serialization/deserialization at component boundaries where zero-copy is possible

For each discovered target, use the KB to confirm it's real:
- Find the matching **symptom** in the KB
- Apply the KB's diagnostic rules
- Estimate impact using the KB's models (show the math)

Record for each target:
- **Component**: which crate
- **File**: relative path
- **Function/region**: what to mutate
- **Description**: what the waste/gap is and why it exists
- **Discovery method**: which of the above methods found it
- **KB reference**: which KB section confirms this diagnosis
- **Expected impact**: estimated improvement range (calculated, not a single unsupported number)
- **Bottleneck trigger**: what workload/platform condition makes this the limiting factor
- **Confidence**: high | medium | low
- **Evidence**: [code path, metric, KB rule, hardware ceiling, grep result]
- **Validation experiment**: how to confirm this target is real before optimizing

### Step 6: Determine Hardware Ceilings

Ceilings are the theoretical max the hardware CAN do — they set the upper bound for scoring.

**Priority order for determining ceilings:**

1. **Existing microbenchmark results** — check for prior runs:
   - Look for `PROFILE.md` or results from `/profile-performance-certus-api-bench`
   - Look for `bandwidth_test`, `fio` results, `cuda_bandwidth_test` output in the repo
   - If found, use measured values (these are the most accurate)

2. **Run quick probes** (if no existing data and explicitly allowed):
   - NVMe: read-only fio on the target device (never run write workloads on raw devices without user confirmation). If no safe path is available, skip and derive from specs.
   - GPU H2D: check for `cuda_bandwidth_test` or `bandwidthTest` binary; or estimate from PCIe link width
   - DRAM: prefer existing STREAM/mbw results; otherwise estimate from memory channels/specs (do NOT use `dd` — it measures kernel copy overhead, not DRAM bandwidth)

3. **Derive from specs** (fallback):
   - Read PCIe link width/speed from `lspci -vvv` (e.g. "LnkSta: Speed 16GT/s, Width x16")
   - NVMe: check `nvme id-ctrl` or `smartctl` for max bandwidth, or estimate from PCIe link
   - GPU: `nvidia-smi -q` for PCIe link info
   - DRAM: `dmidecode -t memory` for speed/channels

**Always state the source** of each ceiling value (measured/spec/estimated) in the output.

### Step 7: Build Taxonomy and Define Scoring

Now that you have all targets, group them into taxonomy dimensions using this algorithm:

**Generation algorithm:**

1. Enumerate all flows from the component graph: read, write/populate, eviction, promotion, prefetch, batch, RPC request/response, background workers.

2. For each flow, identify resources touched: CPU, locks, metadata, DRAM, SSD, PCIe, GPU, network/RPC, cache/tier.

3. For each resource, identify scaling stressors — what workload/platform changes could cause nonlinear performance drops:
   - request rate, object size, working set size
   - number of clients, drives, GPUs
   - cache hit rate, topology, reuse pattern

4. Merge flow/resource/stressor triples into dimensions. Each dimension answers: "Under what condition does this system stop scaling, and which part of the architecture is responsible?"

5. Reject weak dimensions:
   - If two dimensions have the same bottleneck trigger → merge
   - If a dimension has no measurable metric → remove
   - If a dimension has no code target → mark observation-only
   - If a dimension applies to only one function and is not a scaling axis → demote to target

**A dimension is valid only if it has:**
1. A distinct bottleneck trigger
2. Participating components
3. Tunable knobs
4. Measurable metrics
5. One or more optimization targets
6. An evaluator workload that stresses it

**For each dimension, define:**
- **Scoring formula**: how to score improvements (0-1 range, using ceilings from Step 6)
- **Metrics**: what to measure for this dimension
- **Baseline**: estimate from unmodified code (or measure if evaluator exists)
- **Hard constraints**: what must never break
- **Evaluator workload**: workload parameters that trigger this dimension's bottleneck (e.g. storage bandwidth → large cold reads, low hit rate, many drives)

**Platform/workload adaptability:**
- If profile has no GPU component → do not create GPU-related dimensions
- If platform has one SSD → create storage_io but not multi_drive_scheduling
- If workload is write-heavy → prioritize write-through pipeline dimension
- If workload is warm-hit-heavy → prioritize DMA path and metadata synchronization
- If workload is cold-miss-heavy → prioritize NVMe queueing and tier transition
- Mark dimensions that depend on unverified capabilities (P2P, GDS) with `feasibility_status: requires_probe`

**Quality checks before finalizing:**
- No two dimensions overlap significantly (same trigger = merge)
- Each dimension has a DISTINCT bottleneck trigger
- Read, write, eviction, promotion, RPC, and background paths are covered
- Topology and placement is covered (NUMA, drive-to-GPU affinity)
- A dimension is NOT the same as a target (dimension = scaling axis; target = code change within that axis)
- If a dimension has only one target, consider merging it into a broader axis

### Step 8: Define Evolve Regions

For each optimization target, identify precise function boundaries suitable for EVOLVE-BLOCK markers:
- `start`: a unique string matching the function/region start
- `end`: a unique string matching the next function or region end, or null for end-of-file

Rules for good evolve regions:
- Small and focused (50-300 lines, not entire files)
- Self-contained: the region can be modified without changing code outside it
- Type-safe boundaries: the function signature is the contract, internals can change
- Include allocation/strategy decisions (where buffer types are chosen)
- Include the hot loop (where data actually moves)

## Output

Write the following files to the output directory:

### `<output-dir>/component_map.md`

A COMPLETE architecture summary covering:
- System description (what it does, for what workload)
- ALL components and their roles (not just the hot path — include gRPC server, extent manager, background workers, etc.)
- ALL data flows (read path, write path, eviction, populate, batch ops)
- Component interaction diagram (show interface boundaries)
- Hardware platform (detected from lspci/lscpu)
- Available but unused capabilities
- IPC/communication mechanisms (gRPC, channels, shared memory, IPC handles)
- Actor/threading model

Do NOT just describe the "critical path" — describe the FULL system.

### `<output-dir>/optimization_targets.json`

Output a JSON with these keys (discover ALL values from the code — do not copy from this template):

- `taxonomy`: list of dimensions, each with:
  - `dimension`: short name (a scaling axis, NOT a single fix — e.g. "data_movement_path" not "p2p_bypass")
  - `description`: what this dimension controls
  - `bottleneck_when`: list of conditions that trigger this bottleneck
  - `components`: which components participate
  - `flows`: which data flows are involved (read, write, evict, promote, etc.)
  - `knobs`: list of {name, current, range}
  - `metrics`: what to measure for this dimension
  - `targets`: list of target IDs (T01, T02, ...) within this dimension
  - `evaluator_workload`: {description, parameters} — workload that stresses this dimension
  - `applicability`: {requires_components, requires_hardware, workload_triggers, disabled_when} — when this dimension is relevant

  **Naming rule**: A dimension is NOT a component and NOT a target. It is a scaling axis. If a candidate dimension has only one narrow fix, merge it into a broader dimension or demote it to a target. Good: "data_movement_path", "metadata_synchronization", "tier_transition_pipeline". Bad: "p2p_bypass", "increase_queue_depth", "fix_mutex".

- `targets`: numbered list (T01, T02, ...) with {id, component, file, function, description, metric_affected, expected_impact, dimension, bottleneck_trigger, discovery_method, kb_reference, confidence, evidence, validation_experiment}

- `evolve_regions`: dict of filename → list of {name, start, end}
- `constraints`: hard constraints that must never break
- `unused_capabilities`: hardware/software features available but not currently used — for each, include `feasibility_status`: "confirmed" (tested), "likely" (drivers loaded, topology ok), or "requires_probe" (needs validation benchmark before committing)

### `<output-dir>/targets/` (per-dimension target configs)

Create a SEPARATE target.yaml for each taxonomy dimension:

```
<output-dir>/targets/
├── <dimension_1>/
│   └── target.yaml
├── <dimension_2>/
│   └── target.yaml
└── ...
```

Each per-dimension target.yaml should have:
- Only the files relevant to that dimension
- Evolve regions scoped to that dimension's targets
- A scoring formula weighted toward the metric that dimension affects
- An evaluator workload config that triggers THAT dimension's bottleneck
  (e.g. concurrency dimension → many clients; NVMe bandwidth → large sequential writes)

## Evidence Requirements

Every optimization target must include:
- Direct code evidence: file/function/region
- Bottleneck hypothesis: what resource or path is limiting
- KB grounding: section/rule used
- Expected metric affected
- Expected impact range (not a single unsupported number)
- Validation experiment: how to confirm before optimizing
- Confidence level: high | medium | low

Do not report a target if it cannot be tied to either code evidence, measured data, or a KB rule.

Also report dimensions/components inspected where NO credible optimization target was found, with reason. This prevents pretending every component has a target.

## Notes

- Focus on PERFORMANCE-RELEVANT code paths, not logging, error handling, or test infrastructure
- Include both "obvious" targets (hot loop) and "architectural" targets (buffer allocation strategy, concurrency model)
- If hardware capabilities are available but unused, flag them as opportunities
- The component_map.md should be COMPLETE — cover the ENTIRE system
- Dimensions should be INDEPENDENT — a framework can optimize one without breaking another
- Ground ALL estimates in the KB's models — don't guess impact, calculate it
