# dispatcher-verif

Creusot verification crate for dispatcher specification properties.

This crate turns the extracted artifacts into a proof-oriented transition model:

- `docs/first_properties.md`
- `docs/verif_plan.md`
- `docs/creusot_specs_seed.rs`

## What this crate verifies now

- Initialization gating (`NotInitialized` behavior)
- Membership/check behavior
- Core key state transitions (`populate`, `lookup`, `remove`, `touch`)
- Pending write protocol (`prepare_store`, `commit_store`, `cancel_store`)
- Capacity-based eviction bounded by `MAX_EVICT_ATTEMPTS = 512`
- Deterministic drive selection (`key % num_drives`)

## Run

```bash
cd tools/creusot/dispatcher-verif
cargo clean
cargo creusot
```

Optional:

```bash
cargo creusot --only coma
why3find prove verif/dispatcher_verif_rlib/*.coma
```
