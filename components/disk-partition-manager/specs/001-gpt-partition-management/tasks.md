# Tasks

## Review Backfilled Spec

- [ ] Review generated user stories for accuracy
- [ ] Verify requirements match intended behavior (not just current behavior)
- [ ] Remove implementation notes that don't belong in spec
- [ ] Add any missing requirements (e.g., sector alignment constraints)
- [ ] Mark spec status as "Draft" or "Approved"

## Add Unit Tests

- [ ] Add mock block device tests for format → initialize round-trip
- [ ] Test corrupt primary header triggers backup GPT fallback
- [ ] Test layout error: fixed partitions exceed device capacity
- [ ] Test layout error: multiple size_bytes=0 partitions rejected
- [ ] Test UTF-16LE name encoding/decoding (ASCII, max length, empty)
- [ ] Test protective MBR is written correctly

## Documentation

- [ ] Add component README.md with usage examples
- [ ] Document well-known Certus type GUIDs and their purpose
