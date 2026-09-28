# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior (not just current behavior)
- [ ] Confirm the admission gate description (FR-011: protect at MRU iff
      `new_est > victim_est`, no absolute threshold) matches intended tuning
- [ ] Confirm the aging period (FR-013: every `max_len * 10` accesses) is intended
- [ ] Decide whether `clear_pool` preserving the sketch (FR-009) is desired
- [ ] Add direct admission-gate tests (hot key protected vs. cold-key flood)
- [ ] Mark spec status as "Draft" or "Approved"
