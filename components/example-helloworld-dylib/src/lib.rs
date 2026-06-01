//! Dynamic library wrapper for the HelloWorld component.
//!
//! Exports a Rust-ABI factory function that returns a `ComponentRef`.
//! Because this dylib and the host binary dynamically link the same
//! `component-core` and `example-helloworld` shared libraries, `TypeId`
//! values are consistent across the boundary — enabling direct
//! `query_interface` calls from the host without any C-ABI shim.
//!
//! **Requirement**: both sides must be built with the same `rustc` version
//! (Rust has no stable ABI guarantee across compiler releases).

use component_core::component_ref::ComponentRef;
use example_helloworld::HelloWorldComponent;
use std::sync::Arc;

/// Create a new HelloWorld component instance.
///
/// The caller receives a `ComponentRef` (an `Arc<dyn IUnknown>` wrapper)
/// and can use `query_interface!` / `query()` to obtain typed interfaces.
#[no_mangle]
pub fn create_component() -> ComponentRef {
    let comp = HelloWorldComponent::new();
    ComponentRef::from(comp as Arc<_>)
}
