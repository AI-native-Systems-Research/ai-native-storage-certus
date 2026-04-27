# Tasks: Extent Manager V2

**Spec**: [spec.md](spec.md) | **Plan**: [plan.md](plan.md)

## Review Backfilled Spec

- [ ] Review user stories for accuracy -- do they capture the real
      usage patterns and priorities?
- [ ] Verify requirements match intended behavior, not just current
      behavior -- are there any bugs documented as features?
- [ ] Check on-disk format tables against the code -- ensure byte
      offsets and sizes are accurate after the two-device rework
      (v3 superblock, contiguous checkpoint regions)
- [ ] Review success criteria SC-005 (100M extents at scale) -- is
      this the right target? Are there latency requirements?
- [ ] Decide whether FR-014 (background checkpoint interval) needs
      to be configurable at runtime or only at construction time
- [ ] Consider whether incremental checkpointing should be a
      tracked future requirement or out of scope
- [ ] Mark spec status as "Draft" or "Approved" after review
