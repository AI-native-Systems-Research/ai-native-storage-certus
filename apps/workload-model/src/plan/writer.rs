//! Writing a plan: `events.bin`, `manifest.json`, and the invariants a reader
//! is entitled to assume.
//!
//! The writer is where the format's promises are **enforced rather than
//! documented**. `contracts/plan-format.md` tells a consumer it may batch a
//! request by scanning forward to `REQUEST_END` without buffering unboundedly,
//! and may consume the stream as a schedule because `t_ns` never goes backwards.
//! Both are things a consumer cannot check cheaply and would be broken by
//! silently, so every event passes through [`PlanWriter::push`] and a violation
//! is an error at write time — the only point where the cost of the check is paid
//! once rather than by every reader.
//!
//! ## The content hash
//!
//! Identity is over the **normalised input and the realised events together**
//! (FR-026): `blake3(tag || normalised_yaml || blake3(events.bin))`. Nesting the
//! event hash rather than concatenating the two streams means the writer can hash
//! events as they go without holding the plan, and the yaml — which is only
//! complete once defaults are resolved — is folded in at the end. The tag keeps
//! this from ever colliding with a bare hash of either part.
//!
//! An **unbounded** run has no events.bin to hash, so it carries a parameter hash
//! instead ([`unbounded_manifest`]) and says so.

use std::io::Write;
use std::path::Path;

use crate::plan::digest::{parameter_hash, StreamDigest};
use crate::plan::generate::Generator;
use crate::plan::manifest::{CorpusSummary, Identity, Manifest, PLAN_FORMAT};
use crate::plan::record::{flags, PlanEvent, RECORD_BYTES};

/// Version string written into every manifest.
pub const GENERATOR_VERSION: &str = concat!("certus-workload ", env!("CARGO_PKG_VERSION"));

/// Domain separator for the plan content hash.
const CONTENT_TAG: &[u8] = b"certus-plan-content-v1";

/// A plan that would have broken a promise the format makes to its readers.
#[derive(Debug)]
pub enum WriteError {
    /// `t_ns` decreased. The runner consumes the plan as a schedule, so a
    /// backwards step is not a reordering but a different artifact.
    TimeWentBackwards {
        /// The last timestamp written.
        previous: u64,
        /// The one offered.
        found: u64,
    },
    /// A second request's events arrived before the open one ended. A consumer
    /// scanning to `REQUEST_END` would attribute this one's keys to that one.
    RequestInterleaved {
        /// The request still open.
        open: u32,
        /// The one that interrupted it.
        found: u32,
    },
    /// `request_id` did not ascend.
    RequestIdWentBackwards {
        /// The last one written.
        previous: u32,
        /// The one offered.
        found: u32,
    },
    /// An event arrived with no request open and without `REQUEST_START`.
    UnopenedRequest {
        /// Which request it claimed to belong to.
        request_id: u32,
    },
    /// `REQUEST_START` inside an already-open request.
    NestedRequest {
        /// The request still open.
        open: u32,
    },
    /// Keys were not in path order: `depth` must equal the key's ordinal within
    /// its request, which is the whole reason `depth` can be stored rather than
    /// recovered by scanning back to `REQUEST_START`.
    DepthOutOfOrder {
        /// Which request.
        request_id: u32,
        /// The ordinal reached.
        expected: u32,
        /// What the event claimed.
        found: u32,
    },
    /// A reserved flag bit was set.
    ReservedFlag {
        /// The offending flags byte.
        flags: u8,
    },
    /// The stream ended mid-request.
    UnterminatedRequest {
        /// Which request was left open.
        request_id: u32,
    },
    /// A plan file cannot be written for a run with no end.
    UnboundedPlan,
    /// The sink failed.
    Io(std::io::Error),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteError::TimeWentBackwards { previous, found } => write!(
                f,
                "t_ns went backwards ({previous} then {found}); the plan is consumed as a \
                 schedule, so this is a different artifact rather than a reordering of this one"
            ),
            WriteError::RequestInterleaved { open, found } => write!(
                f,
                "request {found} interrupted request {open}; a consumer batching by scanning to \
                 REQUEST_END would attribute one request's keys to the other"
            ),
            WriteError::RequestIdWentBackwards { previous, found } => {
                write!(f, "request_id must ascend; {previous} then {found}")
            }
            WriteError::UnopenedRequest { request_id } => write!(
                f,
                "request {request_id} has no REQUEST_START, so a consumer has no point to batch from"
            ),
            WriteError::NestedRequest { open } => {
                write!(f, "REQUEST_START inside open request {open}")
            }
            WriteError::DepthOutOfOrder {
                request_id,
                expected,
                found,
            } => write!(
                f,
                "request {request_id} is not in path order: expected depth {expected}, found \
                 {found}. depth is stored precisely because it equals the ordinal, so that a \
                 reader indexing by ordinal need not scan back to REQUEST_START"
            ),
            WriteError::ReservedFlag { flags } => write!(
                f,
                "flags {flags:#04x} sets a reserved bit; those exist so a future flag can be \
                 added without moving a field, which requires them to be zero now"
            ),
            WriteError::UnterminatedRequest { request_id } => write!(
                f,
                "the plan ended inside request {request_id}; half a request is not a request"
            ),
            WriteError::UnboundedPlan => write!(
                f,
                "an unbounded run has no events.bin to write: nothing accumulates, which is the \
                 point of it (FR-021e). Its identity is the parameter hash over the normalised \
                 YAML, seed and plan_format (FR-021g)"
            ),
            WriteError::Io(e) => write!(f, "writing the plan: {e}"),
        }
    }
}

