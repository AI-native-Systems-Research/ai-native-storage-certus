//! The unit-suffixed scalars the schema accepts, parsed in one place.
//!
//! `contracts/workload-schema.md` § Units: durations take `ns|us|ms|s|m|h`, sizes
//! take `B|KiB|MiB|GiB` (binary) and `KB|MB|GB` (decimal), rates take a `/s`
//! suffix, and any bare integer may use `_` separators.
//!
//! Every parse **fails rather than defaulting**. A mistyped suffix is the same
//! class of mistake as a mistyped field name, which the schema already refuses
//! (spec FR-005): a `duration: 180x` silently read as 180 seconds would produce a
//! run of the wrong length that reports itself as correct.
//!
//! ```
//! use workload_model::units::{parse_duration_ns, parse_bytes, parse_rate_per_s};
//! assert_eq!(parse_duration_ns("180s").unwrap(), 180_000_000_000);
//! assert_eq!(parse_bytes("128KiB").unwrap(), 131_072);
//! assert_eq!(parse_rate_per_s("4000/s").unwrap(), 4000.0);
//! assert!(parse_duration_ns("180x").is_err());
//! ```

/// A scalar that could not be read as the unit it was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitError {
    /// What was written.
    pub input: String,
    /// What would have been accepted.
    pub expected: &'static str,
}

impl std::fmt::Display for UnitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "cannot read `{}`; expected {}. Refusing rather than defaulting, since a \
             misread unit yields a run of the wrong size that reports itself as correct",
            self.input, self.expected
        )
    }
}

impl std::error::Error for UnitError {}

/// Number and suffix, with `_` separators stripped from the number.
fn parts(s: &str) -> (Option<f64>, String) {
    let t: String = s.trim().chars().filter(|c| *c != '_').collect();
    let end = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(t.len());
    let (n, suf) = t.split_at(end);
    (n.parse::<f64>().ok(), suf.trim().to_string())
}

const DURATION_EXPECTED: &str = "a duration such as `500ms`, `20s`, `5m` or `6h` (ns|us|ms|s|m|h; \
                                 a bare number is seconds)";

/// Parse a duration to nanoseconds.
///
/// A bare number is **seconds**, which is what the schema's own examples mean by
/// `half_life: 0`. Every other reading of a bare number would make `0` ambiguous.
pub fn parse_duration_ns(s: &str) -> Result<u64, UnitError> {
    let (n, suf) = parts(s);
    let err = || UnitError {
        input: s.trim().to_string(),
        expected: DURATION_EXPECTED,
    };
    let n = n.ok_or_else(err)?;
    if n < 0.0 {
        return Err(err());
    }
    let scale = match suf.as_str() {
        "ns" => 1.0,
        "us" | "µs" => 1e3,
        "ms" => 1e6,
        "s" | "" => 1e9,
        "m" => 60e9,
        "h" => 3600e9,
        _ => return Err(err()),
    };
    Ok((n * scale) as u64)
}

/// Parse a rate in events per second; the `/s` suffix is optional.
pub fn parse_rate_per_s(s: &str) -> Result<f64, UnitError> {
    let err = || UnitError {
        input: s.trim().to_string(),
        expected: "a rate such as `4000/s`",
    };
    let t = s.trim().trim_end_matches("/s").trim_end_matches("/sec");
    let (n, suf) = parts(t);
    let n = n.ok_or_else(err)?;
    if !suf.is_empty() || n < 0.0 {
        return Err(err());
    }
    Ok(n)
}

/// Parse a byte size. Binary suffixes are powers of 1024, decimal ones of 1000.
pub fn parse_bytes(s: &str) -> Result<u64, UnitError> {
    let (n, suf) = parts(s);
    let err = || UnitError {
        input: s.trim().to_string(),
        expected: "a size such as `128KiB` (binary: B|KiB|MiB|GiB) or `128KB` (decimal: KB|MB|GB)",
    };
    let n = n.ok_or_else(err)?;
    if n < 0.0 {
        return Err(err());
    }
    // Binary and decimal differ only in case-insensitive spelling of the `i`, so
    // the match is over the lowercased suffix and `kib` stays distinct from `kb`.
    let scale = match suf.to_ascii_lowercase().as_str() {
        "" | "b" => 1.0,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        _ => return Err(err()),
    };
    Ok((n * scale) as u64)
}

