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
// Note the invariant is written over a LOCAL array, not over a struct field: an invariant
// containing a struct field projection fails even when correct (kani #3168). That was measured
// here too — the identical invariant over `self.c[0]` failed, and proved once hoisted.
// ---------------------------------------------------------------------------------------------
