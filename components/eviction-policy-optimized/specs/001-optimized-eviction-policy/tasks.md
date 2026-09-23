# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior (not just current behavior)
- [ ] Confirm the admission gate description (FR-011: `new_est >= 3 && new_est > victim_est`) matches intended tuning
- [ ] Confirm the aging period (FR-013: every `max_len * 2` accesses) is intended
- [ ] Decide whether `clear_pool` preserving the sketch (FR-009) is desired
- [ ] Add direct admission-gate tests (hot key protected vs. cold-key flood)
- [ ] Mark spec status as "Draft" or "Approved"