/// Parse a bare count, allowing `_` separators.
pub fn parse_count(s: &str) -> Result<u64, UnitError> {
    let (n, suf) = parts(s);
    let err = || UnitError {
        input: s.trim().to_string(),
        expected: "a count, optionally with `_` separators, such as `240_000`",
    };
    let n = n.ok_or_else(err)?;
    if !suf.is_empty() || n < 0.0 {
        return Err(err());
    }
    Ok(n as u64)
}

/// Read a YAML value as a count, accepting either an integer or a string.
///
/// `run.wss_window` is canonically a request **count** and a duration is sugar
/// (schema rule 15), so a caller has to be able to tell which was written.
pub fn count_from_yaml(v: &serde_yaml::Value) -> Option<u64> {
    match v {
        serde_yaml::Value::Number(n) => n.as_u64(),
        serde_yaml::Value::String(s) => parse_count(s).ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_cover_every_documented_suffix() {
        for (s, ns) in [
            ("100ns", 100u64),
            ("2us", 2_000),
            ("500ms", 500_000_000),
            ("20s", 20_000_000_000),
            ("5m", 300_000_000_000),
            ("6h", 21_600_000_000_000),
        ] {
            assert_eq!(parse_duration_ns(s).unwrap(), ns, "{s}");
        }
    }

    #[test]
    fn a_bare_duration_is_seconds_so_that_zero_is_unambiguous() {
        // `churn.half_life: 0` means "never", and it has to parse.
        assert_eq!(parse_duration_ns("0").unwrap(), 0);
        assert_eq!(parse_duration_ns("3").unwrap(), 3_000_000_000);
    }

    #[test]
    fn a_mistyped_suffix_is_refused_rather_than_defaulted() {
        // The FR-005 argument applied to units: silently reading `180x` as 180
        // seconds gives a run of the wrong length that reports itself correct.
        let e = parse_duration_ns("180x").unwrap_err();
        assert_eq!(e.input, "180x");
        assert!(format!("{e}").contains("Refusing rather than defaulting"));
        assert!(parse_bytes("128KiBB").is_err());
        assert!(parse_rate_per_s("4000/min").is_err());
        assert!(parse_count("4000/s").is_err());
    }

    #[test]
    fn binary_and_decimal_sizes_stay_distinct() {
        assert_eq!(parse_bytes("128KiB").unwrap(), 131_072);
        assert_eq!(parse_bytes("128KB").unwrap(), 128_000);
        assert_ne!(parse_bytes("1GiB").unwrap(), parse_bytes("1GB").unwrap());
        assert_eq!(parse_bytes("4096").unwrap(), 4096);
        assert_eq!(parse_bytes("4096B").unwrap(), 4096);
    }

    #[test]
    fn separators_are_ignored_wherever_they_appear() {
        assert_eq!(parse_count("240_000").unwrap(), 240_000);
        assert_eq!(parse_rate_per_s("4_000/s").unwrap(), 4000.0);
        assert_eq!(parse_duration_ns("1_000ms").unwrap(), 1_000_000_000);
    }

    #[test]
    fn a_rate_suffix_is_optional_but_a_wrong_one_is_not_accepted() {
        assert_eq!(parse_rate_per_s("4000").unwrap(), 4000.0);
        assert_eq!(parse_rate_per_s("4000/s").unwrap(), 4000.0);
        assert!(parse_rate_per_s("4000/h").is_err());
    }

    #[test]
    fn negative_quantities_are_refused() {
        assert!(parse_duration_ns("-1s").is_err());
        assert!(parse_bytes("-1KiB").is_err());
        assert!(parse_count("-1").is_err());
    }

    #[test]
    fn a_window_reads_from_either_yaml_form() {
        assert_eq!(
            count_from_yaml(&serde_yaml::Value::Number(240_000.into())),
            Some(240_000)
        );
        assert_eq!(
            count_from_yaml(&serde_yaml::Value::String("240_000".into())),
            Some(240_000)
        );
        // A duration is not a count; the caller must notice and convert.
        assert_eq!(
            count_from_yaml(&serde_yaml::Value::String("60s".into())),
            None
        );
    }
}
