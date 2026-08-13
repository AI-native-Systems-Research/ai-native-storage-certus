//! Reading a trace, and refusing to read one wrongly (spec T081, T082).
//!
//! # Normalisation is the whole job
//!
//! `contracts/trace-io.md` admits two mutually exclusive block encodings, and they
//! disagree about the trailing partial block: the **delta** encoding excludes it and
//! the **full** encoding includes it, with `partial_final_valid` giving its valid
//! token count. A reader that assumes one convention is silently off by one block
//! per request on the other — which is not a rounding error but a systematic bias in
//! every statistic downstream.
//!
//! So the encoding is read from the manifest, never guessed, and every invocation is
//! normalised to a **full ordered block list** on ingest. Nothing past this module
//! knows which encoding the trace used, which is the only way to be sure the branch
//! has not leaked into a statistic.
//!
//! Each encoding also carries an arithmetic invariant relating its block list to
//! `input_length`, and both are *checked* rather than trusted. That check is what
//! catches a misread encoding: deriving the full encoding's rule by analogy with the
//! delta one fails on 12 009 of 12 031 rows, which is loud, whereas a reader that
//! quietly dropped a block would not be.
//!
//! # What a manifest is for
//!
//! It is the only documentation a trace has, and it exists to be consulted *before*
//! fitting. FR-055 requires `fit` to refuse a parameter whose source field is
//! `unavailable` rather than default it, so [`Capabilities`] answers that question
//! per parameter and the answer is a refusal a caller cannot ignore.

use std::collections::HashMap;
use std::path::Path;

use workload_model::keys::{CacheKey, SessionId};
use workload_model::stats::Ref;
use workload_model::trace::{Invocation, TraceManifest};

pub mod jsonl;

/// Which of the two block encodings a trace uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// `new_input_blocks` + `new_output_blocks` + `reuse_from`; the trailing partial
    /// block is **excluded**.
    Delta,
    /// `full_input_blocks` is complete and **includes** the trailing partial block.
    Full,
}

impl Encoding {
    /// The encoding the manifest declares, by which block field it calls populated.
    ///
    /// Read from `field_status` rather than inferred from which arrays happen to be
    /// non-empty: a trace whose first rows have no reuse would otherwise be
    /// classified from a sample and misread from then on.
    pub fn from_manifest(m: &TraceManifest) -> Result<Encoding, ReadError> {
        let status = |f: &str| m.field_status.get(f).map(String::as_str);
        let full = matches!(
            status("full_input_blocks"),
            Some("native" | "reconstructed")
        );
        let delta = matches!(status("new_input_blocks"), Some("native" | "reconstructed"));
        match (full, delta) {
            (true, _) => Ok(Encoding::Full),
            (false, true) => Ok(Encoding::Delta),
            (false, false) => Err(ReadError::NoBlockData {
                trace: m.trace_id.clone(),
                source_class: m.source_class.clone(),
            }),
        }
    }
}

/// Why a trace could not be read, or could not be read safely.
#[derive(Debug)]
pub enum ReadError {
    /// I/O or JSON failure.
    Io(String),
    /// The manifest is absent or malformed.
    Manifest(String),
    /// The trace carries no block data at all — a `metadata_only` source.
    ///
    /// Not a defect in the trace: arrival times and token counts are still fittable.
    /// But prefix structure is not, and saying so is better than fitting a corpus
    /// from empty block lists and reporting `roots.count: 0`.
    NoBlockData {
        /// Which trace.
        trace: String,
        /// What it says it is.
        source_class: String,
    },
    /// Fewer records were read than the manifest declares (FR-055e).
    PartialTrace {
        /// Rows consumed.
        consumed: u64,
        /// Rows the manifest says exist.
        declared: u64,
    },
    /// `id_semantics` is not `rolling_prefix`, so prefix structure is not recoverable.
    UnsupportedIdentity(String),
    /// A blocking was asked for that the trace does not carry.
    NoSuchBlocking {
        /// Tokens per block requested.
        requested: u32,
        /// What the trace carries.
        available: Vec<u32>,
    },
    /// The trace carries several blockings and declares no default.
    AmbiguousBlocking {
        /// What the trace carries.
        available: Vec<u32>,
    },
    /// A parquet trace was named by a build without the `parquet` feature.
    ///
    /// Constructed only in a build without that feature, and kept in both so the
    /// error type does not change shape with a feature flag — a caller matching on
    /// `ReadError` should compile either way.
    #[cfg_attr(feature = "parquet", allow(dead_code))]
    ParquetUnavailable {
        /// The directory.
        path: String,
    },
    /// A block list contradicted its own `input_length` under the declared encoding.
    EncodingMismatch {
        /// Which encoding was declared.
        encoding: &'static str,
        /// How many rows failed the invariant.
        rows: u64,
        /// How many rows were checked.
        checked: u64,
        /// One offending row, for a reader to look at.
        example: String,
    },
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(e) => write!(f, "{e}"),
            ReadError::Manifest(e) => write!(f, "manifest: {e}"),
            ReadError::NoBlockData {
                trace,
                source_class,
            } => write!(
                f,
                "MODEL LIMITATION, not a defect in {trace}: it is `{source_class}` and carries \
                 no block data, and this model has no way to express a workload whose prefix \
                 structure is unknown — `corpus.trees` is required and every structural \
                 parameter is derived from block lists. The trace is valid and its arrival and \
                 size distributions are measurable; what is missing is a model that can be \
                 fitted from those alone (FR-054a)"
            ),
            ReadError::PartialTrace { consumed, declared } => write!(
                f,
                "CALLER INPUT: this file holds {consumed} of the {declared} invocations its own \
                 manifest declares, so it is an excerpt of a valid trace rather than the trace. \
                 Sharing, width and reuse distance are properties of the whole stream and every \
                 one is understated by a prefix of it, so point `fit` at the full trace, or pass \
                 --allow-partial to accept a weaker claim (FR-055e)"
            ),
            ReadError::UnsupportedIdentity(s) => write!(
                f,
                "MODEL LIMITATION, not a defect in the trace: its `id_semantics` is `{s}` and \
                 this model is built on `rolling_prefix` identity (FR-008), under which a shared \
                 block id implies a shared path. Under `{s}` that implication does not hold, so \
                 the model cannot interpret the sharing the trace does have. Admitting this \
                 trace would need a corpus model whose keys are not prefix-derived (FR-054a)"
            ),
            ReadError::NoSuchBlocking {
                requested,
                available,
            } => write!(
                f,
                "CALLER INPUT: this trace carries no {requested}-token blocking; it has \
                 {available:?}. Reading a different one would answer a question that was not \
                 asked, since block size sets every depth and every path length"
            ),
            // The empty case is spelled out separately because "several blockings — `[]`
            // tokens" is a sentence that contradicts itself, and a reader who sees it has
            // no way to tell which half is wrong.
            ReadError::AmbiguousBlocking { available } if available.is_empty() => write!(
                f,
                "CALLER INPUT: this trace declares no blocking and lists none available, yet its \
                 manifest does claim populated block fields, so name one with --block-size. \
                 Choosing for you would be choosing the structure the fit measures"
            ),
            ReadError::AmbiguousBlocking { available } => write!(
                f,
                "CALLER INPUT: this trace carries several blockings — {available:?} tokens — and \
                 declares no default, so name one with --block-size. Choosing for you would be \
                 choosing the structure the fit measures"
            ),
            ReadError::ParquetUnavailable { path } => write!(
                f,
                "{path} is a directory, so it is a parquet trace, but this binary was built \
                 without the `parquet` feature. Rebuild with `--features parquet`. Not falling \
                 back to JSONL: the directory is the trace, and reading none of it would look \
                 like an empty one"
            ),
            ReadError::EncodingMismatch {
                encoding,
                rows,
                checked,
                example,
            } => write!(
                f,
                "CORRUPT TRACE — the one thing a trace can be blamed for (FR-054a): {rows} of \
                 {checked} rows contradict the length invariant of the {encoding} encoding their \
                 own manifest declares, so the block lists cannot be interpreted at all. This is \
                 self-inconsistency rather than unsuitability: the rows and the manifest \
                 disagree, and nothing here can decide which is right. First: {example}"
            ),
        }
    }
}

