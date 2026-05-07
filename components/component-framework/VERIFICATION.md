# Kani Verification — component-framework

## Overview

This document records the Phase 0 Kani formal verification setup for the
`component-core` crate. Kani proves absence of panics, arithmetic overflows,
and invariant violations for **all possible inputs** — not just sampled ones.

---

## What Was Verified

**Target crate:** `component-core`
(`components/component-framework/crates/component-core/`)

**Verification target:** `Receptacle<T>` — the typed interface slot that every
component uses to connect required interfaces. It is the core of the
initialization and wiring lifecycle.

`Receptacle<T>` was chosen over `InterfaceMap` (which uses `HashMap` + `Vec`)
because Kani does not support `__rust_alloc_error_handler` from those
allocators. `Receptacle<T>` is backed by `RwLock<Option<Arc<T>>>` — safe to
instantiate and call from within a Kani harness.

---

## Files Changed

| File | Change |
|---|---|
| `crates/component-core/src/lib.rs` | Added `#[cfg(kani)] mod verification` block with 5 harnesses |
| `crates/component-core/Cargo.toml` | Added `[lints.rust] unexpected_cfgs` to suppress `#[cfg(kani)]` warnings |

---

## Harnesses

All five harnesses live in `src/lib.rs` inside:

```rust
#[cfg(kani)]
mod verification { ... }
```

This block is invisible to `cargo build` and `cargo test` — it only exists
when Kani runs.

### 1. `verify_receptacle_new_is_disconnected`

**Proves:** A freshly created `Receptacle::new()` is not connected.

```rust
let r: Receptacle<u32> = Receptacle::new();
kani::assert(!r.is_connected(), "new receptacle must be disconnected");
```

### 2. `verify_receptacle_connect_sets_connected`

**Proves:** After `connect()` with any `u32` provider, `is_connected()` is true.
Uses `kani::any::<u32>()` so Kani verifies over all possible provider values.

```rust
let r: Receptacle<u32> = Receptacle::new();
let provider = Arc::new(kani::any::<u32>());
r.connect(provider).unwrap();
kani::assert(r.is_connected(), "...");
```

### 3. `verify_receptacle_double_connect_returns_error`

**Proves:** The `AlreadyConnected` guard is always enforced — a second `connect()`
on an occupied receptacle never succeeds.

```rust
r.connect(p1).unwrap();
let result = r.connect(p2);
kani::assert(matches!(result, Err(ReceptacleError::AlreadyConnected)), "...");
```

### 4. `verify_receptacle_disconnect_when_empty_returns_error`

**Proves:** The `NotConnected` guard is always enforced — `disconnect()` on a
fresh receptacle never silently succeeds.

```rust
let r: Receptacle<u32> = Receptacle::new();
let result = r.disconnect();
kani::assert(matches!(result, Err(ReceptacleError::NotConnected)), "...");
```

### 5. `verify_receptacle_connect_disconnect_roundtrip`

**Proves:** The connect→disconnect sequence is symmetric: the receptacle returns
to the disconnected state after a full roundtrip, for all possible provider values.

```rust
r.connect(provider).unwrap();
kani::assert(r.is_connected(), "...");
r.disconnect().unwrap();
kani::assert(!r.is_connected(), "...");
```

---

## Verification Result (Phase 0 / Dry Run)

```
$ python kani_evaluator.py '{"unwind_limit": 5}'

{
  "success": true,
  "latency_s": 22.192,
  "unwind_limit": 5,
  "harness_count": 5,
  "failing_count": 0
}

Manual Harness Summary:
Complete — 5 successfully verified harnesses, 0 failures, 5 total.
SUMMARY: 0 of 519 checks failed (8 unreachable)
```

All 5 harnesses pass at `unwind_limit = 5`. CBMC explored 519 proof
obligations; 0 failed.

---

## Evaluator Bridge

`kani_evaluator.py` (in `agentic-strategy-evolution/`) is the bridge between
the Nous agentic loop and `cargo kani`. It:

1. Accepts `{"unwind_limit": N}` as JSON input.
2. Patches every `#[kani::unwind(N)]` annotation in `src/lib.rs`.
3. Runs `cargo kani --package component-core` from the workspace root.
4. Returns `{success, latency_s, unwind_limit, harness_count, failing_count}`.

---

## Next Step — Pareto Campaign

A Nous campaign (`kani_campaign.yaml`) is configured to find the
**Pareto-optimal `unwind_limit`**: the smallest value at which all 5 harnesses
pass, minimising the ~22 s verification runtime.

```bash
# In /home/cornel/agentic-strategy-evolution/
export OPENAI_BASE_URL=https://ete-litellm.ai-models.vpc-int.res.ibm.com
export OPENAI_API_KEY=<litellm-token>
python run_campaign.py kani_campaign.yaml --max-iterations 3 --auto-approve
```

Results land in `kani-pareto-component-core/`. On completion, the optimal
`unwind_limit` should replace the current value of `5` in the harnesses.

---

## How to Run Verification Manually

```bash
# From the workspace root
cd /home/cornel/ai-native-storage-certus
cargo kani --package component-core

# Run a single harness
cargo kani --package component-core \
  --harness verification::verify_receptacle_new_is_disconnected
```

---

## Kani Version

Kani Rust Verifier 0.67.0 (cargo plugin), nightly-2025-11-21.
