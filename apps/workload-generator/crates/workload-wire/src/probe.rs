//! How a turn discovers which of its keys the cache already holds.
//!
//! # Why this lives in the wire crate
//!
//! It is a setting both sides must agree on and neither owns. The generator parses it from
//! its own `--probe` and passes it to every agent it launches; the agent parses the same
//! spelling and hands it to the executor. Putting the type here means one list of valid
//! values rather than two that can drift — and it keeps `workload-gen` free of any
//! dependency on `workload-node-agent`, which is FR-079's crate boundary and the reason the
//! emit-only build links no CUDA.
//!
//! # Why the mode exists at all
//!
//! `Check` is what the production client does and stays the default. `Lookup` exists
//! because `Check` **starves the remote path**: `dispatcher.check()` answers from the local
//! dispatch-map alone (`components/dispatcher/src/lib.rs:2882`), so a key a peer holds
//! reads as absent and the client stores it rather than ever asking for it. Only `LOOKUP`
//! reaches `batch_lookup`, the one dispatcher entry point that forwards a local miss to
//! `remote_lookup`. So this is the switch that decides whether a run exercises remote
//! lookup at all, and it is why the production connector's remote path is unreachable in
//! practice.
//!
//! # What it does *not* change
//!
//! The workload. Plan `OpKind`s never reach the wire — a batch yields only its key path,
//! session, virtual time and poll flag — so the plan is byte-identical in both modes and
//! FR-072 is untouched. This selects a **client rule**.

/// Which operation answers "do you already have this key?".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Probe {
    /// `CHECK` the whole path, then load what came back resident.
    ///
    /// The production-faithful stream FR-039 requires.
    #[default]
    Check,
    /// Skip `CHECK` and let `LOOKUP` answer.
    ///
    /// `op_lookup` sets one `ok` byte per key, only on success, counting `KeyNotFound` as a
    /// miss and still returning `STATUS_OK` — so a zero is the miss, and those keys are
    /// stored. One fewer round trip per turn, and every local miss becomes a remote-lookup
    /// attempt. A documented deviation from FR-039, taken to reach a path the production
    /// client cannot.
    ///
    /// The executor also moves `TOUCH` to **after** the load in this mode, because
    /// `dispatcher.touch` never remote-fetches: touching first would fail for exactly the
    /// keys a peer holds and lose the remotely-fetched block's reference.
    Lookup,
}

impl Probe {
    /// Parse the CLI spelling, which is identical on both sides.
    ///
    /// # Errors
    ///
    /// If the value is neither `check` nor `lookup`. Naming the alternatives, because a
    /// misspelling that fell back to the default would produce a run that silently measured
    /// the wrong rule.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_wire::probe::Probe;
    ///
    /// assert_eq!(Probe::parse("check").unwrap(), Probe::Check);
    /// assert_eq!(Probe::parse("lookup").unwrap(), Probe::Lookup);
    /// assert_eq!(Probe::default(), Probe::Check);
    /// assert!(Probe::parse("Lookup").unwrap_err().contains("lookup"));
    /// ```
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "check" => Ok(Self::Check),
            "lookup" => Ok(Self::Lookup),
            other => Err(format!(
                "unknown probe mode {other:?}: expected \"check\" or \"lookup\""
            )),
        }
    }

    /// The CLI spelling, for the report that must say which rule ran.
    ///
    /// # Examples
    ///
    /// ```
    /// use workload_wire::probe::Probe;
    ///
    /// assert_eq!(Probe::Lookup.as_str(), "lookup");
    /// ```
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Lookup => "lookup",
        }
    }
}
