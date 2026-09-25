//! The workload description: the YAML schema, its parser, and its validation.
//!
//! `contracts/workload-input.example.yml` is normative for this schema, and the
//! shipped example is exercised as a test, so the two cannot drift.
//!
//! # Declaration order is part of the meaning
//!
//! `shared_classes` and `session_classes` are written as YAML mappings but are
//! **not** unordered. A shared class's position is its `class_id` in the key
//! salt, and a session class's `uses` list fixes the order shared prefixes
//! appear in a chain (FR-028). So they parse into [`Declared`], which keeps
//! document order, rather than into a `HashMap`, which would make keys depend on
//! hash iteration order and reuse depend on nothing at all.
//!
//! # Validation happens once, before anything is issued
//!
//! [`WorkloadDescription::validate`] returns either a refusal naming the
//! offending parameter, or a [`Report`] of every place the effective
//! configuration differs from what was written (FR-002, FR-003). Nothing is
//! silently reshaped.
//!
//! # Examples
//!
//! ```
//! use workload_model::description::WorkloadDescription;
//!
//! let yaml = r#"
//! version: 1
//! blocks: {tokens: 16, bytes: 65536}
//! shared_classes:
//!   system_prompt:
//!     length:   {constant: 10}
//!     lifetime: {constant: inf}
//! session_classes:
//!   chat:
//!     uses:
//!       - class: system_prompt
//!         count: {constant: 1}
//!     turns:         {constant: 4}
//!     input_growth:  {constant: 5}
//!     output_growth: {constant: 2}
//!     think_time:    {exponential: {mean: 10}}
//!     pool: {size: 100}
//! "#;
//!
//! let d: WorkloadDescription = yaml.parse().unwrap();
//! let report = d.validate().unwrap();
//! assert_eq!(d.shared_classes.index_of("system_prompt"), Some(0));
//! assert!(report.refusals().is_empty());
//! ```

use std::fmt;
use std::path::Path;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::Deserialize;

use crate::distribution::{Distribution, Effective};
use crate::error::{Error, Result};
use crate::keys;

/// The only schema version this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Fraction of a count distribution's mass that an implied maximum may discard
/// before the description is refused (FR-004).
pub const MAX_DISCARDED_MASS: f64 = 0.05;

// ---------------------------------------------------------------------------
// Declaration-ordered mapping
// ---------------------------------------------------------------------------

/// A `name -> value` mapping that preserves the order the names were declared
/// in, and rejects duplicates.
///
/// # Examples
///
/// ```
/// use workload_model::description::Declared;
///
/// let d: Declared<u32> = serde_yaml::from_str("{b: 2, a: 1}").unwrap();
/// // Document order, not sorted order: `b` was declared first.
/// assert_eq!(d.index_of("b"), Some(0));
/// assert_eq!(d.index_of("a"), Some(1));
/// assert_eq!(d.get("a"), Some(&1));
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Declared<T> {
    entries: Vec<(String, T)>,
}

impl<T> Declared<T> {
    /// Number of declared entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing was declared.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries in declaration order, with their index.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &str, &T)> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, (n, v))| (i, n.as_str(), v))
    }

    /// Look a value up by name.
    pub fn get(&self, name: &str) -> Option<&T> {
        self.entries.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// The declaration index of a name — its `class_id` in a key salt.
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.entries.iter().position(|(n, _)| n == name)
    }

    /// The entry at a declaration index.
    pub fn by_index(&self, index: usize) -> Option<(&str, &T)> {
        self.entries.get(index).map(|(n, v)| (n.as_str(), v))
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Declared<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V<T>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>> Visitor<'de> for V<T> {
            type Value = Declared<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a mapping of names to values")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Declared<T>, M::Error> {
                let mut entries: Vec<(String, T)> = Vec::new();
                while let Some(name) = map.next_key::<String>()? {
                    if entries.iter().any(|(n, _)| *n == name) {
                        // YAML would let the later one silently win, changing
                        // every class_id after it.
                        return Err(de::Error::custom(format!(
                            "{name:?} is declared twice; declaration order is \
                             significant, so a duplicate is ambiguous"
                        )));
                    }
                    let value = map.next_value()?;
                    entries.push((name, value));
                }
                Ok(Declared { entries })
            }
        }
        d.deserialize_map(V(std::marker::PhantomData))
    }
}