impl std::error::Error for WriteError {}

impl From<std::io::Error> for WriteError {
    fn from(e: std::io::Error) -> Self {
        WriteError::Io(e)
    }
}

/// What a completed write realised.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStats {
    /// Events written.
    pub event_count: u64,
    /// Requests written.
    pub request_count: u64,
    /// Payload bytes referenced.
    pub total_bytes: u64,
    /// First timestamp.
    pub time_origin_ns: u64,
    /// Span from the first timestamp to the last.
    pub duration_ns: u64,
    /// `blake3:` hash over the normalised input and the events together.
    pub content_hash: String,
    /// `blake3:` rolling hash over `(key)` in event order, which both executors
    /// recompute as they consume so that two arms can be *proven* equal.
    pub stream_digest: String,
}

/// The request currently open.
#[derive(Debug, Clone, Copy)]
struct Open {
    request_id: u32,
    next_depth: u32,
}

/// Writes `events.bin`, checking every promise the format makes.
///
/// ```
/// use workload_model::plan::writer::PlanWriter;
/// use workload_model::plan::{flags, PlanEvent};
/// use workload_model::keys::{CacheKey, SessionId};
/// let mut w = PlanWriter::new(Vec::new());
/// let e = PlanEvent {
///     t_ns: 0, key: CacheKey(1), size: 8, request_id: 0, session_id: SessionId(0),
///     depth: 0, turn: 1, node: 0, mix_index: 0,
///     flags: flags::REQUEST_START | flags::REQUEST_END,
/// };
/// w.push(&e).unwrap();
/// let (bytes, stats) = w.finish("version: 1\n").unwrap();
/// assert_eq!(bytes.len(), 40);
/// assert_eq!(stats.event_count, 1);
/// ```
#[derive(Debug)]
pub struct PlanWriter<W: Write> {
    out: W,
    events: blake3::Hasher,
    digest: StreamDigest,
    event_count: u64,
    request_count: u64,
    total_bytes: u64,
    first_t_ns: Option<u64>,
    last_t_ns: u64,
    last_request_id: Option<u32>,
    open: Option<Open>,
}

impl<W: Write> PlanWriter<W> {
    /// A writer over `out`.
    pub fn new(out: W) -> PlanWriter<W> {
        PlanWriter {
            out,
            events: blake3::Hasher::new(),
            digest: StreamDigest::new(),
            event_count: 0,
            request_count: 0,
            total_bytes: 0,
            first_t_ns: None,
            last_t_ns: 0,
            last_request_id: None,
            open: None,
        }
    }

