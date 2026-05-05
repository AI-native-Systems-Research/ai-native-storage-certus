use creusot_std::prelude::*;

/// Verified addition: proves no overflow and correct result.
#[requires(a@ + b@ <= i64::MAX@)]
#[requires(a@ + b@ >= i64::MIN@)]
#[ensures(result@ == a@ + b@)]
pub fn safe_add(a: i64, b: i64) -> i64 {
    a + b
}

/// Verified absolute value: proves no overflow and correct result.
#[requires(a@ > i64::MIN@)]
#[ensures(result@ >= 0)]
#[ensures(result@ == if a@ >= 0 { a@ } else { -a@ })]
pub fn abs(a: i64) -> i64 {
    if a >= 0 {
        a
    } else {
        -a
    }
}

/// Verified max function: result is the larger of the two inputs.
#[ensures(result@ >= a@ && result@ >= b@)]
#[ensures(result@ == a@ || result@ == b@)]
pub fn max(a: i64, b: i64) -> i64 {
    if a >= b {
        a
    } else {
        b
    }
}

/// Verified clamping: result is always within [lo, hi].
#[requires(lo@ <= hi@)]
#[ensures(result@ >= lo@ && result@ <= hi@)]
#[ensures(if val@ < lo@ { result@ == lo@ }
          else if val@ > hi@ { result@ == hi@ }
          else { result@ == val@ })]
pub fn clamp(val: i64, lo: i64, hi: i64) -> i64 {
    if val < lo {
        lo
    } else if val > hi {
        hi
    } else {
        val
    }
}
