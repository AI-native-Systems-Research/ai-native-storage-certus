// HELD-OUT regression tests for EPO-INV-STALE-HANDLE-NEVER-CRASHES (FR-012).
//
// The repair agent never sees this file. The evaluator injects it into its OWN working copy of the
// agent's result, as a child test module of `src/lru_list.rs`, and runs `cargo test`.
//
// Why it exists: the Creusot proofs check a MODEL of the code kept in verif-creusot/, not src/
// itself. An agent could therefore patch the model, leave the real code broken, and still satisfy
// the proof oracle. These tests run against the REAL `LruList`, so only a real fix passes them.
// Measured 2026-10-02: both panic on the pre-fix revision and pass on the fix.
use super::*;

#[test]
fn heldout_stale_handle_after_clear_is_silent_noop() {
    let mut lru = LruList::new();
    let a = lru.push_back(1);
    let b = lru.push_back(2);
    lru.clear();
    lru.move_to_back(a);
    lru.move_to_back(b);
    lru.remove(a);
    lru.remove(b);
    assert_eq!(lru.len(), 0);
    assert_eq!(lru.peek_front_key(), None);
    lru.push_back(7);
    assert_eq!(lru.pop_front(), Some(7));
}

#[test]
fn heldout_fabricated_out_of_range_handle_is_silent_noop() {
    let mut lru = LruList::new();
    lru.push_back(1);
    lru.move_to_back(u32::MAX);
    lru.remove(99);
    assert_eq!(lru.len(), 1);
    assert_eq!(lru.pop_front(), Some(1));
}