impl std::error::Error for ReadError {}

impl From<std::io::Error> for ReadError {
    fn from(e: std::io::Error) -> Self {
        ReadError::Io(e.to_string())
    }
}

/// What a trace's manifest says can and cannot be fitted from it (T082).
///
/// Parts of this are the FR-055 gate that `fit` consumes and `validate` does not —
/// [`Capabilities::require`] and [`Unfittable`] in particular. They are built and
/// tested here rather than alongside `fit`, because whether a parameter is fittable
/// is a property of the trace and belongs with the reader that interpreted it.
///
/// Every answer is derived from `field_status` and `source_class`. `supports` is
/// deliberately **not** consulted for anything: its `P` flag has no established
/// meaning — it correlates with neither session identity nor the presence of a
/// popularity table — and `contracts/trace-io.md` records that a reader must not
/// depend on it.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Capabilities {
    /// `source_class`, verbatim.
    pub source_class: String,
    /// The declared block encoding.
    pub encoding: Encoding,
    /// Tokens per block, for the block size read.
    pub block_size: u32,
    /// Per-field status, verbatim.
    pub field_status: HashMap<String, String>,
    /// Whether timestamps are real, so order dependence can be reported.
    pub timestamps_native: bool,
    /// Whether session identity is present. Without it there is no occupancy
    /// denominator and no cross-session sharing, so the trunk cannot be fitted.
    pub sessions_native: bool,
}

#[allow(dead_code)]
impl Capabilities {
    /// Read from a manifest.
    pub fn from_manifest(m: &TraceManifest, block_size: u32) -> Result<Capabilities, ReadError> {
        // Null rather than absent on the `metadata_only` traces, which is a trace
        // saying it has no block identity at all rather than one declining to name
        // its scheme. Both land here, and both are refusals: without prefix-derived
        // identity a shared block id does not imply a shared path.
        match m.id_semantics.as_deref() {
            Some("rolling_prefix") => {}
            Some(other) => return Err(ReadError::UnsupportedIdentity(other.to_string())),
            None => {
                return Err(ReadError::UnsupportedIdentity(
                    "null, which a trace with no block data declares".to_string(),
                ))
            }
        }
        let status = |f: &str| {
            m.field_status
                .get(f)
                .map(String::as_str)
                .unwrap_or("unavailable")
        };
        Ok(Capabilities {
            source_class: m.source_class.clone(),
            encoding: Encoding::from_manifest(m)?,
            block_size,
            timestamps_native: status("request_start") == "native",
            sessions_native: matches!(status("session_id"), "native" | "reconstructed"),
            field_status: m.field_status.clone(),
        })
    }

    /// Whether `field` is present in any form.
    pub fn has(&self, field: &str) -> bool {
        matches!(
            self.field_status.get(field).map(String::as_str),
            Some("native") | Some("reconstructed")
        )
    }

    /// Refuse to fit `parameter` when the field it comes from is unavailable.
    ///
    /// FR-055: a parameter whose source is absent must be **left unset**, never
    /// defaulted. A default here would be indistinguishable in the emitted YAML from
    /// a measurement, which is the failure the whole `field_status` mechanism exists
    /// to prevent.
    pub fn require(&self, parameter: &str, field: &str) -> Result<(), Unfittable> {
        if self.has(field) {
            Ok(())
        } else {
            Err(Unfittable {
                parameter: parameter.to_string(),
                field: field.to_string(),
                status: self
                    .field_status
                    .get(field)
                    .cloned()
                    .unwrap_or_else(|| "absent from the manifest".to_string()),
            })
        }
    }

    /// Whether the trunk can be fitted at all.
    ///
    /// Needs session identity: cross-session sharing is what distinguishes a trunk
    /// from one long private path, and occupancy — which FR-055b requires reported
    /// beside every width ratio — has no denominator without it.
    pub fn trunk_fittable(&self) -> bool {
        self.sessions_native
    }

