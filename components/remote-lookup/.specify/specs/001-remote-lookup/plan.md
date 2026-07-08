# Implementation Plan: Remote Lookup

**Branch**: `001-remote-lookup` | **Date**: 2026-07-08 | **Spec**: [spec.md](spec.md)
**Context**: Backfilled from existing implementation. Documents current architecture.

## Summary

The `remote-lookup` component is a placeholder implementation of the `IRemoteLookup` interface within the Certus distributed storage system. It serves as the network-level cache miss handler: the dispatcher forwards entries that are not found in the DRAM memory-tier or SSD cold path to this component for resolution against remote Certus nodes. The current implementation is a stub that logs each request via its `ILogger` receptacle and returns `NotFound` for every lookup, establishing the interface contract for future RDMA or TCP-based network transport.

## Technical Context

**Language/Version**: Rust stable, edition 2021, MSRV 1.75

**Primary Dependencies**:
- `component-framework` (workspace) - facade re-export of component model macros and traits
- `component-core` (workspace) - core traits (`IUnknown`, `query_interface!`)
- `component-macros` (workspace) - procedural macros (`define_component!`, `define_interface!`)
- `interfaces` (workspace) - shared interface definitions (`IRemoteLookup`, `ILogger`, `CacheKey`, `IpcHandle`, `RemoteLookupError`)

**Storage**: N/A (placeholder; no network I/O or persistent state)

**Testing**: `cargo test -p remote-lookup`, `cargo test --doc -p remote-lookup`

**Target Platform**: Linux (RHEL 9 / Fedora)

**Project Type**: Library component in COM-inspired framework

**Performance Goals**: N/A for placeholder. Future transport implementation will need low-latency batch lookups.

**Constraints**:
- Interface must type-align with `IDispatcher::batch_lookup` signature (`&[(CacheKey, IpcHandle)]`)
- Component must not expose public functions outside the `IRemoteLookup` trait
- Must compile without SPDK (part of workspace default-members)
- Must be `Send + Sync` safe

## Architecture

### Component Layer

```
                         +-----------------------+
                         |   DispatcherComponent |
                         |     (dispatcher-v1 /  |
                         |      dispatcher-p2p)  |
                         +-----------+-----------+
                                     |
                          receptacle: IRemoteLookup
                                     |
                         +-----------v-----------+
                         | RemoteLookupComponent |
                         |  provides:            |
                         |    IRemoteLookup      |
                         |  receptacles:         |
                         |    logger: ILogger    |
                         +-----------+-----------+
                                     |
                          receptacle: ILogger
                                     |
                         +-----------v-----------+
                         |   ConsoleLogger or    |
                         |   other ILogger impl  |
                         +-----------------------+
```

### Internal Module Structure

```text
components/remote-lookup/
├── Cargo.toml                    # Package manifest (v0.1.0, workspace deps)
├── CLAUDE.md                     # Agent context pointer
├── README.md                     # Component documentation
├── src/
│   └── lib.rs                    # Component definition + IRemoteLookup impl + tests
├── info/
│   └── PROMPT.md                 # Component creation prompt
├── specs/
│   └── 001-remote-lookup-placeholder/
│       ├── spec.md               # Original placeholder spec
│       ├── plan.md               # Original implementation plan
│       ├── tasks.md              # Original task breakdown
│       ├── research.md           # Research notes
│       ├── data-model.md         # Data model documentation
│       ├── quickstart.md         # Quick start guide
│       ├── contracts/            # Interface contracts
│       └── checklists/           # Requirements checklists
└── .specify/
    └── specs/
        └── 001-remote-lookup/
            ├── spec.md           # Backfilled spec (current)
            ├── plan.md           # This file
            └── tasks.md          # Task breakdown
```

### Data Flow

