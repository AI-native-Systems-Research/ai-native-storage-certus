//! Transport-agnostic shared-memory-queue control plane for the Certus
//! dispatcher.
//!
//! Factored out of the former `certus-shmq-server` binary so that both
//! `certus-server` (the plain shmq server) and `certus-server-yaml` (the
//! YAML-composed server) share ONE wire protocol ([`wire`]), ONE
//! opcode→`IDispatcher` translator ([`Translator`]), and ONE serve loop
//! ([`serve`]: poller + blocking worker pool + reservation-timeout reaper).
//!
//! # Concurrency model
//!
//! A **single poller thread** busy-scans every channel's request word (never
//! sleeps, so the request path needs no futex) and hands each ready request to
//! a **blocking worker pool** over a crossbeam channel. Workers run the
//! (possibly multi-millisecond, SSD-cold) dispatch and write the reply, so a
//! slow `batch_lookup` on one channel never head-of-line blocks the others.
//! Shared state (`ipc_cache`, `pending_stores`) lives in the [`Translator`]
//! under mutexes, so a Reserve on one channel and a Commit on another agree.
//!
//! # Observability
//!
//! [`serve`] itself emits no per-op metrics. A caller that needs them (e.g. the
//! YAML server's Prometheus/OTel counters) installs a [`TranslatorObserver`] on
//! the [`Translator`] via [`Translator::with_observer`]; the plain server passes
//! none, for zero overhead.

pub mod translate;
pub mod wire;

mod serve;

pub use serve::{serve, ServeConfig};
pub use translate::{OpError, Translator, TranslatorObserver};
