//! Convenience re-exports for common component framework types.
//!
//! Import everything from this module to get started quickly:
//!
//! ```
//! use component_core::prelude::*;
//!
//! // Core types are available after a single import
//! let registry = ComponentRegistry::new();
//! assert!(registry.list().is_empty());
//! let receptacle = Receptacle::<dyn IUnknown>::new();
//! assert!(receptacle.get().is_err());
//! ```
//!
//! The [`query_interface!`](crate::query_interface) macro is also available
//! after importing the prelude, since `#[macro_export]` places it at the
//! crate root.

pub use crate::actor::{pipe, pipe_mpsc, Actor, ActorHandle, ActorHandler};
pub use crate::binding::bind;
pub use crate::channel::mpsc::MpscChannel;
pub use crate::channel::spsc::SpscChannel;
pub use crate::channel::{ChannelError, IReceiver, ISender, Receiver, Sender};
pub use crate::component_ref::ComponentRef;
pub use crate::error::RegistryError;
pub use crate::iunknown::{query, IUnknown};
pub use crate::log::{LogHandler, LogLevel, LogMessage};
pub use crate::receptacle::Receptacle;
pub use crate::registry::ComponentRegistry;