```
Dispatcher.batch_lookup(entries)
    |
    v
[DRAM memory-tier lookup] --> hit? --> return Ok(())
    |
    v (miss)
[SSD cold-path read]     --> hit? --> return Ok(())
    |
    v (miss: KeyNotFound)
[remote_lookup.get()]     --> receptacle unbound? --> skip, leave KeyNotFound
    |
    v (bound)
rl.batch_lookup(not_found_entries)
    |
    v (current placeholder)
[log each entry via ILogger]
[return Err(NotFound) for each]
    |
    v
Dispatcher maps RemoteLookupError --> DispatcherError::IoError
```

**Lifecycle Integration**:
1. Dispatcher `initialize()` calls `rl.join_cluster("certus://local-cluster")`
2. Dispatcher `batch_lookup()` forwards local misses to `rl.batch_lookup()`
3. Dispatcher `shutdown()` calls `rl.leave_cluster()`

### Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Placeholder returns `NotFound` for all entries | Allows integration testing of the full dispatcher pipeline without network infrastructure |
| `ILogger` receptacle is optional (graceful skip) | Component functions correctly in minimal test setups without a bound logger |
| Same parameter types as `IDispatcher::batch_lookup` | Enables zero-copy delegation from dispatcher to remote lookup without type conversion |
| `RemoteLookupError::TransportError` defined but unused | Forward-declares the error variant needed for future network transport implementation |
| Single-file implementation (`lib.rs`) | Appropriate for a stub; future real implementation may split into modules |
| No async/actor model | Placeholder has no blocking I/O; future implementation may adopt actor pattern |
| `define_component!` macro usage | Standard Certus component model; auto-generates `IUnknown`, `new()`, receptacle accessors |

## Dependencies

### Build Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `component-framework` | workspace | Facade re-export |
| `component-core` | workspace | `IUnknown`, `query_interface!` |
| `component-macros` | workspace | `define_component!` proc macro |
| `interfaces` | workspace | `IRemoteLookup`, `ILogger`, `CacheKey`, `IpcHandle`, `RemoteLookupError` |

### Runtime Dependencies

| Component | Interface | Binding |
|-----------|-----------|---------|
| Logger (any `ILogger` impl) | `ILogger` | Receptacle (optional) |

### Downstream Consumers

| Component | Usage |
|-----------|-------|
| `dispatcher-v1` | Receptacle binding; calls `join_cluster`, `batch_lookup`, `leave_cluster` |
| `dispatcher-p2p` | Same as dispatcher-v1 |

## Testing

### Current Test Coverage

| Test | Purpose | Status |
|------|---------|--------|
| `batch_lookup_returns_not_found_for_each_entry` | Verifies N entries yield N `NotFound` results | Passing |
| `batch_lookup_returns_empty_vec_for_empty_input` | Edge case: empty slice | Passing |
| `batch_lookup_preserves_positional_order` | Result count matches input count | Passing |
| `join_cluster_succeeds` | Lifecycle: join returns `Ok(())` | Passing |
| `leave_cluster_succeeds` | Lifecycle: leave returns `Ok(())` | Passing |
| `batch_lookup_accepts_cache_key_ipc_handle_slice` | Type conformance with IDispatcher | Passing |
| Doc test on `batch_lookup` | Runnable example in documentation | Passing |

### Test Strategy

- Unit tests exercise all three interface methods
- Tests run without `ILogger` bound (verifies graceful skip)
- Doc test provides compile-time type verification
- No integration tests needed for placeholder (no external dependencies)

## Future Considerations

- **Network Transport**: Replace stub with RDMA or TCP-based implementation for cross-node cache lookups
- **Actor Model**: Real implementation may need dedicated actor thread(s) for non-blocking network I/O
- **Connection Pooling**: `join_cluster` will need to establish and manage persistent connections
- **Failure Handling**: `TransportError` variant will carry real network error semantics (timeouts, disconnects, partitions)
- **Batching Strategy**: May need to partition batch entries by target node based on consistent hashing
- **Metrics**: Add latency/throughput instrumentation for remote lookup path
- **Retry Logic**: Network failures may warrant configurable retry with backoff
- **Security**: mTLS or similar for inter-node communication
