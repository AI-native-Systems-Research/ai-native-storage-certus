//! Unit normalisation: a pass over the merged YAML before it becomes a document.
//!
//! `contracts/workload-schema.md` § Units says durations take `ns|us|ms|s|m|h`,
//! sizes take `B|KiB|MiB|GiB|KB|MB|GB`, and any bare integer may use `_`
//! separators. Those forms appear throughout the contract's own examples —
//! `block_bytes: 128KiB`, `think_time: {median: 3s}`, `wss_window: 240_000` — and
//! none of them reach a Rust field as anything but a string, because YAML has no
//! notion of either.
//!
//! # Why a pass and not a deserializer
//!
//! The obvious fix is a custom `Deserialize` for [`Dist`](crate::dist::Dist) that
//! accepts a string. It cannot work, because **the unit is a property of the
//! field, not of the value**: a bare `3` means three *seconds* under `think_time`
//! and three *bytes* under `block_bytes`, and a deserializer sees only the value.
//! Making the unit part of the type instead — `Dist<Bytes>`, `Dist<Seconds>` —
//! would thread a parameter through the corpus, session, generation and validation
//! code for the benefit of one parsing concern.
//!
//! So the unit comes from the **path**, which is the one place it is unambiguous.
//! This pass walks the merged tree, and where a path is known to hold a
//! distribution it rewrites that subtree's unit-bearing scalars into the field's
//! base unit. It runs after `extends` merge and before deserialization, exactly as
//! [`extends::resolve`](super::extends) does, so an inherited unit string is
//! normalised once and only the canonical form is ever deserialized.
//!
//! # What "normalised" then means
//!
//! `block_bytes: 128KiB` and `block_bytes: 131072` become the same document, so
//! they hash the same and `--print-normalised` shows the same text. Two spellings
//! of one workload are one workload — which is what makes the content hash a
//! statement about the workload rather than about its punctuation.
//!
//! # The failure mode is loud
//!
//! A suffix that does not parse is an error here, naming the path and what would
//! have been accepted, rather than the `data did not match any variant of untagged
//! enum Dist` that serde produces. And a unit-bearing key this module does not
//! know about is left alone: [`Shape`](crate::dist::Shape) denies unknown fields,
//! so a suffixed string at such a key fails at deserialization instead of being
//! silently misread. Nothing here can turn a unit mistake into a wrong number that
//! reports itself as correct.

use serde_yaml::Value;

use crate::units::{parse_bytes, parse_duration_ns, parse_number, UnitError};

/// The unit a distribution's scalars are written in.
///
/// Only three, because only three appear: sizes, times, and unitless counts. The
/// base unit is what a *bare* number in that field already means, so normalising
/// to it leaves every existing document untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    /// Unitless. `_` separators are still stripped; a suffix is an error.
    Count,
    /// Bytes.
    Bytes,
    /// Seconds — **not** nanoseconds. `session::SessionParams::think_time_s` is
    /// seconds, so a bare `3` is three seconds and normalising to anything else
    /// would silently rescale every existing document.
    Seconds,
}

impl Unit {
    /// Convert one written scalar into this unit.
    fn convert(&self, s: &str) -> Result<f64, UnitError> {
        match self {
            Unit::Count => parse_number(s),
            Unit::Bytes => parse_bytes(s).map(|v| v as f64),
            // parse_duration_ns is exact for every suffix the schema admits, so
            // the division is exact for any value above a nanosecond.
            Unit::Seconds => parse_duration_ns(s).map(|ns| ns as f64 / 1e9),
        }
    }
}

/// Every path that holds a distribution, and the unit its scalars are in.
///
/// The complete set — there are twelve `Dist`-typed fields in the schema and all
/// twelve are here. A `*` segment matches one sequence index, which is what makes
/// one entry cover every `workload.mix` element.
const DIST_PATHS: &[(&str, Unit)] = &[
    ("corpus.block_bytes", Unit::Bytes),
    ("corpus.trees.shared_depth", Unit::Count),
    ("corpus.trees.roots.popularity", Unit::Count),
    ("workload.sessions.turns", Unit::Count),
    ("workload.sessions.think_time", Unit::Seconds),
    ("workload.sessions.private_depth", Unit::Count),
    ("workload.sessions.growth_per_turn", Unit::Count),
    ("workload.sessions.spawn.at_turn", Unit::Count),
    ("workload.mix.*.turns", Unit::Count),
    ("workload.mix.*.think_time", Unit::Seconds),
    ("workload.mix.*.private_depth", Unit::Count),
    ("workload.mix.*.growth_per_turn", Unit::Count),
    ("topology.replication.nodes_per_key", Unit::Count),
];