    /// Which trace field each fitted parameter came from, and that field's status.
    ///
    /// FR-055e requires the fit report to say which fitted values came from
    /// `reconstructed` rather than `native` fields. The distinction is not a nicety:
    /// a reconstructed field is the *corpus author's* inference about their own
    /// trace, so a parameter fitted from one inherits that inference, and a reader
    /// comparing two fitted models has no way to see it from the YAML — the emitted
    /// document records values, never their pedigree.
    ///
    /// Keyed by field rather than by parameter, because the fields are few and each
    /// feeds several parameters; the inverse table would repeat every status.
    ///
    /// The block field depends on the declared [`Encoding`], so this is derived here
    /// rather than written as a constant: reporting `full_input_blocks` for a delta
    /// trace would name a field the reader never opened.
    pub fn provenance(&self) -> Vec<Provenance> {
        let block_fields: &[&str] = match self.encoding {
            Encoding::Full => &["full_input_blocks"],
            Encoding::Delta => &["new_input_blocks", "new_output_blocks"],
        };
        let mut out = Vec::new();
        for f in block_fields {
            out.push(self.provenance_of(
                f,
                "every path measurement: corpus.trees.branching, corpus.roots.count, \
                 sessions.private_depth, sessions.shared_depth, sessions.growth_per_turn",
            ));
        }
        out.push(self.provenance_of(
            "session_id",
            "everything that needs to tell two conversations apart: sessions.turns, \
             sessions.growth_per_turn, corpus.roots.popularity, and the cross-session \
             sharing behind shared_depth and trunk occupancy",
        ));
        out.push(self.provenance_of(
            "request_start",
            "sessions.think_time and arrival.rate_per_s, and the chronological order \
             every order-dependent statistic is measured in",
        ));
        out
    }

    fn provenance_of(&self, field: &str, feeds: &str) -> Provenance {
        Provenance {
            field: field.to_string(),
            status: self
                .field_status
                .get(field)
                .cloned()
                .unwrap_or_else(|| "absent from the manifest".to_string()),
            feeds: feeds.to_string(),
        }
    }
}

/// One trace field, its manifest status, and what was fitted from it (FR-055e).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The trace field.
    pub field: String,
    /// What the manifest says about it, verbatim.
    pub status: String,
    /// The fitted parameters that consumed it.
    pub feeds: String,
}

impl Provenance {
    /// Whether this field was inferred by the corpus author rather than recorded.
    pub fn is_reconstructed(&self) -> bool {
        self.status == "reconstructed"
    }
}

impl std::fmt::Display for Provenance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "`{}` is {} — it fed {}",
            self.field, self.status, self.feeds
        )?;
        if self.is_reconstructed() {
            write!(
                f,
                ". Reconstructed means the corpus author derived this field rather than \
                 recording it, so those values carry that derivation's assumptions and are \
                 not independent evidence of it"
            )?;
        }
        Ok(())
    }
}

/// A parameter that cannot be fitted, and why.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfittable {
    /// The schema parameter.
    pub parameter: String,
    /// The trace field it would have come from.
    pub field: String,
    /// What the manifest says about that field.
    pub status: String,
}

impl std::fmt::Display for Unfittable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MODEL LIMITATION (FR-054a): {} is required by the schema, and the trace does not \
             carry the field it comes from — `{}` is {}. The trace was never obliged to record \
             it, so this is the model asking for something it cannot always have. Left unset \
             rather than defaulted, so the emitted model cannot pass a guess off as a measurement",
            self.parameter, self.field, self.status
        )
    }
}

/// A trace, normalised: every invocation with its full ordered block list.
#[allow(dead_code)]
#[derive(Debug)]
pub struct Trace {
    /// What the manifest says about it.
    pub capabilities: Capabilities,
    /// The manifest itself.
    pub manifest: TraceManifest,
    /// Invocations in the order they will be consumed.
    pub invocations: Vec<NormalisedInvocation>,
    /// Whether the order is chronological or file order.
    ///
    /// FR-055d: a trace without timestamps yields order-dependent statistics, and
    /// they must be reported as order-dependent rather than as measured.
    pub chronological: bool,
}

/// One invocation after normalisation.
#[derive(Debug, Clone)]
pub struct NormalisedInvocation {
    /// Dense session index. Assigned on ingest, since a trace's session ids are
    /// opaque strings and the statistics want a dense integer.
    pub session: SessionId,
    /// 0-based turn within the session.
    pub turn: u32,
    /// Seconds from the trace origin, where the trace has real timestamps.
    pub request_start: Option<f64>,
    /// The complete ordered block list, whichever encoding it came from.
    pub blocks: Vec<CacheKey>,
}

impl Trace {
    /// The reference stream, for `workload_model::stats`.
    ///
    /// Every statistic in a fit report comes from this — the same accumulators the
    /// generator's own `report` uses (FR-021i). Two implementations would make a
    /// comparison between a fitted model and its source trace a comparison of two
    /// definitions rather than of two measurements.
    ///
    /// Sizes are the block size in *tokens*: the corpus carries no `model_config`
    /// from which KV bytes could be derived, so a byte figure here would be
    /// invented. Byte-weighted statistics over a trace are therefore in token units
    /// and the fit report says so.
    pub fn refs(&self) -> impl Iterator<Item = Ref> + '_ {
        let block_size = self.capabilities.block_size;
        self.invocations.iter().flat_map(move |inv| {
            inv.blocks.iter().enumerate().map(move |(depth, key)| Ref {
                key: *key,
                size: block_size,
                depth: depth as u32,
                session: inv.session,
                request_start: depth == 0,
                // A trace has no warmup window: the concept belongs to a measured
                // run. Treating some prefix as warmup would be inventing one.
                warmup: false,
            })
        })
    }

    /// Total block references.
    pub fn references(&self) -> u64 {
        self.invocations.iter().map(|i| i.blocks.len() as u64).sum()
    }

    /// Distinct sessions.
    pub fn sessions(&self) -> u64 {
        self.invocations
            .iter()
            .map(|i| i.session.0)
            .collect::<std::collections::BTreeSet<_>>()
            .len() as u64
    }
}