// ---------------------------------------------------------------------------
// Populations
// ---------------------------------------------------------------------------

/// How a population's size behaves over time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Population {
    /// Free-running births at rate `size / E[lifetime]`, so the live count is
    /// Poisson with relative spread `1/sqrt(size)`. What a real population does,
    /// and the default for an explicit size (FR-013, FR-014).
    Poisson(u64),
    /// Exactly this many alive at all times, each death replaced immediately.
    /// Variance zero — the right choice for a small population, where Poisson's
    /// 1/sqrt(N) spread is large.
    Exact(u64),
}

impl Population {
    /// The nominal size, whichever form this is.
    pub fn nominal(&self) -> u64 {
        match self {
            Population::Poisson(n) | Population::Exact(n) => *n,
        }
    }
}

impl<'de> Deserialize<'de> for Population {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Population;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an integer, or {poisson: N}, or {exact: N}")
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<Population, E> {
                Ok(Population::Poisson(v))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<Population, E> {
                if v < 0 {
                    return Err(E::custom(format!("population {v} is negative")));
                }
                Ok(Population::Poisson(v as u64))
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Population, M::Error> {
                let Some(tag) = map.next_key::<String>()? else {
                    return Err(de::Error::custom(
                        "a population needs one entry, {poisson: N} or {exact: N}",
                    ));
                };
                let n: u64 = map.next_value()?;
                let p = match tag.as_str() {
                    "poisson" => Population::Poisson(n),
                    "exact" => Population::Exact(n),
                    other => {
                        return Err(de::Error::custom(format!(
                            "unknown population form {other:?}; expected poisson or exact"
                        )))
                    }
                };
                if map.next_key::<String>()?.is_some() {
                    return Err(de::Error::custom(
                        "a population takes exactly one form, poisson or exact",
                    ));
                }
                Ok(p)
            }
        }
        d.deserialize_any(V)
    }
}

/// What a shared instance's popularity attaches to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RankBy {
    /// Popularity attaches to a **position**: a new instance inherits its slot's
    /// heat and keeps it for life. `lifetime` is then the only thing that
    /// retires a key. The default, because it leaves the written `lifetime`
    /// doing what its author expects.
    #[default]
    Slot,
    /// Popularity attaches to **newness**: each new instance pushes the others
    /// back, so heat decays with age. An instance goes cold long before its
    /// lifetime expires, which makes `lifetime` nearly inert.
    Recency,
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Block geometry, shared by every class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockGeometry {
    /// Tokens per KV block.
    pub tokens: u64,
    /// Bytes per block on the wire.
    pub bytes: u64,
}

/// A pool of shared objects that sessions reference in common.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedClass {
    /// Blocks per instance. Integral.
    pub length: Distribution,
    /// Virtual seconds an instance lives. May be `inf`.
    pub lifetime: Distribution,
    /// Population dynamics and popularity. Absent means a single immortal
    /// instance — see [`SharedPoolSpec`].
    #[serde(default)]
    pub pool: SharedPoolSpec,
}

/// Population and popularity settings for a shared class.
///
/// An absent `pool:` is **not** the Poisson default. It means `exact: 1`: one
/// instance, and with an infinite lifetime nothing ever dies, so the pool is
/// minted once and never turns over (FR-017). Making the absent case Poisson
/// would refuse the shipped example's own immortal classes, since a Poisson
/// birth rate of `N / E[lifetime]` is undefined for an unbounded lifetime.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SharedPoolSpec {
    /// Nominal population. A bare integer is shorthand for `{poisson: N}`.
    #[serde(default = "default_population")]
    pub size: Population,
    /// What popularity attaches to. Defaults to [`RankBy::Slot`]; the default
    /// being applied to a multi-instance pool is reported at load time.
    #[serde(default)]
    pub rank_by: Option<RankBy>,
    /// Distribution over the instance index. Absent means uniform, which makes
    /// the working set equal the key space and leaves every eviction policy
    /// scoring the same (FR-021).
    #[serde(default)]
    pub selection: Option<Distribution>,
}