/// Keys inside a distribution whose value carries the distribution's unit.
///
/// Classified by key **name**, not by shape, because no name means two different
/// things across the shapes: `value`, `min`, `max`, `mean`, `stddev`, `median` and
/// `scale` are all in the field's unit, while `sigma`, `s`, `n` and `alpha` are
/// dimensionless and must not be touched. `sigma` is the one worth naming: it is
/// the standard deviation of the *log*, so `{median: 128KiB, sigma: 0.4}` has a
/// size in one field and a bare ratio in the other.
///
/// Naming them by key is also what lets a `sweep` axis be normalised: an axis path
/// like `corpus.block_bytes.median` names the key but says nothing about the shape.
const UNIT_BEARING: &[&str] = &["value", "min", "max", "mean", "stddev", "median", "scale"];

/// A scalar that could not be read as the unit its field is written in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormaliseError {
    /// Dotted path to the offending scalar.
    pub path: String,
    /// What was wrong with it.
    pub err: UnitError,
}

impl std::fmt::Display for NormaliseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.err)
    }
}

impl std::error::Error for NormaliseError {}

/// Rewrite every unit-suffixed scalar into its field's base unit, in place.
///
/// Idempotent: a normalised document is already all numbers, so a second pass has
/// nothing to do. That matters because `--print-normalised` output is a valid
/// input.
pub fn normalise(v: &mut Value) -> Result<(), NormaliseError> {
    let mut path: Vec<String> = Vec::new();
    walk(v, &mut path)
}

/// Whether `path` names a distribution, and in what unit.
fn unit_for(path: &[String]) -> Option<Unit> {
    DIST_PATHS
        .iter()
        .find(|(pattern, _)| matches_path(pattern, path))
        .map(|(_, unit)| *unit)
}

/// Match a dotted pattern against a path, `*` matching any one segment.
///
/// A numeric segment matches `*`, and so does a non-numeric one: a sequence index
/// is the only thing that can appear where the schema has a list, so there is
/// nothing to gain by being stricter.
fn matches_path(pattern: &str, path: &[String]) -> bool {
    let mut segments = pattern.split('.');
    let mut n = 0;
    for seg in &mut segments {
        match path.get(n) {
            None => return false,
            Some(actual) if seg == "*" || seg == actual => n += 1,
            Some(_) => return false,
        }
    }
    n == path.len()
}

/// The canonical YAML number for `v`: an integer where it is one.
///
/// Integral values become integers so that a normalised document reads as its
/// author wrote it — `131072`, not `131072.0` — and so that the `_`-stripping rule
/// can serve integer fields like `roots.count`, which will not accept a float.
fn number(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 9.007_199_254_740_992e15 {
        if v >= 0.0 {
            Value::from(v as u64)
        } else {
            Value::from(v as i64)
        }
    } else {
        Value::from(v)
    }
}

/// A string that is a `_`-separated number, and only that.
///
/// The `_` is the signal, deliberately. Without it a bare numeric string is left
/// alone, so a node named `"10"` stays a string and only the form the contract
/// documents — `240_000` — is rewritten. Applied everywhere, since "any bare
/// integer" is not scoped to a section.
fn underscore_number(s: &str) -> Option<f64> {
    if !s.contains('_') {
        return None;
    }
    parse_number(s).ok()
}