/// Read `manifest.json` from a trace directory or beside a trace file.
pub fn read_manifest(path: &Path) -> Result<TraceManifest, ReadError> {
    let candidate = if path.is_dir() {
        path.join("manifest.json")
    } else {
        path.parent()
            .unwrap_or(Path::new("."))
            .join("manifest.json")
    };
    let text = std::fs::read_to_string(&candidate)
        .map_err(|e| ReadError::Manifest(format!("{}: {e}", candidate.display())))?;
    serde_json::from_str(&text).map_err(|e| ReadError::Manifest(e.to_string()))
}

/// Read a trace from either container, picked by what the path is.
///
/// A directory is a parquet trace — `manifest.json` beside
/// `invocations/block_size_<N>/part-*.parquet` — and a file is JSONL. That is the
/// whole dispatch, and it is here rather than in either container module so that
/// "the container is not information" holds at the only place a caller enters
/// (FR-055, FR-021j): every operation accepts either, symmetric with FR-021a's
/// output modes, and nothing downstream of this function can tell which it got.
///
/// `allow_partial` exists for `validate`, which compares shapes and can legitimately
/// work from a sample, and is refused by default for `fit`, which cannot: sharing,
/// width and reuse distance are properties of the whole stream and every one of them
/// is understated by a prefix of it (FR-055e).
pub fn read_trace(
    path: &Path,
    allow_partial: bool,
    block_size: Option<u32>,
) -> Result<Trace, ReadError> {
    if path.is_dir() {
        #[cfg(feature = "parquet")]
        return crate::parquet::read_trace(path, allow_partial, block_size);
        // Not a silent fallback to JSONL: the directory *is* a trace, and reading
        // nothing from it would look like an empty one.
        #[cfg(not(feature = "parquet"))]
        return Err(ReadError::ParquetUnavailable {
            path: path.display().to_string(),
        });
    }
    jsonl::read_trace(path, allow_partial, block_size)
}

/// Everything both containers do once the rows are in hand.
///
/// Shared so that the two readers cannot disagree about normalisation, ordering or
/// the completeness check — the only difference between them is how bytes become
/// [`Invocation`] values.
pub fn assemble(
    manifest: TraceManifest,
    rows: Vec<Invocation>,
    block_size: u32,
    allow_partial: bool,
) -> Result<Trace, ReadError> {
    let caps = Capabilities::from_manifest(&manifest, block_size)?;
    if !allow_partial {
        check_complete(rows.len() as u64, &manifest, block_size)?;
    }
    let mut invocations = normalise(&rows, &caps)?;
    let chronological = order(&mut invocations, &caps);
    Ok(Trace {
        capabilities: caps,
        manifest,
        invocations,
        chronological,
    })
}

/// Which blocking of a trace to read, in tokens per block.
///
/// **A trace may carry several**, and the manifest's `block_size` is a *default*
/// rather than the answer: of 24 real traces, 3 carry four blockings each
/// (16/32/64/128 tokens) as sibling `invocations/block_size_<N>/` partitions, and
/// one of those declares no default at all. So the block size is resolved here,
/// once, from a caller's request and the manifest — never assumed from either alone.
///
/// The rules, in order:
///
/// 1. An explicit request wins, and MUST be one the trace actually carries.
///    Silently reading a different blocking than the one asked for would change
///    every depth and every path length in the fit.
/// 2. Otherwise the manifest's `block_size`.
/// 3. Otherwise, if exactly one blocking is available, that one — an unambiguous
///    trace needs no ceremony.
/// 4. Otherwise refuse and list what is available. Picking the smallest would be
///    choosing the workload's structure on the caller's behalf.
pub fn resolve_block_size(m: &TraceManifest, requested: Option<u32>) -> Result<u32, ReadError> {
    let available = |m: &TraceManifest| -> Vec<u32> {
        let mut v = m.block_sizes_available.clone();
        if let Some(b) = m.block_size {
            if !v.contains(&b) {
                v.push(b);
            }
        }
        v.sort_unstable();
        v
    };
    let avail = available(m);
    // An empty list is **not** ambiguity. It says the trace carries no blocking at all,
    // which on a `metadata_only` source means no block data — a model limitation, not a
    // flag the caller forgot (FR-054b). Checked here rather than left to `assemble`
    // because resolution runs first, and reporting it as ambiguity was wrong twice over:
    // the message contradicted itself ("several blockings — `[]` tokens") and following
    // its own advice produced a bare `No such file or directory`, the least informative
    // outcome in the taxonomy. Measured across the shipped corpus, this mislabelled 7 of
    // 24 traces as the caller's mistake rather than the model's limit — and that label is
    // exactly what a reader uses to decide what to build next.
    //
    // Exactly 7, and the boundary is worth keeping: those are the `metadata_only` sources,
    // which declare no default blocking AND list none available. `paper_review_dag` also
    // declares no default but lists four, so it stays genuine ambiguity and must still be
    // refused as CALLER INPUT — an absent default is not the discriminator, an empty list
    // of alternatives is.
    //
    // Deferring to the encoding check rather than reporting `NoBlockData` directly keeps
    // one definition of "carries block data": if the manifest does declare a populated
    // block field while listing no blocking, that is a different and rarer thing, and the
    // arms below still handle it.
    if avail.is_empty() {
        Encoding::from_manifest(m)?;
    }
    match requested {
        Some(r) if avail.is_empty() || avail.contains(&r) => Ok(r),
        Some(r) => Err(ReadError::NoSuchBlocking {
            requested: r,
            available: avail,
        }),
        None => match m.block_size {
            Some(b) => Ok(b),
            None if avail.len() == 1 => Ok(avail[0]),
            None => Err(ReadError::AmbiguousBlocking { available: avail }),
        },
    }
}

