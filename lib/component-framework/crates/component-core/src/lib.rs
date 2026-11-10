//! Core types and traits for a COM-style component framework.
//!
//! This crate provides the foundational building blocks:
//!
//! - [`IUnknown`] — base trait for all components (interface query, version,
//!   introspection)
//! - [`query`] — typed free function to query a component for an interface
//! - [`Receptacle`] — typed slot for connecting required interfaces between
//!   components
//! - [`InterfaceMap`] — runtime storage of `Arc<dyn Trait>` keyed by `TypeId`
//! - Error types ([`QueryError`], [`ReceptacleError`]) and metadata structs
//!   ([`InterfaceInfo`], [`ReceptacleInfo`])
//!
//! Most users should depend on the `component-framework` facade crate, which
//! re-exports everything from this crate plus the `define_interface!` and
//! `define_component!` macros.

//! Quickstart
//! ----------
//!
//! A minimal actor example (from `actor.rs`) — create a handler, spawn the
//! actor, send messages, and deactivate:
//!
//! ```rust
//! use component_core::actor::{Actor, ActorHandler};
//! use std::sync::{Arc, Mutex};
//!
//! struct Accumulator { sum: Arc<Mutex<i64>> }
//! impl ActorHandler<i64> for Accumulator {
//!     fn handle(&mut self, msg: i64) {
//!         *self.sum.lock().unwrap() += msg;
//!     }
//! }
//!
//! let sum = Arc::new(Mutex::new(0i64));
//! let actor = Actor::new(Accumulator { sum: sum.clone() }, |_| {});
//! let handle = actor.activate().unwrap();
//! for i in 1..=10 { handle.send(i).unwrap(); }
//! handle.deactivate().unwrap();
//! assert_eq!(*sum.lock().unwrap(), 55);
//! ```

pub mod actor;
pub mod binding;
pub mod channel;
pub mod component;
pub mod component_ref;
pub mod error;
pub mod interface;
pub mod iunknown;
pub mod log;
pub mod numa;
pub mod prelude;
pub mod receptacle;
pub mod registry;

pub use actor::{pipe, pipe_mpsc, Actor, ActorError, ActorHandle, ActorHandler};
pub use channel::crossbeam_bounded::CrossbeamBoundedChannel;
pub use channel::crossbeam_unbounded::CrossbeamUnboundedChannel;
pub use channel::kanal_bounded::KanalChannel;
pub use channel::mpsc::{MpscChannel, MpscReceiver, MpscSender};
pub use channel::rtrb_spsc::RtrbChannel;
pub use channel::tokio_mpsc::TokioMpscChannel;
pub use channel::{ChannelError, IReceiver, ISender, Receiver, Sender, SpscChannel};
pub use component::InterfaceMap;
pub use component_ref::ComponentRef;
pub use error::{QueryError, ReceptacleError, RegistryError};
pub use interface::{Interface, InterfaceInfo, ReceptacleInfo};
pub use iunknown::{query, IUnknown};
pub use log::{LogHandler, LogLevel, LogMessage};
pub use receptacle::Receptacle;
pub use registry::{ComponentFactory, ComponentRegistry};