    /// Write one event, or refuse it.
    pub fn push(&mut self, e: &PlanEvent) -> Result<(), WriteError> {
        if e.flags & flags::RESERVED_MASK != 0 {
            return Err(WriteError::ReservedFlag { flags: e.flags });
        }
        if self.first_t_ns.is_some() && e.t_ns < self.last_t_ns {
            return Err(WriteError::TimeWentBackwards {
                previous: self.last_t_ns,
                found: e.t_ns,
            });
        }
        match self.open {
            Some(open) => {
                if e.has(flags::REQUEST_START) {
                    return Err(WriteError::NestedRequest {
                        open: open.request_id,
                    });
                }
                if e.request_id != open.request_id {
                    return Err(WriteError::RequestInterleaved {
                        open: open.request_id,
                        found: e.request_id,
                    });
                }
                if e.depth != open.next_depth {
                    return Err(WriteError::DepthOutOfOrder {
                        request_id: e.request_id,
                        expected: open.next_depth,
                        found: e.depth,
                    });
                }
            }
            None => {
                if !e.has(flags::REQUEST_START) {
                    return Err(WriteError::UnopenedRequest {
                        request_id: e.request_id,
                    });
                }
                if let Some(prev) = self.last_request_id {
                    if e.request_id <= prev {
                        return Err(WriteError::RequestIdWentBackwards {
                            previous: prev,
                            found: e.request_id,
                        });
                    }
                }
                if e.depth != 0 {
                    return Err(WriteError::DepthOutOfOrder {
                        request_id: e.request_id,
                        expected: 0,
                        found: e.depth,
                    });
                }
                self.request_count += 1;
            }
        }
        self.out.write_all(&e.encode())?;
        self.events.update(&e.encode());
        self.digest.push(e.key);
        self.event_count += 1;
        self.total_bytes += u64::from(e.size);
        self.first_t_ns.get_or_insert(e.t_ns);
        self.last_t_ns = e.t_ns;
        if e.has(flags::REQUEST_END) {
            self.last_request_id = Some(e.request_id);
            self.open = None;
        } else {
            self.open = Some(Open {
                request_id: e.request_id,
                next_depth: e.depth + 1,
            });
        }
        Ok(())
    }

    /// Write a whole chunk.
    pub fn push_all(&mut self, chunk: &[PlanEvent]) -> Result<(), WriteError> {
        for e in chunk {
            self.push(e)?;
        }
        Ok(())
    }

    /// Events written so far.
    pub fn event_count(&self) -> u64 {
        self.event_count
    }

    /// Finish, returning the sink and what was realised.
    pub fn finish(mut self, normalised_yaml: &str) -> Result<(W, PlanStats), WriteError> {
        if let Some(open) = self.open {
            return Err(WriteError::UnterminatedRequest {
                request_id: open.request_id,
            });
        }
        self.out.flush()?;
        let mut h = blake3::Hasher::new();
        h.update(CONTENT_TAG);
        h.update(normalised_yaml.as_bytes());
        h.update(self.events.finalize().as_bytes());
        let origin = self.first_t_ns.unwrap_or(0);
        let stats = PlanStats {
            event_count: self.event_count,
            request_count: self.request_count,
            total_bytes: self.total_bytes,
            time_origin_ns: origin,
            duration_ns: self.last_t_ns.saturating_sub(origin),
            content_hash: format!("blake3:{}", h.finalize().to_hex()),
            stream_digest: self.digest.finish(),
        };
        Ok((self.out, stats))
    }
}

/// Generate a whole bounded plan into `dir`, writing `events.bin` and
/// `manifest.json`.
///
/// The plan is generated **once** and distributed; every node verifies the
/// content hash before executing its slice, rather than generating its own copy,
/// which would require pinning floating-point behaviour across compilers and
/// CPUs (`contracts/plan-format.md` § Determinism).
pub fn write_plan(
    dir: &Path,
    g: &mut Generator,
    normalised_yaml: &str,
) -> Result<Manifest, WriteError> {
    if g.budget().is_unbounded() {
        return Err(WriteError::UnboundedPlan);
    }
    std::fs::create_dir_all(dir)?;
    let events_path = dir.join("events.bin");
    let file = std::fs::File::create(&events_path)?;
    // A buffer of one horizon's worth, so the write pattern matches the
    // generation pattern instead of issuing a syscall per record.
    let mut w = PlanWriter::new(std::io::BufWriter::with_capacity(
        g.horizon().bytes.clamp(RECORD_BYTES, 1 << 20),
        file,
    ));
    let mut buf: Vec<PlanEvent> = Vec::with_capacity(g.horizon().events);
    while g.fill(&mut buf) > 0 {
        w.push_all(&buf)?;
    }
    let (inner, stats) = w.finish(normalised_yaml)?;
    inner.into_inner().map_err(|e| WriteError::Io(e.into()))?;
    let manifest = Manifest {
        plan_format: PLAN_FORMAT,
        generator_version: GENERATOR_VERSION.to_string(),
        identity: Identity::ContentHash(stats.content_hash.clone()),
        seed: g.document().seed,
        normalised_yaml: normalised_yaml.to_string(),
        event_count: Some(stats.event_count),
        time_origin_ns: stats.time_origin_ns,
        duration_ns: Some(stats.duration_ns),
        corpus_summary: summary(g, &stats),
        stream_digest: stats.stream_digest,
    };
    std::fs::write(dir.join("manifest.json"), manifest.to_json_or_panic())?;
    Ok(manifest)
}

