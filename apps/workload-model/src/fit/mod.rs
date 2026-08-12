//! Inverting the statistics: parameters out of a reference stream.
//!
//! `stats` measures what a stream *is*; this module works out what document would
//! have produced it. The two are deliberately adjacent rather than merged — a fit is
//! only meaningful if it is validated by the same statistics it was derived from, and
//! `certus-trace validate` does exactly that (spec FR-021i, FR-057).
//!
//! Nothing here defaults a parameter it cannot measure. FR-055 requires a parameter
//! whose source is unavailable to be **left unset**, because a default in the emitted
//! YAML is indistinguishable from a measurement — and the whole value of a fitted
//! model is that a reader can tell which of its numbers came from data.

pub mod branching;
pub mod sessions;
