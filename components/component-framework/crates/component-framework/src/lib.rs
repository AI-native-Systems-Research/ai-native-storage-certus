//! COM-style component framework for Rust.
//!
//! This facade crate re-exports everything from `component-core` (types and
//! traits) and `component-macros` (procedural macros) for convenient
//! single-dependency usage.
//!
//! # Quick Start
//!
//! ```
//! use component_framework::{define_interface, define_component};
//! use component_framework::iunknown::{IUnknown, query};
//! use std::sync::Arc;
//!
//! // 1. Define an interface
//! define_interface! {
//!     pub IGreeter {
//!         fn greet(&self, name: &str) -> String;
//!     }
//! }
//!
//! // 2. Define a component that provides it
//! define_component! {
//!     pub HelloComponent {
//!         version: "1.0.0",
//!         provides: [IGreeter],
//!     }
//! }
//!
//! impl IGreeter for HelloComponent {
//!     fn greet(&self, name: &str) -> String {
//!         format!("Hello, {name}!")
//!     }
//! }
//!
//! // 3. Instantiate and query
//! let comp = HelloComponent::new();
//! let greeter: Arc<dyn IGreeter + Send + Sync> =
//!     query::<dyn IGreeter + Send + Sync>(&*comp).unwrap();
//! assert_eq!(greeter.greet("world"), "Hello, world!");
//! ```

pub use component_core::*;
pub use component_macros::*;

/// Compile-fail tests for macro error paths (FR-007).
///
/// An interface with no methods must be rejected:
///
/// ```compile_fail
/// use component_framework::define_interface;
/// define_interface! {
///     pub IEmpty {
///     }
/// }
/// ```
///
/// An interface with `&mut self` receiver must be rejected:
///
/// ```compile_fail
/// use component_framework::define_interface;
/// define_interface! {
///     pub IMutable {
///         fn mutate(&mut self);
///     }
/// }
/// ```
///
/// A component missing the `version` field must be rejected:
///
/// ```compile_fail
/// use component_framework::{define_interface, define_component};
/// define_interface! {
///     pub IDummy {
///         fn dummy(&self);
///     }
/// }
/// define_component! {
///     pub BadComponent {
///         provides: [IDummy],
///     }
/// }
/// ```
#[cfg(doc)]
mod _compile_fail_tests {}

/// Convenience re-exports including macros.
///
/// ```
/// use component_framework::prelude::*;
/// ```
pub mod prelude {
    pub use component_core::prelude::*;
    pub use component_macros::{define_component, define_interface};
}

/// Backwards-compatible aliases for macro names some users prefer.
///
/// These simply forward to the procedural macros exported from
/// `component_macros`.
#[macro_export]
macro_rules! declare_interface {
    ($($t:tt)*) => {
        $crate::define_interface! { $($t)* }
    };
}

#[macro_export]
macro_rules! declare_component {
    ($($t:tt)*) => {
        $crate::define_component! { $($t)* }
    };
}