/// The manifest an **unbounded** run carries.
///
/// There is no `events.bin`, and that is the feature rather than an omission:
/// nothing accumulates on disk, so identity is the generator's — the parameter
/// hash over normalised YAML, seed and `plan_format` (FR-021g). It is sufficient
/// because FR-024 makes generation fully determined by exactly those inputs, and
/// it is labelled distinctly so it can never be read as a plan hash.
pub fn unbounded_manifest(g: &Generator, normalised_yaml: &str) -> Manifest {
    let mut cs = CorpusSummary {
        total_bytes: 0,
        ..Default::default()
    };
    fill_configured(g, &mut cs);
    Manifest {
        plan_format: PLAN_FORMAT,
        generator_version: GENERATOR_VERSION.to_string(),
        identity: parameter_hash(normalised_yaml, g.document().seed, PLAN_FORMAT).into(),
        seed: g.document().seed,
        normalised_yaml: normalised_yaml.to_string(),
        event_count: None,
        time_origin_ns: 0,
        duration_ns: None,
        corpus_summary: cs,
        stream_digest: StreamDigest::new().finish(),
    }
}

/// The summary a *write* can honestly fill in.
fn summary(g: &Generator, stats: &PlanStats) -> CorpusSummary {
    let mut cs = CorpusSummary {
        total_bytes: stats.total_bytes,
        ..Default::default()
    };
    fill_configured(g, &mut cs);
    cs
}

/// The parts of the summary that come from resolution rather than from events.
fn fill_configured(g: &Generator, cs: &mut CorpusSummary) {
    cs.branching_resolved = g.corpus().profile.segments().to_vec();
    cs.root_boundary_depth = 0;
    cs.wss_window_requests = g
        .document()
        .run
        .wss_window
        .as_ref()
        .and_then(crate::units::count_from_yaml)
        .unwrap_or(0);
    cs.clamps_applied = g.clamps().total();
    // Churn is opt-in and not yet configurable in generation; an immortal trunk
    // is 0 rotations rather than an absent statistic.
    cs.churn_half_life_ns = 0;
    cs.churn_rotations = 0;
}

impl Manifest {
    /// Serialise, or panic — a manifest built in-process cannot fail to
    /// serialise, and threading an error for it would obscure the ones that can.
    fn to_json_or_panic(&self) -> String {
        self.to_json().expect("a Manifest always serialises")
    }
}