/// Reconstruct full block lists and check the encoding's own length invariant.
///
/// Returns the normalised invocations in file order, plus how many rows contradicted
/// the invariant.
pub fn normalise(
    rows: &[Invocation],
    caps: &Capabilities,
) -> Result<Vec<NormalisedInvocation>, ReadError> {
    let block_size = caps.block_size.max(1) as i64;
    // Sessions become dense indices; an absent session id makes every row its own
    // session, which is what `session_id: unavailable` means for sharing purposes.
    let mut session_index: HashMap<String, u32> = HashMap::new();
    let mut next_session = 0u32;

    // Delta reconstruction addresses a session's earlier invocations by index.
    let mut by_session: HashMap<&str, HashMap<i64, &Invocation>> = HashMap::new();
    if caps.encoding == Encoding::Delta {
        for r in rows {
            let key = r.session_id.as_deref().unwrap_or("");
            by_session
                .entry(key)
                .or_default()
                .insert(r.invocation_index, r);
        }
    }

    let mut out = Vec::with_capacity(rows.len());
    let mut bad = 0u64;
    let mut checked = 0u64;
    let mut example = String::new();

    for r in rows {
        let session_key = match r.session_id.as_deref() {
            Some(s) => s.to_string(),
            // No session identity: one session per row rather than one session for
            // the whole trace, since the latter would invent sharing that the trace
            // does not evidence.
            None => format!("__row{}", out.len()),
        };
        let session = *session_index.entry(session_key.clone()).or_insert_with(|| {
            let s = next_session;
            next_session += 1;
            s
        });

        let blocks: Vec<i64> = match caps.encoding {
            Encoding::Full => r.full_input_blocks.clone(),
            Encoding::Delta => {
                let session_rows = by_session
                    .get(r.session_id.as_deref().unwrap_or(""))
                    .ok_or_else(|| ReadError::Io("session vanished during grouping".into()))?;
                let mut acc = Vec::new();
                let mut truncated = false;
                for ancestor in &r.reuse_from {
                    match session_rows.get(ancestor) {
                        Some(a) => {
                            acc.extend_from_slice(&a.new_input_blocks);
                            acc.extend_from_slice(&a.new_output_blocks);
                        }
                        // A truncated read can cut a session's head off. The row's
                        // blocks would then start mid-path and sit at the wrong
                        // depths, so it is dropped rather than misplaced.
                        None => {
                            truncated = true;
                            break;
                        }
                    }
                }
                if truncated {
                    continue;
                }
                acc.extend_from_slice(&r.new_input_blocks);
                acc
            }
        };

        // The invariant each encoding carries, checked rather than trusted.
        if r.input_length > 0 {
            checked += 1;
            let expected = match caps.encoding {
                Encoding::Delta => r.input_length / block_size,
                Encoding::Full => (r.input_length - r.partial_final_valid) / block_size + 1,
            };
            if expected != blocks.len() as i64 {
                bad += 1;
                if example.is_empty() {
                    example = format!(
                        "session {:?} invocation {}: {} blocks, input_length {} implies {}",
                        r.session_id,
                        r.invocation_index,
                        blocks.len(),
                        r.input_length,
                        expected
                    );
                }
            }
        }

        out.push(NormalisedInvocation {
            session: SessionId(session),
            turn: r.invocation_index.max(0) as u32,
            request_start: r.request_start,
            blocks: blocks.into_iter().map(|b| CacheKey(b as u64)).collect(),
        });
    }

    // A handful of violations is a damaged row; a large share is a misread encoding,
    // and the difference is what the threshold distinguishes. Both are reported, and
    // the second refuses.
    if checked > 0 && bad * 100 > checked {
        return Err(ReadError::EncodingMismatch {
            encoding: match caps.encoding {
                Encoding::Delta => "delta",
                Encoding::Full => "full",
            },
            rows: bad,
            checked,
            example,
        });
    }
    Ok(out)
}

/// Order invocations chronologically where the trace has real timestamps.
///
/// Returns whether the order is chronological. FR-055d: where it is not, every
/// order-dependent statistic must be reported as order-dependent.
pub fn order(invocations: &mut [NormalisedInvocation], caps: &Capabilities) -> bool {
    if !caps.timestamps_native {
        return false;
    }
    if invocations.iter().any(|i| i.request_start.is_none()) {
        return false;
    }
    let distinct: std::collections::BTreeSet<u64> = invocations
        .iter()
        .map(|i| (i.request_start.unwrap_or(0.0) * 1e6) as u64)
        .collect();
    // A single timestamp for every row is a placeholder, not a measurement.
    if distinct.len() <= 1 {
        return false;
    }
    invocations.sort_by(|a, b| {
        a.request_start
            .partial_cmp(&b.request_start)
            .unwrap_or(std::cmp::Ordering::Equal)
            // Ties break on session then turn, so the order is total and a fit is
            // reproducible across reads.
            .then(a.session.0.cmp(&b.session.0))
            .then(a.turn.cmp(&b.turn))
    });
    true
}

