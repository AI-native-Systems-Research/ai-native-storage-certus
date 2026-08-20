# Add opt-in per-block CRC-32 integrity check to the KV-offload path

## Summary

Adds an optional, feature-gated CRC-32 integrity check over every KV block that
transits the offload tier. A checksum is **computed on store** and **verified on
load**, so silent corruption of resident/at-rest KV data is caught before the
corrupt bytes can be fed back to the GPU.

The feature is behind a single Cargo feature (`integrity-check`), **off by
default**, with **zero footprint when off** — the dispatch-map index entry stays
56 bytes and no CRC code is compiled in.

## What it does

- **Store path** (`dispatcher::copy_gpu_to_memory_completed`): after the D2H copy
  completes and before the entry is downgraded, the resident slot is hashed
  (CRC-32, IEEE polynomial via `crc32fast`) and the `u32` is recorded on the
  dispatch-map entry (`DispatchEntry.checksum`). A `stream_synchronize` is issued
  before hashing so the D2H is guaranteed complete.
- **Load path** (`dispatcher::batch_lookup` warm arm + `lookup_async`): on a warm
  memory-tier hit, the slot's CRC is recomputed on the CPU **immediately before
  the H2D copy** and compared to the stored checksum. The slot is pinned by a
  read-ref during the check, so the bytes are stable.
- **Demote / promote** (`convert_memory_tier_to_block` / `promote_block_to_memory_tier`):
  transition the entry in place, so the checksum survives an SSD round-trip and is
  re-verified on the next warm load.

## Failure semantics (important for reviewers)

On a CRC-32 mismatch the dispatcher:
1. **Skips the H2D copy** — corrupt KV never reaches the GPU / the model.
2. Releases the read-ref and returns `IoError("integrity: CRC-32 mismatch for key
   {key}: expected {…}, got {…}")` for that key.

That error propagates as a gRPC `EntryResult{success:false, error_code:IO_ERROR}`,
the connector reports the transfer as failed, and vLLM 0.26's offloading worker
hits `assert transfer_result.success` (`offloading/worker.py`, `# we currently do
not support job failures`). The net effect is a **fail-stop engine abort**, with
the offending key + expected/actual CRC logged first.

This is **detect-and-refuse, not detect-and-repair**: a mismatch is only ever
detected on the load path (the store path merely records the CRC), and there is no
quarantine/recompute path today. Softening this (e.g. return a *miss* so vLLM
recomputes the block, or wait for vLLM to honor a load-failure policy here) is a
possible follow-up.

## Implementation notes

- `define_interface!` preserves attributes, so the two new `IDispatchMap` methods
  (`set_checksum` / `get_checksum`) are `#[cfg(feature = "integrity-check")]`-gated
  and vanish entirely when the feature is off.
- Cargo feature unification means enabling `interfaces/integrity-check` makes those
  trait methods appear for **every** crate that depends on `interfaces`. A crate
  cannot `cfg` on a dependency's feature, so each affected crate
  (`dispatch-map`, `dispatcher`, `remote-lookup`) carries its **own** forwarding
  `integrity-check` feature; `certus-server` enables all of them in lockstep. Test
  and bench mocks get cfg-gated stub impls so the workspace builds with the feature
  on or off.

## Testing

- `cargo build`/`cargo test` clean with the feature both on and off.
- E2E: 450-conversation / 5376-generation multiturn run on vLLM 0.26 (granite-4.1-8b)
  against a `certus-server` built with `--features integrity-check,rw-telemetry`:
  - ~129.6 GiB stored to NVMe, ~645.3 GiB promoted back (~5.29 M read ops) over 12
    rounds — the store-CRC and load-verify paths were both heavily exercised.
  - **0 CRC-32 mismatches**, 0 server-side integrity errors, 0 connector load
    failures.
  - Throughput identical to baseline (2260.8s vs 2268.7s) — no measurable overhead.

## Enabling

```bash
cargo build --release -p certus-server --features integrity-check
# (add rw-telemetry if you also want real GetIoStats NVMe counters)
```