fn default_population() -> Population {
    Population::Exact(1)
}

impl Default for SharedPoolSpec {
    fn default() -> Self {
        Self {
            size: default_population(),
            rank_by: None,
            selection: None,
        }
    }
}

/// One entry in a session class's ordered `uses` list.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uses {
    /// Name of a declared shared class.
    pub class: String,
    /// How many instances of it a session draws. Integral, and capped by the
    /// referenced pool's size.
    pub count: Distribution,
}

/// A class of sessions.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionClass {
    /// Shared classes this session draws from, **in order**. The order fixes
    /// where each class's blocks land in the prefix chain, so it is semantic.
    pub uses: Vec<Uses>,
    /// Turns per session. Integral.
    pub turns: Distribution,
    /// Input blocks minted per turn. Integral.
    pub input_growth: Distribution,
    /// Output blocks minted per turn. Integral.
    pub output_growth: Distribution,
    /// Virtual seconds before each turn.
    pub think_time: Distribution,
    /// Virtual seconds between node migrations. Absent means this class never
    /// migrates.
    #[serde(default)]
    pub migration_interval: Option<Distribution>,
    /// Concurrent sessions. Arrival rate is therefore an output (FR-024).
    pub pool: SessionPoolSpec,
}

/// Population settings for a session class.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionPoolSpec {
    /// Concurrent sessions. A bare integer is shorthand for `{poisson: N}`.
    pub size: Population,
}

/// A parsed workload description.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadDescription {
    /// Schema version. Only [`SCHEMA_VERSION`] is understood.
    pub version: u32,
    /// Block geometry.
    pub blocks: BlockGeometry,
    /// Shared object classes, in declaration order.
    pub shared_classes: Declared<SharedClass>,
    /// Session classes, in declaration order.
    pub session_classes: Declared<SessionClass>,
}

impl std::str::FromStr for WorkloadDescription {
    type Err = Error;

    fn from_str(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).map_err(Error::new)
    }
}