/// Refuse a trace read short of what its manifest declares (FR-055e).
pub fn check_complete(
    consumed: u64,
    manifest: &TraceManifest,
    block_size: u32,
) -> Result<(), ReadError> {
    let declared = manifest
        .block_stats
        .get(&block_size.to_string())
        .map(|s| s.invocations);
    match declared {
        // A manifest that declares no count for this block size cannot support the
        // check; that is a gap in the trace, not licence to assume completeness, so
        // it is reported by the caller rather than silently passed here.
        None => Ok(()),
        Some(declared) if consumed >= declared => Ok(()),
        Some(declared) => Err(ReadError::PartialTrace { consumed, declared }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use workload_model::trace::BlockStats;

    fn manifest(encoding: Encoding, invocations: u64) -> TraceManifest {
        let mut m = TraceManifest::synthetic(
            "t",
            16,
            BlockStats {
                sessions: 1,
                invocations,
                unique_blocks: 1,
            },
        );
        if encoding == Encoding::Delta {
            m.field_status
                .insert("full_input_blocks".into(), "unavailable".into());
            m.field_status
                .insert("new_input_blocks".into(), "reconstructed".into());
            m.field_status
                .insert("new_output_blocks".into(), "reconstructed".into());
            m.field_status
                .insert("reuse_from".into(), "reconstructed".into());
        }
        m
    }

    fn row(session: &str, index: i64) -> Invocation {
        Invocation {
            trace_id: "t".into(),
            session_id: Some(session.into()),
            invocation_index: index,
            parent_invocation: index - 1,
            parent_invocations: vec![],
            request_start: Some(index as f64),
            request_end: None,
            timestamp_kind: "start".into(),
            timestamp_is_synthetic: true,
            model: None,
            input_length: 0,
            output_length: 0,
            reuse_from: vec![],
            new_input_blocks: vec![],
            new_output_blocks: vec![],
            full_input_blocks: vec![],
            full_output_blocks: vec![],
            partial_final_valid: 0,
        }
    }

    #[test]
    fn the_encoding_comes_from_the_manifest_not_from_the_rows() {
        // Inferring it from which arrays are non-empty would classify a trace from
        // whatever its first rows happen to look like.
        assert_eq!(
            Encoding::from_manifest(&manifest(Encoding::Full, 1)).unwrap(),
            Encoding::Full
        );
        assert_eq!(
            Encoding::from_manifest(&manifest(Encoding::Delta, 1)).unwrap(),
            Encoding::Delta
        );
    }

    #[test]
    fn a_metadata_only_trace_is_refused_with_its_source_class_named() {
        let mut m = manifest(Encoding::Full, 1);
        m.source_class = "metadata_only".into();
        for f in ["full_input_blocks", "new_input_blocks"] {
            m.field_status.insert(f.into(), "unavailable".into());
        }
        let e = Encoding::from_manifest(&m).expect_err("must refuse");
        let msg = e.to_string();
        assert!(msg.contains("metadata_only"), "{msg}");
        // FR-054a: a trace this model cannot fit is a limitation of the model, and the
        // message must say so rather than reading as a verdict on the data. Asserted on
        // the classification rather than the prose, so a reword cannot quietly turn it
        // back into "this trace is unsuitable".
        assert!(msg.contains("MODEL LIMITATION"), "{msg}");
        assert!(
            msg.contains("The trace is valid"),
            "the trace is valid and its arrival and size distributions are measurable: {msg}"
        );
    }

    /// A `metadata_only` manifest: no block data, and no blocking to name.
    fn metadata_only() -> workload_model::trace::TraceManifest {
        let mut m = manifest(Encoding::Full, 1);
        m.source_class = "metadata_only".into();
        m.block_size = None;
        m.block_sizes_available = vec![];
        m.block_stats = Default::default();
        for f in ["full_input_blocks", "new_input_blocks"] {
            m.field_status.insert(f.into(), "unavailable".into());
        }
        m
    }

    #[test]
    fn a_trace_with_no_blocking_at_all_is_a_model_limitation_not_a_missing_flag() {
        // Regression, measured across the shipped corpus: 7 of 24 traces reported
        // "this trace carries several blockings — [] tokens — name one with --block-size".
        // That sentence contradicts itself, and the advice in it produced a bare
        // `No such file or directory`. An empty list of blockings is not ambiguity; on a
        // metadata_only source it means no block data, which FR-054b classifies as a
        // limit of the model rather than the caller's mistake.
        let m = metadata_only();
        let e = resolve_block_size(&m, None).expect_err("must refuse");
        let msg = e.to_string();
        assert!(msg.contains("MODEL LIMITATION"), "{msg}");
        assert!(!msg.contains("several blockings"), "{msg}");
        assert!(
            !msg.contains("--block-size"),
            "must not send the reader after a flag that cannot help: {msg}"
        );
    }

    #[test]
    fn naming_a_blocking_on_a_trace_that_has_none_is_refused_the_same_way() {
        // The old code accepted any requested size here and failed later opening a
        // partition that cannot exist, so the outcome depended on whether the reader had
        // taken the message's advice yet. Both paths must give the same classification.
        let m = metadata_only();
        let msg = resolve_block_size(&m, Some(16))
            .expect_err("must refuse")
            .to_string();
        assert!(msg.contains("MODEL LIMITATION"), "{msg}");
        assert!(msg.contains("metadata_only"), "{msg}");
    }

    #[test]
    fn a_trace_that_does_carry_block_data_still_resolves_its_blocking() {
        // The guard above must not swallow the ordinary cases: one declared blocking
        // resolves, a request for one the trace lacks is still CALLER INPUT.
        let m = manifest(Encoding::Full, 1);
        assert_eq!(resolve_block_size(&m, None).expect("declared"), 16);
        assert_eq!(resolve_block_size(&m, Some(16)).expect("requested"), 16);
        let msg = resolve_block_size(&m, Some(999))
            .expect_err("must refuse")
            .to_string();
        assert!(msg.contains("CALLER INPUT"), "{msg}");
    }

    #[test]
    fn no_default_blocking_but_several_available_is_still_caller_input() {
        // The `paper_review_dag` shape, and the boundary the empty-list guard must not
        // cross. An absent default is NOT what makes a trace a model limitation — an empty
        // list of alternatives is. This trace has four, so naming one is genuinely the
        // caller's to do, and the guard swallowing it would send a reader looking for
        // missing block data that is right there.
        let mut m = manifest(Encoding::Full, 1);
        m.block_size = None;
        m.block_sizes_available = vec![16, 32, 64, 128];
        let msg = resolve_block_size(&m, None)
            .expect_err("four blockings and no default is ambiguous")
            .to_string();
        assert!(msg.contains("CALLER INPUT"), "{msg}");
        assert!(msg.contains("several blockings"), "{msg}");
        assert!(msg.contains("--block-size"), "{msg}");
        // And naming one of them resolves.
        assert_eq!(resolve_block_size(&m, Some(64)).expect("named"), 64);
    }

    #[test]
    fn a_trace_declaring_block_data_but_listing_no_blocking_gets_the_empty_case_message() {
        // Keeps the separate empty-list arm honest: it is reachable only here, where the
        // manifest claims a populated block field yet lists no blocking to read it at.
        // That is self-contradictory on the trace's part rather than a model limit, so it
        // stays CALLER INPUT — but it must not be described as "several blockings — []".
        let mut m = manifest(Encoding::Full, 1);
        m.block_size = None;
        m.block_sizes_available = vec![];
        let msg = resolve_block_size(&m, None)
            .expect_err("nothing to read at")
            .to_string();
        assert!(msg.contains("CALLER INPUT"), "{msg}");
        assert!(!msg.contains("several blockings"), "{msg}");
        assert!(msg.contains("declares no blocking"), "{msg}");
    }

    #[test]
    fn the_full_encoding_keeps_the_trailing_partial_block() {
        // 3 whole blocks of 16 tokens plus a partial with 5 valid: input_length 53,
        // four blocks in the list. The invariant is
        // (53 - 5)/16 + 1 = 4.
        let caps = Capabilities::from_manifest(&manifest(Encoding::Full, 1), 16).unwrap();
        let mut r = row("s", 0);
        r.input_length = 53;
        r.partial_final_valid = 5;
        r.full_input_blocks = vec![10, 11, 12, 13];
        let out = normalise(&[r], &caps).expect("invariant holds");
        assert_eq!(out[0].blocks.len(), 4);
    }

    #[test]
    fn the_delta_encoding_excludes_it_and_reconstructs_from_ancestors() {
        // Turn 1 re-reads turn 0's input and output, then adds its own input.
        // input_length 64 over a block size of 16 implies exactly 4 blocks.
        let caps = Capabilities::from_manifest(&manifest(Encoding::Delta, 2), 16).unwrap();
        let mut a = row("s", 0);
        a.new_input_blocks = vec![1, 2];
        a.new_output_blocks = vec![3];
        let mut b = row("s", 1);
        b.reuse_from = vec![0];
        b.new_input_blocks = vec![4];
        b.input_length = 64;
        let out = normalise(&[a, b], &caps).expect("invariant holds");
        assert_eq!(
            out[1].blocks,
            vec![CacheKey(1), CacheKey(2), CacheKey(3), CacheKey(4)]
        );
    }

    #[test]
    fn applying_the_wrong_encodings_invariant_is_refused_loudly() {
        // The trap this check exists for: the full encoding's rule derived by
        // analogy with the delta one fails on almost every row, and a reader that
        // quietly dropped a block would not have noticed.
        let caps = Capabilities::from_manifest(&manifest(Encoding::Full, 1), 16).unwrap();
        let rows: Vec<Invocation> = (0..200)
            .map(|i| {
                let mut r = row("s", i);
                r.input_length = 53;
                r.partial_final_valid = 5;
                // One block short of what the invariant requires.
                r.full_input_blocks = vec![10, 11, 12];
                r
            })
            .collect();
        let e = normalise(&rows, &caps).expect_err("must refuse");
        let msg = e.to_string();
        assert!(
            msg.contains("contradict the length invariant of the full encoding"),
            "{msg}"
        );
        // The ONE case FR-054a lets us blame the trace for, and it must name itself as
        // such: the rows contradict the manifest, which is self-inconsistency rather
        // than the trace being unsuitable for this model.
        assert!(msg.contains("CORRUPT TRACE"), "{msg}");
        assert!(msg.contains("self-inconsistency"), "{msg}");
    }

    #[test]
    fn a_partial_trace_is_refused_naming_both_counts() {
        let m = manifest(Encoding::Full, 10_000);
        let e = check_complete(2_500, &m, 16).expect_err("must refuse");
        let msg = e.to_string();
        assert!(msg.contains("2500 of the 10000"), "{msg}");
        assert!(msg.contains("FR-055e"), "{msg}");
        // Neither the model's fault nor the trace's: the wrong file was named.
        assert!(msg.contains("CALLER INPUT"), "{msg}");
        assert!(check_complete(10_000, &m, 16).is_ok());
    }

    #[test]
    fn an_unavailable_field_makes_its_parameter_unfittable_rather_than_defaulted() {
        // FR-055. `synthetic` names `reuse_from` unavailable, so anything derived
        // from it must come back as a refusal.
        let caps = Capabilities::from_manifest(&manifest(Encoding::Full, 1), 16).unwrap();
        let e = caps
            .require("corpus.trees.churn", "reuse_from")
            .expect_err("must refuse");
        assert_eq!(e.parameter, "corpus.trees.churn");
        assert!(e.to_string().contains("Left unset rather than defaulted"));
        assert!(caps
            .require("workload.sessions.turns", "session_id")
            .is_ok());
    }

    #[test]
    fn an_unsupported_identity_scheme_is_refused() {
        let mut m = manifest(Encoding::Full, 1);
        m.id_semantics = Some("opaque".into());
        let e = Capabilities::from_manifest(&m, 16).expect_err("must refuse");
        assert!(e.to_string().contains("rolling_prefix"), "{e}");
        // The model is built on prefix-derived identity; a trace that uses another
        // scheme is not defective (FR-054a).
        assert!(e.to_string().contains("MODEL LIMITATION"), "{e}");
    }

    #[test]
    fn a_trace_without_session_identity_cannot_have_its_trunk_fitted() {
        // Occupancy has no denominator and cross-session sharing is invisible, which
        // is `supports: R = partial` in the manifest's own terms.
        let mut m = manifest(Encoding::Full, 1);
        m.field_status
            .insert("session_id".into(), "unavailable".into());
        let caps = Capabilities::from_manifest(&m, 16).unwrap();
        assert!(!caps.trunk_fittable());
    }

    #[test]
    fn rows_without_session_identity_become_one_session_each() {
        // The alternative — one session for the whole trace — would invent sharing
        // the trace does not evidence.
        let mut m = manifest(Encoding::Full, 2);
        m.field_status
            .insert("session_id".into(), "unavailable".into());
        let caps = Capabilities::from_manifest(&m, 16).unwrap();
        let rows: Vec<Invocation> = (0..3)
            .map(|i| {
                let mut r = row("ignored", i);
                r.session_id = None;
                r.full_input_blocks = vec![1, 2];
                r
            })
            .collect();
        let out = normalise(&rows, &caps).unwrap();
        let sessions: std::collections::BTreeSet<u32> = out.iter().map(|i| i.session.0).collect();
        assert_eq!(sessions.len(), 3);
    }

    #[test]
    fn ordering_is_chronological_only_where_timestamps_are_real() {
        let caps = Capabilities::from_manifest(&manifest(Encoding::Full, 3), 16).unwrap();
        let mut invocations = vec![
            NormalisedInvocation {
                session: SessionId(0),
                turn: 0,
                request_start: Some(30.0),
                blocks: vec![CacheKey(1)],
            },
            NormalisedInvocation {
                session: SessionId(1),
                turn: 0,
                request_start: Some(10.0),
                blocks: vec![CacheKey(2)],
            },
        ];
        assert!(order(&mut invocations, &caps));
        assert_eq!(invocations[0].request_start, Some(10.0));

        // Every row at the same instant is a placeholder, not a measurement.
        let mut flat = vec![
            NormalisedInvocation {
                session: SessionId(0),
                turn: 0,
                request_start: Some(0.0),
                blocks: vec![CacheKey(1)],
            },
            NormalisedInvocation {
                session: SessionId(1),
                turn: 0,
                request_start: Some(0.0),
                blocks: vec![CacheKey(2)],
            },
        ];
        assert!(!order(&mut flat, &caps));
    }

    #[test]
    fn the_reference_stream_puts_blocks_at_their_list_position() {
        // Depth *is* the ordinal within the request, which is what lets the trace
        // and a plan be compared by the same accumulators.
        let caps = Capabilities::from_manifest(&manifest(Encoding::Full, 1), 16).unwrap();
        let trace = Trace {
            capabilities: caps,
            manifest: manifest(Encoding::Full, 1),
            invocations: vec![NormalisedInvocation {
                session: SessionId(4),
                turn: 0,
                request_start: None,
                blocks: vec![CacheKey(7), CacheKey(8), CacheKey(9)],
            }],
            chronological: false,
        };
        let refs: Vec<Ref> = trace.refs().collect();
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].depth, 0);
        assert!(refs[0].request_start);
        assert_eq!(refs[2].depth, 2);
        assert!(!refs[2].request_start);
        assert!(refs.iter().all(|r| r.session == SessionId(4)));
        assert!(refs.iter().all(|r| !r.warmup), "a trace has no warmup");
        assert_eq!(refs[0].size, 16, "sizes are tokens, the block size");
        assert_eq!(trace.references(), 3);
    }

    #[test]
    fn provenance_names_the_block_field_the_reader_actually_opened() {
        // Reporting `full_input_blocks` for a delta trace would attach a status to a
        // field no measurement went near, which is worse than saying nothing: it
        // reads as a claim about where the values came from.
        let full = Capabilities::from_manifest(&manifest(Encoding::Full, 1), 16).unwrap();
        let fields: Vec<String> = full.provenance().iter().map(|p| p.field.clone()).collect();
        assert!(
            fields.contains(&"full_input_blocks".to_string()),
            "{fields:?}"
        );
        assert!(
            !fields.contains(&"new_input_blocks".to_string()),
            "a full-encoding trace's deltas were never read: {fields:?}"
        );

        let delta = Capabilities::from_manifest(&manifest(Encoding::Delta, 1), 16).unwrap();
        let fields: Vec<String> = delta.provenance().iter().map(|p| p.field.clone()).collect();
        assert!(
            fields.contains(&"new_input_blocks".to_string()),
            "{fields:?}"
        );
        assert!(
            fields.contains(&"new_output_blocks".to_string()),
            "{fields:?}"
        );
        assert!(
            !fields.contains(&"full_input_blocks".to_string()),
            "{fields:?}"
        );

        // Both shapes must always account for the two non-block fields, or a
        // reconstructed session id could be fitted from and never mentioned.
        for caps in [&full, &delta] {
            let fields: Vec<String> = caps.provenance().iter().map(|p| p.field.clone()).collect();
            assert!(fields.contains(&"session_id".to_string()), "{fields:?}");
            assert!(fields.contains(&"request_start".to_string()), "{fields:?}");
        }
    }

    #[test]
    fn a_reconstructed_field_is_flagged_and_says_what_it_fed() {
        // The delta fixture's block fields are reconstructed, which is the real
        // corpus's common case — so the status must survive to the report rather
        // than being flattened into "present".
        let caps = Capabilities::from_manifest(&manifest(Encoding::Delta, 1), 16).unwrap();
        let p = caps.provenance();
        let blocks = p
            .iter()
            .find(|p| p.field == "new_input_blocks")
            .expect("read");
        assert!(blocks.is_reconstructed());
        assert!(
            blocks.feeds.contains("branching") && blocks.feeds.contains("shared_depth"),
            "a flag with no consequence named is not provenance: {}",
            blocks.feeds
        );
        let msg = blocks.to_string();
        assert!(msg.contains("reconstructed"), "{msg}");
        assert!(
            msg.contains("rather than recording it"),
            "the report must say what reconstructed means, since the YAML cannot: {msg}"
        );

        // `session_id` is native in the fixture, so it must not be flagged.
        let s = p.iter().find(|p| p.field == "session_id").expect("read");
        assert!(!s.is_reconstructed(), "status was {}", s.status);
    }

    #[test]
    fn a_field_missing_from_the_manifest_is_reported_as_missing_not_as_native() {
        // Defaulting an absent field to "native" would be the one wrong answer here:
        // it claims evidence for a value nothing supports.
        let mut m = manifest(Encoding::Full, 1);
        m.field_status.remove("request_start");
        let caps = Capabilities::from_manifest(&m, 16).unwrap();
        let p = caps.provenance();
        let ts = p.iter().find(|p| p.field == "request_start").expect("read");
        assert_eq!(ts.status, "absent from the manifest");
        assert!(!ts.is_reconstructed());
    }
}
