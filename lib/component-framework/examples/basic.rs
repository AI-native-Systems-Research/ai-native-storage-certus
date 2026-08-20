//! Basic example: define an interface, implement a component, query it.
//!
//! Uses `query_interface!` macro for concise interface queries.

use component_framework::iunknown::IUnknown;
use component_framework::query_interface;
use component_framework::{define_component, define_interface};
use std::sync::Arc;

// 1. Define interfaces
define_interface! {
    pub IGreeter {
        fn greet(&self, name: &str) -> String;
    }
}

define_interface! {
    pub IFarewell {
        fn goodbye(&self, name: &str) -> String;
    }
}

// 2. Define a component that provides both
define_component! {
    pub FriendlyComponent {
        version: "1.0.0",
        provides: [IGreeter, IFarewell],
    }
}

impl IGreeter for FriendlyComponent {
    fn greet(&self, name: &str) -> String {
        format!("Hello, {name}!")
    }
}

impl IFarewell for FriendlyComponent {
    fn goodbye(&self, name: &str) -> String {
        format!("Goodbye, {name}!")
    }
}

fn main() {
    // 3. Instantiate — returns Arc<FriendlyComponent>
    let comp = FriendlyComponent::new();

    // 4. Query interfaces through IUnknown — concise with query_interface!
    let greeter: Arc<dyn IGreeter + Send + Sync> = query_interface!(comp, IGreeter).unwrap();
    println!("{}", greeter.greet("world"));

    let farewell: Arc<dyn IFarewell + Send + Sync> = query_interface!(comp, IFarewell).unwrap();
    println!("{}", farewell.goodbye("world"));

    // 5. Version
    println!("Component version: {}", comp.version());
}