/// Read a plan back: the manifest and an iterator over its events.
///
/// Refuses a `plan_format` this build does not implement *before* decoding a
/// single record, because the record has no length prefix and a guessed width
/// mis-aligns every field silently.
pub fn read_plan(dir: &Path) -> Result<(Manifest, Vec<PlanEvent>), Box<dyn std::error::Error>> {
    let m = Manifest::from_json(&std::fs::read_to_string(dir.join("manifest.json"))?)?;
    let bytes = std::fs::read(dir.join("events.bin"))?;
    let mut out = Vec::with_capacity(bytes.len() / RECORD_BYTES);
    for chunk in bytes.chunks(RECORD_BYTES) {
        out.push(PlanEvent::decode(chunk)?);
    }
    Ok((m, out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{CacheKey, SessionId};
    use crate::schema::Document;

    fn ev(t_ns: u64, request_id: u32, depth: u32, flags: u8) -> PlanEvent {
        PlanEvent {
            t_ns,
            key: CacheKey(u64::from(request_id) << 32 | u64::from(depth)),
            size: 1024,
            request_id,
            session_id: SessionId(request_id),
            depth,
            turn: 1,
            node: 0,
            mix_index: 0,
            flags,
        }
    }

    fn request(t_ns: u64, id: u32, keys: u32) -> Vec<PlanEvent> {
        (0..keys)
            .map(|d| {
                let mut f = 0;
                if d == 0 {
                    f |= flags::REQUEST_START;
                }
                if d == keys - 1 {
                    f |= flags::REQUEST_END;
                }
                ev(t_ns, id, d, f)
            })
            .collect()
    }

    fn write(evs: &[PlanEvent]) -> Result<PlanStats, WriteError> {
        let mut w = PlanWriter::new(Vec::new());
        w.push_all(evs)?;
        w.finish("version: 1\n").map(|(_, s)| s)
    }

    #[test]
    fn a_well_formed_plan_writes_and_counts() {
        let mut evs = request(0, 0, 4);
        evs.extend(request(10, 1, 3));
        let s = write(&evs).unwrap();
        assert_eq!(s.event_count, 7);
        assert_eq!(s.request_count, 2);
        assert_eq!(s.total_bytes, 7 * 1024);
        assert_eq!(s.time_origin_ns, 0);
        assert_eq!(s.duration_ns, 10);
    }

    #[test]
    fn every_record_is_exactly_forty_bytes_on_the_wire() {
        let mut w = PlanWriter::new(Vec::new());
        w.push_all(&request(0, 0, 5)).unwrap();
        let (bytes, _) = w.finish("").unwrap();
        assert_eq!(bytes.len(), 5 * RECORD_BYTES);
    }

    #[test]
    fn time_going_backwards_is_refused() {
        // The runner consumes the plan as a schedule; a backwards step is a
        // different artifact, not a reordering of this one.
        let mut evs = request(100, 0, 2);
        evs.extend(request(50, 1, 2));
        let e = write(&evs).unwrap_err();
        assert!(matches!(
            e,
            WriteError::TimeWentBackwards {
                previous: 100,
                found: 50
            }
        ));
        assert!(format!("{e}").contains("consumed as a schedule"), "{e}");
    }

    #[test]
    fn equal_timestamps_are_fine_because_non_decreasing_is_the_promise() {
        let mut evs = request(7, 0, 2);
        evs.extend(request(7, 1, 2));
        assert!(write(&evs).is_ok());
    }

    #[test]
    fn interleaving_two_requests_is_refused() {
        // The invariant a consumer's batching depends on: scanning to
        // REQUEST_END must not cross into another request.
        let a = request(0, 0, 3);
        let b = request(0, 1, 3);
        let evs = vec![a[0], b[0], a[1], a[2], b[1], b[2]];
        let e = write(&evs).unwrap_err();
        // The interrupting event opens a request, so it is caught as nesting --
        // either way the plan is refused rather than silently written.
        assert!(
            matches!(e, WriteError::NestedRequest { open: 0 }),
            "got {e:?}"
        );
        // And an interruption that does not re-open is caught as interleaving.
        let evs = vec![a[0], ev(0, 1, 1, 0)];
        assert!(matches!(
            write(&evs).unwrap_err(),
            WriteError::RequestInterleaved { open: 0, found: 1 }
        ));
    }

    #[test]
    fn keys_out_of_path_order_are_refused() {
        // `depth` is stored precisely because it equals the ordinal; a plan where
        // it does not would defeat indexing by ordinal.
        let evs = vec![
            ev(0, 0, 0, flags::REQUEST_START),
            ev(0, 0, 2, flags::REQUEST_END),
        ];
        assert!(matches!(
            write(&evs).unwrap_err(),
            WriteError::DepthOutOfOrder {
                request_id: 0,
                expected: 1,
                found: 2
            }
        ));
    }

    #[test]
    fn a_request_that_never_opens_or_never_closes_is_refused() {
        let orphan = vec![ev(0, 3, 0, 0)];
        assert!(matches!(
            write(&orphan).unwrap_err(),
            WriteError::UnopenedRequest { request_id: 3 }
        ));
        let unterminated = vec![ev(0, 0, 0, flags::REQUEST_START)];
        assert!(matches!(
            write(&unterminated).unwrap_err(),
            WriteError::UnterminatedRequest { request_id: 0 }
        ));
    }

    #[test]
    fn request_ids_must_ascend() {
        let mut evs = request(0, 5, 2);
        evs.extend(request(1, 5, 2));
        assert!(matches!(
            write(&evs).unwrap_err(),
            WriteError::RequestIdWentBackwards {
                previous: 5,
                found: 5
            }
        ));
    }

    #[test]
    fn a_reserved_flag_bit_is_refused_at_write_time() {
        // They exist so a future flag can be added without moving a field, which
        // only works while they are zero.
        let evs = vec![ev(0, 0, 0, flags::REQUEST_START | 0x10)];
        assert!(matches!(
            write(&evs).unwrap_err(),
            WriteError::ReservedFlag { flags: 0x11 }
        ));
    }

    #[test]
    fn the_content_hash_covers_the_input_as_well_as_the_events() {
        // FR-026: two plans with identical events but different normalised input
        // are different plans, because the input is what a report is traced to.
        let evs = request(0, 0, 3);
        let mut w = PlanWriter::new(Vec::new());
        w.push_all(&evs).unwrap();
        let a = w.finish("version: 1\nseed: 1\n").unwrap().1;
        let mut w = PlanWriter::new(Vec::new());
        w.push_all(&evs).unwrap();
        let b = w.finish("version: 1\nseed: 2\n").unwrap().1;
        assert_ne!(a.content_hash, b.content_hash);
        // And identical input plus identical events agree.
        let mut w = PlanWriter::new(Vec::new());
        w.push_all(&evs).unwrap();
        let c = w.finish("version: 1\nseed: 1\n").unwrap().1;
        assert_eq!(a.content_hash, c.content_hash);
        // The stream digest is over keys alone, so it agrees where the content
        // hash does not -- which is what makes it usable to prove two arms saw
        // the same stream regardless of how each was configured.
        assert_eq!(a.stream_digest, b.stream_digest);
    }

    const DOC: &str = r#"
version: 1
seed: 99
requests: 200
corpus:
  block_bytes: 131072
  trees:
    roots: {count: 6, popularity: {dist: zipf, s: 0.9}}
    shared_depth: {dist: const, value: 5}
    branching: 1.05
workload:
  arrival: {model: open_loop, rate: 2000/s}
  sessions:
    turns: {dist: const, value: 3}
    think_time: {dist: const, value: 0.2}
    private_depth: {dist: const, value: 2}
    growth_per_turn: {dist: const, value: 1}
run: {mode: hardware, wss_window: 120000}
"#;

    #[test]
    fn a_generated_plan_satisfies_every_invariant_the_writer_checks() {
        // The two halves of US1 meeting: whatever the generator produces, the
        // writer must accept without complaint.
        let d = Document::from_yaml(DOC).unwrap();
        let mut g = Generator::new(&d).unwrap();
        let mut w = PlanWriter::new(Vec::new());
        let mut buf = Vec::new();
        while g.fill(&mut buf) > 0 {
            w.push_all(&buf).expect("generator broke a format promise");
        }
        let (bytes, stats) = w.finish(&d.to_yaml().unwrap()).unwrap();
        assert_eq!(stats.request_count, 200);
        assert_eq!(bytes.len() as u64, stats.event_count * RECORD_BYTES as u64);
    }

    #[test]
    fn a_plan_directory_round_trips_through_the_filesystem() {
        let d = Document::from_yaml(DOC).unwrap();
        let dir = std::env::temp_dir().join(format!(
            "certus-plan-test-{}-{}",
            std::process::id(),
            "roundtrip"
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut g = Generator::new(&d).unwrap();
        let yaml = d.to_yaml().unwrap();
        let m = write_plan(&dir, &mut g, &yaml).unwrap();
        let (back, events) = read_plan(&dir).unwrap();
        assert_eq!(back.identity, m.identity);
        assert_eq!(events.len() as u64, m.event_count.unwrap());
        assert_eq!(back.normalised_yaml, yaml);
        // The summary carries what a write knows and omits what it does not.
        assert_eq!(back.corpus_summary.distinct_keys, None);
        assert!(back.corpus_summary.total_bytes > 0);
        assert_eq!(back.corpus_summary.wss_window_requests, 120_000);
        assert!(!back.corpus_summary.branching_resolved.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unbounded_run_gets_a_parameter_hash_and_no_events_file() {
        // FR-021g. Nothing accumulates, so there is nothing to hash; the
        // generator's identity stands in, and says which kind it is.
        let d = Document::from_yaml(&DOC.replace("requests: 200", "unbounded: true")).unwrap();
        let mut g = Generator::new(&d).unwrap();
        let dir = std::env::temp_dir().join("certus-plan-test-unbounded");
        let e = write_plan(&dir, &mut g, "x").unwrap_err();
        assert!(matches!(e, WriteError::UnboundedPlan));
        assert!(format!("{e}").contains("parameter hash"));
        let m = unbounded_manifest(&g, &d.to_yaml().unwrap());
        assert!(m.is_unbounded());
        assert!(matches!(m.identity, Identity::ParameterHash(_)));
        assert!(m.identity.label().contains("unbounded"));
    }
}
