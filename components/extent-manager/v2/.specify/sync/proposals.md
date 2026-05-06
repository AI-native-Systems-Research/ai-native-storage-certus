# Drift Resolution Proposals

Generated: 2026-05-05
Based on: drift-report from 2026-05-05

## Summary

| Resolution Type | Count |
|-----------------|-------|
| Backfill (Code -> Spec) | 1 |
| Align (Spec -> Code) | 1 |
| Human Decision | 1 |
| New Specs | 0 |
| Remove from Spec | 0 |

## Proposals

### Proposal 1: 001-extent-manager-v2/FR-016

**Direction**: HUMAN_DECISION

**Current State**:
- Spec says: "Default checkpoint interval is 5000 ms"
- Code does: "Duration::from_secs(300) — 5 minutes"

**Options**:
- A) **BACKFILL**: Update spec to 300000 ms (5 minutes). A 5-second default is too aggressive for production — each checkpoint performs disk I/O writing slab descriptors and key vectors.
- B) **ALIGN**: Update code to 5 seconds. Acceptable if the checkpoint is designed to be lightweight.
- C) **HYBRID**: Choose a middle ground (e.g., 30 seconds).

**Recommendation**: Update spec to 5 minutes. The checkpoint writes a potentially large serialized blob; frequent checkpoints would saturate the metadata device and compete with data I/O.

**Confidence**: HIGH

---

### Proposal 2: Superblock Magic Value

**Direction**: ALIGN (Spec -> Code)

**Current State**:
- Spec says: Magic 0x4345_5254_5553_5635 ("CERTUSV5")
- Code does: Magic 0x4345_5254_5553_5634 ("CERTUSV4")

**Proposed Resolution**: Update code constant to 0x4345_5254_5553_5635 to match spec. The spec represents the index-free key-vector design (V5), while the code still uses the V4 constant from the prior design. No deployed V5 disks exist, so this is safe to change.

**Rationale**: The spec was intentionally bumped to V5 when the key-vector design was adopted. The code constant was missed during that update.

**Confidence**: HIGH

---

### Proposal 3: 001-extent-manager-v2/FR-024

**Direction**: ALIGN (Spec -> Code)

**Current State**:
- Spec says: "Component MUST be Send + Sync"
- Code does: "Auto-derived, no explicit assertion"

**Proposed Resolution**: Add a compile-time assertion:
```rust
const _: () = {
    fn assert_send_sync<T: Send + Sync>() {}
    fn check() { assert_send_sync::<ExtentManagerV2>(); }
};
```

**Rationale**: Guards against future field additions that might accidentally break Send/Sync. Trivial change, zero runtime cost.

**Confidence**: HIGH
