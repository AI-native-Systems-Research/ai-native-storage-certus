//! Procedural macros for the COM-style component framework.
//!
//! Provides two macros:
//!
//! - [`define_interface!`] — declares an interface trait with `Send + Sync`
//!   bounds, usable across crate boundaries without access to the
//!   implementing type.
//! - [`define_component!`] — declares a component struct with automatic
//!   `IUnknown` implementation, interface map population, receptacle fields,
//!   and an `Arc`-returning constructor.
//!
//! Most users should depend on the `component-framework` facade crate, which
//! re-exports these macros alongside the core types.

mod define_component;
mod define_interface;

/// Define a new interface for the component framework.
///
/// Generates a trait with `Send + Sync + 'static` bounds and an
/// [`Interface`](component_core::interface::Interface) implementation
/// for the trait object type.
///
/// # Syntax
///
/// ```text
/// define_interface! {
///     [pub] InterfaceName {
///         fn method_name(&self, args...) -> ReturnType;
///         ...
///     }
/// }
/// ```
///
/// # Rules
///
/// - All methods must take `&self` (not `&mut self`). Use interior
///   mutability in implementations.
/// - At least one method is required.
/// - Lifetime parameters on method signatures are supported.
///
/// # Examples
///
/// ```
/// use component_macros::define_interface;
///
/// define_interface! {
///     pub IStorage {
///         fn read(&self, key: &str) -> Option<Vec<u8>>;
///         fn write(&self, key: &str, value: &[u8]) -> Result<(), String>;
///     }
/// }
///
/// // The generated trait can be used as a bound:
/// fn use_storage(s: &dyn IStorage) {
///     let _ = s.read("key");
/// }
/// ```
///
/// With lifetime parameters:
///
/// ```
/// use component_macros::define_interface;
///
/// define_interface! {
///     pub IBorrower {
///         fn borrow_data<'a>(&'a self) -> &'a [u8];
///     }
/// }
/// ```
#[proc_macro]
pub fn define_interface(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parsed = syn::parse_macro_input!(input as define_interface::InterfaceInput);
    define_interface::expand(parsed).into()
}

/// Define a component that implements one or more interfaces.
///
/// Generates a struct with an `IUnknown` implementation, interface map,
/// receptacle fields, and a constructor that returns `Arc<Self>`.
///
/// # Syntax
///
/// ```text
/// define_component! {
///     [pub] ComponentName {
///         version: "x.y.z",
///         provides: [Interface1, Interface2],
///         receptacles: {           // optional
///             slot_name: IFoo,
///         },
///         fields: {                // optional
///             field_name: Type,
///         },
///     }
/// }
/// ```
///
/// # Examples
///
/// ```
/// use component_macros::{define_interface, define_component};
/// use component_core::iunknown::{IUnknown, query};
/// use std::sync::Arc;
///
/// define_interface! {
///     pub IGreeter {
///         fn greet(&self) -> String;
///     }
/// }
///
/// define_component! {
///     pub HelloComponent {
///         version: "1.0.0",
///         provides: [IGreeter],
///     }
/// }
///
/// impl IGreeter for HelloComponent {
///     fn greet(&self) -> String {
///         "Hello, world!".to_string()
///     }
/// }
///
/// let comp = HelloComponent::new();
/// assert_eq!(comp.version(), "1.0.0");
///
/// let greeter: Arc<dyn IGreeter + Send + Sync> =
///     query::<dyn IGreeter + Send + Sync>(&*comp).unwrap();
/// assert_eq!(greeter.greet(), "Hello, world!");
/// ```
#[proc_macro]
pub fn define_component(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let parsed = syn::parse_macro_input!(input as define_component::ComponentInput);
    define_component::expand(parsed).into()
}