impl WorkloadDescription {
    /// Read and parse a description, resolving any `empirical: {file: ...}`
    /// samples relative to the description's own directory.
    ///
    /// # Errors
    ///
    /// If the file cannot be read, does not parse, or names a sample file that
    /// cannot be read.
    ///
    /// # Examples
    ///
    /// Loading `contracts/workload-input.example.yml`, which is **normative** for
    /// this schema. Reading it here rather than inlining a copy is the point: an
    /// inlined copy would let the file and the parser drift apart without anything
    /// noticing, and this schema's own specification is that file.
    ///
    /// The path is resolved against `CARGO_MANIFEST_DIR` rather than the working
    /// directory, so it holds wherever the tests are run from.
    ///
    /// ```
    /// use std::path::PathBuf;
    /// use workload_model::description::WorkloadDescription;
    ///
    /// let example = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
    ///     "../../specs/001-synthetic-workload-generator/contracts/workload-input.example.yml",
    /// );
    ///
    /// let description = WorkloadDescription::from_path(&example).unwrap();
    /// let report = description.validate().unwrap();
    ///
    /// assert_eq!(description.version, 1);
    /// assert_eq!(description.blocks.tokens, 16);
    /// assert!(report.refusals().is_empty());
    /// ```
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let text = std::fs::read_to_string(path)
            .map_err(|e| Error::new(format!("cannot read {path:?}: {e}")))?;
        let mut d: Self = text.parse()?;
        let base = path.parent().unwrap_or(Path::new("."));
        d.load_sample_files(base)?;
        Ok(d)
    }

    /// Resolve every deferred `empirical: {file: ...}` against `base_dir`.
    ///
    /// # Errors
    ///
    /// If a named sample file cannot be read or holds no numbers.
    pub fn load_sample_files(&mut self, base_dir: &Path) -> Result<()> {
        for (name, class) in self.shared_classes.entries.iter_mut() {
            class
                .length
                .load_sample_files(base_dir)
                .map_err(|e| e.within(format!("shared_classes.{name}.length")))?;
            class
                .lifetime
                .load_sample_files(base_dir)
                .map_err(|e| e.within(format!("shared_classes.{name}.lifetime")))?;
            if let Some(sel) = class.pool.selection.as_mut() {
                sel.load_sample_files(base_dir)
                    .map_err(|e| e.within(format!("shared_classes.{name}.pool.selection")))?;
            }
        }
        for (name, class) in self.session_classes.entries.iter_mut() {
            for u in class.uses.iter_mut() {
                let cls = u.class.clone();
                u.count
                    .load_sample_files(base_dir)
                    .map_err(|e| e.within(format!("session_classes.{name}.uses[{cls}].count")))?;
            }
            for (field, dist) in [
                ("turns", &mut class.turns),
                ("input_growth", &mut class.input_growth),
                ("output_growth", &mut class.output_growth),
                ("think_time", &mut class.think_time),
            ] {
                dist.load_sample_files(base_dir)
                    .map_err(|e| e.within(format!("session_classes.{name}.{field}")))?;
            }
            if let Some(m) = class.migration_interval.as_mut() {
                m.load_sample_files(base_dir)
                    .map_err(|e| e.within(format!("session_classes.{name}.migration_interval")))?;
            }
        }
        Ok(())
    }

    /// Validate the whole description and report every effective value that
    /// differs from what was written.
    ///
    /// Runs before any operation is issued (FR-002). Refusals are collected so
    /// that one run surfaces every problem rather than one problem per run;
    /// the returned `Err` carries them all.
    ///
    /// # Errors
    ///
    /// If the description is unusable. The message names each offending
    /// parameter by its path in the file.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::description::WorkloadDescription;
    ///
    /// let d: WorkloadDescription = "version: 2\nblocks: {tokens: 1, bytes: 1}\n\
    ///     shared_classes: {}\nsession_classes: {}".parse().unwrap();
    /// let e = d.validate().unwrap_err();
    /// assert!(e.to_string().contains("version 2"));
    /// ```
    pub fn validate(&self) -> Result<Report> {
        let mut report = Report::default();

        if self.version != SCHEMA_VERSION {
            report.refuse(format!(
                "version {} is not understood; this build reads version {}",
                self.version, SCHEMA_VERSION
            ));
            // Nothing below is meaningful against an unknown schema.
            return report.into_result();
        }

        if self.blocks.tokens == 0 || self.blocks.bytes == 0 {
            report.refuse("blocks.tokens and blocks.bytes must both be positive");
        }
        if self.shared_classes.len() as u64 > keys::MAX_CLASSES {
            report.refuse(format!(
                "{} shared classes exceeds the {} the key salt can encode",
                self.shared_classes.len(),
                keys::MAX_CLASSES
            ));
        }

        self.validate_shared_classes(&mut report);
        self.validate_session_classes(&mut report);
        report.into_result()
    }

    fn validate_shared_classes(&self, report: &mut Report) {
        for (_, name, class) in self.shared_classes.iter() {
            let at = |field: &str| format!("shared_classes.{name}.{field}");

            // Length is integral: blocks per instance.
            match class.length.resolve_integral(None) {
                Ok(r) => {
                    let eff = r.effective();
                    report.note_effective(at("length"), eff);
                    if eff.hi.is_finite() && eff.hi as u64 >= keys::MAX_BLOCKS_PER_STREAM {
                        report.refuse(format!(
                            "{}: an instance of up to {} blocks exceeds the {} the key \
                             salt can encode",
                            at("length"),
                            eff.hi,
                            keys::MAX_BLOCKS_PER_STREAM
                        ));
                    }
                    if eff.hi < 1.0 {
                        report.refuse(format!(
                            "{}: an instance needs at least one block",
                            at("length")
                        ));
                    }
                }
                Err(e) => report.refuse(e.within(at("length")).to_string()),
            }

            let lifetime = class.lifetime.resolve(None);
            let unbounded = match &lifetime {
                Ok(r) => {
                    report.note_effective(at("lifetime"), r.effective());
                    !r.effective().mean.is_finite()
                }
                Err(e) => {
                    report.refuse(e.clone().within(at("lifetime")).to_string());
                    false
                }
            };

            // An unbounded lifetime makes a Poisson birth rate of
            // `size / E[lifetime]` undefined, so the two cannot be combined
            // (FR-017 makes such a class mint-once instead).
            if unbounded {
                if let Population::Poisson(n) = class.pool.size {
                    report.refuse(format!(
                        "{}: a poisson population of {n} needs a finite lifetime, because \
                         its birth rate is size / E[lifetime]; write {{exact: {n}}} for a \
                         pool that is minted once and never turns over",
                        at("pool.size")
                    ));
                }
            }

            let nominal = class.pool.size.nominal();
            if nominal == 0 {
                report.refuse(format!(
                    "{}: a pool of 0 instances has nothing to select",
                    at("pool.size")
                ));
            }
            if nominal > keys::MAX_INSTANCES_PER_POOL {
                report.refuse(format!(
                    "{}: a pool of {nominal} exceeds the {} instances the key salt can encode",
                    at("pool.size"),
                    keys::MAX_INSTANCES_PER_POOL
                ));
            }

            // FR-021: uniform selection makes the working set equal the key
            // space, which flattens popularity and leaves every eviction policy
            // scoring the same. Report it rather than refuse it.
            if nominal > 1 && class.pool.selection.is_none() {
                report.note(format!(
                    "{}: no selection distribution, so selection is uniform over all \
                     {nominal} instances and the implied working set equals the whole \
                     key space; every eviction policy will score alike",
                    at("pool")
                ));
            }
            if nominal > 1 && class.pool.rank_by.is_none() {
                report.note(format!(
                    "{}: rank_by defaulted to slot, so popularity attaches to a position \
                     and lifetime is what retires a key",
                    at("pool")
                ));
            }
            if let Some(sel) = &class.pool.selection {
                match sel.resolve_integral(Some((nominal - 1) as f64)) {
                    Ok(r) => report.note_effective(at("pool.selection"), r.effective()),
                    Err(e) => report.refuse(e.within(at("pool.selection")).to_string()),
                }
            }
        }
    }

    fn validate_session_classes(&self, report: &mut Report) {
        if self.session_classes.is_empty() {
            report.refuse("session_classes is empty, so there is no workload to run");
        }
        for (_, name, class) in self.session_classes.iter() {
            let at = |field: &str| format!("session_classes.{name}.{field}");

            if class.uses.is_empty() {
                report.note(format!(
                    "{}: no shared classes, so this class produces no inter-session reuse",
                    at("uses")
                ));
            }
            // FR-028 makes a session's prefix a pure function of the *set* of
            // instances it chose, and listing a class twice destroys that: the two
            // draws are independent, so the same instance can land at two
            // positions in the prefix, where prefix chaining gives it two
            // different keys. `count` expresses the intent without the ambiguity.
            let mut listed = std::collections::BTreeSet::new();
            for u in &class.uses {
                if !listed.insert(u.class.as_str()) {
                    report.refuse(format!(
                        "{}: shared class {:?} is listed more than once. A session's \
                         prefix must be a function of the set it drew (FR-028), and two \
                         independent draws from one class can place the same instance \
                         twice; raise that entry's count instead",
                        at("uses"),
                        u.class
                    ));
                }
            }

            for u in &class.uses {
                let path = at(&format!("uses[{}].count", u.class));
                let Some(target) = self.shared_classes.get(&u.class) else {
                    report.refuse(format!(
                        "{}: shared class {:?} is not declared",
                        at("uses"),
                        u.class
                    ));
                    continue;
                };
                let nominal = target.pool.size.nominal();
                // The pool size is the implied maximum on the count, and FR-004
                // refuses a truncation that discards more than 5% of the mass.
                match u.count.resolve_integral(Some(nominal as f64)) {
                    Ok(r) => {
                        let eff = r.effective();
                        report.note_effective(path.clone(), eff);
                        if eff.discarded > MAX_DISCARDED_MASS {
                            report.refuse(format!(
                                "{path}: the pool of {nominal} caps this count and discards \
                                 {:.1}% of its mass, above the {:.0}% limit; the requested \
                                 mean is {} but the effective mean is {:.4}. Either raise \
                                 {}.pool.size or lower the count",
                                eff.discarded * 100.0,
                                MAX_DISCARDED_MASS * 100.0,
                                eff.requested_mean,
                                eff.mean,
                                u.class
                            ));
                        }
                    }
                    Err(e) => report.refuse(e.within(path).to_string()),
                }
            }

            for (field, dist) in [
                ("turns", &class.turns),
                ("input_growth", &class.input_growth),
                ("output_growth", &class.output_growth),
            ] {
                match dist.resolve_integral(None) {
                    Ok(r) => {
                        let eff = r.effective();
                        report.note_effective(at(field), eff);
                        if eff.hi < 1.0 {
                            report.refuse(format!("{}: must allow at least 1", at(field)));
                        }
                    }
                    Err(e) => report.refuse(e.within(at(field)).to_string()),
                }
            }

            match class.think_time.resolve(None) {
                Ok(r) => {
                    let eff = r.effective();
                    report.note_effective(at("think_time"), eff);
                    if eff.lo < 0.0 {
                        report.refuse(format!("{}: cannot be negative", at("think_time")));
                    }
                    // FR-024 makes a session's duration emergent —
                    // `E[turns] * E[think_time]` — and the arrival rate
                    // `size / E[duration]`. A mean think time of zero therefore
                    // gives instantaneous sessions, and no finite arrival rate can
                    // sustain any concurrency at all. Refused here rather than left
                    // to divide by zero mid-run.
                    if eff.mean <= 0.0 {
                        report.refuse(format!(
                            "{}: a mean of {} makes sessions instantaneous, so the \
                             arrival rate needed to hold {} concurrent sessions is \
                             unbounded. Session duration is an output of turns and \
                             think time (FR-024), so give think_time a positive mean",
                            at("think_time"),
                            eff.mean,
                            class.pool.size.nominal()
                        ));
                    }
                }
                Err(e) => report.refuse(e.within(at("think_time")).to_string()),
            }

            if let Some(m) = &class.migration_interval {
                match m.resolve(None) {
                    Ok(r) => report.note_effective(at("migration_interval"), r.effective()),
                    Err(e) => report.refuse(e.within(at("migration_interval")).to_string()),
                }
            }

            if class.pool.size.nominal() == 0 {
                report.refuse(format!(
                    "{}: a pool of 0 sessions runs nothing",
                    at("pool.size")
                ));
            }

            // A session's own input and output streams each carry a block
            // ordinal, so a very long session could in principle exceed what the
            // salt encodes. Checked from the effective bounds rather than from
            // the projection, so the refusal happens at load.
            if let (Ok(turns), Ok(growth)) = (
                class.turns.resolve_integral(None),
                class.input_growth.resolve_integral(None),
            ) {
                let (t, g) = (turns.effective().hi, growth.effective().hi);
                if t.is_finite() && g.is_finite() {
                    let worst = t * g;
                    if worst >= keys::MAX_BLOCKS_PER_STREAM as f64 {
                        report.refuse(format!(
                            "{}: up to {t} turns of up to {g} input blocks is {worst} blocks, \
                             at or beyond the {} one session stream can encode",
                            at("turns"),
                            keys::MAX_BLOCKS_PER_STREAM
                        ));
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// What validation found: refusals, and every effective value that differs from
/// what the file said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    refusals: Vec<String>,
    notes: Vec<String>,
    effective: Vec<(String, Effective)>,
}

impl Report {
    fn refuse(&mut self, message: impl fmt::Display) {
        self.refusals.push(message.to_string());
    }

    fn note(&mut self, message: impl fmt::Display) {
        self.notes.push(message.to_string());
    }

    fn note_effective(&mut self, path: String, eff: Effective) {
        if eff.differs() || eff.discarded > 0.0 {
            self.effective.push((path, eff));
        }
    }

    fn into_result(self) -> Result<Self> {
        if self.refusals.is_empty() {
            return Ok(self);
        }
        let mut msg = String::from("the workload description was refused:");
        for r in &self.refusals {
            msg.push_str("\n  - ");
            msg.push_str(r);
        }
        Err(Error::new(msg))
    }

    /// Reasons the description was refused. Empty on success.
    pub fn refusals(&self) -> &[String] {
        &self.refusals
    }

    /// Advisory notes worth printing at load time — a uniform selection that
    /// flattens popularity, a defaulted `rank_by`, and the like.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Every parameter whose effective distribution differs from what was
    /// written, with the realised statistics (FR-003).
    pub fn effective(&self) -> &[(String, Effective)] {
        &self.effective
    }

    /// Render the report the way `validate` prints it.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_model::description::Report;
    ///
    /// assert_eq!(Report::default().render(), "the description is consistent\n");
    /// ```
    pub fn render(&self) -> String {
        let mut out = String::new();
        if self.effective.is_empty() && self.notes.is_empty() {
            return "the description is consistent\n".to_string();
        }
        if !self.effective.is_empty() {
            out.push_str("effective values that differ from the file:\n");
            for (path, e) in &self.effective {
                out.push_str(&format!(
                    "  {path}: requested mean {:.4}, effective mean {:.4}",
                    e.requested_mean, e.mean
                ));
                if e.discarded > 0.0 {
                    out.push_str(&format!(" ({:.2}% of mass discarded)", e.discarded * 100.0));
                }
                out.push('\n');
            }
        }
        for n in &self.notes {
            out.push_str(&format!("note: {n}\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_preserves_document_order() {
        let d: Declared<u32> = serde_yaml::from_str("{zebra: 1, apple: 2, mango: 3}").unwrap();
        let names: Vec<&str> = d.iter().map(|(_, n, _)| n).collect();
        assert_eq!(names, ["zebra", "apple", "mango"]);
        assert_eq!(d.index_of("zebra"), Some(0));
        assert_eq!(d.index_of("mango"), Some(2));
    }

    #[test]
    fn duplicate_declaration_is_refused() {
        // YAML would let the later entry win, shifting every class_id after it.
        let e = serde_yaml::from_str::<Declared<u32>>("{a: 1, a: 2}");
        assert!(e.is_err(), "duplicate name accepted");
    }

    #[test]
    fn population_accepts_all_three_spellings() {
        assert_eq!(
            serde_yaml::from_str::<Population>("50").unwrap(),
            Population::Poisson(50)
        );
        assert_eq!(
            serde_yaml::from_str::<Population>("{poisson: 50}").unwrap(),
            Population::Poisson(50)
        );
        assert_eq!(
            serde_yaml::from_str::<Population>("{exact: 10}").unwrap(),
            Population::Exact(10)
        );
        assert!(serde_yaml::from_str::<Population>("{pareto: 10}").is_err());
    }

    #[test]
    fn absent_pool_is_one_immortal_instance_not_poisson() {
        // The distinction matters: a Poisson default would refuse the shipped
        // example's immortal classes, because a birth rate of size/E[lifetime]
        // is undefined for an unbounded lifetime.
        let spec = SharedPoolSpec::default();
        assert_eq!(spec.size, Population::Exact(1));
        assert_eq!(spec.size.nominal(), 1);
    }

    #[test]
    fn tuning_fields_are_rejected_wherever_they_appear() {
        // FR-005: no host-specific or tuning setting may appear in a
        // description. `deny_unknown_fields` is what enforces it, so this test
        // guards against someone removing it.
        let bad = "version: 1\nblocks: {tokens: 16, bytes: 65536, lanes: 8}\n\
                   shared_classes: {}\nsession_classes: {}";
        let e = bad.parse::<WorkloadDescription>().unwrap_err();
        assert!(e.to_string().contains("lanes"), "{e}");

        let bad2 = "version: 1\nblocks: {tokens: 16, bytes: 65536}\nseed: 7\n\
                    shared_classes: {}\nsession_classes: {}";
        assert!(bad2.parse::<WorkloadDescription>().is_err());
    }
}
