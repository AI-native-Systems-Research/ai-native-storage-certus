#![cfg_attr(kani, feature(stmt_expr_attributes, proc_macro_hygiene))]
pub const COLS: usize = 1024;

/// Same shape as the component's halve(), but operating on a LOCAL array so the loop
/// invariant needs no struct-field projection (those are a known-broken case, kani #3168).
#[cfg(kani)]
#[kani::proof]
fn verify_halve_contract() {
    let mut c = [255u8; COLS];
    let mut i = 0usize;
    #[kani::loop_invariant(i <= COLS && (i == 0 || c[0] <= 127))]
    while i < COLS { c[i] >>= 1; i += 1; }
    assert!(c[0] <= 127);
}

#[cfg(kani)]
#[kani::proof]
fn verify_halve_contract__mutant() {
    let mut c = [255u8; COLS];
    let mut i = 0usize;
    #[kani::loop_invariant(i <= COLS && (i == 0 || c[0] <= 127))]
    while i < COLS { c[i] >>= 1; i += 1; }
    assert!(c[0] > 127);        // the OPPOSITE — must FAIL
}

// ---------------------------------------------------------------------------------------------
// HOW TO RUN, and what it demonstrates (measured with cargo-kani 0.67.0, 2026-09-29):
//
//   cargo kani -Z loop-contracts --output-format terse
//
//   verify_halve_contract           SUCCESSFUL  0.196s   <- sound, UNBOUNDED
//   verify_halve_contract__mutant   FAILED      0.215s   <- correctly fails
//
// Compare the same obligation done the way our harnesses used to do it, at --unwind 4
// --no-unwinding-checks over a 4096-iteration loop:
//
//   verify_halve_bounded            SUCCESSFUL  0.034s
//   verify_halve_bounded__mutant    SUCCESSFUL  0.034s   <- ALSO passes: the proof is EMPTY
//
// Both bounded harnesses "verify" in 34ms because the bound prunes away every path the
// obligation is about. This file is the minimal reproduction of the unsoundness that made
// 52 of 90 harnesses vacuous on eviction-policy-session-lists.
//
// CORRECTION (measured later, 2026-09-29): an earlier version of this note blamed kani #3168
// (struct field projections) for a failure seen while writing this file. That was a MISDIAGNOSIS.
// #3168 does not reproduce on 0.67.0 — invariants over a struct field, a `&mut` array parameter,
// a by-value array parameter and a harness-body local all prove. The real cause was a
// NON-INDUCTIVE invariant: CBMC havocs the loop's write set, so the invariant must be
// re-establishable from itself plus the body, not merely true at the end. `a[0] <= 127` after
// `a[i] >>= 1` is inductive for u8 (halving cannot exceed 127) but NOT for u64. Check
// inductiveness before suspecting the tool, and do not hoist state out of structs to appease it.
// ---------------------------------------------------------------------------------------------
