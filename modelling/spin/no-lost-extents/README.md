# No Lost Extents

**Scope**: Whole system (Client → Dispatcher → ExtentManager, with BgWriter)

Verifies that every extent reservation (`reserve_extent`) is eventually resolved
by either `publish()` (data committed to SSD) or `abort()`/drop (reservation freed).
No extent slot is left permanently in the RESERVED state, which would leak SSD space
that can never be reclaimed.

## Properties Verified

| ID | Property | Type |
|----|----------|------|
| P1 | Every RESERVED extent reaches PUBLISHED or FREE before shutdown completes | Safety |
| P2 | No extent is simultaneously PUBLISHED and ABORTED (mutually exclusive terminals) | Safety |
| P3 | An extent slot is RESERVED at most once at any time (no double-reserve) | Safety |
| P4 | A crashed/errored write path always aborts (drop semantics guarantee cleanup) | Safety |
| P5 | WriteHandle exclusivity: only the holder can publish or abort a reservation | Safety |

## System Abstraction

| Real component | Promela process |
|----------------|-----------------|
| gRPC client calling prepare_store/commit_store/cancel_store | `Client(id)` |
| Background writer (process_write_job) | `BgWriter()` |
| Dispatcher shutdown (pending_writes.clear()) | `ShutdownCleanup()` |
| Extent manager bitmap (reserve/publish/abort) | `extent_state[]` array + inline operations |
| Write job channel (crossbeam_channel) | `chan write_queue[QUEUE_CAP]` |

## Assumptions / Stubs

- The SSD write (`write_buffer_to_ssd`) is modeled as nondeterministic success/failure.
- The dispatch-map is not modeled (not relevant to extent lifecycle).
- Memory-tier is abstracted: mt.peek either succeeds or fails nondeterministically.
- WriteHandle's RAII semantics (drop → abort) are modeled explicitly in all paths.

## Running

```bash
cd modelling/spin/no-lost-extents

# Safety verification (assertions + invalid end-states)
make

# Liveness/deadlock check
make liveness

# Clean generated files
make clean
```

## Tuning the Model

Parameters: `N_CLIENTS=2`, `N_EXTENTS=4`, `QUEUE_CAP=2`

- `N_EXTENTS > N_CLIENTS` ensures the "extent manager full" path is reachable but not dominant.
- `QUEUE_CAP=2` with 2 clients exercises backpressure.
- The nondeterministic choice (commit / cancel / write-fail / abandon) covers all four resolution paths per reservation.

## Correspondence to Source Code

| Model location | Source file | Line range |
|----------------|-------------|------------|
| `Client` / direct path | `components/dispatcher/src/lib.rs` | 1500–1663 |
| `reserve_extent` inline | `components/interfaces/src/iextent_manager.rs` | 95–155 (WriteHandle) |
| `publish_extent` inline | `components/interfaces/src/iextent_manager.rs` | 132–139 |
| `abort_extent` inline | `components/interfaces/src/iextent_manager.rs` | 141–155 (abort + Drop) |
| `BgWriter` / process_write_job | `components/dispatcher/src/lib.rs` | 350–431 |
| `ShutdownCleanup` | `components/dispatcher/src/lib.rs` | 808–819 (shutdown clears pending_writes) |
