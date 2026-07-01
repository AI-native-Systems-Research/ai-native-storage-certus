# Progress Snapshot (Codex) - July 1

## P21 Product-Mode Verification Status

1. Added product-aligned P21 mode split in verification model:
   - `M1`: write-handle path (`prepare -> commit/cancel consume once`).
   - `M2`: staging-only path (`prepare ok`, terminal ops miss with `KeyNotFound`).
2. Added explicit proof chains for the three P21 functions in `src/lib.rs`.
3. Current proof boundary:
   - Core mathematical content is proved in-body.
   - Remaining trusted tail VCs are due to solver tuple-projection unification limits.
4. Related docs:
   - trusted debt ledger: [trusted_ledger.md](/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/docs/trusted_ledger.md)
   - coverage status (`P21`: `Covered (trusted tail VC)`): [property_coverage.md](/home/cornel/ai-native-storage-certus/tools/creusot/dispatcher-verif/docs/property_coverage.md)

## Next De-Trusting Priority

1. Remove `#[trusted]` from the three P21 wrapper functions first.
2. Then remove trusted transport lemmas.
