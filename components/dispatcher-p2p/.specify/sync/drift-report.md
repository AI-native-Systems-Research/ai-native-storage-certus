# Drift Report: dispatcher-p2p (001-gpudirect-cold-path)

**Generated**: 2026-06-12  
**Spec**: `components/dispatcher-p2p/specs/001-gpudirect-cold-path/spec.md`  
**Implementation**: `components/dispatcher-p2p/src/lib.rs`  
**Functional Design**: `components/dispatcher-p2p/info/FUNCTIONAL-DESIGN.md`

## Summary

| Status | Count |
|--------|-------|
| Aligned | 10 |
| Drifted (inherited from base dispatcher) | 1 |

## Drift Items

### DRIFT-A: `evict_for_space` shard-targeted eviction (inherited fix)

**Severity**: N/A — same fix as base dispatcher (see dispatcher drift report 2026-06-12)

The `evict_for_space` function in dispatcher-p2p is identical to the base dispatcher's version. It now accepts `target_key` and calls `evict_lru_for_key(target_key)` instead of `evict_lru()`. This is inherited behavior — the P2P spec does not independently specify eviction mechanics (they come from the base dispatcher contract).

**No spec change needed** for 001-gpudirect-cold-path — eviction is out of scope for this spec.

---

## Notes

The P2P spec (001-gpudirect-cold-path) focuses exclusively on the cold-path data pipeline (NVMe → BAR1 ring → D2D → GPU). Eviction, populate, hot-path, and memory-tier management are inherited from the base dispatcher and covered by its spec (001-dispatcher-cache-interface).