fn walk(v: &mut Value, path: &mut Vec<String>) -> Result<(), NormaliseError> {
    if let Some(unit) = unit_for(path) {
        return normalise_dist(v, unit, path);
    }
    // `sweep.axes` keys are dotted paths into the document, so their values need
    // the unit of wherever they will be substituted (spec FR-021, T089).
    if path.len() == 2 && path[0] == "sweep" && path[1] == "axes" {
        return normalise_sweep(v, path);
    }
    match v {
        Value::Mapping(m) => {
            let keys: Vec<Value> = m.keys().cloned().collect();
            for k in keys {
                let segment = k.as_str().unwrap_or_default().to_string();
                if let Some(val) = m.get_mut(&k) {
                    path.push(segment);
                    walk(val, path)?;
                    path.pop();
                }
            }
        }
        Value::Sequence(seq) => {
            for (i, val) in seq.iter_mut().enumerate() {
                path.push(i.to_string());
                walk(val, path)?;
                path.pop();
            }
        }
        Value::String(s) => {
            if let Some(n) = underscore_number(s) {
                *v = number(n);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Normalise one distribution's subtree.
fn normalise_dist(v: &mut Value, unit: Unit, path: &[String]) -> Result<(), NormaliseError> {
    match v {
        // The bare scalar form: `block_bytes: 128KiB`.
        Value::String(s) => {
            let n = convert(s, unit, path, None)?;
            *v = number(n);
        }
        Value::Mapping(m) => {
            let keys: Vec<Value> = m.keys().cloned().collect();
            for k in keys {
                let key = k.as_str().unwrap_or_default().to_string();
                let Some(val) = m.get_mut(&k) else { continue };
                if key == "points" {
                    normalise_points(val, unit, path)?;
                } else if UNIT_BEARING.contains(&key.as_str()) {
                    if let Value::String(s) = val {
                        let n = convert(s, unit, path, Some(&key))?;
                        *val = number(n);
                    }
                } else if let Value::String(s) = val {
                    // `sigma`, `alpha`, `s`, `n` and the `dist` tag itself. Only
                    // the separator rule applies; a suffix here would be a unit on
                    // a dimensionless quantity, which serde will refuse.
                    if let Some(n) = underscore_number(s) {
                        *val = number(n);
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// An `empirical` distribution's `points: [[value, cumulative_probability], ...]`.
///
/// Only the first element of each pair carries the unit; the second is a
/// probability. Getting this wrong would rescale the CDF rather than the values,
/// which is why the pairs are walked explicitly instead of flattened.
fn normalise_points(v: &mut Value, unit: Unit, path: &[String]) -> Result<(), NormaliseError> {
    let Value::Sequence(pairs) = v else {
        return Ok(());
    };
    for (i, pair) in pairs.iter_mut().enumerate() {
        let Value::Sequence(entries) = pair else {
            continue;
        };
        if let Some(Value::String(s)) = entries.first() {
            let s = s.clone();
            let key = format!("points[{i}][0]");
            let n = convert(&s, unit, path, Some(&key))?;
            entries[0] = number(n);
        }
        if let Some(Value::String(s)) = entries.get(1) {
            // A probability is dimensionless; only the separator rule applies.
            if let Some(n) = underscore_number(s) {
                entries[1] = number(n);
            }
        }
    }
    Ok(())
}

/// Normalise a `sweep.axes` mapping, taking each axis's unit from its own path.
///
/// An axis key such as `corpus.block_bytes.value` is a document path with a
/// trailing parameter name. The longest `DIST_PATHS` prefix gives the unit and the
/// trailing name says whether that parameter carries it — the same two questions
/// the structural walk answers, asked of a path written as a string.
fn normalise_sweep(v: &mut Value, path: &[String]) -> Result<(), NormaliseError> {
    let Value::Mapping(axes) = v else {
        return Ok(());
    };
    let keys: Vec<Value> = axes.keys().cloned().collect();
    for k in keys {
        let axis = k.as_str().unwrap_or_default().to_string();
        let unit = sweep_axis_unit(&axis);
        let Some(values) = axes.get_mut(&k) else {
            continue;
        };
        let Value::Sequence(points) = values else {
            continue;
        };
        for point in points.iter_mut() {
            if let Value::String(s) = point {
                let s = s.clone();
                match unit {
                    Some(unit) => {
                        let n = convert(&s, unit, path, Some(&axis))?;
                        *point = number(n);
                    }
                    None => {
                        if let Some(n) = underscore_number(&s) {
                            *point = number(n);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// The unit of a `sweep` axis path, or `None` if it names no unit-bearing scalar.
fn sweep_axis_unit(axis: &str) -> Option<Unit> {
    let segments: Vec<String> = axis.split('.').map(str::to_string).collect();
    // The axis may name the distribution itself (`...think_time`) or a parameter
    // inside it (`...think_time.median`).
    if let Some(unit) = unit_for(&segments) {
        return Some(unit);
    }
    let (last, head) = segments.split_last()?;
    if !UNIT_BEARING.contains(&last.as_str()) {
        return None;
    }
    unit_for(head)
}

/// Convert, attributing any failure to a path a reader can find.
fn convert(s: &str, unit: Unit, path: &[String], key: Option<&str>) -> Result<f64, NormaliseError> {
    unit.convert(s).map_err(|err| {
        let mut full = path.join(".");
        if let Some(k) = key {
            if !full.is_empty() {
                full.push('.');
            }
            full.push_str(k);
        }
        NormaliseError { path: full, err }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(y: &str) -> Value {
        let mut v: Value = serde_yaml::from_str(y).expect("yaml must parse");
        normalise(&mut v).expect("normalisation must succeed");
        v
    }

    fn at(v: &Value, path: &str) -> Value {
        let mut cur = v.clone();
        for seg in path.split('.') {
            cur = match seg.parse::<usize>() {
                Ok(i) => cur.get(i).cloned().unwrap_or(Value::Null),
                Err(_) => cur.get(seg).cloned().unwrap_or(Value::Null),
            };
        }
        cur
    }

    #[test]
    fn a_bare_size_becomes_bytes() {
        let v = norm("corpus: {block_bytes: 128KiB}");
        assert_eq!(at(&v, "corpus.block_bytes"), Value::from(131_072u64));
    }

    #[test]
    fn a_size_inside_a_shape_becomes_bytes_and_sigma_is_left_alone() {
        // The distinction the whole key classification exists for: median is a
        // size, sigma is the standard deviation of its logarithm.
        let v = norm("corpus: {block_bytes: {dist: lognormal, median: 128KiB, sigma: 0.4}}");
        assert_eq!(at(&v, "corpus.block_bytes.median"), Value::from(131_072u64));
        assert_eq!(at(&v, "corpus.block_bytes.sigma"), Value::from(0.4));
    }

    #[test]
    fn a_duration_becomes_seconds_not_nanoseconds() {
        // think_time is consumed as `think_time_s`, so seconds is the base unit
        // and any other choice would rescale every document that predates this.
        let v =
            norm("workload: {sessions: {think_time: {dist: lognormal, median: 3s, sigma: 1.1}}}");
        assert_eq!(
            at(&v, "workload.sessions.think_time.median"),
            Value::from(3u64)
        );
    }

    #[test]
    fn sub_second_durations_survive_the_conversion_exactly() {
        let v = norm("workload: {sessions: {think_time: {dist: const, value: 500ms}}}");
        assert_eq!(
            at(&v, "workload.sessions.think_time.value"),
            Value::from(0.5)
        );
    }

    #[test]
    fn the_same_quantity_written_two_ways_normalises_to_one_document() {
        // What makes the content hash a statement about the workload rather than
        // about its punctuation.
        let a = norm("corpus: {block_bytes: 128KiB}");
        let b = norm("corpus: {block_bytes: 131072}");
        assert_eq!(a, b);
    }

    #[test]
    fn normalising_twice_changes_nothing() {
        // `--print-normalised` output is a valid input, so the pass has to be
        // idempotent or a round trip would not be one.
        let once = norm("corpus: {block_bytes: 1MiB, trees: {shared_depth: 4_000}}");
        let mut twice = once.clone();
        normalise(&mut twice).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn underscore_separators_are_stripped_anywhere() {
        // The contract says "any bare integer", which is not scoped to a section.
        let v = norm("corpus: {trees: {roots: {count: 12_000}}}\nseed: 1_234");
        assert_eq!(at(&v, "corpus.trees.roots.count"), Value::from(12_000u64));
        assert_eq!(at(&v, "seed"), Value::from(1_234u64));
    }

    #[test]
    fn a_numeric_string_without_separators_is_left_a_string() {
        // The `_` is the signal. A node named "10" is a name, not a number, and
        // rewriting it would break a field that wants a string.
        let v = norm("topology: {nodes: ['10', node7]}");
        assert_eq!(at(&v, "topology.nodes.0"), Value::from("10"));
        assert_eq!(at(&v, "topology.nodes.1"), Value::from("node7"));
    }

    #[test]
    fn an_integral_result_is_an_integer_so_integer_fields_still_accept_it() {
        // roots.count is a u32; a 12000.0 would be refused by serde.
        let v = norm("corpus: {trees: {roots: {count: 12_000}}}");
        assert!(at(&v, "corpus.trees.roots.count").is_u64());
    }

    #[test]
    fn every_dist_path_in_the_schema_is_covered() {
        // The table claims to be complete, so the claim is asserted: one document
        // exercising all twelve, each with a form that only normalisation admits.
        let v = norm(
            r#"
corpus:
  block_bytes: 128KiB
  trees:
    shared_depth: {dist: const, value: 4_000}
    roots: {count: 12, popularity: {dist: zipf, s: 0.9, n: 1_000}}
workload:
  sessions:
    turns: {dist: geometric, mean: 1_0}
    think_time: {dist: const, value: 2s}
    private_depth: {dist: const, value: 4_000}
    growth_per_turn: {dist: const, value: 1_2}
    spawn: {fanout: 2, probability: 0.1, at_turn: {dist: const, value: 1_0}}
  mix:
    - {weight: 1.0, turns: {dist: const, value: 1_0}, think_time: 3s,
       private_depth: {dist: const, value: 2_000}, growth_per_turn: {dist: const, value: 6}}
topology:
  nodes: [a, b]
  replication: {nodes_per_key: {dist: const, value: 1_0}}
"#,
        );
        assert_eq!(at(&v, "corpus.block_bytes"), Value::from(131_072u64));
        assert_eq!(
            at(&v, "corpus.trees.shared_depth.value"),
            Value::from(4_000u64)
        );
        assert_eq!(
            at(&v, "corpus.trees.roots.popularity.n"),
            Value::from(1_000u64)
        );
        assert_eq!(at(&v, "workload.sessions.turns.mean"), Value::from(10u64));
        assert_eq!(
            at(&v, "workload.sessions.think_time.value"),
            Value::from(2u64)
        );
        assert_eq!(
            at(&v, "workload.sessions.private_depth.value"),
            Value::from(4_000u64)
        );
        assert_eq!(
            at(&v, "workload.sessions.growth_per_turn.value"),
            Value::from(12u64)
        );
        assert_eq!(
            at(&v, "workload.sessions.spawn.at_turn.value"),
            Value::from(10u64)
        );
        assert_eq!(at(&v, "workload.mix.0.turns.value"), Value::from(10u64));
        assert_eq!(at(&v, "workload.mix.0.think_time"), Value::from(3u64));
        assert_eq!(
            at(&v, "workload.mix.0.private_depth.value"),
            Value::from(2_000u64)
        );
        assert_eq!(
            at(&v, "topology.replication.nodes_per_key.value"),
            Value::from(10u64)
        );
    }

    #[test]
    fn an_empirical_distributions_values_are_converted_and_its_probabilities_are_not() {
        // Rescaling the CDF instead of the values would be a silent disaster: the
        // shape would still be a valid distribution, of the wrong thing.
        let v =
            norm("corpus: {block_bytes: {dist: empirical, points: [[64KiB, 0.5], [1MiB, 1.0]]}}");
        let points = at(&v, "corpus.block_bytes.points");
        assert_eq!(points.get(0).unwrap().get(0), Some(&Value::from(65_536u64)));
        assert_eq!(points.get(0).unwrap().get(1), Some(&Value::from(0.5)));
        assert_eq!(
            points.get(1).unwrap().get(0),
            Some(&Value::from(1_048_576u64))
        );
        assert_eq!(points.get(1).unwrap().get(1), Some(&Value::from(1.0)));
    }

    #[test]
    fn a_sweep_axis_takes_its_unit_from_its_own_path() {
        // The payoff of keying on paths: an axis key *is* a path, so the same
        // table answers a question the structural walk never sees.
        let v = norm(
            "sweep: {axes: {corpus.block_bytes.median: [64KiB, 128KiB], \
             workload.sessions.think_time: [1s, 2s], \
             workload.mix.0.weight: [0.4, 0.6]}, repeat: 8}",
        );
        let axes = at(&v, "sweep.axes");
        assert_eq!(
            axes.get("corpus.block_bytes.median").unwrap().get(0),
            Some(&Value::from(65_536u64))
        );
        assert_eq!(
            axes.get("workload.sessions.think_time").unwrap().get(1),
            Some(&Value::from(2u64))
        );
        // A dimensionless axis is untouched.
        assert_eq!(
            axes.get("workload.mix.0.weight").unwrap().get(0),
            Some(&Value::from(0.4))
        );
    }

    #[test]
    fn a_sweep_axis_over_a_mix_entry_matches_the_wildcard() {
        let v = norm("sweep: {axes: {workload.mix.2.think_time.median: [1s, 4s]}, repeat: 8}");
        let axes = at(&v, "sweep.axes");
        assert_eq!(
            axes.get("workload.mix.2.think_time.median").unwrap().get(1),
            Some(&Value::from(4u64))
        );
    }

    #[test]
    fn a_bad_suffix_names_the_path_and_what_would_have_worked() {
        let mut v: Value =
            serde_yaml::from_str("corpus: {block_bytes: {dist: const, value: 128QiB}}").unwrap();
        let e = normalise(&mut v).expect_err("must refuse");
        assert_eq!(e.path, "corpus.block_bytes.value");
        assert!(e.err.input == "128QiB", "{:?}", e.err);
        assert!(e.to_string().contains("128KiB"), "{e}");
    }

    #[test]
    fn a_duration_suffix_on_a_size_is_refused_rather_than_read_as_a_number() {
        // `3s` under block_bytes is not 3 bytes. Refusing rather than defaulting
        // is the whole point of units.rs, and the pass must not undo it.
        let mut v: Value = serde_yaml::from_str("corpus: {block_bytes: 3s}").unwrap();
        assert!(normalise(&mut v).is_err());
    }

    #[test]
    fn a_unit_on_a_count_is_refused() {
        // A count has no unit to convert from, so a suffix there is a mistake
        // about what the field means.
        let mut v: Value =
            serde_yaml::from_str("workload: {sessions: {turns: {dist: const, value: 6s}}}")
                .unwrap();
        let e = normalise(&mut v).expect_err("must refuse");
        assert_eq!(e.path, "workload.sessions.turns.value");
    }

    #[test]
    fn the_string_valued_duration_fields_are_left_as_strings() {
        // `duration`, `warmup`, `rate`, `gpu_buffer` and a duration-valued
        // `wss_window` are `String` fields parsed later by `units`. Rewriting them
        // into numbers here would strip the unit they carry and, for `wss_window`,
        // turn a duration into a request count — silently changing what the field
        // means rather than how it is spelled.
        let v = norm(
            "duration: 120s\nrun: {mode: hardware, warmup: 30s, gpu_buffer: 8GiB, \
             wss_window: 60s}\nworkload: {arrival: {model: open_loop, rate: 4000/s}}",
        );
        assert_eq!(at(&v, "duration"), Value::from("120s"));
        assert_eq!(at(&v, "run.warmup"), Value::from("30s"));
        assert_eq!(at(&v, "run.gpu_buffer"), Value::from("8GiB"));
        assert_eq!(at(&v, "run.wss_window"), Value::from("60s"));
        assert_eq!(at(&v, "workload.arrival.rate"), Value::from("4000/s"));
    }

    #[test]
    fn a_count_valued_window_loses_only_its_separators() {
        let v = norm("run: {mode: plan, wss_window: 240_000}");
        assert_eq!(at(&v, "run.wss_window"), Value::from(240_000u64));
    }

    #[test]
    fn a_document_with_no_unit_strings_is_returned_unchanged() {
        let y =
            "corpus: {block_bytes: 131072}\nworkload: {sessions: {turns: {dist: const, value: 6}}}";
        let mut v: Value = serde_yaml::from_str(y).unwrap();
        let before = v.clone();
        normalise(&mut v).unwrap();
        assert_eq!(v, before);
    }

    #[test]
    fn path_matching_requires_the_whole_path_not_a_prefix() {
        // `corpus.block_bytes.median` must not itself match the `corpus.block_bytes`
        // entry, or the walk would treat a shape's parameter as a whole
        // distribution.
        let p: Vec<String> = ["corpus", "block_bytes"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(matches_path("corpus.block_bytes", &p));
        let deeper: Vec<String> = ["corpus", "block_bytes", "median"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(!matches_path("corpus.block_bytes", &deeper));
        let shorter: Vec<String> = ["corpus"].iter().map(|s| s.to_string()).collect();
        assert!(!matches_path("corpus.block_bytes", &shorter));
    }

    #[test]
    fn every_table_entry_is_a_field_that_exists() {
        // A path with a typo would silently never match, so the table is checked
        // against a document that names all of them. `serde(deny_unknown_fields)`
        // on the schema makes the document itself the assertion.
        for (pattern, _) in DIST_PATHS {
            let segments: Vec<String> = pattern.split('.').map(str::to_string).collect();
            assert!(
                unit_for(&segments)
                    == Some(DIST_PATHS.iter().find(|(p, _)| p == pattern).unwrap().1)
                    || segments.contains(&"*".to_string()),
                "{pattern} does not match itself"
            );
        }
    }
}
